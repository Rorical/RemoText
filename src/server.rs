use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Semaphore, mpsc},
};
use tracing::{debug, warn};
use zeroize::Zeroize;

use crate::{
    PROTOCOL_ALPN, PROTOCOL_VERSION, auth,
    files::{ensure_parent, temp_sibling},
    framing::{read_message, write_message},
    protocol::{
        DEFAULT_MAX_COMMAND_SECS, DEFAULT_MAX_CONCURRENT_COMMANDS, DEFAULT_MAX_CONNECTIONS,
        DEFAULT_MAX_FILE_SIZE, ErrorCode, ExecRequest, FILE_CHUNK_SIZE, GetRequest, Message,
        OUTPUT_CHUNK_SIZE, OutputStream, PutRequest, RemoteError, Response,
    },
    ticket,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Public,
    LocalOnly,
}

#[derive(Debug, Clone)]
pub struct ServerLimits {
    pub max_connections: usize,
    pub max_concurrent_commands: usize,
    pub max_file_size: u64,
    pub max_command_secs: u64,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_concurrent_commands: DEFAULT_MAX_CONCURRENT_COMMANDS,
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_command_secs: DEFAULT_MAX_COMMAND_SECS,
        }
    }
}

struct AuthRateLimiter {
    failures: AtomicU32,
    last_failure: AtomicU64,
}

impl AuthRateLimiter {
    fn new() -> Self {
        Self {
            failures: AtomicU32::new(0),
            last_failure: AtomicU64::new(0),
        }
    }

    fn check_and_record_failure(&self) -> Duration {
        let now = now_millis();
        let last = self.last_failure.swap(now, Ordering::SeqCst);
        if last != 0 && now - last > 300_000 {
            self.failures.store(0, Ordering::SeqCst);
        }
        let failures = self.failures.fetch_add(1, Ordering::SeqCst);
        Duration::from_millis(100 * 2u64.pow(failures.min(10)))
    }

    fn should_wait(&self) -> Option<Duration> {
        let failures = self.failures.load(Ordering::SeqCst);
        if failures == 0 {
            return None;
        }
        let last = self.last_failure.load(Ordering::SeqCst);
        if last == 0 {
            return None;
        }
        let elapsed = now_millis() - last;
        let delay_ms = 100 * 2u64.pow(failures.min(10));
        if elapsed >= delay_ms {
            return Some(Duration::ZERO);
        }
        Some(Duration::from_millis(delay_ms - elapsed))
    }

    fn reset(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.last_failure.store(0, Ordering::SeqCst);
    }
}

fn now_millis() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone)]
pub struct ServerConfig {
    pub password: String,
    pub name: String,
    pub data_dir: Option<PathBuf>,
    pub network_mode: NetworkMode,
    pub limits: Option<ServerLimits>,
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("password", &"<redacted>")
            .field("name", &self.name)
            .field("data_dir", &self.data_dir)
            .field("network_mode", &self.network_mode)
            .field("limits", &self.limits)
            .finish()
    }
}

pub struct Server {
    endpoint: Endpoint,
    auth: Arc<auth::ServerAuth>,
    name: String,
    data_dir: PathBuf,
    limits: ServerLimits,
    conn_semaphore: Arc<Semaphore>,
    cmd_semaphore: Arc<Semaphore>,
    rate_limiter: Arc<AuthRateLimiter>,
}

