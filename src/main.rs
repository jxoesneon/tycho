//! Tycho CLI binary entrypoint.

use clap::{Parser, Subcommand};
use rust_voice_assistant::{ModelManager, TychoConfig, TychoPipelineCoordinator};

#[derive(Parser)]
#[command(name = "tycho")]
#[command(about = "Voice control daemon for Linux compositors", long_about = None)]
struct Cli {
    /// Path to a tycho.toml configuration file (overrides TYCHO_CONFIG and
    /// the default ~/.config/tycho/tycho.toml lookup)
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start voice listener daemon
    Run,
    /// Run as background daemon process
    Daemon,
    /// Process a direct text query
    Query { text: String },
    /// Test compositor integration and return desktop context
    TestDesktop,
    /// Fetch or refresh local ONNX model cache
    PullModels {
        #[arg(short, long, help = "Force download even if cache exists")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config = match &cli.config {
        Some(path) => TychoConfig::from_file(path)?,
        None => TychoConfig::load()?,
    };

    match cli.command {
        Commands::PullModels { force } => {
            let mgr = ModelManager::new(config.models);
            if force {
                for target in [
                    mgr.stt_model_target_path(),
                    mgr.tts_model_target_path(),
                    mgr.tts_voices_target_path(),
                ]
                .into_iter()
                .chain(mgr.router_model_target_path())
                {
                    let _ = std::fs::remove_file(&target);
                    let _ = std::fs::remove_file(target.with_extension("tmp_download"));
                }
            }
            let inventory = mgr.ensure_models().await?;
            println!("STT model:    {:?}", inventory.stt_model_path);
            println!("TTS model:    {:?}", inventory.tts_model_path);
            println!("TTS voices:   {:?}", inventory.tts_voices_path);
            if let Some(router) = inventory.router_model_path {
                println!("Router model: {:?}", router);
            }
        }
        Commands::Query { text } => {
            let mut coordinator = TychoPipelineCoordinator::init(config).await?;
            let response = coordinator.process_query(&text).await?;
            println!("{}", response);
        }
        Commands::TestDesktop => {
            let coordinator = TychoPipelineCoordinator::init(config).await?;
            let summary = coordinator.executor.context_summary().await;
            println!("Desktop context: {}", summary);
        }
        Commands::Run => {
            let mut coordinator = TychoPipelineCoordinator::init(config).await?;
            coordinator.run_forever().await?;
        }
        Commands::Daemon => {
            // Detach `tycho run` into a new session so the daemon survives
            // the calling terminal; logs and a PID file land in the state dir.
            let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
            let state_dir = std::path::PathBuf::from(home).join(".local/state/tycho");
            std::fs::create_dir_all(&state_dir)?;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(state_dir.join("daemon.log"))?;
            let log_err = log.try_clone()?;

            let exe = std::env::current_exe()?;
            let mut cmd = std::process::Command::new("setsid");
            cmd.arg(exe);
            if let Some(path) = &cli.config {
                cmd.arg("--config").arg(path);
            }
            let mut child = cmd
                .arg("run")
                .stdin(std::process::Stdio::null())
                .stdout(log)
                .stderr(log_err)
                .spawn()?;
            // If the PID file can't be written, kill the detached child —
            // without a recorded PID it would be unmanageable.
            if let Err(e) = std::fs::write(state_dir.join("tycho.pid"), child.id().to_string()) {
                let _ = child.kill();
                return Err(e.into());
            }
            println!("tycho daemon started (pid {})", child.id());
        }
    }

    Ok(())
}
