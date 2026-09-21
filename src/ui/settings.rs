//! Settings window for the orb: a layer-shell panel listing every
//! user-facing setting with click-to-cycle editing. The model is pure
//! so layout, hit-testing and rendering are unit-testable headless.

use super::menu::{draw_text, draw_title, text_width, GLYPH_PX};
use super::{SettingEntry, SettingsSnapshot, UiCommand};
use tiny_skia::{
    Color, FillRule, LinearGradient, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode,
    Transform,
};

/// Settings panel width in logical pixels (excludes shadow margin).
pub const SETTINGS_WIDTH: u32 = 340;
/// Transparent margin around the panel reserved for the drop shadow.
/// The compositor surface is `SETTINGS_WIDTH + 2*SURFACE_PAD` wide.
pub const SURFACE_PAD: u32 = 14;
/// Title band height.
pub const TITLE_H: u32 = 36;
/// Section header row height.
pub const HEADER_H: u32 = 26;
/// Editable setting row height.
pub const SETTING_H: u32 = 31;
/// Read-only info row height.
pub const INFO_H: u32 = 21;
/// Footer button band height.
pub const FOOTER_H: u32 = 42;
/// Separator row height.
pub const SEP_H: u32 = 10;
/// Footnote row height.
pub const NOTE_H: u32 = 20;
/// Horizontal text inset.
pub const PAD_X: f32 = 16.0;
/// Maximum panel height — content beyond this scrolls so the window
/// stays inside small outputs.
pub const MAX_PANEL_H: u32 = 620;
/// Height of one dropdown option row.
const OPT_H: f32 = 22.0;
// Shared accent palette (matches the orb's listening hue).
const ACCENT: (u8, u8, u8, u8) = (116, 162, 255, 255);
const TXT: (u8, u8, u8, u8) = (226, 229, 240, 255);
const TXT_DIM: (u8, u8, u8, u8) = (146, 152, 172, 255);
const TXT_FAINT: (u8, u8, u8, u8) = (108, 113, 134, 255);

/// Non-interactive footer actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAction {
    /// Hand the raw config file to the desktop's default editor.
    OpenConfigFile,
    /// Close the settings window.
    Done,
}

