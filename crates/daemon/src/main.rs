//! InputSync daemon entry point.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;

mod app;
mod clipboard_mgr;
mod edge;
mod filetransfer_mgr;
mod heartbeat;
mod ipc_server;
mod logging;
mod session;

#[derive(Parser, Debug)]
#[command(
    name = "inputsync-daemon",
    version,
    about = "InputSync background service"
)]
struct Cli {
    /// Path to config file. Defaults to OS-specific user config dir.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the daemon in the foreground.
    Run {
        /// Override role from config: server or client.
        #[arg(long)]
        role: Option<String>,

        /// Override listen address (server mode).
        #[arg(long)]
        listen: Option<SocketAddr>,

        /// Server address to connect to (client mode).
        #[arg(long)]
        connect: Option<SocketAddr>,

        /// Pin a server fingerprint (client mode).
        #[arg(long)]
        pin: Vec<String>,

        /// Don't install the IPC socket (testing).
        #[arg(long)]
        no_ipc: bool,
    },

    /// Write a default config file and exit.
    InitConfig {
        #[arg(long)]
        force: bool,
    },

    /// Print the local certificate fingerprint and exit.
    Fingerprint,

    /// Install background service (systemd unit / Windows Service).
    #[cfg(any(windows, target_os = "linux"))]
    InstallService,

    /// Remove background service.
    #[cfg(any(windows, target_os = "linux"))]
    UninstallService,
}

fn main() -> Result<()> {
    logging::init();

    let cli = Cli::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("inputsync")
        .build()
        .context("build tokio runtime")?;

    runtime.block_on(async move {
        match cli.command {
            Command::Run {
                role,
                listen,
                connect,
                pin,
                no_ipc,
            } => {
                app::run(app::RunArgs {
                    config_path: cli.config,
                    role_override: role,
                    listen_override: listen,
                    connect_override: connect,
                    pinned_fingerprints: pin,
                    ipc_enabled: !no_ipc,
                })
                .await
            }
            Command::InitConfig { force } => app::init_config(cli.config, force),
            Command::Fingerprint => app::print_fingerprint(cli.config),
            #[cfg(any(windows, target_os = "linux"))]
            Command::InstallService => app::install_service(),
            #[cfg(any(windows, target_os = "linux"))]
            Command::UninstallService => app::uninstall_service(),
        }
    })
}
