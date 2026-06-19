# Cross-Platform Notes

## Target Matrix

Initial targets:

- `x86_64-pc-windows-msvc`.
- `aarch64-pc-windows-msvc` when dependencies support it.
- `x86_64-unknown-linux-gnu`.
- `aarch64-unknown-linux-gnu`.
- `x86_64-apple-darwin`.
- `aarch64-apple-darwin`.

Linux musl targets should be evaluated after iroh and TLS dependencies are validated.

## Command Execution

### Windows

- Use `std::process::Command` or Tokio process APIs with Windows-specific creation options where needed.
- Do not assume a POSIX shell.
- Examples should use `cmd /C` or `powershell -NoProfile -Command` when shell evaluation is required.
- Process termination should map Ctrl+C cancellation to a graceful signal where possible, then terminate if the process does not exit.

### Linux

- Do not implicitly wrap commands with a shell.
- Shell examples should use `sh -lc` or `bash -lc` explicitly.
- Preserve executable bit for downloads only when protocol metadata includes Unix mode bits.
- Service mode can use systemd after foreground server mode is stable.

### macOS

- Behavior is close to Linux for process execution, but default shells and system paths differ.
- Service mode can use launchd after foreground server mode is stable.
- iroh fast Apple datapath support is enabled by the default iroh feature set.

## Paths

- Protocol paths are UTF-8 strings in the first version.
- Windows clients and servers must handle drive prefixes and backslashes carefully.
- Future sandbox mode should canonicalize paths before access checks.
- Remote paths are interpreted by the remote server OS, not by the client OS.

## Local State Directories

Recommended defaults:

- Linux: `$XDG_DATA_HOME/remotext` or `$HOME/.local/share/remotext`.
- macOS: `$HOME/Library/Application Support/RemoText`.
- Windows: `%APPDATA%\\RemoText`.

Runtime socket or pipe locations:

- Linux: `$XDG_RUNTIME_DIR/remotext/session.sock` when available.
- macOS: a user-owned directory under `/tmp` or `~/Library/Caches/RemoText`.
- Windows: named pipe under `\\.\\pipe\\remotext-<user-hash>`.

## File Transfer Semantics

- Atomic rename is used when source and destination are on the same filesystem.
- If atomic rename is unavailable, the operation should fail or explicitly report degraded behavior.
- Modification time preservation is best-effort and platform-dependent.
- Symbolic link handling should default to transferring link targets only after explicit policy is defined.

## Packaging

- Ship one binary per target platform.
- Avoid requiring OpenSSL dynamic libraries if the selected iroh TLS backend allows it.
- Sign macOS and Windows binaries later if distributing outside internal use.
- Provide checksums for release archives.

## Testing Requirements

- Unit tests for protocol encoding and decoding on all targets.
- Integration tests for local loopback command execution.
- Platform tests for path handling and process cancellation.
- Manual NAT traversal testing with iroh relay fallback before public release.
