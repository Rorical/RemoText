use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

const NETWORK_LAYER: &str = "iroh";
const PROTOCOL_ALPN: &str = "remotext/1";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .without_time()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Server(args) => run_server(args).await,
        Commands::Connect(args) => run_connect(args).await,
        Commands::Exec(args) => run_exec(args).await,
        Commands::Put(args) => run_put(args).await,
        Commands::Get(args) => run_get(args).await,
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "remotext",
    version,
    about = "Portable remote command execution agent over iroh",
    long_about = "RemoText is a planned no-GUI, cross-platform remote command and file transfer agent. The current binary provides the stable CLI surface while the iroh transport runtime is implemented."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run this machine as a controllable RemoText server.
    Server(ServerArgs),
    /// Open or warm a persistent client connection to a server.
    Connect(ConnectArgs),
    /// Execute one command on a remote server.
    Exec(ExecArgs),
    /// Upload a local file to the remote server.
    Put(PutArgs),
    /// Download a remote file from the server.
    Get(GetArgs),
}

#[derive(Debug, Args)]
struct ServerArgs {
    /// Shared password for client authentication. Prefer REMOTEXT_PASSWORD in scripts.
    #[arg(long, env = "REMOTEXT_PASSWORD", value_name = "PASSWORD")]
    password: String,

    /// Friendly server name shown in client-side session lists.
    #[arg(long, default_value = "remotext", value_name = "NAME")]
    name: String,

    /// Directory for server identity, session state, and receive staging files.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ConnectArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Keep the local background connection alive for this many seconds while idle.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    keepalive_secs: u64,
}

#[derive(Debug, Args)]
struct ExecArgs {
    #[command(flatten)]
    endpoint: EndpointArgs,

    /// Keep the local background connection alive for this many seconds after the command.
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

    /// Remote source path.
    #[arg(value_name = "REMOTE")]
    remote: String,

    /// Local destination path.
    #[arg(value_name = "LOCAL")]
    local: PathBuf,
}

#[derive(Debug, Args)]
struct EndpointArgs {
    /// Server address or ticket printed by `remotext server`.
    #[arg(long, env = "REMOTEXT_ADDR", value_name = "ADDR")]
    addr: String,

    /// Shared password. Prefer REMOTEXT_PASSWORD for one-line scripted use.
    #[arg(long, env = "REMOTEXT_PASSWORD", value_name = "PASSWORD")]
    password: String,
}

async fn run_server(args: ServerArgs) -> Result<()> {
    println!("RemoText server");
    println!("network: {NETWORK_LAYER}");
    println!("protocol: {PROTOCOL_ALPN}");
    println!("name: {}", args.name);
    println!("password: {}", describe_secret(&args.password));
    println!(
        "data-dir: {}",
        args.data_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<platform default>".to_string())
    );
    println!("address: <pending iroh node ticket>");
    println!("status: iroh server runtime is not implemented yet; see docs/technical-design.md");

    Ok(())
}

async fn run_connect(args: ConnectArgs) -> Result<()> {
    print_endpoint("connect", &args.endpoint);
    println!("keepalive-secs: {}", args.keepalive_secs);
    println!("status: persistent client session manager is not implemented yet");

    Ok(())
}

async fn run_exec(args: ExecArgs) -> Result<()> {
    print_endpoint("exec", &args.endpoint);
    println!("keepalive-secs: {}", args.keepalive_secs);
    println!("command: {}", args.command.join(" "));
    println!("status: remote command execution is not implemented yet");

    Ok(())
}

async fn run_put(args: PutArgs) -> Result<()> {
    print_endpoint("put", &args.endpoint);
    println!("local: {}", args.local.display());
    println!("remote: {}", args.remote);
    println!("status: file upload is not implemented yet");

    Ok(())
}

async fn run_get(args: GetArgs) -> Result<()> {
    print_endpoint("get", &args.endpoint);
    println!("remote: {}", args.remote);
    println!("local: {}", args.local.display());
    println!("status: file download is not implemented yet");

    Ok(())
}

fn print_endpoint(action: &str, endpoint: &EndpointArgs) {
    println!("RemoText client {action}");
    println!("network: {NETWORK_LAYER}");
    println!("protocol: {PROTOCOL_ALPN}");
    println!("addr: {}", endpoint.addr);
    println!("password: {}", describe_secret(&endpoint.password));
}

fn describe_secret(secret: &str) -> String {
    if secret.is_empty() {
        "<empty>".to_string()
    } else {
        format!("{} chars configured", secret.chars().count())
    }
}