/// One row in the settings panel.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsRow {
    /// Small accent section label with a hairline rule. Inert.
    Header(&'static str),
    /// Editable setting; the value is the index into
    /// `SettingsSnapshot::entries`.
    Setting(usize),
    /// Read-only label/value pair. Inert.
    Info(usize),
    /// Bottom band holding both action buttons.
    Footer,
    /// Inert hairline gap.
    Separator,
    /// Dim footnote line. Inert.
    Note(&'static str),
}

/// The whole panel: editable entries plus the ordered scrollable row
/// list. `scroll` is the content offset in px; `open` is the index of
/// the entry whose dropdown is expanded.
#[derive(Debug, Clone)]
pub struct SettingsModel {
    pub entries: Vec<SettingEntry>,
    pub info: Vec<(String, String)>,
    pub rows: Vec<SettingsRow>,
    pub scroll: f32,
    /// Entry index with an open dropdown.
    pub open: Option<usize>,
    /// Option values shown by the open dropdown.
    pub open_opts: Vec<String>,
}

/// Builds the panel contents from a configuration snapshot.
pub fn build_model(snapshot: &SettingsSnapshot) -> SettingsModel {
    let entries = snapshot.entries.clone();
    let info: Vec<(String, String)> = snapshot
        .info
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let e = |key: &str| -> SettingsRow {
        SettingsRow::Setting(
            entries
                .iter()
                .position(|s| s.key == key)
                .expect("settings_snapshot defines every key used here"),
        )
    };
    let mut rows = vec![
        SettingsRow::Header("INTERFACE"),
        e("ui.mode"),
        e("ui.position"),
        e("ui.orb"),
        e("ui.chat_verbosity"),
        SettingsRow::Header("LISTENING"),
        e("vad.energy_threshold"),
        e("vad.min_speech_duration_ms"),
        e("vad.min_silence_duration_ms"),
        e("stt.language"),
        e("ui.earcons"),
        SettingsRow::Header("WAKE WORD"),
        e("wake.model"),
        e("wake.threshold"),
        e("wake.vad_gate"),
        e("wake.follow_up_seconds"),
        SettingsRow::Header("ROUTING"),
        e("router.fast_path_confidence_threshold"),
        SettingsRow::Header("VOICE"),
        e("tts.speed"),
        e("tts.voice"),
        e("tts.engine"),
        SettingsRow::Header("GENERATION"),
        e("generation.persona"),
        e("generation.model"),
        e("generation.temperature"),
        e("generation.max_tokens"),
        SettingsRow::Header("MEMORY"),
        e("memory.enable_long_term"),
        SettingsRow::Header("BACKEND"),
        e("desktop.backend"),
    ];
    for i in 0..info.len() {
        rows.push(SettingsRow::Info(i));
    }
    SettingsModel {
        entries,
        info,
        rows,
        scroll: 0.0,
        open: None,
        open_opts: Vec::new(),
    }
}

fn row_height(row: &SettingsRow) -> u32 {
    match row {
        SettingsRow::Header(_) => HEADER_H,
        SettingsRow::Setting(_) => SETTING_H,
        SettingsRow::Info(_) => INFO_H,
        SettingsRow::Footer => FOOTER_H,
        SettingsRow::Separator => SEP_H,
        SettingsRow::Note(_) => NOTE_H,
    }
}

/// Natural height of the scrollable content rows.
pub fn content_height(model: &SettingsModel) -> u32 {
    model.rows.iter().map(row_height).sum()
}

/// Fixed bottom chrome: separator + footnote + footer.
const CHROME_BOTTOM: u32 = SEP_H + NOTE_H + FOOTER_H;

/// Visible scrollable region height (panel minus fixed chrome).
pub fn viewport_height(model: &SettingsModel) -> u32 {
    content_height(model).min(MAX_PANEL_H - TITLE_H - CHROME_BOTTOM)
}

/// Total panel height including title band and bottom chrome
/// (excludes shadow).
pub fn panel_height(model: &SettingsModel) -> u32 {
    TITLE_H + viewport_height(model) + CHROME_BOTTOM
}

/// Maximum downward scroll in px.
pub fn max_scroll(model: &SettingsModel) -> f32 {
    (content_height(model) as f32 - viewport_height(model) as f32).max(0.0)
}

/// Applies a scroll delta (px), clamped to the content range, and
/// closes any open dropdown (its anchor moved).
pub fn scroll_by(model: &mut SettingsModel, delta: f32) {
    model.open = None;
    model.scroll = (model.scroll + delta).clamp(0.0, max_scroll(model));
}

/// Opens (or toggles) the dropdown for a setting entry.
pub fn toggle_dropdown(model: &mut SettingsModel, idx: usize) {
    if model.open == Some(idx) {
        model.open = None;
    } else {
        model.open_opts = model.entries[idx].values();
        model.open = Some(idx);
    }
}

/// Compositor surface width including the shadow margin.
pub fn surface_width() -> u32 {
    SETTINGS_WIDTH + 2 * SURFACE_PAD
}

/// Compositor surface height including the shadow margin.
pub fn surface_height(model: &SettingsModel) -> u32 {
    panel_height(model) + 2 * SURFACE_PAD
}

/// What a click on a row means.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsHit {
    /// Open/close the dropdown for this entry index.
    Toggle(usize),
    /// Pick `open_opts[i]` for the open dropdown.
    Select(usize),
    /// Click outside an open dropdown — close it, swallow the click.
    Dismiss,
    /// Run a footer action.
    Action(SettingsAction),
    /// Title-band close button.
    Close,
}

/// Hover state for redraw: a row, dropdown option, or footer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverMark {
    /// Row index into `model.rows`.
    Row(usize),
    /// Option index in `model.open_opts`.
    Option(usize),
    /// Footer button under the pointer.
    Button(SettingsAction),
    /// Title-band close button.
    Close,
}

/// Width of the title-band close-button hit zone.
const CLOSE_ZONE: f64 = 44.0;

/// Panel-local rect of an entry's value chip `(x, y, w, h)` given the
/// row's visual top. Shared by rendering, dropdown anchoring, and
/// hit-testing.
fn chip_rect(model: &SettingsModel, idx: usize, row_top: f32) -> (f32, f32, f32, f32) {
    let entry = &model.entries[idx];
    let w = SETTINGS_WIDTH as f32;
    let chip_w = (text_width(&entry.current) + 30.0).max(72.0);
    let star_w = if entry.live { 0.0 } else { 9.0 };
    let cx = w - PAD_X - chip_w - star_w;
    (cx, row_top + (SETTING_H as f32 - 20.0) / 2.0, chip_w, 20.0)
}

