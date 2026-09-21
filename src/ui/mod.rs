//! Optional overlay UI: a persistent floating status orb.
//!
//! The orb is a small `zwlr_layer_shell_v1` overlay surface rendered
//! entirely on the CPU with tiny-skia and presented via `wl_shm`. It
//! subscribes to the pipeline event broadcast and animates according to
//! the assistant's current activity.

pub mod chat;
pub mod menu;
pub mod orb;
pub mod settings;

use crate::pipeline::events::PipelineEvent;
use std::sync::atomic::{AtomicU8, Ordering};

/// How user input reaches the assistant. Stored as an `AtomicU8` so the
/// orb thread and the pipeline coordinator share it without locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationMode {
    /// VAD continuously watches for speech onset.
    Auto,
    /// Only a left-click on the orb starts an utterance.
    Manual,
    /// The assistant ignores all input until reactivated.
    Off,
}

impl ActivationMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Manual,
            2 => Self::Off,
            _ => Self::Auto,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Manual => 1,
            Self::Off => 2,
        }
    }

    /// Parses a config value; unknown values fall back to `Auto`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" | "push-to-talk" | "ptt" => Self::Manual,
            "off" | "disabled" => Self::Off,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
            Self::Off => "off",
        }
    }

    /// Shared handle helpers so both sides speak the same encoding.
    pub fn load(shared: &AtomicU8) -> Self {
        Self::from_u8(shared.load(Ordering::Relaxed))
    }
}

/// Where the orb anchors when the user has not dragged it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbPosition {
    BottomLeft,
    BottomCenter,
    BottomRight,
    TopLeft,
    TopCenter,
    TopRight,
    /// Freely dragged position — uses `ui.margin_x`/`ui.margin_y`.
    Custom,
}

impl OrbPosition {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bottom-left" | "left" => Self::BottomLeft,
            "bottom-right" | "right" => Self::BottomRight,
            "top-left" => Self::TopLeft,
            "top-center" => Self::TopCenter,
            "top-right" => Self::TopRight,
            "custom" => Self::Custom,
            _ => Self::BottomCenter,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BottomLeft => "bottom-left",
            Self::BottomCenter => "bottom-center",
            Self::BottomRight => "bottom-right",
            Self::TopLeft => "top-left",
            Self::TopCenter => "top-center",
            Self::TopRight => "top-right",
            Self::Custom => "custom",
        }
    }

    /// Named presets the menu cycles through (Custom excluded).
    pub fn presets() -> &'static [OrbPosition] {
        &[
            Self::BottomLeft,
            Self::BottomCenter,
            Self::BottomRight,
            Self::TopLeft,
            Self::TopCenter,
            Self::TopRight,
        ]
    }

    /// Compositor-aware default: Hyprland docks bottom-left, KDE
    /// bottom-right (panel convention), anything else bottom-center.
    pub fn auto_detect() -> Self {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            Self::BottomLeft
        } else if std::env::var_os("KDE_FULL_SESSION").is_some()
            || std::env::var("XDG_CURRENT_DESKTOP")
                .map(|d| d.to_ascii_lowercase().contains("kde"))
                .unwrap_or(false)
        {
            Self::BottomRight
        } else {
            Self::BottomCenter
        }
    }

    /// Anchor edge flags `(left, right, top, bottom)` for this position.
    pub fn anchor(self) -> (bool, bool, bool, bool) {
        match self {
            Self::BottomLeft => (true, false, false, true),
            Self::BottomCenter => (false, false, false, true),
            Self::BottomRight => (false, true, false, true),
            Self::TopLeft => (true, false, true, false),
            Self::TopCenter => (false, false, true, false),
            Self::TopRight => (false, true, true, false),
            Self::Custom => (true, false, true, false),
        }
    }
}

