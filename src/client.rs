use std::{fmt, future::Future, path::Path};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
};
use zeroize::Zeroize;

use crate::{
    PROTOCOL_ALPN, PROTOCOL_VERSION, auth,
    files::{ensure_parent, temp_sibling},
    framing::{read_message, write_message},
    protocol::{
        ClientHello, ClientRequest, ErrorCode, ExecRequest, FILE_CHUNK_SIZE, GetRequest, Message,
        OutputStream, PutRequest, RemoteError, Request, Response,
    },
    server::NetworkMode,
};

#[derive(Clone)]
pub struct Client {
    addr: EndpointAddr,
    password: String,
    network_mode: NetworkMode,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("addr", &self.addr)
            .field("password", &"<redacted>")
            .field("network_mode", &self.network_mode)
            .finish()
    }
}

#[derive(Clone)]
pub struct PersistentClient {
    _endpoint: Endpoint,
    conn: iroh::endpoint::Connection,
    password: String,
}

impl fmt::Debug for PersistentClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistentClient")
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl Drop for PersistentClient {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

struct RequestIo {
    _endpoint: Option<Endpoint>,
    conn: iroh::endpoint::Connection,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    close_connection: bool,
    completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Client {
    pub fn new(addr: EndpointAddr, password: impl Into<String>, network_mode: NetworkMode) -> Self {
        Self {
            addr,
            password: password.into(),
            network_mode,
        }
    }

    pub async fn connect_persistent(&self) -> Result<PersistentClient> {
        PersistentClient::connect(self.addr.clone(), self.password.clone(), self.network_mode).await
    }

    pub async fn ping(&self) -> Result<()> {
        ping_io(self.open_request(Request::Ping).await?).await
    }

    pub async fn exec_collect(&self, command: Vec<String>) -> Result<ExecResult> {
        let request = exec_request(command);
        exec_collect_io(self.open_request(Request::Exec(request)).await?).await
    }

    pub async fn exec_collect_with_cancel<F>(
        &self,
        command: Vec<String>,
        cancel: F,
    ) -> Result<ExecResult>
    where
        F: Future<Output = ()>,
    {
        let request = exec_request(command);
        exec_collect_io_with_cancel(self.open_request(Request::Exec(request)).await?, cancel).await
    }

    pub async fn exec_stream<W1, W2>(
        &self,
        command: Vec<String>,
        stdout: &mut W1,
        stderr: &mut W2,
    ) -> Result<i32>
    where
        W1: AsyncWrite + Unpin,
        W2: AsyncWrite + Unpin,
    {
        let request = exec_request(command);
        exec_stream_io(
            self.open_request(Request::Exec(request)).await?,
            stdout,
            stderr,
        )
        .await
    }

    pub async fn exec_stream_with_cancel<W1, W2, F>(
        &self,
        command: Vec<String>,
        stdout: &mut W1,
        stderr: &mut W2,
        cancel: F,
    ) -> Result<i32>
    where
        W1: AsyncWrite + Unpin,
        W2: AsyncWrite + Unpin,
        F: Future<Output = ()>,
    {
        let request = exec_request(command);
        exec_stream_io_with_cancel(
            self.open_request(Request::Exec(request)).await?,
            stdout,
            stderr,
            cancel,
        )
        .await
    }

    pub async fn put(&self, local: &Path, remote: &str) -> Result<u64> {
        let metadata = local_file_metadata(local).await?;
        put_io(
            self.open_request(Request::Put(PutRequest {
                remote_path: remote.to_string(),
                size: metadata.len(),
            }))
            .await?,
            local,
            metadata.len(),
        )
        .await
    }

    pub async fn get(&self, remote: &str, local: &Path) -> Result<u64> {
        get_io(
            self.open_request(Request::Get(GetRequest {
                remote_path: remote.to_string(),
            }))
            .await?,
            local,
        )
        .await
    }

    async fn open_request(&self, request: Request) -> Result<RequestIo> {
        let endpoint = bind_client_endpoint(self.network_mode).await?;
        let conn = endpoint
            .connect(self.addr.clone(), PROTOCOL_ALPN)
            .await
            .context("connect to RemoText server")?;
        let (send, recv) = open_authenticated_stream(&conn, &self.password, request).await?;
        Ok(RequestIo {
            _endpoint: Some(endpoint),
            conn,
            send,
            recv,
            close_connection: true,
            completed: false,
        })
    }
}

impl PersistentClient {
    pub async fn connect(
        addr: EndpointAddr,
        password: impl Into<String>,
        network_mode: NetworkMode,
    ) -> Result<Self> {
        let endpoint = bind_client_endpoint(network_mode).await?;
        let conn = endpoint
            .connect(addr, PROTOCOL_ALPN)
            .await
            .context("connect persistent RemoText session")?;
        Ok(Self {
            _endpoint: endpoint,
            conn,
            password: password.into(),
        })
    }

    pub async fn ping(&self) -> Result<()> {
        ping_io(self.open_request(Request::Ping).await?).await
    }

    pub async fn exec_collect(&self, command: Vec<String>) -> Result<ExecResult> {
        exec_collect_io(
            self.open_request(Request::Exec(exec_request(command)))
                .await?,
        )
        .await
    }

    pub async fn exec_stream<W1, W2>(
        &self,
        command: Vec<String>,
        stdout: &mut W1,
        stderr: &mut W2,
    ) -> Result<i32>
    where
        W1: AsyncWrite + Unpin,
        W2: AsyncWrite + Unpin,
    {
        exec_stream_io(
            self.open_request(Request::Exec(exec_request(command)))
                .await?,
            stdout,
            stderr,
        )
        .await
    }

    pub async fn exec_stream_with_cancel<W1, W2, F>(
        &self,
        command: Vec<String>,
        stdout: &mut W1,
        stderr: &mut W2,
        cancel: F,
    ) -> Result<i32>
    where
        W1: AsyncWrite + Unpin,
        W2: AsyncWrite + Unpin,
        F: Future<Output = ()>,
    {
        exec_stream_io_with_cancel(
            self.open_request(Request::Exec(exec_request(command)))
                .await?,
            stdout,
            stderr,
            cancel,
        )
        .await
    }

    pub async fn put(&self, local: &Path, remote: &str) -> Result<u64> {
        let metadata = local_file_metadata(local).await?;
        put_io(
            self.open_request(Request::Put(PutRequest {
                remote_path: remote.to_string(),
                size: metadata.len(),
            }))
            .await?,
            local,
            metadata.len(),
        )
        .await
    }

    pub async fn get(&self, remote: &str, local: &Path) -> Result<u64> {
        get_io(
            self.open_request(Request::Get(GetRequest {
                remote_path: remote.to_string(),
            }))
            .await?,
            local,
        )
        .await
    }

    async fn open_request(&self, request: Request) -> Result<RequestIo> {
        let (send, recv) = open_authenticated_stream(&self.conn, &self.password, request).await?;
        Ok(RequestIo {
            _endpoint: None,
            conn: self.conn.clone(),
            send,
            recv,
            close_connection: false,
            completed: false,
        })
    }
}

async fn open_authenticated_stream(
    conn: &iroh::endpoint::Connection,
    password: &str,
    request: Request,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    let (mut send, mut recv) = conn.open_bi().await.context("open request stream")?;

    let login = auth::ClientLoginStart::new(password)?;
    write_message(
        &mut send,
        &Message::ClientHello(ClientHello {
            version: PROTOCOL_VERSION,
            credential_request: login.credential_request().to_vec(),
        }),
    )
    .await?;

    let server_hello = match read_message(&mut recv).await? {
        Message::ServerHello(server_hello) => server_hello,
        Message::Response(Response::Error(err)) => return Err(remote_error(err)),
        other => bail!("unexpected handshake response: {other:?}"),
    };

    if server_hello.version != PROTOCOL_VERSION {
        bail!(
            "server protocol version {} is unsupported by client version {}",
            server_hello.version,
            PROTOCOL_VERSION
        );
    }

    let remote_id = *conn.remote_id().as_bytes();
    if server_hello.server_id != remote_id {
        bail!("server identity in PAKE handshake does not match iroh connection identity");
    }

    let (credential_finalization, request_mac) = login.finish(
        password,
        &server_hello.server_id,
        &server_hello.credential_response,
        &request,
    )?;
    write_message(
        &mut send,
        &Message::ClientRequest(ClientRequest {
            credential_finalization,
            request_mac,
            request,
        }),
    )
    .await?;

    Ok((send, recv))
}

async fn ping_io(mut io: RequestIo) -> Result<()> {
    match read_message(&mut io.recv).await? {
        Message::Response(Response::Pong) => {}
        Message::Response(Response::Error(err)) => return Err(remote_error(err)),
        other => bail!("unexpected ping response: {other:?}"),
    }
    io.finish();
    Ok(())
}

async fn exec_collect_io(mut io: RequestIo) -> Result<ExecResult> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = loop {
        match read_message(&mut io.recv).await? {
            Message::Response(Response::ExecStarted) => {}
            Message::Response(Response::ExecOutput { stream, data }) => match stream {
                OutputStream::Stdout => stdout.extend_from_slice(&data),
                OutputStream::Stderr => stderr.extend_from_slice(&data),
            },
            Message::Response(Response::ExecExit { code }) => break code.unwrap_or(1),
            Message::Response(Response::Error(err)) => return Err(remote_error(err)),
            other => bail!("unexpected exec response: {other:?}"),
        }
    };

    io.finish();
    Ok(ExecResult {
        code,
        stdout,
        stderr,
    })
}

async fn exec_collect_io_with_cancel<F>(mut io: RequestIo, cancel: F) -> Result<ExecResult>
where
    F: Future<Output = ()>,
{
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut cancelled = false;
    tokio::pin!(cancel);

    let code = loop {
        tokio::select! {
            message = read_message(&mut io.recv) => {
                match message? {
                    Message::Response(Response::ExecStarted) => {}
                    Message::Response(Response::ExecOutput { stream, data }) => match stream {
                        OutputStream::Stdout => stdout.extend_from_slice(&data),
                        OutputStream::Stderr => stderr.extend_from_slice(&data),
                    },
                    Message::Response(Response::ExecExit { code }) => break code.unwrap_or(1),
                    Message::Response(Response::Error(err)) => return Err(remote_error(err)),
                    other => bail!("unexpected exec response: {other:?}"),
                }
            }
            _ = &mut cancel, if !cancelled => {
                write_message(&mut io.send, &Message::Cancel).await?;
                cancelled = true;
            }
        }
    };

    io.finish();
    Ok(ExecResult {
        code,
        stdout,
        stderr,
    })
}

async fn exec_stream_io<W1, W2>(mut io: RequestIo, stdout: &mut W1, stderr: &mut W2) -> Result<i32>
where
    W1: AsyncWrite + Unpin,
    W2: AsyncWrite + Unpin,
{
    let code = loop {
        match read_message(&mut io.recv).await? {
            Message::Response(Response::ExecStarted) => {}
            Message::Response(Response::ExecOutput { stream, data }) => match stream {
                OutputStream::Stdout => {
                    stdout.write_all(&data).await?;
                    stdout.flush().await?;
                }
                OutputStream::Stderr => {
                    stderr.write_all(&data).await?;
                    stderr.flush().await?;
                }
            },
            Message::Response(Response::ExecExit { code }) => break code.unwrap_or(1),
            Message::Response(Response::Error(err)) => return Err(remote_error(err)),
            other => bail!("unexpected exec response: {other:?}"),
        }
    };

    io.finish();
    Ok(code)
}

async fn exec_stream_io_with_cancel<W1, W2, F>(
    mut io: RequestIo,
    stdout: &mut W1,
    stderr: &mut W2,
    cancel: F,
) -> Result<i32>
where
    W1: AsyncWrite + Unpin,
    W2: AsyncWrite + Unpin,
    F: Future<Output = ()>,
{
    let mut cancelled = false;
    tokio::pin!(cancel);

    let code = loop {
        tokio::select! {
            message = read_message(&mut io.recv) => {
                match message? {
                    Message::Response(Response::ExecStarted) => {}
                    Message::Response(Response::ExecOutput { stream, data }) => match stream {
                        OutputStream::Stdout => {
                            stdout.write_all(&data).await?;
                            stdout.flush().await?;
                        }
                        OutputStream::Stderr => {
                            stderr.write_all(&data).await?;
                            stderr.flush().await?;
                        }
                    },
                    Message::Response(Response::ExecExit { code }) => break code.unwrap_or(1),
                    Message::Response(Response::Error(err)) => return Err(remote_error(err)),
                    other => bail!("unexpected exec response: {other:?}"),
                }
            }
            _ = &mut cancel, if !cancelled => {
                write_message(&mut io.send, &Message::Cancel).await?;
                cancelled = true;
            }
        }
    };

    io.finish();
    Ok(code)
}

async fn put_io(mut io: RequestIo, local: &Path, expected_len: u64) -> Result<u64> {
    match read_message(&mut io.recv).await? {
        Message::Response(Response::PutReady) => {}
        Message::Response(Response::Error(err)) => return Err(remote_error(err)),
        other => bail!("unexpected put response: {other:?}"),
    }

    let mut file = File::open(local)
        .await
        .with_context(|| format!("open local file {}", local.display()))?;
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
        write_message(&mut io.send, &Message::FileChunk(buf[..read].to_vec())).await?;
    }
    write_message(&mut io.send, &Message::FileEnd).await?;

    let transferred = match read_message(&mut io.recv).await? {
        Message::Response(Response::TransferDone { bytes, hash }) => {
            if let Some(server_hash) = &hash {
                let local_hash = hasher.finalize().to_vec();
                if server_hash != &local_hash {
                    bail!("upload hash mismatch: server received corrupt data");
                }
            }
            bytes
        }
        Message::Response(Response::Error(err)) => return Err(remote_error(err)),
        other => bail!("unexpected put completion response: {other:?}"),
    };
    if transferred != sent || transferred != expected_len {
        bail!(
            "upload byte count mismatch: local={}, sent={}, remote={}",
            expected_len,
            sent,
            transferred
        );
    }

    io.finish();
    Ok(transferred)
}

