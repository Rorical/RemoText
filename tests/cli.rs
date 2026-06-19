use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

#[test]
fn cli_server_connect_and_exec_with_background_session() -> Result<()> {
    let bin = env!("CARGO_BIN_EXE_remotext");
    let temp = tempfile::tempdir()?;
    let data_dir = temp.path().join("server");

    let mut server = Command::new(bin)
        .args([
            "server",
            "--local-only",
            "--data-dir",
            data_dir.to_str().context("server data dir is not UTF-8")?,
            "--password",
            "secret",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn RemoText server")?;

    let stdout = server
        .stdout
        .take()
        .context("server stdout was not piped")?;
    let mut reader = BufReader::new(stdout);
    let addr = read_server_address(&mut reader)?;

    let connect = Command::new(bin)
        .args(["connect", "--local-only", "--keepalive-secs", "1"])
        .env("REMOTEXT_ADDR", &addr)
        .env("REMOTEXT_PASSWORD", "secret")
        .output()
        .context("run RemoText connect")?;
    assert_success("connect", &connect);
    assert_eq!(String::from_utf8(connect.stdout)?, "connected\n");

    let exec = Command::new(bin)
        .args(exec_args())
        .env("REMOTEXT_ADDR", &addr)
        .env("REMOTEXT_PASSWORD", "secret")
        .output()
        .context("run RemoText exec")?;
    assert_success("exec", &exec);
    assert_eq!(String::from_utf8(exec.stdout)?, "cli-session");
    assert!(exec.stderr.is_empty());

    let _ = server.kill();
    let _ = server.wait();
    Ok(())
}

fn read_server_address(reader: &mut BufReader<std::process::ChildStdout>) -> Result<String> {
    for _ in 0..10 {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if let Some(addr) = line.strip_prefix("address: ") {
            return Ok(addr.trim().to_string());
        }
    }
    bail!("server did not print an address")
}

fn assert_success(name: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{name} failed\nstatus: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
fn exec_args() -> Vec<&'static str> {
    vec![
        "exec",
        "--local-only",
        "--keepalive-secs",
        "1",
        "--",
        "cmd",
        "/C",
        "<NUL set /p dummy=cli-session",
    ]
}

#[cfg(not(windows))]
fn exec_args() -> Vec<&'static str> {
    vec![
        "exec",
        "--local-only",
        "--keepalive-secs",
        "1",
        "--",
        "printf",
        "cli-session",
    ]
}