/// Panel-local rect `(x, y, w, h)` of the open dropdown overlay, or
/// None when closed. Flips above the chip when it would clip the
/// viewport bottom.
fn dropdown_rect(model: &SettingsModel) -> Option<(f32, f32, f32, f32)> {
    let idx = model.open?;
    let row_vis = row_visual_top(model, idx)?; // panel-local y of row top
    let (cx, cy, cw, ch) = chip_rect(model, idx, row_vis);
    let n = model.open_opts.len() as f32;
    let widest = model
        .open_opts
        .iter()
        .map(|o| text_width(o) + 36.0)
        .fold(cw + 20.0, f32::max);
    let w = widest.min(SETTINGS_WIDTH as f32 - 2.0 * PAD_X);
    let x = (cx + cw - w).max(PAD_X - 4.0);
    let h = n * OPT_H + 8.0;
    let view_bot = TITLE_H as f32 + viewport_height(model) as f32;
    let y = if cy + ch + 4.0 + h <= view_bot {
        cy + ch + 4.0
    } else {
        (cy - 4.0 - h).max(TITLE_H as f32 + 4.0)
    };
    Some((x, y, w, h))
}

/// Panel-local y of a setting row's top inside the scrolled viewport.
fn row_visual_top(model: &SettingsModel, entry_idx: usize) -> Option<f32> {
    let mut top = TITLE_H as f32 - model.scroll;
    for row in &model.rows {
        let h = row_height(row) as f32;
        if let SettingsRow::Setting(i) = row {
            if *i == entry_idx {
                return Some(top);
            }
        }
        top += h;
    }
    None
}

/// The row index and content-space y band under a scrolled content y.
fn content_row_at(model: &SettingsModel, cy: f32) -> Option<(usize, &SettingsRow)> {
    let mut top = 0.0f32;
    for (i, row) in model.rows.iter().enumerate() {
        let h = row_height(row) as f32;
        if cy >= top && cy < top + h {
            return Some((i, row));
        }
        top += h;
    }
    None
}

/// Hit-test a panel-local coordinate (shadow margin already removed).
pub fn hit_row(model: &SettingsModel, x: f32, y: f32) -> Option<SettingsHit> {
    // Open dropdown captures all clicks.
    if let Some((dx, dy, dw, dh)) = dropdown_rect(model) {
        if x >= dx && x < dx + dw && y >= dy && y < dy + dh {
            let i = ((y - dy - 4.0) / OPT_H).max(0.0) as usize;
            if i < model.open_opts.len() {
                return Some(SettingsHit::Select(i));
            }
        }
        return Some(SettingsHit::Dismiss);
    }
    if y < TITLE_H as f32 {
        if x > SETTINGS_WIDTH as f32 - CLOSE_ZONE as f32 {
            return Some(SettingsHit::Close);
        }
        return None;
    }
    let footer_top = panel_height(model) as f32 - FOOTER_H as f32;
    if y >= footer_top && y < panel_height(model) as f32 {
        let mid = SETTINGS_WIDTH as f32 / 2.0;
        return Some(SettingsHit::Action(if x < mid {
            SettingsAction::OpenConfigFile
        } else {
            SettingsAction::Done
        }));
    }
    if y >= TITLE_H as f32 + viewport_height(model) as f32 {
        return None; // separator + footnote band
    }
    let cy = y - TITLE_H as f32 + model.scroll;
    match content_row_at(model, cy) {
        Some((_, SettingsRow::Setting(i))) => Some(SettingsHit::Toggle(*i)),
        _ => None,
    }
}