/// Commands the orb sends back into the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    /// Left-click: capture one utterance now, bypassing VAD.
    ListenNow,
    /// Menu selection: change activation mode (persisted).
    SetMode(ActivationMode),
    /// Drag release or menu selection: persist placement prefs.
    SetPlacement {
        position: OrbPosition,
        margin_x: i32,
        margin_y: i32,
    },
    /// Open the graphical settings window (handled by the orb itself;
    /// never reaches the pipeline).
    OpenSettings,
    /// Open the configuration file in the default editor.
    OpenConfig,
    /// Orb-local: toggle the chat panel (double-click or menu row);
    /// never reaches the pipeline.
    ToggleChat,
    /// Double-click companion: abort a pending orb-triggered capture
    /// without a reprompt — the first click of a double-click may have
    /// opened one the user never meant to start.
    CancelListen,
    /// Chat widget: run a typed query through the normal pipeline.
    SubmitQuery(String),
    /// Settings window: write a dotted-key config value. Live-applicable
    /// keys take effect immediately; the rest persist for next launch.
    SetConfig { key: String, value: String },
    /// Internal: TTS provisioning finished — rebuild the synthesizer
    /// from the current engine/voice/speed settings.
    ReloadTts,
    /// Internal: wake-word provisioning finished — start the sidecar
    /// with the current model/threshold.
    ReloadWake,
    /// Quit the assistant.
    Shutdown,
}

/// One editable setting shown in the settings window. `options` are the
/// values a click cycles through; `live` marks settings that apply
/// immediately instead of on next launch.
#[derive(Debug, Clone)]
pub struct SettingEntry {
    /// Dotted TOML key, e.g. `vad.energy_threshold`.
    pub key: &'static str,
    /// Human-readable row label.
    pub label: &'static str,
    /// Candidate values offered by the dropdown.
    pub options: Vec<String>,
    /// Current value (inserted into `options` when not already listed).
    pub current: String,
    /// True when the pipeline applies the change without a restart.
    pub live: bool,
}

impl SettingEntry {
    /// All dropdown values, with the current value prepended when it is
    /// a custom value outside the preset list.
    pub fn values(&self) -> Vec<String> {
        let mut v = self.options.clone();
        if !v.iter().any(|o| o == &self.current) {
            v.insert(0, self.current.clone());
        }
        v
    }

    /// Advances `current` to the next option, wrapping around.
    pub fn cycle_next(&mut self) {
        let v = self.values();
        let idx = v.iter().position(|o| o == &self.current).unwrap_or(0);
        self.current = v[(idx + 1) % v.len()].clone();
    }
}

