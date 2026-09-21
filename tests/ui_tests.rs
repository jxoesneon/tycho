//! Orb UI coverage: event→state mapping, the pure renderer, and the
//! Wayland handshake when a live session is available.

use rust_voice_assistant::pipeline::events::PipelineEvent;
use rust_voice_assistant::ui::menu;
use rust_voice_assistant::ui::orb::OrbHandle;
use rust_voice_assistant::ui::{
    render_frame, rgba_to_argb8888, state_alpha, state_color, ActivationMode, OrbPosition,
    OrbVisual, UiCommand, UiState,
};
use std::sync::atomic::AtomicU8;
use std::sync::Arc;

fn spawn_orb(rx: tokio::sync::broadcast::Receiver<PipelineEvent>) -> Option<OrbHandle> {
    let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel();
    OrbHandle::spawn(
        rx,
        cmd_tx,
        Arc::new(AtomicU8::new(0)),
        rust_voice_assistant::ui::orb::StartupPlacement {
            position: OrbPosition::auto_detect(),
            margin_x: -1,
            margin_y: -1,
        },
        rust_voice_assistant::ui::settings_snapshot(
            &rust_voice_assistant::config::TychoConfig::default(),
        ),
    )
}

#[test]
fn event_mapping_drives_orb_state() {
    let mut v = OrbVisual::default();
    assert_eq!(v.state, UiState::Idle);
    assert!(!v.animating());

    v.apply_event(&PipelineEvent::SpeechStarted);
    assert_eq!(v.state, UiState::Listening);
    assert!(v.animating());
    assert!(v.dirty);

    v.apply_event(&PipelineEvent::SpeechProgress { rms: 0.8 });
    assert!((v.amplitude - 0.8).abs() < f32::EPSILON);
    // Amplitude only applies while listening.
    v.apply_event(&PipelineEvent::SpeechEnded);
    v.apply_event(&PipelineEvent::SpeechProgress { rms: 1.0 });
    assert_eq!(v.state, UiState::Thinking);
    assert!((v.amplitude - 0.8).abs() < f32::EPSILON);

    for ev in [
        PipelineEvent::TranscriptionCompleted("hi".into()),
        PipelineEvent::FastPathRouted {
            intent: "volume".into(),
            parameter: None,
            confidence: 0.9,
        },
        PipelineEvent::DeliberationRouted { query: "hi".into() },
    ] {
        v.state = UiState::Idle;
        v.apply_event(&ev);
        assert_eq!(v.state, UiState::Thinking);
    }

    v.apply_event(&PipelineEvent::GenerationCompleted {
        response: "ok".into(),
    });
    assert_eq!(v.state, UiState::Thinking);

    v.apply_event(&PipelineEvent::SpeakingStarted);
    assert_eq!(v.state, UiState::Speaking);

    for ev in [
        PipelineEvent::SpeakingFinished,
        PipelineEvent::SynthesisCompleted { sample_count: 4 },
        PipelineEvent::DesktopActionExecuted {
            output: "done".into(),
            latency_micros: 1,
        },
        PipelineEvent::Idle,
    ] {
        v.state = UiState::Speaking;
        v.apply_event(&ev);
        assert_eq!(v.state, UiState::Idle);
    }
}

#[test]
fn render_produces_transparent_corners_opaque_core() {
    let size = 96;
    let px = render_frame(size, UiState::Listening, 0.0, 0.5);
    assert_eq!(px.len(), (size * size * 4) as usize);

    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(at(0, 0)[3], 0, "corner must stay transparent");
    assert_eq!(at(size - 1, size - 1)[3], 0);
    assert!(at(size / 2, size / 2)[3] > 0, "core must be opaque");
}

