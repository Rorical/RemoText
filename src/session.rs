use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    task::{Context as TaskContext, Poll},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    time::{Instant, sleep, timeout},
};
use tracing::debug;

use crate::{client::Client, protocol::OutputStream, server::NetworkMode, ticket};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const SESSION_PING_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_ATTEMPTS: usize = 50;
const STARTUP_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SessionInfo {
    port: u16,
    token: [u8; 32],
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
enum SessionFrame {
    Hello { token: [u8; 32] },
    Request(SessionRequest),
    Ok,
    ExecOutput { stream: OutputStream, data: Vec<u8> },
    ExecExit { code: i32 },
    Cancel,
    TransferDone { bytes: u64 },
    Error(String),
}

impl std::fmt::Debug for SessionFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hello { .. } => f
                .debug_struct("Hello")
                .field("token", &"<redacted>")
                .finish(),
            Self::Request(req) => f.debug_tuple("Request").field(req).finish(),
            Self::Ok => f.write_str("Ok"),
            Self::ExecOutput { stream, data: _ } => f
                .debug_struct("ExecOutput")
                .field("stream", stream)
                .field("data", &format!("<{} bytes>", self.data_len()))
                .finish(),
            Self::ExecExit { code } => f.debug_struct("ExecExit").field("code", code).finish(),
            Self::Cancel => f.write_str("Cancel"),
            Self::TransferDone { bytes } => f
                .debug_struct("TransferDone")
                .field("bytes", bytes)
                .finish(),
            Self::Error(msg) => f.debug_tuple("Error").field(msg).finish(),
        }
    }
}

impl SessionFrame {
    fn data_len(&self) -> usize {
        match self {
            Self::ExecOutput { data, .. } => data.len(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum SessionRequest {
    Ping,
    Exec { command: Vec<String> },
    Put { local: PathBuf, remote: String },
    Get { remote: String, local: PathBuf },
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    info: SessionInfo,
}

pub async fn ensure_session(
    addr: &str,
    password: &str,
    network_mode: NetworkMode,
    keepalive_secs: u64,
) -> Result<SessionHandle> {
    let session_file = session_file(addr)?;
    if let Ok(handle) = load_handle(&session_file).await {
        if session_ready(&handle).await {
            return Ok(handle);
        }
        let _ = tokio::fs::remove_file(&session_file).await;
    }

    start_background(addr, password, network_mode, keepalive_secs, &session_file).await?;
    for _ in 0..STARTUP_ATTEMPTS {
        if let Ok(handle) = load_handle(&session_file).await
            && session_ready(&handle).await
        {
            return Ok(handle);
        }
        sleep(STARTUP_DELAY).await;
    }

    bail!("background RemoText session did not become ready")
}

pub async fn run_background(
    addr: String,
    password: String,
    token: [u8; 32],
    session_file: PathBuf,
    network_mode: NetworkMode,
    keepalive_secs: u64,
) -> Result<()> {
    let client = Client::new(ticket::decode_addr(&addr)?, password, network_mode)
        .connect_persistent()
        .await?;
    client.ping().await?;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("bind local RemoText session listener")?;
    let port = listener.local_addr()?.port();
    write_info(&session_file, &SessionInfo { port, token }).await?;

    let idle = Duration::from_secs(keepalive_secs.max(1));
    let mut tasks = tokio::task::JoinSet::new();
    let idle_sleep = sleep(idle);
    tokio::pin!(idle_sleep);

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    let client = client.clone();
                    tasks.spawn(async move { handle_connection(stream, client, token).await });
                }
                Err(err) => return Err(err).context("accept local RemoText session request"),
            },
            completed = tasks.join_next(), if !tasks.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(err))) => debug!(?err, "local RemoText session request failed"),
                    Some(Err(err)) => debug!(?err, "local RemoText session task failed"),
                    None => {}
                }
                if tasks.is_empty() {
                    idle_sleep.as_mut().reset(Instant::now() + idle);
                }
            }
            _ = &mut idle_sleep, if tasks.is_empty() => break,
        }
    }

    client.close().await;
    let _ = tokio::fs::remove_file(session_file).await;
    Ok(())
}