async fn get_io(mut io: RequestIo, local: &Path) -> Result<u64> {
    let expected = match read_message(&mut io.recv).await? {
        Message::Response(Response::GetMetadata { size }) => size,
        Message::Response(Response::Error(err)) => return Err(remote_error(err)),
        other => bail!("unexpected get metadata response: {other:?}"),
    };

    ensure_parent(local).await?;
    let tmp = temp_sibling(local, "download");
    let mut file = File::create(&tmp)
        .await
        .with_context(|| format!("create local temporary file {}", tmp.display()))?;
    let mut received = 0u64;
    let mut hasher = Sha256::new();

    let result = async {
        loop {
            match read_message(&mut io.recv).await? {
                Message::FileChunk(bytes) => {
                    received += bytes.len() as u64;
                    hasher.update(&bytes);
                    file.write_all(&bytes).await?;
                }
                Message::Response(Response::TransferDone { bytes, hash }) => {
                    file.flush().await?;
                    drop(file);
                    if bytes != received || bytes != expected {
                        bail!(
                            "download byte count mismatch: expected={expected}, received={received}, remote={bytes}"
                        );
                    }
                    if let Some(server_hash) = &hash {
                        let local_hash = hasher.finalize().to_vec();
                        if server_hash != &local_hash {
                            bail!("download hash mismatch: received corrupt data");
                        }
                    }
                    tokio::fs::rename(&tmp, local).await.with_context(|| {
                        format!(
                            "move downloaded file {} to {}",
                            tmp.display(),
                            local.display()
                        )
                    })?;
                    break Ok(bytes);
                }
                Message::Response(Response::Error(err)) => break Err(remote_error(err)),
                other => bail!("unexpected get response: {other:?}"),
            }
        }
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }

    io.finish();
    result
}

