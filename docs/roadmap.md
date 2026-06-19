# Roadmap

## Milestone 0: Project Scaffold

Status: current.

Deliverables:

- Rust binary crate initialized.
- CLI command surface defined.
- iroh dependency selected.
- Requirements and technical documents written.

## Milestone 1: iroh Connectivity

Deliverables:

- Server creates or loads an iroh endpoint.
- Server prints a real connection address or ticket.
- Client dials the server address.
- Protocol ALPN negotiation works.
- Basic ping or hello request succeeds.

## Milestone 2: Authentication

Deliverables:

- Password-based challenge-response or PAKE handshake.
- Failed authentication returns stable errors.
- Authentication attempts are rate limited.
- Passwords are redacted in logs.

## Milestone 3: Direct Command Execution

Deliverables:

- `remotext exec` runs commands without the background session manager.
- Stdout and stderr stream independently.
- Exit code is propagated to the client.
- Ctrl+C cancellation is implemented.
- Windows, Linux, and macOS command examples are tested.

## Milestone 4: File Transfer

Deliverables:

- `put` uploads one file with streaming.
- `get` downloads one file with streaming.
- Temporary-file plus rename behavior is implemented.
- Transfer errors are stable and script-friendly.

## Milestone 5: Background Session Manager

Deliverables:

- Local per-user session manager starts automatically.
- Repeated one-line commands reuse authenticated connections.
- Idle keepalive timeout works.
- Unix socket and Windows named pipe permissions are validated.

## Milestone 6: Service And Release Packaging

Deliverables:

- Foreground server remains the default portable mode.
- Optional Windows service install helper.
- Optional systemd user or system service helper.
- Optional macOS launchd helper.
- CI release builds and checksums.

## Milestone 7: Policy And Hardening

Deliverables:

- Command policy file.
- File transfer path policy.
- Known-server trust store.
- Protocol fuzz tests.
- Security review before public release.
