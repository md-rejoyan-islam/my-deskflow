use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use inputsync_ipc::{IpcClient, IpcRequest, IpcResponse};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "inputsync-cli", version, about = "Talk to a running inputsync-daemon")]
struct Cli {
    /// Override the daemon socket path.
    #[arg(short, long)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print daemon status (uptime, peers, fingerprint).
    Status,
    /// Print the live config in TOML.
    Config,
    /// Print the local cert fingerprint.
    Fingerprint,
    /// Stream daemon events as JSON lines.
    Watch,
    /// Trigger emergency stop.
    Emergency,
    /// Ask the daemon to shut down.
    Shutdown,
    /// Add a peer at runtime (server mode).
    Connect { addr: String },
    /// Disconnect a peer (server mode).
    Disconnect { peer: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = match cli.socket {
        Some(p) => p,
        None => inputsync_ipc::default_socket_path().context("default socket path")?,
    };

    let mut client = IpcClient::connect(&socket)
        .await
        .with_context(|| format!("connect to daemon at {}", socket.display()))?;

    match cli.command {
        Command::Status => {
            let resp = client.request(&IpcRequest::GetStatus).await?;
            print_response(&resp);
        }
        Command::Config => {
            let resp = client.request(&IpcRequest::GetConfig).await?;
            if let IpcResponse::Config(c) = resp {
                println!("{}", toml::to_string_pretty(&c)?);
            } else {
                print_response(&resp);
            }
        }
        Command::Fingerprint => {
            let resp = client.request(&IpcRequest::GetStatus).await?;
            if let IpcResponse::Status(s) = resp {
                println!("{}", s.local_fingerprint);
            } else {
                print_response(&resp);
            }
        }
        Command::Watch => {
            let _ = client.request(&IpcRequest::SubscribeEvents).await?;
            loop {
                let msg = client.next_message().await?;
                println!("{}", serde_json::to_string(&msg)?);
            }
        }
        Command::Emergency => {
            let resp = client.request(&IpcRequest::EmergencyStop).await?;
            print_response(&resp);
        }
        Command::Shutdown => {
            let resp = client.request(&IpcRequest::Shutdown).await?;
            print_response(&resp);
        }
        Command::Connect { addr } => {
            let resp = client.request(&IpcRequest::Connect { addr }).await?;
            print_response(&resp);
        }
        Command::Disconnect { peer } => {
            let resp = client.request(&IpcRequest::Disconnect { peer }).await?;
            print_response(&resp);
        }
    }

    Ok(())
}

fn print_response(resp: &IpcResponse) {
    match serde_json::to_string_pretty(resp) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{resp:?}"),
    }
}
