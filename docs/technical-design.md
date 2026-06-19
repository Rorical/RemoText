# RemoText Technical Design

## Current State

The repository currently contains a Rust CLI scaffold. The CLI commands, dependency selection, and documentation are in place. The iroh server runtime, client session manager, authentication handshake, command execution, and file transfer protocol are still pending implementation.

## Technology Choices

- Language: Rust 2024 edition.
- Async runtime: Tokio.
- CLI parser: Clap.
- Network layer: `iroh` 1.0.0.
- Transport security: iroh QUIC transport security plus RemoText application authentication.
- Serialization: to be finalized. A compact binary format such as postcard or bincode is suitable for framed protocol messages; JSON can be reserved for CLI output.

## High-Level Architecture

```text
+-------------------+        local IPC         +--------------------------+
| remotext CLI      | <----------------------> | client session manager   |
+-------------------+                          +--------------------------+
        |                                                   |
        | one-shot fallback                                 | iroh QUIC
        v                                                   v
+-------------------+                          +--------------------------+
| direct client     | -----------------------> | remotext server runtime  |
+-------------------+                          +--------------------------+
                                                   |
                                                   v
                                      +----------------------------+
                                      | command and file executor  |
                                      +----------------------------+
```

## Components

### CLI Frontend

The CLI frontend owns argument parsing, environment variable integration, terminal behavior, and process exit codes. It should stay thin and delegate network work to either the local session manager or a direct client fallback.

Primary commands:

- `server`: starts the remote agent on the current machine.
- `connect`: opens or warms a persistent connection to a server.
- `exec`: executes a remote command.
- `put`: uploads a local file.
- `get`: downloads a remote file.

### Client Session Manager

The session manager is a per-user background helper. Its purpose is to make one-line commands behave like `sshpass` while still reusing a long-lived iroh connection.

Responsibilities:

- Maintain authenticated connections keyed by server address and local user.
- Accept local requests from `remotext exec`, `put`, and `get` over platform-local IPC.
- Keep sessions alive for the configured idle timeout.
- Shut down when no sessions remain.
- Avoid exposing passwords over local IPC after initial authentication.

Initial IPC options:

- Unix domain socket on Linux and macOS.
- Named pipe on Windows.
- Loopback TCP only as a fallback because it requires stricter binding and authentication checks.

### Server Runtime

The server runtime owns the iroh endpoint, authentication, protocol stream handling, and local execution policy.

Responsibilities:

- Create or load the server node identity.
- Bind an iroh endpoint with the RemoText ALPN value.
- Print a connection ticket or address for clients.
- Authenticate clients before accepting command or file transfer requests.
- Enforce concurrency, runtime, transfer size, and path policy limits.
- Emit structured logs without leaking secrets.

### Network Layer

RemoText uses iroh for direct peer-to-peer connectivity, hole punching, relay fallback, node addressing, and QUIC streams. The server address should be represented as an iroh ticket or a compact string containing the node identity and any relay or discovery information required by iroh.

Implementation principles:

- Use a dedicated ALPN, currently planned as `remotext/1`.
- Use separate bidirectional QUIC streams for logical operations.
- Keep command stdout and stderr as independent protocol streams or independent framed channels.
- Apply backpressure from local stdout, stderr, and file writers to network reads.

### Authentication

The password is an application-level shared secret used to authorize clients after an iroh connection is established. It must not be transmitted directly.

Recommended flow for MVP:

- Server stores a salted password verifier, not the raw password, when persistence is enabled.
- Client opens an iroh connection with the RemoText ALPN.
- Server sends a nonce, server identity, protocol version, and KDF parameters.
- Client derives a proof from the password and nonce.
- Server verifies the proof and returns a short-lived session token.
- All later requests on that connection reference the authenticated session state rather than resending the password.

The exact primitive should be selected during implementation. If a mature PAKE crate is chosen, prefer PAKE over ad-hoc challenge-response. If using HMAC challenge-response, include server identity, client nonce, server nonce, protocol version, and transcript hash in the proof to reduce replay and confusion risk.

### Command Execution

Command execution is platform-specific at the process boundary but protocol-level behavior should remain identical.

Execution model:

- Client sends command arguments, optional working directory, optional environment overrides, stdin mode, terminal mode, and timeout.
- Server starts a child process using platform-native process APIs.
- Server streams stdout and stderr back to the client.
- Server sends a final exit status frame after all output streams close.
- Client exits with the same numeric exit code when available.

Shell behavior:

- By default, treat the command as an executable plus arguments.
- Provide explicit shell helpers later, for example `--shell`, to avoid surprising quoting differences.
- Document platform examples for `cmd /C`, `powershell -NoProfile -Command`, `sh -lc`, and `bash -lc`.

### File Transfer

File transfer should use streaming and avoid full-file buffering.

Upload flow:

- Client sends metadata and desired remote path.
- Server validates path policy and available space when possible.
- Client streams file chunks.
- Server writes to a temporary path.
- Server verifies size and optional hash.
- Server atomically renames the temporary file to the final path.

Download flow:

- Client requests a remote path.
- Server validates read policy.
- Server sends metadata.
- Server streams file chunks.
- Client writes to a temporary path and renames on success.

Future resume support can use chunk hashes or byte ranges, but MVP should first provide reliable single-pass transfer.

### Local State

Server state:

- iroh node secret key.
- Password verifier and KDF metadata.
- Optional server configuration.
- Transfer staging directory.

Client state:

- Session manager socket or pipe path.
- Cached authenticated connection metadata.
- Optional known-server trust records.

Default directories should use platform conventions:

- Linux: `$XDG_DATA_HOME/remotext` or `$HOME/.local/share/remotext`.
- macOS: `$HOME/Library/Application Support/RemoText`.
- Windows: `%APPDATA%\\RemoText`.

### Error Handling

Errors should be specific and stable enough for scripts.

Recommended categories:

- Connection failed.
- Authentication failed.
- Protocol version mismatch.
- Command start failed.
- Command exited non-zero.
- Transfer source not found.
- Transfer destination denied.
- Transfer interrupted.
- Local session manager unavailable.

### Observability

- Use structured logging through `tracing`.
- Default logs should describe connection lifecycle and errors.
- Command output is user data and should only go to stdout and stderr for the active request.
- Secrets must be redacted in all logs.

## Build And Packaging Plan

- Keep one binary named `remotext`.
- Provide release archives per platform.
- Add CI builds for Windows, Linux, and macOS.
- Evaluate static or mostly-static Linux builds after iroh dependencies are validated.
- Add install helpers only after portable foreground mode is stable.

## Implementation Order

1. Wire iroh endpoint startup in `server` and print a real address or ticket.
2. Implement direct client dial and protocol version handshake.
3. Add password authentication.
4. Implement `exec` without session manager.
5. Implement file upload and download.
6. Add local client session manager and connection reuse.
7. Add service installation helpers and CI release packaging.
