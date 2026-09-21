//! Chat widget: a layer-shell panel docked beside the orb, opened by
//! double-clicking it. Shows the running transcript (voice and typed
//! turns alike — everything flows through `PipelineEvent`s) and
//! accepts typed queries via keyboard focus. The model is pure so
//! wrapping, scrolling and hit-testing are unit-testable headless.

use super::menu::{char_width, draw_text, draw_title, text_width, GLYPH_PX};
use crate::pipeline::events::PipelineEvent;
use tiny_skia::{
    Color, FillRule, LinearGradient, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode,
    Transform,
};

/// Chat panel width in logical pixels (excludes shadow margin).
pub const CHAT_WIDTH: u32 = 340;
/// Chat panel height in logical pixels (fixed — the transcript
/// scrolls inside it).
pub const CHAT_HEIGHT: u32 = 430;
/// Transparent margin around the panel reserved for the drop shadow.
pub const SURFACE_PAD: u32 = 14;
/// Title band height.
pub const TITLE_H: u32 = 36;
/// Bottom input band height.
pub const INPUT_H: u32 = 44;
/// Transcript line height.
const LINE_H: f32 = 19.0;
/// Vertical gap between message bubbles.
const MSG_GAP: f32 = 7.0;
/// Inner bubble padding (x, y).
const BUBBLE_PAD: (f32, f32) = (9.0, 5.0);
/// Transcript retention — older turns drop off the panel (the
/// persistent memory store keeps the full history).
const MAX_MESSAGES: usize = 64;

/// Upper bound on the compose field so a stuck key or paste flood
/// can't grow it without limit.
const MAX_INPUT_LEN: usize = 2048;
/// Horizontal text inset.
const PAD_X: f32 = 14.0;
/// Width of the title-band close-button hit zone.
const CLOSE_ZONE: f32 = 44.0;

const ACCENT: (u8, u8, u8, u8) = (116, 162, 255, 255);
const TXT: (u8, u8, u8, u8) = (226, 229, 240, 255);
const TXT_DIM: (u8, u8, u8, u8) = (146, 152, 172, 255);
const TXT_FAINT: (u8, u8, u8, u8) = (108, 113, 134, 255);

/// Who a transcript line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Tycho,
    /// Pipeline lifecycle line — faint, centered, no bubble.
    Status,
}

/// How much of the pipeline's event stream surfaces in the
/// transcript. Conversation turns always show; status lines are
/// gated by level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatVerbosity {
    /// User and Tycho turns only.
    Quiet,
    /// + listening/thinking/speaking lifecycle and routed actions.
    #[default]
    Normal,
    /// + routing confidence, synthesis stats, state settle lines.
    Verbose,
}

impl ChatVerbosity {
    /// Cycles Quiet → Normal → Verbose.
    pub fn next(self) -> Self {
        match self {
            Self::Quiet => Self::Normal,
            Self::Normal => Self::Verbose,
            Self::Verbose => Self::Quiet,
        }
    }

    /// Short label for the title-band chip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Quiet => "Quiet",
            Self::Normal => "Normal",
            Self::Verbose => "Verbose",
        }
    }

    /// Parses a persisted value; unknown strings fall back to Normal.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "quiet" | "minimal" => Self::Quiet,
            "verbose" | "debug" | "all" => Self::Verbose,
            _ => Self::Normal,
        }
    }
}

/// One transcript turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMsg {
    pub role: ChatRole,
    pub text: String,
    /// Transient status lines update in place — the next transient
    /// status replaces this one instead of appending.
    pub transient: bool,
}

/// Pure chat state: transcript, in-progress input, and scrollback.
/// `scroll_up` is the px distance above the newest message — 0 pins
/// the view to the latest turn.
#[derive(Debug, Clone)]
pub struct ChatModel {
    pub messages: Vec<ChatMsg>,
    pub input: String,
    pub scroll_up: f32,
    /// Which event classes reach the transcript.
    pub verbosity: ChatVerbosity,
    /// Throttle for per-frame `SpeechProgress` writes into the
    /// transient listening line.
    last_progress: Option<std::time::Instant>,
}

impl Default for ChatModel {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            scroll_up: 0.0,
            verbosity: ChatVerbosity::Normal,
            last_progress: None,
        }
    }
}

impl ChatModel {
    /// A model starting at a specific verbosity level (e.g. the
    /// persisted `ui.chat_verbosity` value).
    pub fn with_verbosity(verbosity: ChatVerbosity) -> Self {
        Self {
            verbosity,
            ..Default::default()
        }
    }

