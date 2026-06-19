# RemoText Requirements

## Product Summary

RemoText is a portable, lightweight remote command execution agent. It provides an SSH-like operational model without requiring users to know the server IP address or configure port forwarding. The server starts on a machine, prints a connection address and password, and a client can use those values to execute commands or transfer files.

The product is intentionally command-line only. It should be easy to copy as a single binary, run temporarily, or install as a background service depending on the target environment.

## Goals

- Provide remote command execution across Windows, Linux, and macOS.
- Use `iroh` as the network layer for peer-to-peer QUIC connectivity, relay fallback, NAT traversal, and addressing by node identity or ticket.
- Avoid GUI dependencies and avoid heavyweight runtime requirements.
- Support one-line command execution similar to `sshpass` for automation.
- Keep client connections warm in the background so repeated one-line commands do not pay the full connection setup cost every time.
- Support reliable file upload and download with progress, resume hooks, and atomic destination behavior where possible.
- Keep the security model simple enough for portable use while avoiding plaintext password transmission.

## Non-Goals

- RemoText is not a full SSH server replacement in the first version.
- RemoText does not provide graphical remote desktop capabilities.
- RemoText does not implement multi-user Unix account management in the first version.
- RemoText does not expose an unauthenticated HTTP API.
- RemoText does not require a central RemoText account service for the core direct connection flow.

## Supported Platforms

- Windows 10 and newer on x86_64 and aarch64 where Rust and iroh support the target.
- Linux on x86_64 and aarch64 using glibc targets first, with musl evaluated for static portability.
- macOS on x86_64 and Apple Silicon.

## User Roles

- Server operator: starts `remotext server`, shares the generated address and password, and controls local execution policy.
- Client operator: connects with the address and password to execute commands or transfer files.
- Automation user: runs single-line `remotext exec` or transfer commands from scripts and CI-like jobs.

## Core Functional Requirements

### Server Mode

- `remotext server --password <password>` starts a server runtime.
- Server mode initializes or loads a stable iroh node identity.
- Server mode prints a connection address or ticket that contains enough information for the client to dial the server through iroh.
- Server mode prints only non-secret connection information by default.
- Server mode accepts authenticated clients and rejects unauthenticated clients.
- Server mode can run in foreground for portable use.
- Server mode should later support service installation for Windows services, systemd, and launchd.

### Client Connection

- Client commands accept `--addr` and `--password`.
- `REMOTEXT_ADDR` and `REMOTEXT_PASSWORD` provide non-interactive configuration.
- `remotext connect` opens or warms a persistent local background session.
- One-line commands automatically start or reuse the local background session.
- Connection reuse is keyed by server address, authenticated identity, and local user.
- Idle sessions expire after a configurable keepalive timeout.

### Remote Command Execution

- `remotext exec --addr <addr> --password <password> -- <command> [args...]` executes a command remotely.
- Stdout and stderr stream back to the client independently.
- The client process exits with the remote exit code when possible.
- The protocol reports command start failure separately from command exit failure.
- Ctrl+C on the client should send a cancellation signal to the remote process.
- Server policy can restrict shell use, working directory, environment variables, and maximum runtime.

### File Transfer

- `remotext put <local> <remote>` uploads a file.
- `remotext get <remote> <local>` downloads a file.
- Transfers should use chunked streaming over iroh QUIC streams.
- The receiver writes to a temporary file and atomically renames on successful completion when supported by the platform.
- Transfer metadata includes file size, modification time when available, executable bit on Unix when applicable, and content hash when requested.
- Directory transfer can be added after single-file transfer is stable.

### Script-Friendly Operation

- Commands support environment variables for address and password.
- Commands are non-interactive unless a prompt is explicitly requested.
- Output defaults to raw remote stdout and stderr for `exec` so shell pipelines work.
- Machine-readable output should be available with a future `--json` flag for connection and transfer metadata.

## Non-Functional Requirements

- Startup should be fast enough for ad-hoc command usage.
- Idle resource usage should remain low for background client sessions.
- The binary should avoid external runtime dependencies.
- The protocol must include version negotiation.
- The implementation must handle interrupted network links and return clear errors.
- Logs must not include passwords, derived authentication secrets, or command output unless explicitly requested.

## Security Requirements

- The password must never be sent over the network in plaintext.
- Authentication should use challenge-response or PAKE-style verification over the encrypted iroh connection.
- Brute-force attempts must be rate limited by the server.
- Client-side password input through environment variables is supported for automation, but documentation must warn about command-line secret exposure.
- Server command execution should default to the current process user and must document that privilege boundary clearly.
- File writes should reject path traversal in any future sandboxed mode.
- Sensitive local state must be stored with user-only permissions where the platform supports it.

## Acceptance Criteria For MVP

- A server can start and print a dialable iroh address or ticket.
- A client can authenticate with the server using the printed address and password.
- `exec` can run commands and return stdout, stderr, and exit code on all three target OS families.
- `put` and `get` can transfer a single file larger than memory without buffering the entire file.
- Repeated `exec` calls reuse a local persistent connection when run by the same local user.
- Cargo builds pass on Windows, Linux, and macOS CI targets.
- Documentation describes CLI usage, security properties, and known limitations.