async fn session_ready(handle: &SessionHandle) -> bool {
    matches!(
        timeout(SESSION_PING_TIMEOUT, handle.ping()).await,
        Ok(Ok(()))
    )
}

pub async fn ping(
    addr: &str,
    password: &str,
    network_mode: NetworkMode,
    keepalive_secs: u64,
) -> Result<()> {
    ensure_session(addr, password, network_mode, keepalive_secs)
        .await?
        .ping()
        .await
}

pub struct ExecSessionRequest<'a, W1, W2> {
    pub addr: &'a str,
    pub password: &'a str,
    pub network_mode: NetworkMode,
    pub keepalive_secs: u64,
    pub command: Vec<String>,
    pub stdout: &'a mut W1,
    pub stderr: &'a mut W2,
}

pub async fn exec<W1, W2>(request: ExecSessionRequest<'_, W1, W2>) -> Result<i32>
where
    W1: AsyncWrite + Unpin,
    W2: AsyncWrite + Unpin,
{
    exec_with_cancel(request, std::future::pending()).await
}

pub async fn exec_with_cancel<W1, W2, F>(
    request: ExecSessionRequest<'_, W1, W2>,
    cancel: F,
) -> Result<i32>
where
    W1: AsyncWrite + Unpin,
    W2: AsyncWrite + Unpin,
    F: Future<Output = ()>,
{
    let handle = ensure_session(
        request.addr,
        request.password,
        request.network_mode,
        request.keepalive_secs,
    )
    .await?;
    let mut stream = handle.open().await?;
    write_frame(
        &mut stream,
        &SessionFrame::Request(SessionRequest::Exec {
            command: request.command,
        }),
    )
    .await?;
    let (mut reader, mut writer) = stream.into_split();
    let stdout = request.stdout;
    let stderr = request.stderr;
    let mut cancelled = false;
    tokio::pin!(cancel);

    loop {
        tokio::select! {
            frame = read_frame(&mut reader) => {
                match frame? {
                    SessionFrame::ExecOutput { stream, data } => match stream {
                        OutputStream::Stdout => {
                            stdout.write_all(&data).await?;
                            stdout.flush().await?;
                        }
                        OutputStream::Stderr => {
                            stderr.write_all(&data).await?;
                            stderr.flush().await?;
                        }
                    },
                    SessionFrame::ExecExit { code } => return Ok(code),
                    SessionFrame::Error(message) => bail!(message),
                    other => bail!("unexpected session exec response: {other:?}"),
                }
            }
            _ = &mut cancel, if !cancelled => {
                write_frame(&mut writer, &SessionFrame::Cancel).await?;
                cancelled = true;
            }
        }
    }
}

pub async fn put(
    addr: &str,
    password: &str,
    network_mode: NetworkMode,
    keepalive_secs: u64,
    local: &Path,
    remote: &str,
) -> Result<u64> {
    let handle = ensure_session(addr, password, network_mode, keepalive_secs).await?;
    let local = absolute_path(local)?;
    let mut stream = handle.open().await?;
    write_frame(
        &mut stream,
        &SessionFrame::Request(SessionRequest::Put {
            local,
            remote: remote.to_string(),
        }),
    )
    .await?;
    transfer_done(stream).await
}

pub async fn get(
    addr: &str,
    password: &str,
    network_mode: NetworkMode,
    keepalive_secs: u64,
    remote: &str,
    local: &Path,
) -> Result<u64> {
    let handle = ensure_session(addr, password, network_mode, keepalive_secs).await?;
    let local = absolute_path(local)?;
    let mut stream = handle.open().await?;
    write_frame(
        &mut stream,
        &SessionFrame::Request(SessionRequest::Get {
            remote: remote.to_string(),
            local,
        }),
    )
    .await?;
    transfer_done(stream).await
}

impl SessionHandle {
    async fn ping(&self) -> Result<()> {
        let mut stream = self.open().await?;
        write_frame(&mut stream, &SessionFrame::Request(SessionRequest::Ping)).await?;
        match read_frame(&mut stream).await? {
            SessionFrame::Ok => Ok(()),
            SessionFrame::Error(message) => bail!(message),
            other => bail!("unexpected session ping response: {other:?}"),
        }
    }

    async fn open(&self) -> Result<TcpStream> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.info.port));
        let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .context("connect to local RemoText session timed out")?
            .context("connect to local RemoText session")?;
        write_frame(
            &mut stream,
            &SessionFrame::Hello {
                token: self.info.token,
            },
        )
        .await?;
        match read_frame(&mut stream).await? {
            SessionFrame::Ok => Ok(stream),
            SessionFrame::Error(message) => bail!(message),
            other => bail!("unexpected session hello response: {other:?}"),
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    client: crate::client::PersistentClient,
    token: [u8; 32],
) -> Result<()> {
    match read_frame(&mut stream).await? {
        SessionFrame::Hello { token: candidate } if token_eq(&token, &candidate) => {
            write_frame(&mut stream, &SessionFrame::Ok).await?;
        }
        SessionFrame::Hello { .. } => {
            write_frame(
                &mut stream,
                &SessionFrame::Error("invalid session token".to_string()),
            )
            .await?;
            return Ok(());
        }
        other => {
            write_frame(
                &mut stream,
                &SessionFrame::Error(format!("expected session hello, got {other:?}")),
            )
            .await?;
            return Ok(());
        }
    }

    let request = match read_frame(&mut stream).await? {
        SessionFrame::Request(request) => request,
        other => {
            write_frame(
                &mut stream,
                &SessionFrame::Error(format!("expected session request, got {other:?}")),
            )
            .await?;
            return Ok(());
        }
    };

    let result = match request {
        SessionRequest::Ping => {
            client.ping().await?;
            write_frame(&mut stream, &SessionFrame::Ok).await
        }
        SessionRequest::Exec { command } => {
            let (mut reader, mut writer) = stream.into_split();
            run_exec_session(&mut reader, &mut writer, client, command).await
        }
        SessionRequest::Put { local, remote } => match client.put(&local, &remote).await {
            Ok(bytes) => write_frame(&mut stream, &SessionFrame::TransferDone { bytes }).await,
            Err(err) => write_frame(&mut stream, &SessionFrame::Error(err.to_string())).await,
        },
        SessionRequest::Get { remote, local } => match client.get(&remote, &local).await {
            Ok(bytes) => write_frame(&mut stream, &SessionFrame::TransferDone { bytes }).await,
            Err(err) => write_frame(&mut stream, &SessionFrame::Error(err.to_string())).await,
        },
    };

    result.context("write local session response")
}

