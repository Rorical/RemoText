use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use remotext::{
    NETWORK_LAYER, PROTOCOL_ALPN_STR,
    client::Client,
    server::{NetworkMode, Server, ServerConfig, ServerLimits},
    session, ticket, update,
};
use tokio::io::{self};

#[tokio::main]
async fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .without_time()
        .init();

    let cli = Cli::parse();

    let code = match cli.command {
        Commands::Server(args) => run_server(args).await?,
        Commands::Connect(args) => run_connect(args).await?,
        Commands::Exec(args) => run_exec(args).await?,
        Commands::Put(args) => run_put(args).await?,
        Commands::Get(args) => run_get(args).await?,
        Commands::Session(args) => run_session(args).await?,
        Commands::Update(args) => run_update(args).await?,
    };

    Ok(ExitCode::from(code))
}

#[derive(Debug, Parser)]
#[command(
    name = "remotext",
    version,
    about = "Portable remote command execution agent over iroh",
    long_about = "RemoText is a no-GUI, cross-platform remote command and file transfer agent over iroh."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run this machine as a controllable RemoText server.
    Server(ServerArgs),
    /// Authenticate and verify connectivity to a server.
    Connect(ConnectArgs),
    /// Execute one command on a remote server.
    Exec(ExecArgs),
    /// Upload a local file to the remote server.
    Put(PutArgs),
    /// Download a remote file from the server.
    Get(GetArgs),
    /// Internal background client session process.
    #[command(name = "__session", hide = true)]
    Session(SessionArgs),
    /// Check for a newer version on GitHub and optionally self-update.
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Shared password for client authentication. Prefer REMOTEXT_PASSWORD in scripts.
    #[arg(long, env = "REMOTEXT_PASSWORD", value_name = "PASSWORD")]
    password: String,

    /// Friendly server name shown in logs and status output.
    #[arg(long, default_value = "remotext", value_name = "NAME")]
    name: String,

    /// Directory for server identity and receive staging files.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Disable relay and discovery services. Useful for local tests and LAN-only operation.
    #[arg(long)]
    local_only: bool,

    /// Maximum concurrent iroh connections.
    #[arg(long, default_value_t = remotext::protocol::DEFAULT_MAX_CONNECTIONS as u64, value_name = "N")]
    max_connections: u64,

    /// Maximum concurrent command executions.
    #[arg(long, default_value_t = remotext::protocol::DEFAULT_MAX_CONCURRENT_COMMANDS as u64, value_name = "N")]
    max_concurrent_commands: u64,

    /// Maximum file transfer size in bytes.
    #[arg(long, default_value_t = remotext::protocol::DEFAULT_MAX_FILE_SIZE, value_name = "BYTES")]
    max_file_size: u64,

    /// Maximum command runtime in seconds before forced termination.
    #[arg(long, default_value_t = remotext::protocol::DEFAULT_MAX_COMMAND_SECS, value_name = "SECONDS")]
    max_command_secs: u64,
}

#[derive(Debug, Args)]
struct ConnectArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Disable relay and discovery services for the local client endpoint.
    #[arg(long)]
    local_only: bool,

    /// Keep the local background connection alive for this many idle seconds.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,
}

#[derive(Debug, Args)]
struct ExecArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Disable relay and discovery services for the local client endpoint.
    #[arg(long)]
    local_only: bool,

    /// Bypass the background connection manager and connect directly.
    #[arg(long)]
    no_session: bool,

    /// Keep the local background connection alive for this many idle seconds.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,

    /// Remote command to run. Put it after -- so command flags are not parsed by RemoText.
    #[arg(last = true, required = true, value_name = "COMMAND")]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct PutArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Disable relay and discovery services for the local client endpoint.
    #[arg(long)]
    local_only: bool,

    /// Bypass the background connection manager and connect directly.
    #[arg(long)]
    no_session: bool,

    /// Keep the local background connection alive for this many idle seconds.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,

    /// Local file path to upload.
    #[arg(value_name = "LOCAL")]
    local: PathBuf,

    /// Remote destination path.
    #[arg(value_name = "REMOTE")]
    remote: String,
}

#[derive(Debug, Args)]
struct GetArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Disable relay and discovery services for the local client endpoint.
    #[arg(long)]
    local_only: bool,

    /// Bypass the background connection manager and connect directly.
    #[arg(long)]
    no_session: bool,

    /// Keep the local background connection alive for this many idle seconds.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,

    /// Remote source path.
    #[arg(value_name = "REMOTE")]
    remote: String,

    /// Local destination path.
    #[arg(value_name = "LOCAL")]
    local: PathBuf,
}

#[derive(Debug, Args)]
struct EndpointArgs {
    /// Server address ticket printed by `remotext server`.
    #[arg(long, env = "REMOTEXT_ADDR", value_name = "ADDR")]
    addr: String,

    /// Shared password. Prefer REMOTEXT_PASSWORD for one-line scripted use.
    #[arg(long, env = "REMOTEXT_PASSWORD", value_name = "PASSWORD")]
    password: String,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Only check for updates, do not install.
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Args)]
struct SessionArgs {
    /// Server address ticket printed by `remotext server`.
    #[arg(long, env = "REMOTEXT_ADDR", value_name = "ADDR")]
    addr: String,

    /// Shared password inherited from the foreground client command.
    #[arg(long, env = "REMOTEXT_PASSWORD", value_name = "PASSWORD")]
    password: String,

