use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};
use tracing::{debug, warn};

use crate::{
    PROTOCOL_ALPN, PROTOCOL_VERSION, auth,
    files::{ensure_parent, temp_sibling},
    framing::{read_message, write_message},
    protocol::{
        ErrorCode, ExecRequest, FILE_CHUNK_SIZE, GetRequest, Message, OUTPUT_CHUNK_SIZE,
        OutputStream, PutRequest, RemoteError, Response,
    },
    ticket,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Public,
    LocalOnly,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub password: String,
    pub name: String,
    pub data_dir: Option<PathBuf>,
    pub network_mode: NetworkMode,
}

pub struct Server {
    endpoint: Endpoint,
    auth: Arc<auth::ServerAuth>,
    name: String,
    data_dir: PathBuf,
}

impl Server {
    pub async fn bind(config: ServerConfig) -> Result<Self> {
        if config.password.is_empty() {
            bail!("server password must not be empty");
        }

        let data_dir = match config.data_dir {
            Some(path) => path,
            None => default_data_dir()?,
        };
        tokio::fs::create_dir_all(&data_dir)
            .await
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        let secret_key = load_or_create_secret(&data_dir).await?;
        let server_id = *secret_key.public().as_bytes();
        let auth = Arc::new(auth::ServerAuth::new(&config.password, server_id)?);

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

        Ok(Self {
            endpoint,
            auth,
            name: config.name,
            data_dir,
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
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        break;
                    };
                    let auth = auth.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_incoming(incoming, auth).await {
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
) -> Result<()> {
    let conn = incoming.await.context("accept iroh connection")?;
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let auth = auth.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_stream(send, recv, auth).await {
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
) -> Result<()> {
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
        write_error(&mut send, ErrorCode::AuthFailed, "invalid password").await?;
        return Ok(());
    }

    match request.request {
        crate::protocol::Request::Ping => {
            write_message(&mut send, &Message::Response(Response::Pong)).await?;
        }
        crate::protocol::Request::Exec(request) => run_exec(request, &mut send, &mut recv).await?,
        crate::protocol::Request::Put(request) => run_put(request, &mut send, &mut recv).await?,
        crate::protocol::Request::Get(request) => run_get(request, &mut send).await?,
    }

    send.finish().ok();
    Ok(())
}

async fn run_exec(
    request: ExecRequest,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<()> {
    let Some((program, args)) = request.command.split_first() else {
        write_error(send, ErrorCode::ExecStartFailed, "empty command").await?;
        return Ok(());
    };

    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    command.envs(request.env);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            write_error(send, ErrorCode::ExecStartFailed, err.to_string()).await?;
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
        }
    }

    let code = status.and_then(|status| status.code());
    write_message(send, &Message::Response(Response::ExecExit { code })).await?;
    Ok(())
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
) -> Result<()> {
    let remote = PathBuf::from(&request.remote_path);
    ensure_parent(&remote).await?;
    let tmp = temp_sibling(&remote, "upload");
    let mut file = File::create(&tmp)
        .await
        .with_context(|| format!("create remote temporary file {}", tmp.display()))?;

    write_message(send, &Message::Response(Response::PutReady)).await?;

    let mut received = 0u64;
    let result = async {
        loop {
            match read_message(recv).await? {
                Message::FileChunk(bytes) => {
                    received += bytes.len() as u64;
                    if received > request.size {
                        bail!("received more bytes than declared upload size");
                    }
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
            write_message(send, &Message::Response(Response::TransferDone { bytes })).await?
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            write_error(send, ErrorCode::TransferInterrupted, err.to_string()).await?;
        }
    }
    Ok(())
}

async fn run_get(request: GetRequest, send: &mut iroh::endpoint::SendStream) -> Result<()> {
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
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        sent += read as u64;
        write_message(send, &Message::FileChunk(buf[..read].to_vec())).await?;
    }

    write_message(
        send,
        &Message::Response(Response::TransferDone { bytes: sent }),
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
