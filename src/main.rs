//! Tycho CLI binary entrypoint.

use clap::{Parser, Subcommand};
use rust_voice_assistant::{ModelManager, TychoConfig, TychoPipelineCoordinator};

#[derive(Parser)]
#[command(name = "tycho")]
#[command(about = "Voice control daemon for Linux compositors", long_about = None)]
struct Cli {
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
    let cli = Cli::parse();
    let config = TychoConfig::default();

    match cli.command {
        Commands::PullModels { force } => {
            let mgr = ModelManager::new(config.models);
            if force {
                let _ = std::fs::remove_dir_all(mgr.resolved_cache_dir());
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
        Commands::Run | Commands::Daemon => {
            let coordinator = TychoPipelineCoordinator::init(config).await?;
            println!("Initialized with model: {:?}", coordinator.models.stt_model_path);
            println!("Tycho daemon listening on audio inputs...");
        }
    }

    Ok(())
}