/// Hover target for a panel-local coordinate.
pub fn hover_at(model: &SettingsModel, x: f32, y: f32) -> Option<HoverMark> {
    if let Some((dx, dy, dw, dh)) = dropdown_rect(model) {
        if x >= dx && x < dx + dw && y >= dy && y < dy + dh {
            let i = ((y - dy - 4.0) / OPT_H).max(0.0) as usize;
            if i < model.open_opts.len() {
                return Some(HoverMark::Option(i));
            }
        }
        return None;
    }
    if y < TITLE_H as f32 {
        return if x > SETTINGS_WIDTH as f32 - CLOSE_ZONE as f32 {
            Some(HoverMark::Close)
        } else {
            None
        };
    }
    let footer_top = panel_height(model) as f32 - FOOTER_H as f32;
    if y >= footer_top && y < panel_height(model) as f32 {
        let mid = SETTINGS_WIDTH as f32 / 2.0;
        return Some(HoverMark::Button(if x < mid {
            SettingsAction::OpenConfigFile
        } else {
            SettingsAction::Done
        }));
    }
    if y >= TITLE_H as f32 + viewport_height(model) as f32 {
        return None;
    }
    let cy = y - TITLE_H as f32 + model.scroll;
    match content_row_at(model, cy) {
        Some((i, SettingsRow::Setting(_))) => Some(HoverMark::Row(i)),
        _ => None,
    }
}

/// Maps a chosen option value to the command the pipeline expects.
/// `ui.*` keys reuse the typed commands so the orb applies them locally
/// too; everything else goes through the generic `SetConfig` write.
pub fn command_for_value(entry: &SettingEntry, value: &str) -> UiCommand {
    match entry.key {
        "ui.mode" => UiCommand::SetMode(super::ActivationMode::parse(value)),
        "ui.position" => UiCommand::SetPlacement {
            position: super::OrbPosition::parse(value),
            margin_x: -1,
            margin_y: -1,
        },
        _ => UiCommand::SetConfig {
            key: entry.key.to_string(),
            value: value.to_string(),
        },
    }
}

/// Maps an entry's current value (post-mutation) to its command.
pub fn command_for_entry(entry: &SettingEntry) -> UiCommand {
    command_for_value(entry, &entry.current.clone())
}