async fn run_exec_session<R, W>(
    reader: &mut R,
    writer: &mut W,
    client: crate::client::PersistentClient,
    command: Vec<String>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();

    tokio::spawn(async move {
        let mut stdout = SessionOutputWriter::new(OutputStream::Stdout, output_tx.clone());
        let mut stderr = SessionOutputWriter::new(OutputStream::Stderr, output_tx);
        let result = client
            .exec_stream_with_cancel(command, &mut stdout, &mut stderr, async {
                let _ = cancel_rx.await;
            })
            .await;
        let _ = done_tx.send(result);
    });

    let mut done = None;
    let mut output_closed = false;
    let mut local_cancelled = false;
    let mut cancel_tx = Some(cancel_tx);
    while done.is_none() || !output_closed {
        tokio::select! {
            output = output_rx.recv(), if !output_closed => {
                match output {
                    Some((stream_kind, data)) => {
                        write_frame(
                            writer,
                            &SessionFrame::ExecOutput {
                                stream: stream_kind,
                                data,
                            },
                        )
                        .await?;
                    }
                    None => output_closed = true,
                }
            }
            result = &mut done_rx, if done.is_none() => {
                done = Some(match result {
                    Ok(result) => result,
                    Err(err) => Err(anyhow::anyhow!("remote exec task ended unexpectedly: {err}")),
                });
            }
            frame = read_frame(reader), if !local_cancelled && done.is_none() => {
                local_cancelled = true;
                match frame {
                    Ok(SessionFrame::Cancel) => {
                        if let Some(cancel_tx) = cancel_tx.take() {
                            let _ = cancel_tx.send(());
                        }
                    }
                    Ok(other) => {
                        if let Some(cancel_tx) = cancel_tx.take() {
                            let _ = cancel_tx.send(());
                        }
                        done = Some(Err(anyhow::anyhow!("unexpected session exec control frame: {other:?}")));
                    }
                    Err(_) => {
                        if let Some(cancel_tx) = cancel_tx.take() {
                            let _ = cancel_tx.send(());
                        }
                    }
                }
            }
        }
    }

    match done.expect("loop exits only after exec finishes") {
        Ok(code) => write_frame(writer, &SessionFrame::ExecExit { code }).await,
        Err(err) => write_frame(writer, &SessionFrame::Error(err.to_string())).await,
    }
}

