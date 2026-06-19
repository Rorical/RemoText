# RemoText Protocol Draft

## Status

This document defines the current RemoText protocol shape. The first interoperable version uses ALPN `remotext/1` and postcard-encoded length-prefixed frames.

## Transport

- Carrier: iroh QUIC connections.
- ALPN: `remotext/1`.
- Stream model: one authenticated connection can carry multiple operation streams.
- Operation stream: a bidirectional stream containing framed messages for one command or one transfer.

## Framing

Each frame should contain:

- 4-byte big-endian payload length.
- Postcard-encoded `Message` payload.

The payload contains the message type through serde enum tagging.

## Connection Handshake

Implemented sequence for each operation stream:

```text
client -> server: iroh connect with ALPN remotext/1
client -> server: ClientHello(version, opaque_credential_request)
server -> client: ServerHello(version, server_identity, opaque_credential_response)
client -> server: ClientRequest(opaque_credential_finalization, request_mac, request)
server -> client: Response(...) or Response(Error(...))
```

Authentication uses OPAQUE through `opaque-ke` with Ristretto255, TripleDH, SHA-512, and Argon2. The OPAQUE context binds the password-authenticated key exchange to:

- Server iroh identity.
- Protocol version.
- ALPN value.

The client must reject the handshake if `ServerHello.server_identity` does not match the iroh connection's authenticated remote endpoint identity.

After OPAQUE completes, both sides derive a per-login session key. `ClientRequest.request_mac` is an HMAC-SHA256 over the serialized `Request`, keyed by that OPAQUE session key and also bound to the server identity, ALPN, and protocol version. This prevents a valid PAKE login from being reused with a modified request.

## Capability Negotiation

Capabilities allow newer clients and servers to interoperate safely.

Initial capability flags:

- `exec.basic`: execute process with argv.
- `exec.stdin`: stream stdin to remote process.
- `exec.cancel`: cancel remote process.
- `file.put`: upload single file.
- `file.get`: download single file.
- `file.hash`: verify transfer hash.
- `session.keepalive`: reuse authenticated sessions.

## Command Execution Messages

The current implementation uses these message variants:

- `Request::Exec` to start a command.
- `Response::ExecStarted` once the child process starts.
- `Response::ExecOutput` for stdout and stderr chunks.
- `Message::Cancel` to request cancellation.
- `Response::ExecExit` for the final status.

### ExecRequest

Fields:

- Request id.
- Program and arguments.
- Optional working directory.
- Optional environment allowlist or overrides.
- Stdin mode.
- Timeout.
- Shell mode flag if explicitly requested.

### ExecStarted

Fields:

- Request id.
- Remote process id if safe to expose.
- Server start timestamp.

### ExecOutput

Fields:

- Request id.
- Stream kind: stdout or stderr.
- Byte chunk.

### ExecExit

Fields:

- Request id.
- Exit code when available.
- Signal or platform termination reason when available.
- Final error message if process failed before start.

### ExecCancel

Fields:

- Request id.
- Cancellation mode: graceful, interrupt, terminate, or kill.

## File Transfer Messages

### PutRequest

Fields:

- Request id.
- Remote path.
- File size.
- Optional modification time.
- Optional mode bits.
- Optional expected hash.

### PutReady

Fields:

- Request id.
- Chunk size limit.
- Temporary transfer id.

### FileChunk

Fields:

- Request id.
- Offset.
- Bytes.

### TransferComplete

Fields:

- Request id.
- Total bytes received.
- Final hash if calculated.

### GetRequest

Fields:

- Request id.
- Remote path.
- Optional byte range for future resume support.

### GetMetadata

Fields:

- Request id.
- File size.
- Optional modification time.
- Optional mode bits.
- Optional hash.

## Error Model

Every operation can return an error frame with:

- Stable error code.
- Human-readable message.
- Retryable flag.
- Optional platform error code.

Initial error codes:

- `AUTH_FAILED`.
- `VERSION_UNSUPPORTED`.
- `CAPABILITY_UNSUPPORTED`.
- `EXEC_START_FAILED`.
- `EXEC_TIMEOUT`.
- `TRANSFER_DENIED`.
- `TRANSFER_NOT_FOUND`.
- `TRANSFER_INTERRUPTED`.
- `INTERNAL`.

## Backpressure

The implementation must not read unlimited remote output or file chunks into memory. Network reads should be coupled to local writes. If stdout, stderr, or file output blocks, the corresponding iroh stream should naturally apply backpressure.

## Compatibility Rules

- Breaking protocol changes require a new ALPN suffix or major protocol version.
- Unknown optional capabilities are ignored.
- Unknown required capabilities cause `CAPABILITY_UNSUPPORTED`.
- Error codes are stable once released.
