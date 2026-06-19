# Changelog

## [0.3.0] — 2026-06-19

### Added

- **Self-update**: `remotext update` checks GitHub releases and replaces the current binary in-place. `remotext update --check` only reports if a newer version is available (`src/update.rs`).
- **One-click install scripts**: `scripts/install.sh` (Linux/macOS) and `scripts/install.ps1` (Windows) detect platform, download the latest release, extract, and install to PATH.

## [0.2.0] — 2026-06-19

### Security

- **Authentication rate limiting**: server applies exponential backoff delay on consecutive auth failures (`src/server.rs:56-88`).
- **Configurable resource limits**: new `ServerLimits` struct and CLI flags for `--max-connections`, `--max-file-size`, `--max-concurrent-commands`, `--max-command-secs` (`src/server.rs:44-56`, `src/main.rs:88-104`).
- **Password redaction in Debug output**: `ServerConfig`, `Client`, `PersistentClient`, and `SessionFrame` use custom `Debug` impls that print `<redacted>` for password fields (`src/server.rs:39-52`, `src/client.rs:23-59`, `src/session.rs:36-66`).
- **Dangerous env var filtering**: `LD_PRELOAD`, `DYLD_*` and other injection vectors blocked from client-provided env in exec requests (`src/server.rs:335-348`).
- **OS error redaction**: command spawn failures return a generic message to clients; the real error is logged server-side only (`src/server.rs:292-297`).
- **File transfer integrity**: SHA256 hash computed during file transfer and embedded in `TransferDone`; verified by the receiving side (`src/protocol.rs:73`, `src/server.rs:362-419`, `src/client.rs:494-536`).
- **Session file no longer leaks password hash**: session file name derived from server address only via `SHA256(addr)`, not `SHA256(addr || password)` (`src/session.rs:517-523`).
- **Session secrets via env vars**: background `__session` process receives token via `REMOTEXT_TOKEN` and session file via `REMOTEXT_SESSION_FILE`, not CLI args visible in `/proc` (`src/session.rs:478-502`, `src/main.rs:190-197`).
- **Password zeroization**: `zeroize` crate clears password memory on server setup and client drop (`src/server.rs:156`, `src/client.rs:62-72`).
- **Path traversal helper**: `canonicalize_or_bail()` function available for restricting file operations to a base directory (`src/files.rs:33-45`).

### Added

- 14 new regression tests covering rate limiter, env filtering, Debug redaction, path checks, file size limits, command timeout, transfer hash verification, and session frame redaction (29 tests total).

## [0.1.0] — initial release

- Remote command execution (exec, streaming exec, cancel)
- File transfer (put / get)
- Persistent background session manager on localhost TCP
- OPAQUE PAKE authentication (Ristretto255 + Argon2)
- iroh QUIC transport with relay fallback
- CLI with env var support (`REMOTEXT_ADDR`, `REMOTEXT_PASSWORD`)