struct SessionOutputWriter {
    stream: OutputStream,
    tx: mpsc::UnboundedSender<(OutputStream, Vec<u8>)>,
}

impl SessionOutputWriter {
    fn new(stream: OutputStream, tx: mpsc::UnboundedSender<(OutputStream, Vec<u8>)>) -> Self {
        Self { stream, tx }
    }
}

impl AsyncWrite for SessionOutputWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.tx.send((this.stream, buf.to_vec())).is_err() {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "session output receiver closed",
            )));
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn transfer_done(mut stream: TcpStream) -> Result<u64> {
    match read_frame(&mut stream).await? {
        SessionFrame::TransferDone { bytes } => Ok(bytes),
        SessionFrame::Error(message) => bail!(message),
        other => bail!("unexpected session transfer response: {other:?}"),
    }
}

async fn start_background(
    addr: &str,
    password: &str,
    network_mode: NetworkMode,
    keepalive_secs: u64,
    session_file: &Path,
) -> Result<()> {
    let token: [u8; 32] = rand::random();
    let token_b64 = URL_SAFE_NO_PAD.encode(token);
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("__session")
        .arg("--addr")
        .arg(addr)
        .arg("--keepalive-secs")
        .arg(keepalive_secs.to_string())
        .env("REMOTEXT_PASSWORD", password)
        .env("REMOTEXT_TOKEN", &token_b64)
        .env("REMOTEXT_SESSION_FILE", session_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if matches!(network_mode, NetworkMode::LocalOnly) {
        command.arg("--local-only");
    }
    command
        .spawn()
        .context("spawn RemoText background session")?;
    Ok(())
}

pub fn decode_token(input: &str) -> Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(input)
        .context("decode session token")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("session token has invalid length"))
}

fn session_file(addr: &str) -> Result<PathBuf> {
    let mut hasher = Sha256::new();
    hasher.update(addr.as_bytes());
    let digest = hasher.finalize();
    let id = URL_SAFE_NO_PAD.encode(digest);
    Ok(std::env::temp_dir()
        .join("remotext-sessions")
        .join(format!("{id}.session")))
}

async fn load_handle(path: &Path) -> Result<SessionHandle> {
    let encoded = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read session file {}", path.display()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .context("decode session file")?;
    let info: SessionInfo = postcard::from_bytes(&bytes).context("parse session file")?;
    Ok(SessionHandle { info })
}

async fn write_info(path: &Path, info: &SessionInfo) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create session directory {}", parent.display()))?;
    }
    let bytes = postcard::to_stdvec(info).context("encode session info")?;
    tokio::fs::write(path, URL_SAFE_NO_PAD.encode(bytes))
        .await
        .with_context(|| format!("write session file {}", path.display()))?;
    restrict_session_file(path).await?;
    Ok(())
}