impl Server {
    pub async fn bind(mut config: ServerConfig) -> Result<Self> {
        if config.password.is_empty() {
            bail!("server password must not be empty");
        }

        let limits = config.limits.take().unwrap_or_default();

        let data_dir = match config.data_dir.take() {
            Some(path) => path,
            None => default_data_dir()?,
        };
        tokio::fs::create_dir_all(&data_dir)
            .await
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        let secret_key = load_or_create_secret(&data_dir).await?;
        let server_id = *secret_key.public().as_bytes();
        let auth = Arc::new(auth::ServerAuth::new(&config.password, server_id)?);

        config.password.zeroize();

        let endpoint = match config.network_mode {
            NetworkMode::Public => Endpoint::builder(presets::N0)
                .secret_key(secret_key)
                .alpns(vec![PROTOCOL_ALPN.to_vec()])
                .bind()
                .await
                .context("bind public iroh server endpoint")?,
            NetworkMode::LocalOnly => Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(vec![PROTOCOL_ALPN.to_vec()])
                .bind()
                .await
                .context("bind local-only iroh server endpoint")?,
        };

        let max_connections = limits.max_connections;
        let max_concurrent_commands = limits.max_concurrent_commands;

        Ok(Self {
            endpoint,
            auth,
            name: config.name,
            data_dir,
            limits,
            conn_semaphore: Arc::new(Semaphore::new(max_connections)),
            cmd_semaphore: Arc::new(Semaphore::new(max_concurrent_commands)),
            rate_limiter: Arc::new(AuthRateLimiter::new()),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn ticket(&self) -> Result<String> {
        ticket::encode_addr(&self.endpoint.addr())
    }

    pub async fn run_until<F>(self, shutdown: F) -> Result<()>
    where
        F: Future<Output = ()>,
    {
        let endpoint = self.endpoint.clone();
        let auth = self.auth.clone();
        let conn_semaphore = self.conn_semaphore.clone();
        let cmd_semaphore = self.cmd_semaphore.clone();
        let rate_limiter = self.rate_limiter.clone();
        let limits = self.limits.clone();
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        break;
                    };
                    let auth = auth.clone();
                    let conn_semaphore = conn_semaphore.clone();
                    let cmd_semaphore = cmd_semaphore.clone();
                    let rate_limiter = rate_limiter.clone();
                    let limits = limits.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_incoming(
                            incoming,
                            auth,
                            conn_semaphore,
                            cmd_semaphore,
                            rate_limiter,
                            limits,
                        ).await {
                            debug!(?err, "connection handler stopped");
                        }
                    });
                }
            }
        }

        endpoint.close().await;
        Ok(())
    }
}

async fn handle_incoming(
    incoming: iroh::endpoint::Incoming,
    auth: Arc<auth::ServerAuth>,
    conn_semaphore: Arc<Semaphore>,
    cmd_semaphore: Arc<Semaphore>,
    rate_limiter: Arc<AuthRateLimiter>,
    limits: ServerLimits,
) -> Result<()> {
    let _permit = conn_semaphore.acquire().await;
    let conn = incoming.await.context("accept iroh connection")?;
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let auth = auth.clone();
                let cmd_semaphore = cmd_semaphore.clone();
                let rate_limiter = rate_limiter.clone();
                let limits = limits.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        handle_stream(send, recv, auth, cmd_semaphore, rate_limiter, limits).await
                    {
                        debug!(?err, "request stream handler stopped");
                    }
                });
            }
            Err(err) => {
                debug!(?err, "connection closed while accepting streams");
                break;
            }
        }
    }
    Ok(())
}

