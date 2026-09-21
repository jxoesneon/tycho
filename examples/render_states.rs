//! Dumps one PNG per orb state for visual design review.

use rust_voice_assistant::ui::{render_frame, UiState};

fn main() {
    let states = [
        ("idle", UiState::Idle, 0.7, 0.0),
        ("listening", UiState::Listening, 0.7, 0.6),
        ("thinking", UiState::Thinking, 0.7, 0.0),
        ("speaking", UiState::Speaking, 0.7, 0.0),
        ("off", UiState::Off, 0.0, 0.0),
    ];
    for (name, state, phase, amp) in states {
        let px = render_frame(192, state, phase, amp);
        let size = tiny_skia::IntSize::from_wh(192, 192).unwrap();
        let pm = tiny_skia::Pixmap::from_vec(px, size).unwrap();
        pm.save_png(format!("/tmp/orb-{name}.png")).unwrap();
    }
    println!("wrote /tmp/orb-*.png");

    // Settings panel for font/layout review.
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let model = rust_voice_assistant::ui::settings::build_model(&snap);
    let (sw, sh) = (
        rust_voice_assistant::ui::settings::surface_width(),
        rust_voice_assistant::ui::settings::surface_height(&model),
    );
    let px = rust_voice_assistant::ui::settings::render_settings(&model, None);
    let size = tiny_skia::IntSize::from_wh(sw, sh).unwrap();
    let pm = tiny_skia::Pixmap::from_vec(px, size).unwrap();
    pm.save_png("/tmp/settings-panel.png").unwrap();
    println!("wrote /tmp/settings-panel.png ({sw}x{sh})");
}