async fn write_frame<W>(writer: &mut W, frame: &SessionFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = postcard::to_stdvec(frame).context("serialize session frame")?;
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R>(reader: &mut R) -> Result<SessionFrame>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    reader.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > 16 * 1024 * 1024 {
        bail!("session frame too large: {len} bytes");
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    postcard::from_bytes(&bytes).context("deserialize session frame")
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn token_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(unix)]
async fn restrict_session_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    tokio::fs::set_permissions(path, permissions)
        .await
        .with_context(|| format!("restrict session file permissions {}", path.display()))
}

#[cfg(not(unix))]
async fn restrict_session_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_token_roundtrip() {
        let token = [42u8; 32];
        let encoded = URL_SAFE_NO_PAD.encode(token);
        assert_eq!(decode_token(&encoded).unwrap(), token);
    }

    #[test]
    fn session_file_changes_with_address() {
        let a = session_file("rt1_test_a").unwrap();
        let b = session_file("rt1_test_b").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn session_frame_debug_redacts_token() {
        let token = [7u8; 32];
        let frame = SessionFrame::Hello { token };
        let debug = format!("{:?}", frame);
        assert!(!debug.contains("777777"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn session_frame_debug_redacts_exec_output_data() {
        let frame = SessionFrame::ExecOutput {
            stream: OutputStream::Stdout,
            data: vec![1, 2, 3, 4],
        };
        let debug = format!("{:?}", frame);
        assert!(!debug.contains("1, 2, 3, 4"));
        assert!(debug.contains("4 bytes"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn background_session_accepts_ping_while_exec_runs() -> Result<()> {
        use crate::server::{Server, ServerConfig};
        use tokio::sync::oneshot;

        let server_dir = tempfile::tempdir()?;
        let server = Server::bind(ServerConfig {
            password: "secret".to_string(),
            name: "test".to_string(),
            data_dir: Some(server_dir.path().to_path_buf()),
            network_mode: NetworkMode::LocalOnly,
            limits: None,
        })
        .await?;
        let addr = server.ticket()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_handle = tokio::spawn(server.run_until(async {
            let _ = shutdown_rx.await;
        }));

        let session_path = session_file(&addr)?;
        let _ = tokio::fs::remove_file(&session_path).await;
        let token = [9u8; 32];
        let background = tokio::spawn(run_background(
            addr.clone(),
            "secret".to_string(),
            token,
            session_path.clone(),
            NetworkMode::LocalOnly,
            30,
        ));

        let handle = wait_for_session_handle(&session_path).await?;
        assert!(session_ready(&handle).await);

        let work = tempfile::tempdir()?;
        let started = work.path().join("started");
        let script = format!(
            "printf started > {}; sleep 2",
            shell_quote(&started.to_string_lossy())
        );
        let exec_addr = addr.clone();
        let exec_task = tokio::spawn(async move {
            let mut stdout = tokio::io::sink();
            let mut stderr = tokio::io::sink();
            exec(ExecSessionRequest {
                addr: &exec_addr,
                password: "secret",
                network_mode: NetworkMode::LocalOnly,
                keepalive_secs: 30,
                command: vec!["sh".to_string(), "-c".to_string(), script],
                stdout: &mut stdout,
                stderr: &mut stderr,
            })
            .await
        });

        for _ in 0..50 {
            if tokio::fs::metadata(&started).await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        assert!(started.exists(), "long-running command did not start");

        timeout(
            Duration::from_secs(2),
            ping(&addr, "secret", NetworkMode::LocalOnly, 30),
        )
        .await??;

        assert_eq!(exec_task.await??, 0);
        background.abort();
        let _ = tokio::fs::remove_file(&session_path).await;
        let _ = shutdown_tx.send(());
        server_handle.await??;
        Ok(())
    }

    async fn wait_for_session_handle(path: &Path) -> Result<SessionHandle> {
        for _ in 0..50 {
            if let Ok(handle) = load_handle(path).await
                && session_ready(&handle).await
            {
                return Ok(handle);
            }
            sleep(Duration::from_millis(100)).await;
        }
        bail!("background session did not become ready in test")
    }

    #[cfg(not(windows))]
    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
