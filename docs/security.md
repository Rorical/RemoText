# Security Design

## Security Boundary

RemoText executes commands as the operating-system user that runs the server process. It is not a privilege separation system. If the server runs as an administrator or root user, authenticated clients can effectively operate with that level of access unless server-side policy restricts them.

## Threat Model

Primary threats:

- Unauthorized remote command execution.
- Password guessing or replay.
- Secret leakage through logs, process arguments, shell history, or local IPC.
- Malicious file path input causing overwrite outside intended locations.
- Resource exhaustion from long-running commands or large transfers.
- Downgrade or confusion between protocol versions.

## Authentication Requirements

- Never send the raw password over the network.
- Bind authentication proofs to server identity and both client and server nonces.
- Reject replayed proofs.
- Rate limit failed authentication attempts.
- Support password rotation by restarting or reconfiguring the server.
- Store a password verifier rather than the raw password when server configuration is persisted.

## Transport Security

iroh provides encrypted QUIC connections keyed by peer identity. RemoText still needs application-level authorization because knowing or dialing a node address must not be enough to execute commands.

The application protocol should verify:

- The negotiated ALPN is `remotext/1`.
- The server identity matches the address or ticket the client intended to dial.
- The authentication transcript includes the negotiated protocol version.

## Password Handling

CLI passwords are convenient but risky. Recommended order of preference:

- Interactive prompt for humans once implemented.
- Environment variable for automation.
- Command-line flag only for quick local testing.

The implementation must redact password values in debug output, logs, panic messages, and diagnostics.

## Local Session Manager Security

The background client session manager holds authenticated connections. It must be protected as local user state.

Requirements:

- IPC endpoint must be accessible only by the current local user.
- Requests must include a local process authentication check where the platform allows it.
- The session manager should not expose raw passwords after initial authentication.
- Idle sessions must expire.
- A future `remotext disconnect` command should close sessions explicitly.

## Command Execution Controls

MVP can start permissive, but the design should leave room for policy controls:

- Maximum concurrent commands.
- Maximum command runtime.
- Optional working directory allowlist.
- Optional executable allowlist or denylist.
- Environment variable filtering.
- Optional stdin disablement.
- Output size limits for non-streaming modes.

## File Transfer Controls

File transfer controls should include:

- Maximum file size.
- Destination overwrite policy.
- Path canonicalization before policy checks.
- Temporary-file staging with safe permissions.
- Hash verification for integrity when requested.
- Clear behavior for symbolic links and special files.

## Logging Rules

Allowed by default:

- Connection opened or closed.
- Authentication success or failure without password values.
- Request type and request id.
- Error category.

Not allowed by default:

- Passwords or derived secrets.
- Full command output.
- File contents.
- Authentication proofs.

Command strings can reveal secrets. Logging full command arguments should require an explicit debug or audit mode.

## Hardening Backlog

- Add a memory zeroization strategy for password buffers where practical.
- Evaluate a mature PAKE implementation instead of custom HMAC-only authentication.
- Add known-server trust records to reduce accidental connection to the wrong node.
- Add optional command policy files.
- Add signed release artifacts and checksums.
- Add fuzz tests for protocol frame decoding.
