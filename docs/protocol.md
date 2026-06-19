# RemoText Protocol Draft

## Status

This document defines the intended RemoText protocol shape. It is a draft and should be versioned before the first interoperable release.

## Transport

- Carrier: iroh QUIC connections.
- ALPN: `remotext/1`.
- Stream model: one authenticated connection can carry multiple operation streams.
- Operation stream: a bidirectional stream containing framed messages for one command or one transfer.

## Framing

Each frame should contain:

- Magic or protocol marker for early validation.
- Protocol version.
- Request id.
- Message type.
- Payload length.
- Payload bytes.

The exact encoding is not finalized. Use a binary encoding for runtime messages and reserve JSON for diagnostics or CLI `--json` output.

## Connection Handshake

Planned sequence:

```text
client -> server: iroh connect with ALPN remotext/1
server -> client: ServerHello(version, server_nonce, server_identity, auth_params)
client -> server: ClientHello(version, client_nonce, auth_proof, client_caps)
server -> client: AuthResult(session_id, server_caps) or AuthFailure(reason)
```

The authentication proof must bind the password-derived secret to:

- Server nonce.
- Client nonce.
- Server iroh identity.
- Protocol version.
- ALPN value.
- Authentication parameter set.

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
