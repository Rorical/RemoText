# RemoText

RemoText is a planned lightweight, portable remote command execution agent written in Rust. It uses `iroh` as the network layer so a client can connect by a server-provided address or ticket instead of manually managing public IPs, port forwarding, or VPNs.

The current repository is an initialized Rust project with a stable CLI scaffold and detailed requirements and design documents. The iroh runtime, authentication protocol, command execution, and file transfer implementation are intentionally still marked as pending.

## Target Capabilities

- Cross-platform: Windows, Linux, and macOS.
- No GUI: single CLI binary for server and client modes.
- Remote command execution with stdout, stderr, exit code, and cancellation support.
- File upload and download.
- Server mode prints a connection address and uses a password for client authentication.
- Client mode supports one-line scripted invocation similar to `sshpass`, while maintaining a background long-lived connection for reuse.
- Network layer based on `iroh` 1.0.0.

## Current CLI Shape

```bash
remotext server --password <password>
remotext connect --addr <address> --password <password>
remotext exec --addr <address> --password <password> -- <command> [args...]
remotext put --addr <address> --password <password> <local> <remote>
remotext get --addr <address> --password <password> <remote> <local>
```

For scripts, prefer environment variables so secrets are not exposed in shell history:

```bash
REMOTEXT_ADDR=<address> REMOTEXT_PASSWORD=<password> remotext exec -- uname -a
```

## Documents

- `docs/requirements.md`: product requirements and acceptance criteria.
- `docs/technical-design.md`: architecture and implementation design.
- `docs/protocol.md`: protocol framing, handshake, command, and file transfer messages.
- `docs/cli.md`: CLI behavior and examples.
- `docs/cross-platform.md`: Windows, Linux, and macOS implementation notes.
- `docs/security.md`: security model, authentication, secrets, and hardening requirements.
- `docs/roadmap.md`: milestone plan from scaffold to usable releases.

## Development

```bash
cargo check
cargo run -- --help
```