    /// Base64url-encoded local session token.
    #[arg(long, env = "REMOTEXT_TOKEN", value_name = "TOKEN")]
    token: String,

    /// Path where the background process writes its local listener metadata.
    #[arg(long, env = "REMOTEXT_SESSION_FILE", value_name = "FILE")]
    session_file: PathBuf,

    /// Keep the local background connection alive for this many idle seconds.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,

    /// Disable relay and discovery services for the local client endpoint.
    #[arg(long)]
    local_only: bool,
}

async fn run_server(args: ServerArgs) -> Result<u8> {
    let limits = ServerLimits {
        max_connections: args.max_connections as usize,
        max_concurrent_commands: args.max_concurrent_commands as usize,
        max_file_size: args.max_file_size,
        max_command_secs: args.max_command_secs,
    };
    let server = Server::bind(ServerConfig {
        password: args.password,
        name: args.name,
        data_dir: args.data_dir,
        network_mode: network_mode(args.local_only),
        limits: Some(limits),
    })
    .await?;

    println!("RemoText server");
    println!("network: {NETWORK_LAYER}");
    println!("protocol: {PROTOCOL_ALPN_STR}");
    println!("name: {}", server.name());
    println!("address: {}", server.ticket()?);
    println!("data-dir: {}", server.data_dir().display());
    println!("status: ready");

    server
        .run_until(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(0)
}

async fn run_connect(args: ConnectArgs) -> Result<u8> {
    session::ping(
        &args.endpoint.addr,
        &args.endpoint.password,
        network_mode(args.local_only),
        args.keepalive_secs,
    )
    .await?;
    println!("connected");
    Ok(0)
}

async fn run_exec(args: ExecArgs) -> Result<u8> {
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    if !args.no_session {
        match session::exec_with_cancel(
            session::ExecSessionRequest {
                addr: &args.endpoint.addr,
                password: &args.endpoint.password,
                network_mode: network_mode(args.local_only),
                keepalive_secs: args.keepalive_secs,
                command: args.command.clone(),
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            async {
                let _ = tokio::signal::ctrl_c().await;
            },
        )
        .await
        {
            Ok(code) => return Ok(clamp_exit_code(code)),
            Err(err) => tracing::debug!(
                ?err,
                "background session failed; falling back to direct connection"
            ),
        }
    }

    let client = make_client(args.endpoint, args.local_only)?;
    let code = client
        .exec_stream_with_cancel(args.command, &mut stdout, &mut stderr, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(clamp_exit_code(code))
}

async fn run_put(args: PutArgs) -> Result<u8> {
    if !args.no_session {
        match session::put(
            &args.endpoint.addr,
            &args.endpoint.password,
            network_mode(args.local_only),
            args.keepalive_secs,
            &args.local,
            &args.remote,
        )
        .await
        {
            Ok(_) => return Ok(0),
            Err(err) => tracing::debug!(
                ?err,
                "background session failed; falling back to direct connection"
            ),
        }
    }

    let client = make_client(args.endpoint, args.local_only)?;
    client.put(&args.local, &args.remote).await?;
    Ok(0)
}

async fn run_get(args: GetArgs) -> Result<u8> {
    if !args.no_session {
        match session::get(
            &args.endpoint.addr,
            &args.endpoint.password,
            network_mode(args.local_only),
            args.keepalive_secs,
            &args.remote,
            &args.local,
        )
        .await
        {
            Ok(_) => return Ok(0),
            Err(err) => tracing::debug!(
                ?err,
                "background session failed; falling back to direct connection"
            ),
        }
    }

    let client = make_client(args.endpoint, args.local_only)?;
    client.get(&args.remote, &args.local).await?;
    Ok(0)
}

async fn run_update(args: UpdateArgs) -> Result<u8> {
    let current = env!("CARGO_PKG_VERSION");
    if args.check {
        match update::check_for_update(current) {
            Ok(Some(latest)) => {
                println!("Update available: v{latest} (current: v{current})");
                Ok(0)
            }
            Ok(None) => {
                println!("Already up to date (v{current})");
                Ok(0)
            }
            Err(err) => {
                eprintln!("Update check failed: {err}");
                Ok(1)
            }
        }
    } else {
        match update::self_update() {
            Ok(prev) => {
                println!("Updated from v{prev} to v{current}");
                Ok(0)
            }
            Err(err) => {
                eprintln!("Self-update failed: {err}");
                Ok(1)
            }
        }
    }
}

async fn run_session(args: SessionArgs) -> Result<u8> {
    session::run_background(
        args.addr,
        args.password,
        session::decode_token(&args.token)?,
        args.session_file,
        network_mode(args.local_only),
        args.keepalive_secs,
    )
    .await?;
    Ok(0)
}

fn make_client(endpoint: EndpointArgs, local_only: bool) -> Result<Client> {
    let addr = ticket::decode_addr(&endpoint.addr)?;
    Ok(Client::new(
        addr,
        endpoint.password,
        network_mode(local_only),
    ))
}

fn network_mode(local_only: bool) -> NetworkMode {
    if local_only {
        NetworkMode::LocalOnly
    } else {
        NetworkMode::Public
    }
}

fn clamp_exit_code(code: i32) -> u8 {
    if code < 0 {
        1
    } else {
        u8::try_from(code).unwrap_or(1)
    }
}