async fn handle_stream(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    auth: Arc<auth::ServerAuth>,
    cmd_semaphore: Arc<Semaphore>,
    rate_limiter: Arc<AuthRateLimiter>,
    limits: ServerLimits,
) -> Result<()> {
    if let Some(delay) = rate_limiter.should_wait() {
        tokio::time::sleep(delay).await;
    }

    let hello = match read_message(&mut recv).await? {
        Message::ClientHello(hello) => hello,
        other => {
            write_error(
                &mut send,
                ErrorCode::Protocol,
                format!("expected client hello, got {other:?}"),
            )
            .await?;
            return Ok(());
        }
    };

    if hello.version != PROTOCOL_VERSION {
        write_error(
            &mut send,
            ErrorCode::VersionUnsupported,
            format!(
                "client protocol version {} is unsupported by server version {}",
                hello.version, PROTOCOL_VERSION
            ),
        )
        .await?;
        return Ok(());
    }

    let login = match auth.start_login(&hello.credential_request) {
        Ok(login) => login,
        Err(err) => {
            write_error(
                &mut send,
                ErrorCode::Protocol,
                format!("invalid OPAQUE credential request: {err}"),
            )
            .await?;
            return Ok(());
        }
    };

    write_message(
        &mut send,
        &Message::ServerHello(crate::protocol::ServerHello {
            version: PROTOCOL_VERSION,
            server_id: *auth.server_id(),
            credential_response: login.credential_response().to_vec(),
        }),
    )
    .await?;

    let request = match read_message(&mut recv).await? {
        Message::ClientRequest(request) => request,
        other => {
            write_error(
                &mut send,
                ErrorCode::Protocol,
                format!("expected request, got {other:?}"),
            )
            .await?;
            return Ok(());
        }
    };

    let session_key = match login.finish(&request.credential_finalization) {
        Ok(session_key) => session_key,
        Err(_) => {
            let delay = rate_limiter.check_and_record_failure();
            warn!(?delay, "authentication failed; applying rate-limit delay");
            tokio::time::sleep(delay).await;
            write_error(&mut send, ErrorCode::AuthFailed, "invalid password").await?;
            return Ok(());
        }
    };

    if !auth::verify_request_mac(
        &session_key,
        auth.server_id(),
        &request.request,
        &request.request_mac,
    )? {
        let delay = rate_limiter.check_and_record_failure();
        warn!(
            ?delay,
            "request MAC verification failed; applying rate-limit delay"
        );
        tokio::time::sleep(delay).await;
        write_error(&mut send, ErrorCode::AuthFailed, "invalid password").await?;
        return Ok(());
    }

    rate_limiter.reset();

    match request.request {
        crate::protocol::Request::Ping => {
            write_message(&mut send, &Message::Response(Response::Pong)).await?;
        }
        crate::protocol::Request::Exec(request) => {
            run_exec(request, &mut send, &mut recv, &cmd_semaphore, &limits).await?
        }
        crate::protocol::Request::Put(request) => {
            run_put(request, &mut send, &mut recv, &limits).await?
        }
        crate::protocol::Request::Get(request) => run_get(request, &mut send, &limits).await?,
    }

    send.finish().ok();
    Ok(())
}

async fn run_exec(
    request: ExecRequest,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    cmd_semaphore: &Semaphore,
    limits: &ServerLimits,
) -> Result<()> {
    let Some((program, args)) = request.command.split_first() else {
        write_error(send, ErrorCode::ExecStartFailed, "empty command").await?;
        return Ok(());
    };

    let _permit = cmd_semaphore.acquire().await;

    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    command.envs(filter_env(request.env));
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            warn!(?err, "command spawn failed");
            write_error(send, ErrorCode::ExecStartFailed, "failed to start command").await?;
            return Ok(());
        }
    };

    write_message(send, &Message::Response(Response::ExecStarted)).await?;

    let (tx, mut rx) = mpsc::channel::<(OutputStream, Vec<u8>)>(16);
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_output(stdout, OutputStream::Stdout, tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(read_output(stderr, OutputStream::Stderr, tx.clone()));
    }
    drop(tx);

    let mut status = None;
    let mut output_closed = false;
    let mut cancel_buf = [0u8; 1];
    let stopped = send.stopped();
    tokio::pin!(stopped);

    let timeout = Duration::from_secs(limits.max_command_secs);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    while status.is_none() || !output_closed {
        tokio::select! {
            output = rx.recv(), if !output_closed => {
                match output {
                    Some((stream, data)) => {
                        write_message(send, &Message::Response(Response::ExecOutput { stream, data })).await?;
                    }
                    None => output_closed = true,
                }
            }
            result = child.wait(), if status.is_none() => {
                status = Some(result.context("wait for remote command")?);
            }
            cancelled = recv.read(&mut cancel_buf), if status.is_none() => {
                debug!(?cancelled, "client request stream closed before command exit; killing remote process");
                let _ = child.kill().await;
                status = Some(child.wait().await.context("wait for cancelled remote command")?);
                output_closed = true;
            }
            stopped_result = &mut stopped, if status.is_none() => {
                debug!(?stopped_result, "client stopped receiving command output; killing remote process");
                let _ = child.kill().await;
                status = Some(child.wait().await.context("wait for stopped remote command")?);
                output_closed = true;
            }
            _ = &mut deadline, if status.is_none() => {
                warn!("command exceeded max runtime; killing remote process");
                let _ = child.kill().await;
                status = Some(child.wait().await.context("wait for timed-out remote command")?);
                output_closed = true;
            }
        }
    }

    let code = status.and_then(|status| status.code());
    write_message(send, &Message::Response(Response::ExecExit { code })).await?;
    Ok(())
}