/// Snapshot of editable configuration handed to the orb at startup.
#[derive(Debug, Clone)]
pub struct SettingsSnapshot {
    pub entries: Vec<SettingEntry>,
    /// Read-only rows (key, value) shown under the Backend section.
    pub info: Vec<(&'static str, String)>,
    /// Persisted chat transcript verbosity ("quiet" | "normal" |
    /// "verbose") — the chat panel owns the live control.
    pub chat_verbosity: String,
}

fn opts(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Builds the settings-window contents from the active configuration.
pub fn settings_snapshot(config: &crate::config::TychoConfig) -> SettingsSnapshot {
    let pos = if config.ui.position.trim().eq_ignore_ascii_case("auto") {
        OrbPosition::auto_detect().as_str().to_string()
    } else {
        config.ui.position.clone()
    };
    SettingsSnapshot {
        entries: vec![
            SettingEntry {
                key: "ui.mode",
                label: "Activation",
                options: opts(&["auto", "manual", "off"]),
                current: config.ui.mode.clone(),
                live: true,
            },
            SettingEntry {
                key: "ui.position",
                label: "Orb position",
                options: opts(&[
                    "bottom-left",
                    "bottom-center",
                    "bottom-right",
                    "top-left",
                    "top-center",
                    "top-right",
                ]),
                current: pos,
                live: true,
            },
            SettingEntry {
                key: "ui.orb",
                label: "Orb enabled",
                options: opts(&["true", "false"]),
                current: config.ui.orb.to_string(),
                live: false,
            },
            SettingEntry {
                key: "vad.energy_threshold",
                label: "VAD sensitivity",
                options: opts(&["0.005", "0.01", "0.02", "0.035", "0.05", "0.08"]),
                current: format!("{}", config.vad.energy_threshold),
                live: true,
            },
            SettingEntry {
                key: "vad.min_speech_duration_ms",
                label: "Min speech ms",
                options: opts(&["150", "250", "400", "600"]),
                current: config.vad.min_speech_duration_ms.to_string(),
                live: true,
            },
            SettingEntry {
                key: "vad.min_silence_duration_ms",
                label: "Silence timeout ms",
                options: opts(&["400", "600", "900", "1200", "1600"]),
                current: config.vad.min_silence_duration_ms.to_string(),
                live: true,
            },
            SettingEntry {
                key: "stt.language",
                label: "STT language",
                options: opts(&["en", "es", "de", "fr", "it", "pt"]),
                current: config.stt.language.clone(),
                live: false,
            },
            SettingEntry {
                key: "ui.earcons",
                label: "Earcons",
                options: opts(&["true", "false"]),
                current: config.ui.earcons.to_string(),
                live: true,
            },
            SettingEntry {
                key: "wake.model",
                label: "Wake word",
                options: crate::wake::model_options(),
                current: config.wake.model.clone(),
                live: true,
            },
            SettingEntry {
                key: "wake.threshold",
                label: "Wake sensitivity",
                options: opts(&["0.3", "0.5", "0.7", "0.9"]),
                current: format!("{}", config.wake.threshold),
                live: true,
            },
            SettingEntry {
                key: "wake.vad_gate",
                label: "Wake speech gate",
                options: opts(&["0", "0.3", "0.5", "0.7"]),
                current: format!("{}", config.wake.vad_gate),
                live: true,
            },
            SettingEntry {
                key: "wake.follow_up_seconds",
                label: "Follow-up window s",
                options: opts(&["0", "4", "6", "10"]),
                current: config.wake.follow_up_seconds.to_string(),
                live: true,
            },
            SettingEntry {
                key: "router.fast_path_confidence_threshold",
                label: "Fast-path confidence",
                options: opts(&["0.5", "0.65", "0.82", "0.9", "0.95"]),
                current: format!("{}", config.router.fast_path_confidence_threshold),
                live: true,
            },
            SettingEntry {
                key: "tts.speed",
                label: "Speech rate",
                options: opts(&["0.8", "0.9", "1", "1.1", "1.25", "1.5"]),
                current: format!("{}", config.tts.speed),
                live: true,
            },
            SettingEntry {
                key: "tts.voice",
                label: "Voice",
                options: crate::tts::catalog::voice_options(&config.models.cache_dir),
                current: config.tts.voice.clone(),
                live: true,
            },
            SettingEntry {
                key: "tts.engine",
                label: "TTS engine",
                options: crate::tts::catalog::engine_options(),
                current: config.tts.engine.clone(),
                live: true,
            },
            SettingEntry {
                key: "generation.persona",
                label: "Persona",
                options: opts(&["sentinel", "scholar", "consigliere", "companion"]),
                current: config.generation.persona.name().to_lowercase(),
                live: true,
            },
            SettingEntry {
                key: "generation.model",
                label: "Chat model",
                options: opts(&[
                    "llama3.2:1b",
                    "llama3.2:3b",
                    "qwen2.5:3b",
                    "qwen2.5:7b",
                    "mistral:7b",
                ]),
                current: config.generation.model.clone(),
                live: false,
            },
            SettingEntry {
                key: "generation.temperature",
                label: "Temperature",
                options: opts(&["0.0", "0.3", "0.5", "0.7", "1.0"]),
                current: format!("{}", config.generation.temperature),
                live: true,
            },
            SettingEntry {
                key: "generation.max_tokens",
                label: "Max tokens",
                options: opts(&["128", "256", "512", "1024", "2048"]),
                current: config.generation.max_tokens.to_string(),
                live: true,
            },
            SettingEntry {
                key: "memory.enable_long_term",
                label: "Long-term memory",
                options: opts(&["true", "false"]),
                current: config.memory.enable_long_term.to_string(),
                live: false,
            },
            SettingEntry {
                key: "desktop.backend",
                label: "Desktop backend",
                options: opts(&["auto", "hyprland", "kde"]),
                current: config.desktop.backend.clone(),
                live: false,
            },
            SettingEntry {
                key: "ui.chat_verbosity",
                label: "Chat verbosity",
                options: opts(&["quiet", "normal", "verbose"]),
                current: config.ui.chat_verbosity.clone(),
                live: true,
            },
        ],
        info: vec![
            ("Endpoint", config.generation.endpoint.clone()),
            ("STT", config.models.stt_model.target_filename.clone()),
            ("TTS", config.tts.voice.clone()),
        ],
        chat_verbosity: config.ui.chat_verbosity.clone(),
    }
}

/// High-level assistant state rendered by the orb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiState {
    /// Passive listening; the orb is a dim, static disc.
    Idle,
    /// VAD detected speech; the orb pulses with voice amplitude.
    Listening,
    /// Transcription, routing, or generation is in flight.
    Thinking,
    /// TTS audio is being played back.
    Speaking,
    /// Activation is Off — the orb renders nearly invisible.
    Off,
}

/// Mutable visual state driven by pipeline events.
#[derive(Debug, Clone)]
pub struct OrbVisual {
    pub state: UiState,
    /// Recent voice loudness (RMS), 0.0..=1.0. Decays each frame.
    pub amplitude: f32,
    /// Animation phase in radians, advanced once per rendered frame.
    pub phase: f32,
    /// Set when state changed since the last presented frame.
    pub dirty: bool,
}

impl Default for OrbVisual {
    fn default() -> Self {
        Self {
            state: UiState::Idle,
            amplitude: 0.0,
            phase: 0.0,
            dirty: true,
        }
    }
}

impl OrbVisual {
    /// Maps a pipeline event onto orb visuals.
    pub fn apply_event(&mut self, event: &PipelineEvent) {
        match event {
            PipelineEvent::SpeechStarted => self.state = UiState::Listening,
            PipelineEvent::SpeechProgress { rms } => {
                if self.state == UiState::Listening {
                    self.amplitude = self.amplitude.max(rms.clamp(0.0, 1.0));
                }
            }
            PipelineEvent::SpeechEnded
            | PipelineEvent::TranscriptionCompleted(_)
            | PipelineEvent::FastPathRouted { .. }
            | PipelineEvent::DeliberationRouted { .. } => self.state = UiState::Thinking,
            PipelineEvent::SpeakingStarted => self.state = UiState::Speaking,
            PipelineEvent::SpeakingFinished
            | PipelineEvent::DesktopActionExecuted { .. }
            | PipelineEvent::SynthesisCompleted { .. }
            | PipelineEvent::Idle => self.state = UiState::Idle,
            PipelineEvent::GenerationCompleted { .. } => {}
        }
        self.dirty = true;
    }

