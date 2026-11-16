use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, ValueEnum};
use romstore_server::{api, catalog::Catalog, config::Config, reload::Reload};
use std::{io::IsTerminal, path::PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, ValueEnum)]
enum Command {
    Serve,
    Index,
}

#[derive(Clone, Copy, ValueEnum)]
enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}
impl LogLevel {
    fn filter(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warn",
            Self::Error | Self::Critical => "error",
        }
    }
}

#[derive(Parser)]
#[command(version, about = "Serveur de ROMs Batocera en lecture seule")]
struct Args {
    #[arg(value_enum, default_value = "serve")]
    command: Command,
    #[arg(long)]
    library: PathBuf,
    #[arg(long, default_value = ".romstore/catalog.sqlite3")]
    database: PathBuf,
    #[arg(
        long,
        value_name = "DOSSIER",
        help = "Icônes <plateforme>.png/.jpg/.webp… ; défaut : system-icons à côté de la base"
    )]
    system_icon: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8000)]
    port: u16,
    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(args.log_level.filter())),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let config = Config::new(args.library, args.database, args.system_icon.as_deref())
        .unwrap_or_else(|error| {
            Args::command()
                .error(clap::error::ErrorKind::ValueValidation, error.to_string())
                .exit()
        });
    let catalog = Catalog::new(config)?;
    if matches!(args.command, Command::Index) {
        let report = tokio::task::spawn_blocking(move || catalog.reindex()).await??;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port))
        .await
        .context("Impossible d'écouter sur l'adresse demandée")?;
    tracing::info!(
        "Serveur disponible sur http://{} — gestion : / — documentation : /docs",
        listener.local_addr()?
    );
    let reload = Reload::default();
    reload.start(catalog.clone());
    axum::serve(listener, api::router_with_reload(catalog, reload))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = signal(SignalKind::terminate()).expect("Gestion du signal SIGTERM");
    tokio::select! { _ = tokio::signal::ctrl_c() => (), _ = terminate.recv() => () }
    tracing::info!("Arrêt du serveur ; attente des opérations en cours");
}