fn filter_env(env: Vec<(String, String)>) -> Vec<(String, String)> {
    const BLOCKED_PREFIXES: &[&str] = &["LD_", "DYLD_"];
    const BLOCKED_SUBSTRINGS: &[&str] = &["PRELOAD", "LIBRARY_PATH"];

    env.into_iter()
        .filter(|(key, _)| {
            let upper = key.to_uppercase();
            !BLOCKED_PREFIXES.iter().any(|p| upper.starts_with(p))
                && !BLOCKED_SUBSTRINGS.iter().any(|s| upper.contains(s))
        })
        .collect()
}

async fn read_output<R>(
    mut reader: R,
    stream: OutputStream,
    tx: mpsc::Sender<(OutputStream, Vec<u8>)>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = vec![0u8; OUTPUT_CHUNK_SIZE];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(read) => {
                if tx.send((stream, buf[..read].to_vec())).await.is_err() {
                    break;
                }
            }
            Err(err) => {
                warn!(?err, "failed reading child output");
                break;
            }
        }
    }
}

async fn run_put(
    request: PutRequest,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    limits: &ServerLimits,
) -> Result<()> {
    if request.size > limits.max_file_size {
        write_error(
            send,
            ErrorCode::TransferDenied,
            format!(
                "declared upload size {} exceeds maximum {}",
                request.size, limits.max_file_size
            ),
        )
        .await?;
        return Ok(());
    }

    let remote = PathBuf::from(&request.remote_path);
    ensure_parent(&remote).await?;
    let tmp = temp_sibling(&remote, "upload");
    let mut file = File::create(&tmp)
        .await
        .with_context(|| format!("create remote temporary file {}", tmp.display()))?;

    write_message(send, &Message::Response(Response::PutReady)).await?;

    let mut received = 0u64;
    let mut hasher = Sha256::new();
    let result = async {
        loop {
            match read_message(recv).await? {
                Message::FileChunk(bytes) => {
                    received += bytes.len() as u64;
                    if received > request.size {
                        bail!("received more bytes than declared upload size");
                    }
                    if received > limits.max_file_size {
                        bail!("upload exceeded maximum file size {}", limits.max_file_size);
                    }
                    hasher.update(&bytes);
                    file.write_all(&bytes).await?;
                }
                Message::FileEnd => {
                    file.flush().await?;
                    drop(file);
                    if received != request.size {
                        bail!(
                            "upload byte count mismatch: declared={}, received={}",
                            request.size,
                            received
                        );
                    }
                    tokio::fs::rename(&tmp, &remote).await.with_context(|| {
                        format!(
                            "move uploaded file {} to {}",
                            tmp.display(),
                            remote.display()
                        )
                    })?;
                    break Ok(received);
                }
                other => bail!("unexpected upload message: {other:?}"),
            }
        }
    }
    .await;

    match result {
        Ok(bytes) => {
            let hash = hasher.finalize().to_vec();
            write_message(
                send,
                &Message::Response(Response::TransferDone {
                    bytes,
                    hash: Some(hash),
                }),
            )
            .await?
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            write_error(send, ErrorCode::TransferInterrupted, "transfer failed").await?;
            debug!(?err, "upload transfer error");
        }
    }
    Ok(())
}