/// Renders the settings surface (panel + shadow) into premultiplied
/// RGBA8 pixels sized `surface_width() x surface_height(model)`.
pub fn render_settings(model: &SettingsModel, hover: Option<HoverMark>) -> Vec<u8> {
    let (sw, sh) = (surface_width(), surface_height(model));
    let mut pixmap = match Pixmap::new(sw, sh) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let pad = SURFACE_PAD as f32;
    let (w, h) = (SETTINGS_WIDTH as f32, panel_height(model) as f32);

    // Soft drop shadow: stacked rounded rects growing outward.
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

    // Panel: vertical ambient gradient (lit top easing to deep base)
    // + soft accent bloom behind the title, then a hairline border.
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

    // Title band: top corners rounded, accent rule underneath.
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
        "Tycho Settings",
        pad + PAD_X,
        pad + (TITLE_H as f32 - GLYPH_PX) / 2.0,
        (206, 212, 244, 255),
    );

    // Close button: bordered square, accent when hovered.
    let cb = (pad + w - 34.0, pad + 9.0, 20.0, 18.0);
    if let Some(btn) = rounded(cb.0, cb.1, cb.2, cb.3, 4.0) {
        let mut paint = Paint::default();
        paint.set_color(if hover == Some(HoverMark::Close) {
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
        if hover == Some(HoverMark::Close) {
            (235, 240, 255, 255)
        } else {
            TXT_DIM
        },
    );

    // Scrollable content renders into a sub-pixmap the size of the
    // viewport — its bounds are the clip. Rows are offset by -scroll.
    let vh = viewport_height(model);
    let mut content = match Pixmap::new(sw, vh) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let mut top = -model.scroll;
    for (i, row) in model.rows.iter().enumerate() {
        let rh = row_height(row) as f32;
        if top + rh >= 0.0 && top < vh as f32 {
            match row {
                SettingsRow::Header(label) => {
                    draw_text(
                        &mut content,
                        label,
                        pad + PAD_X,
                        top + (rh - GLYPH_PX) / 2.0 + 2.0,
                        (124, 146, 205, 255),
                    );
                    let lx = pad + PAD_X + text_width(label) + 10.0;
                    if let Some(r) = tiny_skia::Rect::from_ltrb(
                        lx,
                        top + rh / 2.0 + 1.5,
                        pad + w - PAD_X,
                        top + rh / 2.0 + 2.5,
                    ) {
                        let line = PathBuilder::from_rect(r);
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(255, 255, 255, 20));
                        content.fill_path(
                            &line,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                    }
                }
                SettingsRow::Setting(idx) => {
                    let entry = &model.entries[*idx];
                    let open = model.open == Some(*idx);
                    if hover == Some(HoverMark::Row(i)) || open {
                        fill_row(&mut content, top, rh);
                    }
                    draw_text(
                        &mut content,
                        entry.label,
                        pad + PAD_X + 2.0,
                        top + (rh - GLYPH_PX) / 2.0,
                        TXT,
                    );
                    // Value chip: bordered control, `v` affordance,
                    // dim `*` outside it for restart-required settings.
                    let (cx, cy, cw, ch) = chip_rect(model, *idx, top + TITLE_H as f32);
                    let cy = cy - TITLE_H as f32; // into content space
                    if let Some(chip) = rounded(cx + pad, cy, cw, ch, 6.0) {
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(
                            ACCENT.0,
                            ACCENT.1,
                            ACCENT.2,
                            if open { 60 } else { 22 },
                        ));
                        content.fill_path(
                            &chip,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 70));
                        content.stroke_path(
                            &chip,
                            &paint,
                            &tiny_skia::Stroke::default(),
                            Transform::default(),
                            None,
                        );
                    }
                    draw_text(
                        &mut content,
                        &entry.current,
                        cx + pad + 9.0,
                        cy + (ch - GLYPH_PX) / 2.0,
                        (148, 196, 255, 255),
                    );
                    // Dropdown chevron inside the chip's right edge.
                    let (tx, ty) = (cx + pad + cw - 13.0, cy + ch / 2.0 - 2.0);
                    let mut pb = PathBuilder::new();
                    pb.move_to(tx, ty);
                    pb.line_to(tx + 7.0, ty);
                    pb.line_to(tx + 3.5, ty + 4.5);
                    pb.close();
                    if let Some(tri) = pb.finish() {
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(148, 196, 255, 200));
                        content.fill_path(
                            &tri,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                    }
                    if !entry.live {
                        draw_text(
                            &mut content,
                            "*",
                            cx + pad + cw + 4.0,
                            cy + (ch - GLYPH_PX) / 2.0,
                            TXT_FAINT,
                        );
                    }
                }
                SettingsRow::Info(idx) => {
                    let (label, value) = &model.info[*idx];
                    draw_text(
                        &mut content,
                        label,
                        pad + PAD_X + 2.0,
                        top + (rh - GLYPH_PX) / 2.0,
                        TXT_DIM,
                    );
                    let avail = w - 2.0 * PAD_X - text_width(label) - 14.0;
                    let shown: String = if text_width(value) <= avail {
                        value.clone()
                    } else {
                        let mut shown = String::new();
                        for ch in value.chars() {
                            if text_width(&shown) + text_width(&ch.to_string()) + text_width("..")
                                > avail
                            {
                                break;
                            }
                            shown.push(ch);
                        }
                        format!("{}..", shown.trim_end())
                    };
                    let vx = pad + w - PAD_X - text_width(&shown);
                    draw_text(
                        &mut content,
                        &shown,
                        vx,
                        top + (rh - GLYPH_PX) / 2.0,
                        TXT_FAINT,
                    );
                }
                _ => {}
            }
        }
        top += rh;
    }

    // Dropdown overlay: drawn last inside the viewport so it floats
    // over the rows beneath its anchor chip.
    if let Some((dx, dy, dw, dh)) = dropdown_rect(model) {
        let dy = dy - TITLE_H as f32; // into content space
        if let Some(boxp) = rounded(dx + pad, dy, dw, dh, 8.0) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(16, 18, 26, 252));
            content.fill_path(&boxp, &paint, FillRule::Winding, Transform::default(), None);
            paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 80));
            content.stroke_path(
                &boxp,
                &paint,
                &tiny_skia::Stroke::default(),
                Transform::default(),
                None,
            );
        }
        let cur = model.open.map(|i| model.entries[i].current.clone());
        for (i, opt) in model.open_opts.iter().enumerate() {
            let oy = dy + 4.0 + i as f32 * OPT_H;
            if hover == Some(HoverMark::Option(i)) {
                if let Some(r) = tiny_skia::Rect::from_ltrb(
                    dx + pad + 3.0,
                    oy + 1.0,
                    dx + pad + dw - 3.0,
                    oy + OPT_H - 1.0,
                ) {
                    let line = PathBuilder::from_rect(r);
                    let mut paint = Paint::default();
                    paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 46));
                    content.fill_path(&line, &paint, FillRule::Winding, Transform::default(), None);
                }
            }
            let selected = cur.as_deref() == Some(opt.as_str());
            draw_text(
                &mut content,
                if selected { "\u{2713}" } else { " " },
                dx + pad + 10.0,
                oy + (OPT_H - GLYPH_PX) / 2.0,
                ACCENT,
            );
            draw_text(
                &mut content,
                opt,
                dx + pad + 24.0,
                oy + (OPT_H - GLYPH_PX) / 2.0,
                if selected { TXT } else { TXT_DIM },
            );
        }
    }

    // Blit the viewport band into the panel below the title band.
    pixmap.draw_pixmap(
        0,
        TITLE_H as i32 + SURFACE_PAD as i32,
        content.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::default(),
        None,
    );

    // Fixed bottom chrome: separator, footnote, buttons.
    let chrome_top = TITLE_H as f32 + vh as f32;
    if let Some(r) = tiny_skia::Rect::from_ltrb(
        pad + 12.0,
        pad + chrome_top + 4.5,
        pad + w - 12.0,
        pad + chrome_top + 5.5,
    ) {
        let line = PathBuilder::from_rect(r);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(255, 255, 255, 22));
        pixmap.fill_path(&line, &paint, FillRule::Winding, Transform::default(), None);
    }
    draw_text(
        &mut pixmap,
        "* applies on next launch",
        pad + PAD_X + 2.0,
        pad + chrome_top + SEP_H as f32 + (NOTE_H as f32 - GLYPH_PX) / 2.0,
        TXT_FAINT,
    );
    let footer_top = chrome_top + SEP_H as f32 + NOTE_H as f32;
    draw_button(
        &mut pixmap,
        pad,
        footer_top,
        "Open tycho.toml",
        w / 2.0 - 6.0,
        PAD_X,
        hover == Some(HoverMark::Button(SettingsAction::OpenConfigFile)),
    );
    draw_button(
        &mut pixmap,
        pad,
        footer_top,
        "Done",
        w / 2.0 - 6.0,
        PAD_X + w / 2.0 + 6.0,
        hover == Some(HoverMark::Button(SettingsAction::Done)),
    );

    pixmap.data().to_vec()
}