#[test]
fn render_states_differ_and_amplitude_widens_glow() {
    let size = 96;
    let mid = size / 2;
    let alpha_at = |px: &[u8], x: u32, y: u32| px[((y * size + x) * 4 + 3) as usize];

    let idle = render_frame(size, UiState::Idle, 0.0, 0.0);
    let listening = render_frame(size, UiState::Listening, 0.0, 0.0);
    // Listening glow exceeds idle dimness at the halo ring.
    let ring = mid - 14;
    assert!(alpha_at(&listening, ring, mid) >= alpha_at(&idle, ring, mid));

    // Amplitude widens the glow: a pixel beyond the core brightens.
    let soft = render_frame(size, UiState::Listening, 0.0, 0.0);
    let loud = render_frame(size, UiState::Listening, 0.0, 1.0);
    let outer = mid - 30;
    assert!(alpha_at(&loud, outer, mid) > alpha_at(&soft, outer, mid));

    // Thinking paints off-center satellite dots.
    let thinking = render_frame(size, UiState::Thinking, 0.0, 0.0);
    let mut off_center_alpha = 0u32;
    for y in 0..size {
        for x in 0..size {
            let dx = x as i32 - mid as i32;
            let dy = y as i32 - mid as i32;
            let d2 = dx * dx + dy * dy;
            let r = (mid as i32) * 72 / 100;
            // Satellite orbits at ~0.72*center, radius ~4px.
            if (d2 as f32).sqrt().abs() - r as f32 <= 5.0 {
                off_center_alpha = off_center_alpha.max(alpha_at(&thinking, x, y) as u32);
            }
        }
    }
    assert!(off_center_alpha > 0, "satellite dots must render");

    // State palette is distinct.
    let colors: Vec<_> = [
        UiState::Idle,
        UiState::Listening,
        UiState::Thinking,
        UiState::Speaking,
    ]
    .iter()
    .map(|s| state_color(*s))
    .collect();
    for (i, a) in colors.iter().enumerate() {
        for b in &colors[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert!(state_alpha(UiState::Idle) < state_alpha(UiState::Listening));
}

#[test]
fn render_zero_size_returns_empty() {
    assert!(render_frame(0, UiState::Listening, 1.0, 1.0).is_empty());
}

#[test]
fn rgba_swizzle_to_argb8888() {
    // RGBA [1,2,3,4] -> ARGB8888 little-endian bytes [B,G,R,A] = [3,2,1,4]
    let out = rgba_to_argb8888(&[1, 2, 3, 4]);
    assert_eq!(out, vec![3, 2, 1, 4]);
    assert!(rgba_to_argb8888(&[]).is_empty());
}

#[test]
fn orb_handle_respects_wayland_env() {
    let saved_display = std::env::var_os("WAYLAND_DISPLAY");
    let saved_socket = std::env::var_os("WAYLAND_SOCKET");

    // Flood past the channel capacity so the orb thread sees Lagged.
    let (tx, rx) = tokio::sync::broadcast::channel(8);
    for _ in 0..40 {
        let _ = tx.send(PipelineEvent::Idle);
    }
    let live = spawn_orb(rx);
    if saved_display.is_some() || saved_socket.is_some() {
        // Live session: the overlay must connect, map, and accept events.
        let handle = live.expect("orb must spawn on a live Wayland session");
        let _ = tx.send(PipelineEvent::SpeechStarted);
        let _ = tx.send(PipelineEvent::SpeakingStarted);
        std::thread::sleep(std::time::Duration::from_millis(120));
        drop(handle);
    } else {
        assert!(live.is_none());
    }

    // Headless: spawn must decline cleanly.
    std::env::remove_var("WAYLAND_DISPLAY");
    std::env::remove_var("WAYLAND_SOCKET");
    let (_tx, rx) = tokio::sync::broadcast::channel(8);
    assert!(spawn_orb(rx).is_none());

    if let Some(v) = saved_display {
        std::env::set_var("WAYLAND_DISPLAY", v);
    }
    if let Some(v) = saved_socket {
        std::env::set_var("WAYLAND_SOCKET", v);
    }
}

#[test]
fn activation_mode_roundtrip_and_parse() {
    for m in [
        ActivationMode::Auto,
        ActivationMode::Manual,
        ActivationMode::Off,
    ] {
        assert_eq!(ActivationMode::from_u8(m.as_u8()), m);
        assert_eq!(ActivationMode::parse(m.as_str()), m);
    }
    assert_eq!(ActivationMode::from_u8(99), ActivationMode::Auto);
    assert_eq!(ActivationMode::parse("ptt"), ActivationMode::Manual);
    assert_eq!(ActivationMode::parse("Disabled"), ActivationMode::Off);
    assert_eq!(ActivationMode::parse("nonsense"), ActivationMode::Auto);

    let shared = AtomicU8::new(ActivationMode::Manual.as_u8());
    assert_eq!(ActivationMode::load(&shared), ActivationMode::Manual);
}

#[test]
fn orb_position_parse_presets_and_anchors() {
    assert_eq!(OrbPosition::parse("bottom-left"), OrbPosition::BottomLeft);
    assert_eq!(OrbPosition::parse("TOP-RIGHT"), OrbPosition::TopRight);
    assert_eq!(OrbPosition::parse("custom"), OrbPosition::Custom);
    assert_eq!(OrbPosition::parse("bogus"), OrbPosition::BottomCenter);

    for p in OrbPosition::presets() {
        assert_eq!(OrbPosition::parse(p.as_str()), *p);
    }
    assert_eq!(OrbPosition::presets().len(), 6);

    // Every preset anchors exactly one vertical edge.
    for p in OrbPosition::presets() {
        let (_, _, t, b) = p.anchor();
        assert_ne!(t, b, "{:?} must anchor top xor bottom", p);
    }
    let (l, r, t, b) = OrbPosition::BottomLeft.anchor();
    assert!(l && !r && !t && b);

    let detected = OrbPosition::auto_detect();
    assert!(OrbPosition::presets().contains(&detected));
}

#[test]
fn off_state_renders_dim_and_never_animates() {
    let mut v = OrbVisual::default();
    v.apply_event(&PipelineEvent::SpeechStarted);
    v.state = UiState::Off;
    assert!(!v.animating());
    assert!(state_alpha(UiState::Off) < state_alpha(UiState::Idle));

    let px = render_frame(96, UiState::Off, 0.0, 1.0);
    assert_eq!(px.len(), 96 * 96 * 4);
}

#[test]
fn menu_rows_map_to_commands() {
    let rows = menu::menu_rows(ActivationMode::Manual, OrbPosition::TopRight);
    assert_eq!(rows.len(), 9);
    assert_eq!(menu::menu_height(&rows), 7 * menu::ROW_H + 2 * menu::SEP_H);

    match menu::row_action(0, &rows) {
        Some(UiCommand::SetMode(m)) => assert_eq!(m, ActivationMode::Auto),
        other => panic!("row 0: {:?}", other),
    }
    match menu::row_action(1, &rows) {
        Some(UiCommand::SetMode(m)) => assert_eq!(m, ActivationMode::Manual),
        other => panic!("row 1: {:?}", other),
    }
    match menu::row_action(2, &rows) {
        Some(UiCommand::SetMode(m)) => assert_eq!(m, ActivationMode::Off),
        other => panic!("row 2: {:?}", other),
    }
    // Position row cycles presets; TopRight wraps to BottomLeft.
    match menu::row_action(4, &rows) {
        Some(UiCommand::SetPlacement { position, .. }) => {
            assert_eq!(position, OrbPosition::BottomLeft);
        }
        other => panic!("row 4: {:?}", other),
    }
    let mid_rows = menu::menu_rows(ActivationMode::Auto, OrbPosition::BottomLeft);
    match menu::row_action(4, &mid_rows) {
        Some(UiCommand::SetPlacement { position, .. }) => {
            assert_eq!(position, OrbPosition::BottomCenter);
        }
        other => panic!("row 4 cycle: {:?}", other),
    }
    assert!(matches!(
        menu::row_action(6, &rows),
        Some(UiCommand::ToggleChat)
    ));
    assert!(matches!(
        menu::row_action(7, &rows),
        Some(UiCommand::OpenSettings)
    ));
    assert!(matches!(
        menu::row_action(8, &rows),
        Some(UiCommand::Shutdown)
    ));
    assert!(menu::row_action(3, &rows).is_none());
    assert!(menu::row_action(5, &rows).is_none());
    assert!(menu::row_action(99, &rows).is_none());
}

#[test]
fn menu_hit_testing_and_geometry() {
    let rows = menu::menu_rows(ActivationMode::Auto, OrbPosition::BottomLeft);
    assert_eq!(menu::hit_row(&rows, 1.0), Some(0));
    assert_eq!(menu::hit_row(&rows, menu::ROW_H as f32 + 1.0), Some(1));
    // The separator at index 3 is inert.
    let sep_top = menu::row_top(&rows, 3);
    assert_eq!(menu::hit_row(&rows, sep_top as f32 + 1.0), None);
    assert_eq!(menu::hit_row(&rows, -1.0), None);
    assert_eq!(
        menu::hit_row(&rows, menu::menu_height(&rows) as f32 + 10.0),
        None
    );
    assert_eq!(menu::hit_row(&[], 0.0), None);
}

#[test]
fn menu_renders_pixels() {
    let rows = menu::menu_rows(ActivationMode::Auto, OrbPosition::BottomLeft);
    let px = menu::render_menu(&rows, Some(0));
    assert_eq!(
        px.len(),
        (menu::MENU_WIDTH * menu::menu_height(&rows) * 4) as usize
    );
    // Panel background is opaque near the center.
    let mid =
        ((menu::menu_height(&rows) / 2 * menu::MENU_WIDTH + menu::MENU_WIDTH / 2) * 4) as usize;
    assert!(px[mid + 3] > 200);
    // Label pixels are drawn: row 0's text area contains lit glyph pixels.
    let mut bright = 0u32;
    for y in 8..22 {
        for x in 10..110 {
            let i = ((y * menu::MENU_WIDTH + x) * 4) as usize;
            if px[i + 3] == 255 && px[i] > 200 {
                bright += 1;
            }
        }
    }
    assert!(bright > 20, "bitmap text must draw visible pixels");
}

#[test]
fn settings_model_builds_all_sections() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let model = settings::build_model(&snap);

    // Every snapshot entry appears exactly once as a Setting row.
    let setting_rows: Vec<usize> = model
        .rows
        .iter()
        .filter_map(|r| match r {
            settings::SettingsRow::Setting(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(setting_rows.len(), model.entries.len());
    for i in 0..model.entries.len() {
        assert!(setting_rows.contains(&i));
    }

    // Section headers in order.
    let headers: Vec<&str> = model
        .rows
        .iter()
        .filter_map(|r| match r {
            settings::SettingsRow::Header(h) => Some(*h),
            _ => None,
        })
        .collect();
    assert_eq!(
        headers,
        vec![
            "INTERFACE",
            "LISTENING",
            "WAKE WORD",
            "ROUTING",
            "VOICE",
            "GENERATION",
            "MEMORY",
            "BACKEND"
        ]
    );

    // Info rows are last; footer chrome lives outside the row list.
    assert!(matches!(
        model.rows.last(),
        Some(settings::SettingsRow::Info(_))
    ));
    assert!(settings::panel_height(&model) > settings::TITLE_H + 300);
    // The panel is capped for small outputs; content scrolls.
    assert!(settings::panel_height(&model) <= settings::MAX_PANEL_H);
    assert!(settings::content_height(&model) > settings::viewport_height(&model));
}

#[test]
fn settings_hit_testing_targets_rows() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let model = settings::build_model(&snap);

    // First Setting row is "ui.mode" (entry 0), right after the
    // INTERFACE header.
    let y = (settings::TITLE_H + settings::HEADER_H + 2) as f32;
    assert_eq!(
        settings::hit_row(&model, 40.0, y),
        Some(settings::SettingsHit::Toggle(0))
    );
    assert_eq!(
        settings::hover_at(&model, 40.0, y),
        Some(settings::HoverMark::Row(1))
    );

    // Header rows are inert.
    let header_y = settings::TITLE_H as f32 + 1.0;
    assert_eq!(settings::hit_row(&model, 40.0, header_y), None);
    assert_eq!(settings::hover_at(&model, 40.0, header_y), None);

    // Title band: [x] is interactive, the rest is inert.
    assert_eq!(settings::hit_row(&model, 5.0, 5.0), None);
    assert_eq!(
        settings::hit_row(&model, settings::SETTINGS_WIDTH as f32 - 10.0, 5.0),
        Some(settings::SettingsHit::Close)
    );
    assert_eq!(
        settings::hover_at(&model, settings::SETTINGS_WIDTH as f32 - 10.0, 5.0),
        Some(settings::HoverMark::Close)
    );
    assert_eq!(
        settings::hit_row(&model, 40.0, settings::panel_height(&model) as f32 + 4.0),
        None
    );

    // Footer splits into two buttons at the midline.
    let footer_top = (settings::panel_height(&model) - settings::FOOTER_H) as f32 + 2.0;
    assert_eq!(
        settings::hit_row(&model, 60.0, footer_top),
        Some(settings::SettingsHit::Action(
            settings::SettingsAction::OpenConfigFile
        ))
    );
    assert_eq!(
        settings::hit_row(&model, settings::SETTINGS_WIDTH as f32 - 60.0, footer_top),
        Some(settings::SettingsHit::Action(
            settings::SettingsAction::Done
        ))
    );
    assert_eq!(
        settings::hover_at(&model, settings::SETTINGS_WIDTH as f32 - 60.0, footer_top),
        Some(settings::HoverMark::Button(settings::SettingsAction::Done))
    );
}

#[test]
fn settings_cycle_and_command_mapping() {
    use rust_voice_assistant::ui::settings;
    let mut entry = rust_voice_assistant::ui::SettingEntry {
        key: "ui.mode",
        label: "Activation",
        options: vec!["auto".into(), "manual".into(), "off".into()],
        current: "auto".to_string(),
        live: true,
    };
    entry.cycle_next();
    assert_eq!(entry.current, "manual");
    entry.cycle_next();
    entry.cycle_next();
    assert_eq!(entry.current, "auto", "cycle wraps");

    // Custom value outside presets cycles into the preset list.
    entry.current = "weird".into();
    entry.cycle_next();
    assert_eq!(entry.current, "auto");

    match settings::command_for_entry(&entry) {
        UiCommand::SetMode(m) => assert_eq!(m, ActivationMode::Auto),
        other => panic!("ui.mode must map to SetMode: {:?}", other),
    }

    let pos = rust_voice_assistant::ui::SettingEntry {
        key: "ui.position",
        label: "Orb position",
        options: vec!["bottom-left".into(), "bottom-right".into()],
        current: "bottom-right".to_string(),
        live: true,
    };
    match settings::command_for_entry(&pos) {
        UiCommand::SetPlacement { position, .. } => {
            assert_eq!(position, OrbPosition::BottomRight)
        }
        other => panic!("ui.position must map to SetPlacement: {:?}", other),
    }

    let generic = rust_voice_assistant::ui::SettingEntry {
        key: "tts.speed",
        label: "Speech rate",
        options: vec!["1.0".into(), "1.5".into()],
        current: "1.5".to_string(),
        live: false,
    };
    match settings::command_for_entry(&generic) {
        UiCommand::SetConfig { key, value } => {
            assert_eq!(key, "tts.speed");
            assert_eq!(value, "1.5");
        }
        other => panic!("generic keys must map to SetConfig: {:?}", other),
    }
}

#[test]
fn settings_snapshot_reflects_config() {
    let mut cfg = rust_voice_assistant::config::TychoConfig::default();
    cfg.ui.mode = "manual".into();
    cfg.vad.energy_threshold = 0.05;
    cfg.generation.model = "custom-model:9b".into();
    let snap = rust_voice_assistant::ui::settings_snapshot(&cfg);

    let mode = snap.entries.iter().find(|e| e.key == "ui.mode").unwrap();
    assert_eq!(mode.current, "manual");
    assert!(mode.live);
    let model = snap
        .entries
        .iter()
        .find(|e| e.key == "generation.model")
        .unwrap();
    assert_eq!(model.current, "custom-model:9b");
    assert!(!model.live, "model change requires restart");
    // Custom model is cycled into the option list.
    assert!(model.values().contains(&"custom-model:9b".to_string()));

    // Wake-word entries ship disabled by default, live-applicable,
    // and offer the pretrained openWakeWord models.
    let wake = snap.entries.iter().find(|e| e.key == "wake.model").unwrap();
    assert_eq!(wake.current, "off");
    assert!(wake.live);
    assert!(wake.values().contains(&"hey_jarvis".to_string()));
    let sens = snap
        .entries
        .iter()
        .find(|e| e.key == "wake.threshold")
        .unwrap();
    assert_eq!(sens.current, "0.5");
    assert!(sens.live);

    assert!(snap.info.iter().any(|(k, _)| *k == "Endpoint"));
}

#[test]
fn settings_renders_pixels() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let model = settings::build_model(&snap);
    let (sw, sh) = (settings::surface_width(), settings::surface_height(&model));
    let px = settings::render_settings(&model, Some(settings::HoverMark::Row(1)));
    assert_eq!(px.len(), (sw * sh * 4) as usize);

    // Opaque panel center.
    let mid = ((sh / 2 * sw + sw / 2) * 4) as usize;
    assert!(px[mid + 3] > 200);

    // Corners of the surface are the transparent shadow margin.
    assert_eq!(px[3], 0, "top-left shadow margin must be transparent");
    let last = px.len() - 4;
    assert_eq!(px[last + 3], 0);

    // Title text drew bright pixels inside the title band (offset by
    // the shadow pad).
    let pad = settings::SURFACE_PAD;
    let mut bright = 0u32;
    for y in (pad + 6)..(pad + 26) {
        for x in (pad + 14)..(pad + 140) {
            let i = ((y * sw + x) * 4) as usize;
            if px[i + 3] == 255 && px[i] > 150 {
                bright += 1;
            }
        }
    }
    assert!(bright > 20, "title text must draw visible pixels");

    // The drop shadow darkened pixels just outside the panel edge.
    let below = ((sh - 8) * sw + sw / 2) as usize * 4;
    assert!(px[below + 3] > 0, "shadow pixels must exist below panel");
}

#[test]
fn settings_dropdown_select_and_dismiss() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let mut model = settings::build_model(&snap);

    // ui.mode is entry 0, first row after the INTERFACE header.
    let row_y = (settings::TITLE_H + settings::HEADER_H + 2) as f32;
    settings::toggle_dropdown(&mut model, 0);
    assert_eq!(model.open, Some(0));
    assert_eq!(model.open_opts, vec!["auto", "manual", "off"]);

    // Clicking outside the dropdown while it is open is a Dismiss.
    assert!(matches!(
        settings::hit_row(&model, 40.0, row_y),
        Some(settings::SettingsHit::Dismiss)
    ));

    // The dropdown opens below the chip; probe downward from the row for
    // a hoverable option row.
    let mut opt_y = None;
    for yy in ((row_y as u32)..(settings::panel_height(&model) - 1)).step_by(2) {
        if let Some(settings::HoverMark::Option(_)) = settings::hover_at(&model, 250.0, yy as f32) {
            opt_y = Some(yy as f32);
            break;
        }
    }
    let opt_y = opt_y.expect("dropdown option must be hoverable below the chip");
    assert_eq!(
        settings::hit_row(&model, 250.0, opt_y),
        Some(settings::SettingsHit::Select(0))
    );

    // A click far outside the dropdown is a Dismiss, not a row hit.
    assert!(matches!(
        settings::hit_row(&model, 40.0, row_y),
        Some(settings::SettingsHit::Dismiss)
    ));

    // Selecting a non-current option maps to the right command.
    model.entries[0].current = model.open_opts[1].clone();
    match settings::command_for_entry(&model.entries[0]) {
        UiCommand::SetMode(m) => assert_eq!(m, ActivationMode::Manual),
        other => panic!("ui.mode select must map to SetMode: {:?}", other),
    }
}

#[test]
fn settings_scroll_clamps_and_closes_dropdown() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let mut model = settings::build_model(&snap);
    let max = settings::max_scroll(&model);
    assert!(max > 0.0, "19-row model must exceed the viewport");

    settings::toggle_dropdown(&mut model, 0);
    settings::scroll_by(&mut model, max + 500.0);
    assert_eq!(model.scroll, max);
    assert_eq!(model.open, None, "scrolling closes the dropdown");

    settings::scroll_by(&mut model, -9999.0);
    assert_eq!(model.scroll, 0.0);

    // Scrolled hit-testing: after scrolling past the INTERFACE header
    // plus the ui.mode row (26 + 31 = 57), the viewport top lands on
    // ui.position (entry 1).
    let top_y = settings::TITLE_H as f32 + 2.0;
    settings::scroll_by(&mut model, 58.0);
    assert_eq!(
        settings::hit_row(&model, 40.0, top_y),
        Some(settings::SettingsHit::Toggle(1))
    );
}

#[test]
fn settings_dropdown_renders_overlay() {
    use rust_voice_assistant::ui::settings;
    let snap = rust_voice_assistant::ui::settings_snapshot(
        &rust_voice_assistant::config::TychoConfig::default(),
    );
    let mut model = settings::build_model(&snap);
    settings::toggle_dropdown(&mut model, 0);
    let px = settings::render_settings(&model, Some(settings::HoverMark::Option(1)));
    let (sw, sh) = (settings::surface_width(), settings::surface_height(&model));
    assert_eq!(px.len(), (sw * sh * 4) as usize);
    // The option rows draw bright text inside the viewport band.
    let pad = settings::SURFACE_PAD;
    let mut bright = 0u32;
    for y in (pad + 40)..(pad + 120) {
        for x in (pad + 150)..(sw - pad - 20) {
            let i = ((y * sw + x) * 4) as usize;
            if px[i + 3] == 255 && px[i] > 200 {
                bright += 1;
            }
        }
    }
    assert!(bright > 10, "dropdown options must draw visible pixels");
}