    /// Appends a transcript turn, capping retention. When the view is
    /// pinned to the bottom it stays pinned; a scrolled-up reader
    /// keeps their position.
    pub fn push(&mut self, role: ChatRole, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        self.messages.push(ChatMsg {
            role,
            text,
            transient: false,
        });
        if self.messages.len() > MAX_MESSAGES {
            let drop = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..drop);
        }
    }

    /// Appends a faint status line. When `transient` is set and the
    /// tail message is already a transient status, it is updated in
    /// place — live-updating lines like "Listening…" stay one row
    /// instead of flooding the scrollback.
    pub fn push_status(&mut self, text: impl Into<String>, transient: bool) -> bool {
        let text = text.into();
        if text.trim().is_empty() {
            return false;
        }
        if transient {
            if let Some(last) = self.messages.last_mut() {
                if last.role == ChatRole::Status && last.transient {
                    last.text = text;
                    return true;
                }
            }
        }
        self.messages.push(ChatMsg {
            role: ChatRole::Status,
            text,
            transient,
        });
        if self.messages.len() > MAX_MESSAGES {
            let drop = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..drop);
        }
        true
    }

    /// Inserts typed text (utf8 from the keymap) into the input field,
    /// bounded so a stuck key or paste flood can't grow it without limit.
    pub fn input_insert(&mut self, s: &str) {
        for c in s.chars() {
            if !c.is_control() && self.input.len() < MAX_INPUT_LEN {
                self.input.push(c);
            }
        }
    }

    pub fn input_backspace(&mut self) {
        self.input.pop();
    }

    /// Takes the input for submission. `None` when blank.
    pub fn submit(&mut self) -> Option<String> {
        let q = self.input.trim().to_string();
        self.input.clear();
        (!q.is_empty()).then_some(q)
    }

    /// Maps a pipeline event to a transcript turn. Returns true when
    /// the transcript changed (a redraw is due). Conversation turns
    /// always land; lifecycle lines are gated by `verbosity` — Quiet
    /// shows speech only, Normal adds the listening/thinking/speaking
    /// lifecycle and routed actions, Verbose adds confidence,
    /// synthesis stats and settle lines.
    pub fn observe(&mut self, event: &PipelineEvent) -> bool {
        use ChatVerbosity as V;
        match event {
            PipelineEvent::SpeechStarted => {
                if self.verbosity == V::Quiet {
                    return false;
                }
                self.last_progress = None;
                self.push_status("Listening…", true)
            }
            PipelineEvent::SpeechProgress { rms } => {
                // Per-frame loudness rides the transient listening
                // line, throttled so the panel repaints ~8x/s max.
                if self.verbosity != V::Verbose {
                    return false;
                }
                let now = std::time::Instant::now();
                if self
                    .last_progress
                    .map(|t| now.duration_since(t).as_millis() < 120)
                    .unwrap_or(false)
                {
                    return false;
                }
                self.last_progress = Some(now);
                self.push_status(format!("Listening · rms {rms:.3}"), true)
            }
            PipelineEvent::SpeechEnded => {
                if self.verbosity == V::Quiet {
                    return false;
                }
                self.push_status("Transcribing…", false)
            }
            PipelineEvent::TranscriptionCompleted(text) => {
                self.push(ChatRole::User, text.clone());
                true
            }
            PipelineEvent::FastPathRouted {
                intent,
                parameter,
                confidence,
            } => {
                if self.verbosity == V::Quiet {
                    return false;
                }
                let detail = match (parameter, self.verbosity) {
                    (Some(p), V::Verbose) => {
                        format!("→ {intent}({p}) · {:.0}%", confidence * 100.0)
                    }
                    (Some(p), _) => format!("→ {intent}({p})"),
                    (None, V::Verbose) => {
                        format!("→ {intent} · {:.0}%", confidence * 100.0)
                    }
                    (None, _) => format!("→ {intent}"),
                };
                self.push_status(detail, false)
            }
            PipelineEvent::DeliberationRouted { query } => {
                if self.verbosity == V::Quiet {
                    return false;
                }
                let q: String = query.chars().take(48).collect();
                self.push_status(
                    if self.verbosity == V::Verbose {
                        format!("Thinking about: {q}")
                    } else {
                        "Thinking…".to_string()
                    },
                    true,
                )
            }
            PipelineEvent::DesktopActionExecuted {
                output,
                latency_micros,
            } => {
                self.push(ChatRole::Tycho, output.clone());
                if self.verbosity == V::Verbose {
                    self.push_status(
                        format!("action took {:.1}ms", *latency_micros as f32 / 1000.0),
                        false,
                    );
                }
                true
            }
            PipelineEvent::GenerationCompleted { response } => {
                self.push(ChatRole::Tycho, response.clone());
                true
            }
            PipelineEvent::SynthesisCompleted { sample_count } => {
                if self.verbosity != V::Verbose {
                    return false;
                }
                self.push_status(format!("synthesized {sample_count} samples"), false)
            }
            PipelineEvent::SpeakingStarted => {
                if self.verbosity == V::Quiet {
                    return false;
                }
                self.push_status("Speaking…", true)
            }
            PipelineEvent::SpeakingFinished => {
                if self.verbosity != V::Verbose {
                    return false;
                }
                self.push_status("Done speaking", false)
            }
            PipelineEvent::Idle => {
                // A dangling transient ("Listening…", "Speaking…")
                // resolves to nothing — drop it rather than leave a
                // stale claim on screen.
                let popped = matches!(
                    self.messages.last(),
                    Some(m) if m.role == ChatRole::Status && m.transient
                ) && self.messages.pop().is_some();
                if self.verbosity == V::Verbose {
                    return self.push_status("Idle", false) || popped;
                }
                popped
            }
        }
    }
}