impl RequestIo {
    fn finish(&mut self) {
        self.completed = true;
        self.send.finish().ok();
        if self.close_connection {
            self.conn.close(0u8.into(), b"done");
        }
    }
}

impl Drop for RequestIo {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.send.reset(1u8.into()).ok();
        self.recv.stop(1u8.into()).ok();
        if self.close_connection {
            self.conn.close(1u8.into(), b"cancelled");
        }
    }
}

async fn bind_client_endpoint(network_mode: NetworkMode) -> Result<Endpoint> {
    match network_mode {
        NetworkMode::Public => Endpoint::bind(presets::N0)
            .await
            .context("bind iroh client endpoint"),
        NetworkMode::LocalOnly => Endpoint::bind(presets::Minimal)
            .await
            .context("bind local-only iroh client endpoint"),
    }
}

async fn local_file_metadata(local: &Path) -> Result<std::fs::Metadata> {
    let metadata = tokio::fs::metadata(local)
        .await
        .with_context(|| format!("read local file metadata {}", local.display()))?;
    if !metadata.is_file() {
        bail!("local path is not a file: {}", local.display());
    }
    Ok(metadata)
}

fn exec_request(command: Vec<String>) -> ExecRequest {
    ExecRequest {
        command,
        cwd: None,
        env: Vec::new(),
    }
}

fn remote_error(error: RemoteError) -> anyhow::Error {
    let prefix = match error.code {
        ErrorCode::AuthFailed => "authentication failed",
        ErrorCode::VersionUnsupported => "protocol version unsupported",
        ErrorCode::Protocol => "protocol error",
        ErrorCode::ExecStartFailed => "remote command failed to start",
        ErrorCode::TransferDenied => "transfer denied",
        ErrorCode::TransferNotFound => "remote path not found",
        ErrorCode::TransferInterrupted => "transfer interrupted",
        ErrorCode::Internal => "remote internal error",
    };
    anyhow::anyhow!("{prefix}: {}", error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_debug_redacts_password() {
        let key = iroh::SecretKey::generate();
        let addr = iroh::EndpointAddr::new(key.public());
        let client = Client::new(addr, "secret123", NetworkMode::LocalOnly);
        let debug = format!("{:?}", client);
        assert!(!debug.contains("secret123"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn persistent_client_debug_redacts_password() {
        let key = iroh::SecretKey::generate();
        let addr = iroh::EndpointAddr::new(key.public());
        let client = Client::new(addr, "secret123", NetworkMode::LocalOnly);
        let debug = format!("{:?}", client);
        assert!(debug.contains("<redacted>"));
    }
}