fn draw_button(
    pixmap: &mut Pixmap,
    pad: f32,
    top: f32,
    label: &str,
    bw: f32,
    bx: f32,
    hovered: bool,
) {
    let by = pad + top + 5.0;
    if let Some(btn) = rounded(bx, by, bw, 30.0, 8.0) {
        let mut paint = Paint::default();
        paint.set_color(if hovered {
            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 70)
        } else {
            Color::from_rgba8(255, 255, 255, 12)
        });
        pixmap.fill_path(&btn, &paint, FillRule::Winding, Transform::default(), None);
        paint.set_color(if hovered {
            Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 140)
        } else {
            Color::from_rgba8(255, 255, 255, 42)
        });
        pixmap.stroke_path(
            &btn,
            &paint,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }
    let lx = bx + (bw - text_width(label)) / 2.0;
    draw_text(
        pixmap,
        label,
        lx,
        by + (30.0 - GLYPH_PX) / 2.0,
        if hovered {
            (240, 244, 255, 255)
        } else {
            TXT_DIM
        },
    );
}

fn fill_row(pixmap: &mut Pixmap, top: f32, h: f32) {
    let pad = SURFACE_PAD as f32;
    let w = pixmap.width() as f32;
    if let Some(bg) = rounded(pad + 5.0, top + 2.0, w - 2.0 * pad - 10.0, h - 4.0, 6.0) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 42));
        pixmap.fill_path(&bg, &paint, FillRule::Winding, Transform::default(), None);
    }
    // Accent bar on the left edge of the hovered row.
    if let Some(bar) = tiny_skia::Rect::from_ltrb(pad + 5.0, top + 6.0, pad + 8.0, top + h - 6.0) {
        let path = PathBuilder::from_rect(bar);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(ACCENT.0, ACCENT.1, ACCENT.2, 190));
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::default(), None);
    }
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