/// One laid-out visual line: the message it belongs to plus the text
/// to draw.
struct VisualLine {
    role: ChatRole,
    text: String,
    /// First line of its bubble (controls the bubble top).
    first: bool,
}

/// Word-wraps `text` to `max_w` px, hard-splitting words that exceed
/// the line on their own.
pub fn wrap_text(text: &str, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut cur = String::new();
        for word in raw.split_whitespace() {
            // Hard-split a word wider than the field. Widths accumulate
            // incrementally so a long unbroken token stays O(n) instead
            // of re-measuring the accumulated prefix per character.
            let mut w = word;
            let mut w_width = text_width(w);
            while w_width > max_w && w.chars().count() > 1 {
                let mut cut = String::new();
                let mut cut_w = 0.0f32;
                for c in w.chars() {
                    let cw = char_width(c);
                    if cut_w + cw > max_w {
                        break;
                    }
                    cut.push(c);
                    cut_w += cw;
                }
                if cut.is_empty() {
                    break;
                }
                w_width -= cut_w;
                if cur.is_empty() {
                    lines.push(cut.clone());
                } else {
                    lines.push(format!("{cur} {cut}"));
                    cur.clear();
                }
                w = &w[cut.len()..];
            }
            let joined = if cur.is_empty() {
                w.to_string()
            } else {
                format!("{cur} {w}")
            };
            if text_width(&joined) <= max_w {
                cur = joined;
            } else {
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                cur = w.to_string();
            }
        }
        if !cur.is_empty() || lines.is_empty() {
            lines.push(cur);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Flattens the transcript into visual lines with bubble boundaries.
fn visual_lines(model: &ChatModel) -> Vec<VisualLine> {
    let bubble_w = CHAT_WIDTH as f32 - 2.0 * PAD_X - 24.0;
    let text_w = bubble_w - 2.0 * BUBBLE_PAD.0;
    let mut out = Vec::new();
    for msg in &model.messages {
        let lines = wrap_text(&msg.text, text_w);
        for (i, line) in lines.into_iter().enumerate() {
            out.push(VisualLine {
                role: msg.role,
                text: line,
                first: i == 0,
            });
        }
    }
    out
}

/// Natural transcript height in px (lines + bubble padding + gaps).
pub fn content_height(model: &ChatModel) -> f32 {
    let lines = visual_lines(model);
    if lines.is_empty() {
        return 0.0;
    }
    let mut h = 0.0f32;
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && !lines[j].first {
            j += 1;
        }
        let group = &lines[i..j];
        h += group.len() as f32 * LINE_H;
        if group[0].role != ChatRole::Status {
            h += 2.0 * BUBBLE_PAD.1;
        }
        h += MSG_GAP;
        i = j;
    }
    h - MSG_GAP
}

/// Visible transcript region height.
pub fn viewport_height() -> f32 {
    (CHAT_HEIGHT - TITLE_H - INPUT_H) as f32
}

/// Maximum upward scroll in px.
pub fn max_scroll(model: &ChatModel) -> f32 {
    (content_height(model) - viewport_height()).max(0.0)
}

/// Applies a wheel delta (px; positive = scroll back into history).
pub fn scroll_by(model: &mut ChatModel, delta: f32) {
    model.scroll_up = (model.scroll_up + delta).clamp(0.0, max_scroll(model));
}

/// What a click on the panel means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatHit {
    /// Title-band close button.
    Close,
    /// Title-band verbosity chip — cycles Quiet/Normal/Verbose.
    Verbosity,
    /// Anywhere else — keeps keyboard focus, no action.
    Body,
}