    /// True while a state animation should keep producing frames.
    pub fn animating(&self) -> bool {
        matches!(
            self.state,
            UiState::Listening | UiState::Thinking | UiState::Speaking
        )
    }
}

/// Base RGB color for each state.
pub fn state_color(state: UiState) -> (u8, u8, u8) {
    match state {
        UiState::Idle => (96, 104, 148),
        UiState::Listening => (86, 168, 255),
        UiState::Thinking => (172, 118, 255),
        UiState::Speaking => (88, 220, 172),
        UiState::Off => (70, 74, 86),
    }
}

/// Base alpha (0..=1) for each state; Idle stays subdued.
pub fn state_alpha(state: UiState) -> f32 {
    match state {
        UiState::Off => 0.30,
        UiState::Idle => 0.55,
        UiState::Listening | UiState::Thinking | UiState::Speaking => 1.0,
    }
}

/// Source-over composite of a soft circular dot into premultiplied px.
fn splat(px: &mut [u8], size: u32, cx: f32, cy: f32, rad: f32, rgb: (u8, u8, u8), alpha: f32) {
    let min_x = (cx - rad - 1.0).max(0.0) as u32;
    let max_x = (cx + rad + 1.0).min(size as f32) as u32;
    let min_y = (cy - rad - 1.0).max(0.0) as u32;
    let max_y = (cy + rad + 1.0).min(size as f32) as u32;
    for y in min_y..=max_y.min(size - 1) {
        for x in min_x..=max_x.min(size - 1) {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let a = alpha * (1.0 - d / rad).clamp(0.0, 1.0) / 255.0;
            if a <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            for (k, s) in [rgb.0, rgb.1, rgb.2].iter().enumerate() {
                let src = *s as f32 * a;
                px[i + k] = (src + px[i + k] as f32 * (1.0 - a)) as u8;
            }
            let da = px[i + 3] as f32 / 255.0;
            px[i + 3] = ((a + da * (1.0 - a)) * 255.0) as u8;
        }
    }
}

/// Renders one orb frame into premultiplied RGBA8 pixels.
///
/// The orb is procedural: a luminous sphere with a hot inner core,
/// diffuse limb shading, a specular glint keyed from the upper-left,
/// and a fresnel rim. Listening shimmers the surface with voice
/// amplitude; Speaking sends luminance ripples outward; Thinking
/// adds orbiting satellites; Off freezes every phase-driven term.
/// `phase` drives animation and `amplitude` scales voice response.
/// Pixels are `size*size*4`, R,G,B,A.
pub fn render_frame(size: u32, state: UiState, phase: f32, amplitude: f32) -> Vec<u8> {
    if size == 0 {
        return Vec::new();
    }
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let (r, g, b) = state_color(state);
    let base_a = state_alpha(state);
    let amp = amplitude.clamp(0.0, 1.0);
    // Off is inert: freeze every phase-driven term.
    let ph = if state == UiState::Off { 0.0 } else { phase };
    let pulse = 0.5 + 0.5 * ph.sin();

    // Key light from the upper-left, slightly toward the viewer.
    let (lx, ly, lz) = (-0.55f32, -0.70f32, 0.46f32);
    let linv = 1.0 / (lx * lx + ly * ly + lz * lz).sqrt();
    let (lx, ly, lz) = (lx * linv, ly * linv, lz * linv);
    // Half-vector between the light and the (0,0,1) view direction.
    let (hx, hy, hz) = (lx, ly, lz + 1.0);
    let hinv = 1.0 / (hx * hx + hy * hy + hz * hz).sqrt();
    let (hx, hy, hz) = (hx * hinv, hy * hinv, hz * hinv);

    let disc_r = c * (0.56 + 0.03 * pulse + 0.04 * amp);

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let d = (dx * dx + dy * dy).sqrt();
            let i = ((y * size + x) * 4) as usize;

            if d <= disc_r {
                // Sphere normal from screen position.
                let nx = dx / disc_r;
                let ny = dy / disc_r;
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let diffuse = (nx * lx + ny * ly + nz * lz).max(0.0);
                let spec = (nx * hx + ny * hy + nz * hz).max(0.0).powi(48);
                let fresnel = (1.0 - nz).powf(2.6);
                // Luminous core: hot center cooling into the state hue.
                let core = (-3.0 * d / disc_r).exp();
                // State-driven surface detail on top of the shading.
                let ang = dy.atan2(dx);
                let shimmer = match state {
                    UiState::Listening => {
                        amp * ((ang * 3.0 + d * 1.2 - ph * 6.0).sin() * 0.5 + 0.5)
                    }
                    UiState::Speaking => (d * 1.4 - ph * 7.0).sin() * 0.5 + 0.5,
                    UiState::Thinking => ((d * 0.9 - ph * 2.0).sin() * 0.5 + 0.5) * 0.4,
                    _ => 0.0,
                };
                let lum = 0.18 + 0.82 * core + shimmer * 0.35;
                let shade = 0.40 + 0.60 * diffuse;
                let cr = (r as f32 * lum * shade + 255.0 * spec * 0.85 + r as f32 * fresnel * 0.55)
                    .min(255.0);
                let cg = (g as f32 * lum * shade + 255.0 * spec * 0.85 + g as f32 * fresnel * 0.55)
                    .min(255.0);
                let cb = (b as f32 * lum * shade + 255.0 * spec * 0.85 + b as f32 * fresnel * 0.55)
                    .min(255.0);
                // Opaque body — the halo softens the silhouette.
                let a = base_a;
                px[i] = (cr * a) as u8;
                px[i + 1] = (cg * a) as u8;
                px[i + 2] = (cb * a) as u8;
                px[i + 3] = (a * 255.0) as u8;
            } else {
                // Halo: exponential falloff reaching zero exactly at the
                // surface edge; amplitude widens it.
                let span = (c - disc_r).max(1.0);
                let t = ((d - disc_r) / span).clamp(0.0, 1.0);
                let strength = base_a * (0.55 + 0.20 * pulse + 0.35 * amp);
                let a = strength * (-4.5 * t).exp() * (1.0 - t);
                px[i] = (r as f32 * a) as u8;
                px[i + 1] = (g as f32 * a) as u8;
                px[i + 2] = (b as f32 * a) as u8;
                px[i + 3] = (a * 255.0) as u8;
            }
        }
    }

    // Thinking: three satellites orbiting the disc rim.
    if state == UiState::Thinking {
        let orbit = c * 0.72;
        for k in 0..3 {
            let angle = ph + k as f32 * (std::f32::consts::TAU / 3.0);
            splat(
                &mut px,
                size,
                c + orbit * angle.cos(),
                c + orbit * angle.sin(),
                size as f32 * 0.045,
                (r, g, b),
                140.0 + 115.0 * (k as f32 / 3.0),
            );
        }
    }

    px
}

/// Converts premultiplied RGBA8 into the B,G,R,A byte order of
/// `wl_shm` `Argb8888` buffers on little-endian hosts.
pub fn rgba_to_argb8888(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}
