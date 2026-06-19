use anyhow::Result;
use remotext::{
    client::Client,
    server::{NetworkMode, Server, ServerConfig, ServerLimits},
};
use tempfile::TempDir;
use tokio::sync::oneshot;

struct TestServer {
    ticket: String,
    shutdown: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<Result<()>>,
    _dir: TempDir,
}

impl TestServer {
    async fn start(password: &str) -> Result<Self> {
        Self::start_with_limits(password, None).await
    }

    async fn start_with_limits(password: &str, limits: Option<ServerLimits>) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let server = Server::bind(ServerConfig {
            password: password.to_string(),
            name: "test".to_string(),
            data_dir: Some(dir.path().to_path_buf()),
            network_mode: NetworkMode::LocalOnly,
            limits,
        })
        .await?;
        let ticket = server.ticket()?;
        let (tx, rx) = oneshot::channel();
        let handle = tokio::spawn(server.run_until(async {
            let _ = rx.await;
        }));
        Ok(Self {
            ticket,
            shutdown: Some(tx),
            handle,
            _dir: dir,
        })
    }

    fn client(&self, password: &str) -> Client {
        Client::new(
            remotext::ticket::decode_addr(&self.ticket).unwrap(),
            password,
            NetworkMode::LocalOnly,
        )
    }

    async fn stop(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.handle.await??;
        Ok(())
    }
}

#[tokio::test]
async fn ping_authenticates() -> Result<()> {
    let server = TestServer::start("secret").await?;
    server.client("secret").ping().await?;
    server.stop().await
}

#[tokio::test]
async fn ping_rejects_wrong_password() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let err = server.client("wrong").ping().await.unwrap_err();
    assert!(err.to_string().contains("authentication failed"));
    server.stop().await
}

#[tokio::test]
async fn exec_returns_output_and_exit_code() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let command = success_command();
    let output = server.client("secret").exec_collect(command).await?;

    assert_eq!(output.code, 0);
    assert_eq!(String::from_utf8(output.stdout)?, "hello");
    assert!(output.stderr.is_empty());

    server.stop().await
}

#[tokio::test]
async fn exec_propagates_nonzero_exit_code() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let output = server
        .client("secret")
        .exec_collect(failing_command())
        .await?;
    assert_eq!(output.code, 7);
    server.stop().await
}

#[tokio::test]
async fn persistent_client_reuses_connection_for_multiple_requests() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let client = server.client("secret").connect_persistent().await?;

    client.ping().await?;
    let first = client.exec_collect(success_command()).await?;
    let second = client.exec_collect(success_command()).await?;

    assert_eq!(first.code, 0);
    assert_eq!(second.code, 0);
    assert_eq!(String::from_utf8(first.stdout)?, "hello");
    assert_eq!(String::from_utf8(second.stdout)?, "hello");

    server.stop().await
}

#[cfg(not(windows))]
#[tokio::test]
async fn explicit_cancel_stops_remote_process() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let work = tempfile::tempdir()?;
    let started = work.path().join("started");
    let done = work.path().join("done");
    let script = format!(
        "printf started > {}; sleep 5; printf done > {}",
        shell_quote(&started.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    let client = server.client("secret");
    let started_for_cancel = started.clone();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.exec_collect_with_cancel(
            vec!["sh".to_string(), "-c".to_string(), script],
            async move {
                for _ in 0..50 {
                    if tokio::fs::metadata(&started_for_cancel).await.is_ok() {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
        ),
    )
    .await??;

    assert_ne!(output.code, 0);
    assert!(started.exists(), "remote process did not start");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(
        !done.exists(),
        "remote process continued after explicit cancel"
    );

    server.stop().await
}

#[cfg(not(windows))]
#[tokio::test]
async fn put_and_get_transfer_file() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let work = tempfile::tempdir()?;
    let local_source = work.path().join("source.txt");
    let remote_path = work.path().join("remote.txt");
    let local_dest = work.path().join("dest.txt");
    let mut payload = Vec::new();
    for i in 0..200_000 {
        payload.push((i % 251) as u8);
    }
    tokio::fs::write(&local_source, &payload).await?;

    let client = server.client("secret");
    let uploaded = client
        .put(&local_source, &remote_path.to_string_lossy())
        .await?;
    assert_eq!(uploaded, payload.len() as u64);
    assert_eq!(tokio::fs::read(&remote_path).await?, payload);

    let downloaded = client
        .get(&remote_path.to_string_lossy(), &local_dest)
        .await?;
    assert_eq!(downloaded, payload.len() as u64);
    assert_eq!(tokio::fs::read(&local_dest).await?, payload);

    server.stop().await
}

#[cfg(windows)]
fn success_command() -> Vec<String> {
    vec![
        "cmd".to_string(),
        "/C".to_string(),
        "<NUL set /p dummy=hello& exit /b 0".to_string(),
    ]
}

#[cfg(not(windows))]
fn success_command() -> Vec<String> {
    vec!["printf".to_string(), "hello".to_string()]
}

#[cfg(windows)]
fn failing_command() -> Vec<String> {
    vec!["cmd".to_string(), "/C".to_string(), "exit 7".to_string()]
}

#[cfg(not(windows))]
fn failing_command() -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()]
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[tokio::test]
async fn put_rejects_file_exceeding_max_size() -> Result<()> {
    let limits = ServerLimits {
        max_file_size: 1024,
        ..Default::default()
    };
    let server = TestServer::start_with_limits("secret", Some(limits)).await?;
    let work = tempfile::tempdir()?;
    let source = work.path().join("big.txt");
    let payload = vec![0u8; 2048];
    tokio::fs::write(&source, &payload).await?;

    let err = server
        .client("secret")
        .put(&source, &work.path().join("dest.txt").to_string_lossy())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("transfer denied"),
        "expected transfer denied, got: {err}"
    );
    server.stop().await
}

#[tokio::test]
async fn exec_timeout_kills_long_running_command() -> Result<()> {
    let limits = ServerLimits {
        max_command_secs: 1,
        ..Default::default()
    };
    let server = TestServer::start_with_limits("secret", Some(limits)).await?;

    #[cfg(not(windows))]
    let command = vec!["sleep".to_string(), "30".to_string()];
    #[cfg(windows)]
    let command = vec![
        "cmd".to_string(),
        "/C".to_string(),
        "timeout /t 30".to_string(),
    ];

    let result = server.client("secret").exec_collect(command).await?;
    assert_ne!(
        result.code, 0,
        "timed-out command should have nonzero exit code"
    );
    server.stop().await
}

#[cfg(not(windows))]
#[tokio::test]
async fn put_get_verifies_transfer_hash() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let work = tempfile::tempdir()?;
    let source = work.path().join("source.bin");
    let remote = work.path().join("remote.bin");
    let dest = work.path().join("dest.bin");
    let payload: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    tokio::fs::write(&source, &payload).await?;

    let client = server.client("secret");
    let uploaded = client.put(&source, &remote.to_string_lossy()).await?;
    assert_eq!(uploaded, payload.len() as u64);

    let downloaded = client.get(&remote.to_string_lossy(), &dest).await?;
    assert_eq!(downloaded, payload.len() as u64);
    assert_eq!(tokio::fs::read(&dest).await?, payload);

    server.stop().await
}

#[tokio::test]
async fn authentication_failure_is_rejected() -> Result<()> {
    let server = TestServer::start("secret").await?;
    let bad_client = server.client("wrong");

    let err = bad_client.ping().await.unwrap_err();
    assert!(
        err.to_string().contains("authentication failed"),
        "expected auth failure, got: {err}"
    );
    server.stop().await
}