/// Geometry of the title-band verbosity chip `(x, y, w, h)` in
/// panel-local coordinates.
pub fn verbosity_chip_rect(model: &ChatModel) -> (f32, f32, f32, f32) {
    let w = text_width(model.verbosity.label()) + 22.0;
    let x = CHAT_WIDTH as f32 - CLOSE_ZONE - w - 6.0;
    (x, (TITLE_H as f32 - 20.0) / 2.0, w, 20.0)
}

/// Panel-local hit test.
pub fn hit_at(model: &ChatModel, x: f32, y: f32) -> ChatHit {
    if y < TITLE_H as f32 && x > CHAT_WIDTH as f32 - CLOSE_ZONE {
        return ChatHit::Close;
    }
    let (cx, cy, cw, ch) = verbosity_chip_rect(model);
    if x >= cx && x <= cx + cw && y >= cy && y <= cy + ch {
        return ChatHit::Verbosity;
    }
    ChatHit::Body
}

/// Compositor surface width including the shadow margin.
pub fn surface_width() -> u32 {
    CHAT_WIDTH + 2 * SURFACE_PAD
}

/// Compositor surface height including the shadow margin.
pub fn surface_height() -> u32 {
    CHAT_HEIGHT + 2 * SURFACE_PAD
}

/// Renders the chat surface (panel + shadow) into premultiplied RGBA8
/// pixels sized `surface_width() x surface_height()`.
pub fn render_chat(model: &ChatModel, close_hover: bool, verb_hover: bool) -> Vec<u8> {
    let (sw, sh) = (surface_width(), surface_height());
    let mut pixmap = match Pixmap::new(sw, sh) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let pad = SURFACE_PAD as f32;
    let (w, h) = (CHAT_WIDTH as f32, CHAT_HEIGHT as f32);

    // Drop shadow, same stacked-rounded-rect recipe as settings.
    for (grow, alpha) in [(10.0f32, 14u8), (6.0, 18), (3.0, 24)] {
        if let Some(sh) = rounded(
            pad - grow,
            pad - grow + 3.0,
            w + grow * 2.0,
            h + grow * 2.0 - 4.0,
            14.0 + grow,
        ) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(0, 0, 0, alpha));
            pixmap.fill_path(&sh, &paint, FillRule::Winding, Transform::default(), None);
        }
    }

    // Panel: ambient gradient + accent bloom + hairline border.
    if let Some(path) = rounded(pad, pad, w, h, 12.0) {
        let grad = LinearGradient::new(
            Point::from_xy(pad, pad),
            Point::from_xy(pad, pad + h),
            vec![
                tiny_skia::GradientStop::new(0.0, Color::from_rgba8(28, 31, 44, 250)),
                tiny_skia::GradientStop::new(1.0, Color::from_rgba8(12, 14, 21, 250)),
            ],
            SpreadMode::Pad,
            Transform::default(),
        );
        let paint = match grad {
            Some(g) => Paint {
                shader: g,
                ..Paint::default()
            },
            None => Paint {
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(19, 21, 29, 248)),
                ..Paint::default()
            },
        };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::default(), None);
        if let Some(bloom) = RadialGradient::new(
            Point::from_xy(pad + w * 0.5, pad + 18.0),
            8.0,
            Point::from_xy(pad + w * 0.5, pad + 18.0),
            w * 0.62,
            vec![
                tiny_skia::GradientStop::new(
                    0.0,
                    Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 34),
                ),
                tiny_skia::GradientStop::new(
                    1.0,
                    Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 0),
                ),
            ],
            SpreadMode::Pad,
            Transform::default(),
        ) {
            let bp = Paint {
                shader: bloom,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &bp, FillRule::Winding, Transform::default(), None);
        }
        let mut border = Paint::default();
        border.set_color(Color::from_rgba8(255, 255, 255, 34));
        pixmap.stroke_path(
            &path,
            &border,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }

    // Title band + accent rule.
    if let Some(band) = rounded_top(pad, pad, w, TITLE_H as f32, 12.0) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(56, 66, 116, 90));
        pixmap.fill_path(&band, &paint, FillRule::Winding, Transform::default(), None);
    }
    if let Some(rule) = tiny_skia::Rect::from_ltrb(
        pad + 10.0,
        pad + TITLE_H as f32 - 0.5,
        pad + w - 10.0,
        pad + TITLE_H as f32 + 0.5,
    ) {
        let line = PathBuilder::from_rect(rule);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 60));
        pixmap.fill_path(&line, &paint, FillRule::Winding, Transform::default(), None);
    }
    draw_title(
        &mut pixmap,
        "Chat",
        pad + PAD_X,
        pad + (TITLE_H as f32 - GLYPH_PX) / 2.0,
        (206, 212, 244, 255),
    );

    // Close button.
    let cb = (pad + w - 34.0, pad + 9.0, 20.0, 18.0);
    if let Some(btn) = rounded(cb.0, cb.1, cb.2, cb.3, 4.0) {
        let mut paint = Paint::default();
        paint.set_color(if close_hover {
            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 80)
        } else {
            Color::from_rgba8(255, 255, 255, 14)
        });
        pixmap.fill_path(&btn, &paint, FillRule::Winding, Transform::default(), None);
        paint.set_color(Color::from_rgba8(255, 255, 255, 40));
        pixmap.stroke_path(
            &btn,
            &paint,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }
    draw_text(
        &mut pixmap,
        "\u{00d7}",
        cb.0 + (cb.2 - text_width("\u{00d7}")) / 2.0,
        cb.1 + (cb.3 - GLYPH_PX) / 2.0,
        if close_hover {
            (235, 240, 255, 255)
        } else {
            TXT_DIM
        },
    );

    // Verbosity chip: the chat's own setting, cycling on click.
    let (vx, vy, vw, vh) = verbosity_chip_rect(model);
    let (vx, vy) = (pad + vx, pad + vy);
    if let Some(chip) = rounded(vx, vy, vw, vh, 10.0) {
        let mut paint = Paint::default();
        paint.set_color(if verb_hover {
            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 90)
        } else {
            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 34)
        });
        pixmap.fill_path(&chip, &paint, FillRule::Winding, Transform::default(), None);
        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 70));
        pixmap.stroke_path(
            &chip,
            &paint,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }
    let label = model.verbosity.label();
    draw_text(
        &mut pixmap,
        label,
        vx + (vw - 10.0 - text_width(label)) / 2.0,
        vy + (vh - GLYPH_PX) / 2.0,
        (190, 200, 235, 255),
    );
    // Procedural chevron on the chip's right edge.
    {
        let cx = vx + vw - 11.0;
        let cy = vy + vh / 2.0;
        let mut pb = PathBuilder::new();
        pb.move_to(cx - 3.0, cy - 2.0);
        pb.line_to(cx + 3.0, cy - 2.0);
        pb.line_to(cx, cy + 2.5);
        pb.close();
        if let Some(tri) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(190, 200, 235, 200));
            pixmap.fill_path(&tri, &paint, FillRule::Winding, Transform::default(), None);
        }
    }

    // Transcript viewport — rendered into a sub-pixmap so the band is
    // the clip, then blitted under the title.
    let vh = viewport_height();
    let mut content = match Pixmap::new(sw, vh as u32) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let lines = visual_lines(model);
    if lines.is_empty() {
        let hint = "Double-click the orb — ask Tycho anything";
        draw_text(
            &mut content,
            hint,
            pad + (w - text_width(hint)) / 2.0,
            vh / 2.0 - GLYPH_PX / 2.0,
            TXT_FAINT,
        );
    } else {
        // Bottom-anchored: content bottom sits at the viewport bottom
        // minus scroll_up.
        let mut top = vh - content_height(model) + model.scroll_up;
        let mut i = 0;
        while i < lines.len() {
            // Group contiguous lines into their bubble: a group starts
            // at a `first` line and ends before the next one.
            let mut j = i + 1;
            while j < lines.len() && !lines[j].first {
                j += 1;
            }
            let group = &lines[i..j];
            let n = group.len();
            let is_status = group[0].role == ChatRole::Status;
            let bh = if is_status {
                n as f32 * LINE_H
            } else {
                n as f32 * LINE_H + 2.0 * BUBBLE_PAD.1
            };
            let bw = group
                .iter()
                .map(|l| text_width(&l.text))
                .fold(0.0f32, f32::max)
                + 2.0 * BUBBLE_PAD.0;
            let is_user = group[0].role == ChatRole::User;
            let bx = if is_user {
                pad + w - PAD_X - bw
            } else {
                pad + PAD_X
            };
            if top + bh > 0.0 && top < vh {
                if is_status {
                    // Lifecycle line: faint, centered, no bubble.
                    for (k, line) in group.iter().enumerate() {
                        let tw = text_width(&line.text);
                        draw_text(
                            &mut content,
                            &line.text,
                            pad + (w - tw) / 2.0,
                            top + k as f32 * LINE_H + (LINE_H - GLYPH_PX) / 2.0,
                            TXT_FAINT,
                        );
                    }
                } else {
                    if let Some(bub) = rounded(bx, top, bw, bh, 9.0) {
                        let mut paint = Paint::default();
                        paint.set_color(if is_user {
                            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 46)
                        } else {
                            Color::from_rgba8(255, 255, 255, 16)
                        });
                        content.fill_path(
                            &bub,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                        if is_user {
                            paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 70));
                            content.stroke_path(
                                &bub,
                                &paint,
                                &tiny_skia::Stroke::default(),
                                Transform::default(),
                                None,
                            );
                        }
                    }
                    for (k, line) in group.iter().enumerate() {
                        draw_text(
                            &mut content,
                            &line.text,
                            bx + BUBBLE_PAD.0,
                            top + BUBBLE_PAD.1 + k as f32 * LINE_H + (LINE_H - GLYPH_PX) / 2.0,
                            if is_user { (200, 220, 255, 255) } else { TXT },
                        );
                    }
                }
            }
            top += bh + MSG_GAP;
            i = j;
        }
    }
    pixmap.draw_pixmap(
        0,
        TITLE_H as i32 + SURFACE_PAD as i32,
        content.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::default(),
        None,
    );

    // Input field: bordered rounded rect, placeholder or text+cursor.
    let iy = pad + TITLE_H as f32 + vh + 6.0;
    let (ix, iw, ih) = (pad + 10.0, w - 20.0, INPUT_H as f32 - 12.0);
    if let Some(sep) = tiny_skia::Rect::from_ltrb(pad + 10.0, iy - 5.0, pad + w - 10.0, iy - 4.0) {
        let line = PathBuilder::from_rect(sep);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(255, 255, 255, 22));
        pixmap.fill_path(&line, &paint, FillRule::Winding, Transform::default(), None);
    }
    if let Some(field) = rounded(ix, iy, iw, ih, 8.0) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(255, 255, 255, 12));
        pixmap.fill_path(
            &field,
            &paint,
            FillRule::Winding,
            Transform::default(),
            None,
        );
        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 60));
        pixmap.stroke_path(
            &field,
            &paint,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }
    let ty = iy + (ih - GLYPH_PX) / 2.0;
    if model.input.is_empty() {
        draw_text(
            &mut pixmap,
            "Message Tycho\u{2026}",
            ix + 10.0,
            ty,
            TXT_FAINT,
        );
    } else {
        // Show the tail of the input that fits the field — walk back
        // from the end accumulating char widths instead of shifting the
        // string per removed char.
        let avail = iw - 22.0;
        let mut start = model.input.len();
        let mut width = 0.0f32;
        for (i, c) in model.input.char_indices().rev() {
            if width + char_width(c) > avail {
                break;
            }
            width += char_width(c);
            start = i;
        }
        let shown = &model.input[start..];
        draw_text(&mut pixmap, shown, ix + 10.0, ty, TXT);
        // Caret.
        let cx = ix + 10.0 + text_width(shown) + 2.0;
        if let Some(r) = tiny_skia::Rect::from_ltrb(cx, iy + 7.0, cx + 1.5, iy + ih - 7.0) {
            let caret = PathBuilder::from_rect(r);
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 230));
            pixmap.fill_path(
                &caret,
                &paint,
                FillRule::Winding,
                Transform::default(),
                None,
            );
        }
    }

    pixmap.data().to_vec()
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    let (x1, y1, x2, y2) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x1 + r, y1);
    pb.line_to(x2 - r, y1);
    pb.quad_to(x2, y1, x2, y1 + r);
    pb.line_to(x2, y2 - r);
    pb.quad_to(x2, y2, x2 - r, y2);
    pb.line_to(x1 + r, y2);
    pb.quad_to(x1, y2, x1, y2 - r);
    pb.line_to(x1, y1 + r);
    pb.quad_to(x1, y1, x1 + r, y1);
    pb.close();
    pb.finish()
}

