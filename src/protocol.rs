use serde::{Deserialize, Serialize};

pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;
pub const FILE_CHUNK_SIZE: usize = 64 * 1024;
pub const OUTPUT_CHUNK_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Message {
    ClientHello(ClientHello),
    ServerHello(ServerHello),
    ClientRequest(ClientRequest),
    Cancel,
    Response(Response),
    FileChunk(Vec<u8>),
    FileEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientHello {
    pub version: u16,
    pub client_nonce: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerHello {
    pub version: u16,
    pub server_nonce: [u8; 32],
    pub server_id: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientRequest {
    pub proof: [u8; 32],
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Request {
    Ping,
    Exec(ExecRequest),
    Put(PutRequest),
    Get(GetRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRequest {
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PutRequest {
    pub remote_path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetRequest {
    pub remote_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Response {
    Pong,
    PutReady,
    GetMetadata { size: u64 },
    TransferDone { bytes: u64 },
    ExecStarted,
    ExecOutput { stream: OutputStream, data: Vec<u8> },
    ExecExit { code: Option<i32> },
    Error(RemoteError),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    AuthFailed,
    VersionUnsupported,
    Protocol,
    ExecStartFailed,
    TransferDenied,
    TransferNotFound,
    TransferInterrupted,
    Internal,
}

impl RemoteError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
