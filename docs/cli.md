# RemoText CLI Design

## Command Overview

```bash
remotext server --password <password>
remotext connect --addr <address> --password <password>
remotext exec --addr <address> --password <password> -- <command> [args...]
remotext exec --no-session --addr <address> --password <password> -- <command> [args...]
remotext put --addr <address> --password <password> <local> <remote>
remotext get --addr <address> --password <password> <remote> <local>
```

`--addr` can be supplied through `REMOTEXT_ADDR` and `--password` can be supplied through `REMOTEXT_PASSWORD`.

## Server Mode

```bash
remotext server --password correct-horse-battery-staple
```

Behavior:

- Load or create the server iroh identity.
- Bind the RemoText iroh endpoint.
- Print an `rt1_` connection address ticket.
- Wait for authenticated clients.

Example output:

```text
RemoText server
network: iroh
protocol: remotext/1
name: remotext
address: rt1_...
data-dir: /path/to/RemoText
status: ready
```

## Persistent Client Connection

```bash
remotext connect --addr rt1_... --password correct-horse-battery-staple
```

Behavior:

- Dial the server with iroh.
- Start or reuse a local session manager.
- Authenticate once with the provided password.
- Keep the connection alive for the idle timeout.

## One-Line Command Execution

Linux or macOS:

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=secret remotext exec -- uname -a
```

Run through a shell when shell features are needed:

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=secret remotext exec -- sh -lc 'echo $HOME && id'
```

Windows `cmd.exe`:

```powershell
$env:REMOTEXT_ADDR="rt1_..."
$env:REMOTEXT_PASSWORD="secret"
remotext exec -- cmd /C dir
```

Windows PowerShell:

```powershell
remotext exec -- powershell -NoProfile -Command "Get-ChildItem Env:"
```

The `--` separator is important. Everything after it belongs to the remote command and is not parsed as RemoText flags.

Use `--no-session` to bypass the local background connection manager and open a direct one-shot iroh connection.

## File Upload

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=secret remotext put ./local.txt /tmp/remote.txt
```

Expected behavior:

- Stream `local.txt` to the server.
- Write to a remote temporary file first.
- Rename to `/tmp/remote.txt` only after a complete transfer.
- Return non-zero if the transfer fails.

## File Download

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=secret remotext get /tmp/remote.txt ./local.txt
```

Expected behavior mirrors upload: stream chunks, write to a temporary local file, verify completion, and rename on success.

## Password Handling

Supported input methods:

- `--password <password>` for direct manual testing.
- `REMOTEXT_PASSWORD=<password>` for script-friendly use.

Avoid putting passwords directly on shared systems where process lists or shell history are visible to other users.

## Exit Codes

Exit code behavior:

- `0`: operation succeeded.
- `1`: generic local failure.
- `2`: CLI usage error.
- `10`: connection failed.
- `11`: authentication failed.
- `12`: protocol mismatch.
- `20`: remote command failed to start.
- Remote command non-zero exit returns the same exit code when it fits the local platform range.

## Background Session Behavior

One-line commands should use this flow:

```text
remotext exec
  -> locate local session manager
  -> start manager if missing
  -> authenticate if connection is not already warm
  -> submit command request
  -> stream output to this terminal
  -> keep manager alive until idle timeout
```

This gives scripts a simple `sshpass`-like command while repeated calls avoid reconnecting each time. If the session manager cannot be reached, `exec`, `put`, and `get` fall back to a direct iroh connection.
