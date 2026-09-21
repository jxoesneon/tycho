//! Manual visual check for the status orb: cycles through all states.
//! Run on a Wayland session with `cargo run --example orb_demo`.

use rust_voice_assistant::pipeline::events::PipelineEvent;
use rust_voice_assistant::ui::orb::OrbHandle;
use rust_voice_assistant::ui::OrbPosition;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;

fn main() {
    let (tx, rx) = tokio::sync::broadcast::channel(64);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let mode = Arc::new(AtomicU8::new(0));
    let snapshot = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let Some(_orb) = OrbHandle::spawn(
        rx,
        cmd_tx,
        mode,
        rust_voice_assistant::ui::orb::StartupPlacement {
            position: OrbPosition::auto_detect(),
            margin_x: -1,
            margin_y: -1,
        },
        snapshot,
    ) else {
        eprintln!("no Wayland session — orb requires WAYLAND_DISPLAY");
        std::process::exit(1);
    };
    let cycle = [
        PipelineEvent::SpeechStarted,
        PipelineEvent::SpeechEnded,
        PipelineEvent::SpeakingStarted,
        PipelineEvent::SpeakingFinished,
    ];
    for ev in cycle.into_iter().cycle() {
        while let Ok(cmd) = cmd_rx.try_recv() {
            eprintln!("ui command: {:?}", cmd);
        }
        if tx.send(ev).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