async fn run_get(
    request: GetRequest,
    send: &mut iroh::endpoint::SendStream,
    limits: &ServerLimits,
) -> Result<()> {
    let remote = PathBuf::from(&request.remote_path);
    let metadata = match tokio::fs::metadata(&remote).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            write_error(
                send,
                ErrorCode::TransferNotFound,
                format!("remote file not found: {}", remote.display()),
            )
            .await?;
            return Ok(());
        }
        Err(err) => return Err(err).context("read remote file metadata"),
    };
    if !metadata.is_file() {
        write_error(
            send,
            ErrorCode::TransferDenied,
            format!("remote path is not a file: {}", remote.display()),
        )
        .await?;
        return Ok(());
    }
    if metadata.len() > limits.max_file_size {
        write_error(
            send,
            ErrorCode::TransferDenied,
            format!(
                "remote file size {} exceeds maximum {}",
                metadata.len(),
                limits.max_file_size
            ),
        )
        .await?;
        return Ok(());
    }

    write_message(
        send,
        &Message::Response(Response::GetMetadata {
            size: metadata.len(),
        }),
    )
    .await?;

    let mut file = File::open(&remote)
        .await
        .with_context(|| format!("open remote file {}", remote.display()))?;
    let mut buf = vec![0u8; FILE_CHUNK_SIZE];
    let mut sent = 0u64;
    let mut hasher = Sha256::new();
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        sent += read as u64;
        hasher.update(&buf[..read]);
        write_message(send, &Message::FileChunk(buf[..read].to_vec())).await?;
    }

    let hash = hasher.finalize().to_vec();
    write_message(
        send,
        &Message::Response(Response::TransferDone {
            bytes: sent,
            hash: Some(hash),
        }),
    )
    .await?;
    Ok(())
}

async fn write_error(
    send: &mut iroh::endpoint::SendStream,
    code: ErrorCode,
    message: impl Into<String>,
) -> Result<()> {
    write_message(
        send,
        &Message::Response(Response::Error(RemoteError::new(code, message))),
    )
    .await
}

fn default_data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir()
        .or_else(|| std::env::current_dir().ok())
        .context("determine default data directory")?;
    Ok(base.join("RemoText"))
}

async fn load_or_create_secret(data_dir: &Path) -> Result<SecretKey> {
    let path = data_dir.join("identity.key");
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => decode_secret(contents.trim()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let secret = SecretKey::generate();
            let encoded = URL_SAFE_NO_PAD.encode(secret.to_bytes());
            tokio::fs::write(&path, encoded)
                .await
                .with_context(|| format!("write server identity {}", path.display()))?;
            restrict_secret_file(&path).await?;
            Ok(secret)
        }
        Err(err) => Err(err).with_context(|| format!("read server identity {}", path.display())),
    }
}

fn decode_secret(encoded: &str) -> Result<SecretKey> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("decode server identity")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("server identity has invalid length"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

#[cfg(unix)]
async fn restrict_secret_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    tokio::fs::set_permissions(path, permissions)
        .await
        .with_context(|| format!("restrict identity file permissions {}", path.display()))
}

#[cfg(not(unix))]
async fn restrict_secret_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_exponential_backoff() {
        let limiter = AuthRateLimiter::new();
        assert!(limiter.should_wait().is_none());

        let d1 = limiter.check_and_record_failure();
        let d2 = limiter.check_and_record_failure();
        let d3 = limiter.check_and_record_failure();
        assert!(d3 >= d2);
        assert!(d2 >= d1);

        let wait = limiter.should_wait();
        assert!(wait.is_some());

        limiter.reset();
        assert!(limiter.should_wait().is_none());
    }

    #[test]
    fn filter_env_blocks_dangerous_vars() {
        let env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("LD_PRELOAD".to_string(), "/evil.so".to_string()),
            ("DYLD_LIBRARY_PATH".to_string(), "/evil".to_string()),
            ("HOME".to_string(), "/home/user".to_string()),
        ];
        let filtered = filter_env(env);
        let names: Vec<&str> = filtered.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"PATH"));
        assert!(names.contains(&"HOME"));
        assert!(!names.contains(&"LD_PRELOAD"));
        assert!(!names.contains(&"DYLD_LIBRARY_PATH"));
    }

    #[test]
    fn filter_env_blocks_preload_substring() {
        let env = vec![
            ("MY_PRELOAD_LIB".to_string(), "/evil.so".to_string()),
            ("SAFE_VAR".to_string(), "ok".to_string()),
        ];
        let filtered = filter_env(env);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "SAFE_VAR");
    }

    #[test]
    fn server_config_debug_redacts_password() {
        let config = ServerConfig {
            password: "secret123".to_string(),
            name: "test".to_string(),
            data_dir: None,
            network_mode: NetworkMode::LocalOnly,
            limits: None,
        };
        let debug = format!("{:?}", config);
        assert!(!debug.contains("secret123"));
        assert!(debug.contains("<redacted>"));
    }
}