/// Rounded on the two top corners only; bottom edge stays square.
fn rounded_top(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    let (x1, y1, x2, y2) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y2);
    pb.line_to(x1, y1 + r);
    pb.quad_to(x1, y1, x1 + r, y1);
    pb.line_to(x2 - r, y1);
    pb.quad_to(x2, y1, x2, y1 + r);
    pb.line_to(x2, y2);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_width() {
        let lines = wrap_text("the quick brown fox jumps over the lazy dog", 80.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(text_width(l) <= 80.0, "{l:?} too wide");
        }
        // Reassembles to the original words.
        assert_eq!(
            lines.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );
    }

    #[test]
    fn wrap_hard_splits_long_words() {
        let long = "supercalifragilisticexpialidocious_supercalifragilistic";
        let lines = wrap_text(long, 60.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(text_width(l) <= 60.0);
        }
        assert_eq!(lines.concat(), long);
    }

    #[test]
    fn push_caps_and_skips_blank() {
        let mut m = ChatModel::default();
        m.push(ChatRole::User, "   ");
        assert!(m.messages.is_empty());
        for i in 0..(MAX_MESSAGES + 10) {
            m.push(ChatRole::Tycho, format!("msg {i}"));
        }
        assert_eq!(m.messages.len(), MAX_MESSAGES);
        assert_eq!(m.messages[0].text, format!("msg {}", 10));
    }

    #[test]
    fn submit_trims_and_clears() {
        let mut m = ChatModel::default();
        assert!(m.submit().is_none());
        m.input_insert("  switch to workspace 2  ");
        assert_eq!(m.submit().as_deref(), Some("switch to workspace 2"));
        assert!(m.input.is_empty());
        // Control chars never enter the input.
        m.input_insert("a\u{0007}b\n");
        assert_eq!(m.input, "ab");
    }

    #[test]
    fn scroll_clamps_to_content() {
        let mut m = ChatModel::default();
        scroll_by(&mut m, 500.0);
        assert_eq!(m.scroll_up, 0.0); // empty: nothing to scroll
        for i in 0..40 {
            m.push(
                ChatRole::Tycho,
                format!("line {i} with enough text to wrap around"),
            );
        }
        let max = max_scroll(&m);
        assert!(max > 0.0);
        scroll_by(&mut m, 10_000.0);
        assert_eq!(m.scroll_up, max);
        scroll_by(&mut m, -10_000.0);
        assert_eq!(m.scroll_up, 0.0);
    }

    #[test]
    fn observe_maps_events_to_turns() {
        let mut m = ChatModel::default();
        assert!(m.observe(&PipelineEvent::TranscriptionCompleted("hello".into())));
        assert!(m.observe(&PipelineEvent::GenerationCompleted {
            response: "hi there".into()
        }));
        assert!(m.observe(&PipelineEvent::DesktopActionExecuted {
            output: "Muted".into(),
            latency_micros: 3,
        }));
        // Lifecycle events land as status lines at the default level.
        assert!(m.observe(&PipelineEvent::SpeechStarted));
        assert_eq!(m.messages.len(), 4);
        assert_eq!(m.messages[0].role, ChatRole::User);
        assert_eq!(m.messages[2].role, ChatRole::Tycho);
        assert_eq!(m.messages[3].role, ChatRole::Status);
    }

    #[test]
    fn render_produces_opaque_panel_with_transparent_margin() {
        let mut m = ChatModel::default();
        m.push(ChatRole::User, "what time is it");
        m.push(ChatRole::Tycho, "It is half past four.");
        let px = render_chat(&m, false, false);
        let (sw, sh) = (surface_width() as usize, surface_height() as usize);
        assert_eq!(px.len(), sw * sh * 4);
        // Panel center is effectively opaque; outer corner stays
        // transparent.
        let center = ((sh / 2) * sw + sw / 2) * 4;
        assert!(px[center + 3] >= 240);
        assert_eq!(px[3], 0);
    }

    #[test]
    fn hit_at_finds_close_only_in_title_corner() {
        let m = ChatModel::default();
        assert_eq!(hit_at(&m, CHAT_WIDTH as f32 - 10.0, 10.0), ChatHit::Close);
        assert_eq!(hit_at(&m, 10.0, 10.0), ChatHit::Body);
        assert_eq!(hit_at(&m, 200.0, 300.0), ChatHit::Body);
        // Verbosity chip sits in the title band left of the close zone.
        let (cx, cy, cw, ch) = verbosity_chip_rect(&m);
        assert_eq!(hit_at(&m, cx + cw / 2.0, cy + ch / 2.0), ChatHit::Verbosity);
        // Just left of the chip is plain body.
        assert_eq!(hit_at(&m, cx - 4.0, cy + ch / 2.0), ChatHit::Body);
    }

    #[test]
    fn verbosity_cycles_and_parses() {
        assert_eq!(ChatVerbosity::Quiet.next(), ChatVerbosity::Normal);
        assert_eq!(ChatVerbosity::Normal.next(), ChatVerbosity::Verbose);
        assert_eq!(ChatVerbosity::Verbose.next(), ChatVerbosity::Quiet);
        assert_eq!(ChatVerbosity::parse("verbose"), ChatVerbosity::Verbose);
        assert_eq!(ChatVerbosity::parse("QUIET"), ChatVerbosity::Quiet);
        assert_eq!(ChatVerbosity::parse("bogus"), ChatVerbosity::Normal);
    }

    #[test]
    fn quiet_hides_status_lines() {
        let mut m = ChatModel::with_verbosity(ChatVerbosity::Quiet);
        assert!(!m.observe(&PipelineEvent::SpeechStarted));
        assert!(!m.observe(&PipelineEvent::FastPathRouted {
            intent: "mute_audio".into(),
            parameter: None,
            confidence: 0.9,
        }));
        assert!(m.observe(&PipelineEvent::DesktopActionExecuted {
            output: "Muted".into(),
            latency_micros: 10,
        }));
        assert_eq!(m.messages.len(), 1);
        assert_eq!(m.messages[0].role, ChatRole::Tycho);
    }

    #[test]
    fn normal_shows_lifecycle_but_not_stats() {
        let mut m = ChatModel::default(); // Normal
        assert!(m.observe(&PipelineEvent::SpeechStarted));
        assert!(m.observe(&PipelineEvent::FastPathRouted {
            intent: "mute_audio".into(),
            parameter: None,
            confidence: 0.9,
        }));
        assert!(!m.observe(&PipelineEvent::SpeechProgress { rms: 0.4 }));
        assert!(!m.observe(&PipelineEvent::SynthesisCompleted { sample_count: 1000 }));
        assert!(!m.observe(&PipelineEvent::Idle));
    }

    #[test]
    fn transient_status_updates_in_place() {
        let mut m = ChatModel::default();
        m.observe(&PipelineEvent::SpeechStarted);
        m.observe(&PipelineEvent::SpeakingStarted);
        assert_eq!(m.messages.len(), 1);
        assert_eq!(m.messages[0].text, "Speaking…");
        // A real turn pushes after the transient line.
        m.observe(&PipelineEvent::TranscriptionCompleted("hi".into()));
        assert_eq!(m.messages.len(), 2);
        assert_eq!(m.messages[1].role, ChatRole::User);
    }

    #[test]
    fn verbose_shows_routing_and_progress() {
        let mut m = ChatModel::with_verbosity(ChatVerbosity::Verbose);
        m.observe(&PipelineEvent::SpeechStarted);
        assert!(m.observe(&PipelineEvent::SpeechProgress { rms: 0.5 }));
        // Throttled: an immediate second progress event is dropped.
        assert!(!m.observe(&PipelineEvent::SpeechProgress { rms: 0.6 }));
        assert!(m.messages.last().unwrap().text.contains("rms 0.500"));
        // Idle resolves the dangling transient line, then logs itself.
        assert!(m.observe(&PipelineEvent::Idle));
        assert_eq!(m.messages.last().unwrap().text, "Idle");
        assert!(!m.messages.iter().any(|t| t.text.contains("rms")));
        assert!(m.observe(&PipelineEvent::SynthesisCompleted { sample_count: 7 }));
        let texts: Vec<&str> = m.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"Idle"));
        assert!(texts.contains(&"synthesized 7 samples"));
    }

    #[test]
    fn idle_pops_dangling_transient_at_normal() {
        let mut m = ChatModel::default();
        m.observe(&PipelineEvent::SpeechStarted);
        assert_eq!(m.messages.last().unwrap().text, "Listening…");
        // Cancelled listen → Idle — the stale line disappears even
        // though "Idle" itself is a verbose-only line.
        assert!(m.observe(&PipelineEvent::Idle));
        assert!(m.messages.is_empty());
    }
}
