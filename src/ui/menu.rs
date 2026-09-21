//! Context menu for the orb: a second layer-shell surface with real
//! text, hover highlight, and click hit-testing. The model is pure so
//! the layout and actions are unit-testable without a display.

use super::{ActivationMode, OrbPosition, UiCommand};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

/// Menu surface width in logical pixels.
pub const MENU_WIDTH: u32 = 208;
/// Height of a normal menu row.
pub const ROW_H: u32 = 30;
/// Height of a separator row.
pub const SEP_H: u32 = 9;
/// Horizontal text inset.
pub const PAD_X: f32 = 14.0;

/// One row in the menu. Separators are inert.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuRow {
    Item { label: String, selected: bool },
    Separator,
}

/// Builds the menu contents for the current mode and placement.
pub fn menu_rows(mode: ActivationMode, pos: OrbPosition) -> Vec<MenuRow> {
    let item = |label: &str, selected: bool| MenuRow::Item {
        label: label.to_string(),
        selected,
    };
    vec![
        item("Auto listen", mode == ActivationMode::Auto),
        item("Push-to-talk", mode == ActivationMode::Manual),
        item("Disabled", mode == ActivationMode::Off),
        MenuRow::Separator,
        item(&format!("Position: {}", pos.as_str()), false),
        MenuRow::Separator,
        item("Chat", false),
        item("Settings", false),
        item("Quit Tycho", false),
    ]
}

/// Maps a menu row index to the command it issues. `None` rows are
/// inert (separators or unknown rows).
pub fn row_action(index: usize, rows: &[MenuRow]) -> Option<UiCommand> {
    match index {
        0 => Some(UiCommand::SetMode(ActivationMode::Auto)),
        1 => Some(UiCommand::SetMode(ActivationMode::Manual)),
        2 => Some(UiCommand::SetMode(ActivationMode::Off)),
        4 => Some(UiCommand::SetPlacement {
            position: next_position(rows),
            margin_x: -1,
            margin_y: -1,
        }),
        6 => Some(UiCommand::ToggleChat),
        7 => Some(UiCommand::OpenSettings),
        8 => Some(UiCommand::Shutdown),
        _ => None,
    }
}

/// Cycles the named position shown in the menu row to the next preset.
fn next_position(rows: &[MenuRow]) -> OrbPosition {
    const PREFIX: &str = "Position: ";
    let cur = rows
        .get(4)
        .and_then(|r| match r {
            MenuRow::Item { label, .. } => label.strip_prefix(PREFIX),
            _ => None,
        })
        .map(OrbPosition::parse)
        .unwrap_or(OrbPosition::BottomCenter);
    let presets = OrbPosition::presets();
    let idx = presets.iter().position(|p| *p == cur).unwrap_or(0);
    presets[(idx + 1) % presets.len()]
}

/// Total menu height for a row list.
pub fn menu_height(rows: &[MenuRow]) -> u32 {
    rows.iter()
        .map(|r| match r {
            MenuRow::Item { .. } => ROW_H,
            MenuRow::Separator => SEP_H,
        })
        .sum()
}

/// Y offset where row `i` starts.
pub fn row_top(rows: &[MenuRow], i: usize) -> u32 {
    rows[..i]
        .iter()
        .map(|r| match r {
            MenuRow::Item { .. } => ROW_H,
            MenuRow::Separator => SEP_H,
        })
        .sum()
}

/// Hit-test a y coordinate against the row list.
pub fn hit_row(rows: &[MenuRow], y: f32) -> Option<usize> {
    let mut top = 0.0f32;
    for (i, row) in rows.iter().enumerate() {
        let h = match row {
            MenuRow::Item { .. } => ROW_H,
            MenuRow::Separator => SEP_H,
        } as f32;
        if y >= top && y < top + h {
            return match row {
                MenuRow::Item { .. } => Some(i),
                MenuRow::Separator => None,
            };
        }
        top += h;
    }
    None
}

/// Renders the menu into premultiplied RGBA8 pixels.
pub fn render_menu(rows: &[MenuRow], hover: Option<usize>) -> Vec<u8> {
    let height = menu_height(rows);
    let mut pixmap = match Pixmap::new(MENU_WIDTH, height) {
        Some(p) => p,
        None => return Vec::new(),
    };

    // Panel: rounded rect with the same ambient vertical gradient as
    // the settings panel, plus a hairline border.
    let (w, h) = (MENU_WIDTH as f32, height as f32);
    if let Some(path) = rounded_rect(0.5, 0.5, w - 1.0, h - 1.0, 10.0) {
        let grad = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, 0.0),
            tiny_skia::Point::from_xy(0.0, h),
            vec![
                tiny_skia::GradientStop::new(0.0, Color::from_rgba8(30, 33, 45, 240)),
                tiny_skia::GradientStop::new(1.0, Color::from_rgba8(15, 17, 25, 240)),
            ],
            tiny_skia::SpreadMode::Pad,
            Transform::default(),
        );
        let mut paint = match grad {
            Some(g) => Paint {
                shader: g,
                ..Paint::default()
            },
            None => Paint {
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(24, 26, 34, 235)),
                ..Paint::default()
            },
        };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::default(), None);
        paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 40));
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke::default(),
            Transform::default(),
            None,
        );
    }

    for (i, row) in rows.iter().enumerate() {
        let top = row_top(rows, i) as f32;
        match row {
            MenuRow::Separator => {
                let line = PathBuilder::from_rect(
                    tiny_skia::Rect::from_ltrb(10.0, top + 4.0, w - 10.0, top + 5.0).unwrap(),
                );
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(255, 255, 255, 30));
                pixmap.fill_path(&line, &paint, FillRule::Winding, Transform::default(), None);
            }
            MenuRow::Item { label, selected } => {
                if hover == Some(i) {
                    if let Some(bg) = rounded_rect(4.0, top + 2.0, w - 8.0, ROW_H as f32 - 4.0, 6.0)
                    {
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(116, 162, 255, 45));
                        pixmap.fill_path(
                            &bg,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                    }
                    if let Some(bar) =
                        tiny_skia::Rect::from_ltrb(4.0, top + 6.0, 7.0, top + ROW_H as f32 - 6.0)
                    {
                        let path = PathBuilder::from_rect(bar);
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(116, 162, 255, 190));
                        pixmap.fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                    }
                }
                if *selected {
                    if let Some(dot) = PathBuilder::from_circle(8.0, top + ROW_H as f32 / 2.0, 3.0)
                    {
                        let mut paint = Paint::default();
                        paint.set_color(Color::from_rgba8(120, 190, 255, 255));
                        pixmap.fill_path(
                            &dot,
                            &paint,
                            FillRule::Winding,
                            Transform::default(),
                            None,
                        );
                    }
                }
                draw_text(
                    &mut pixmap,
                    label,
                    PAD_X,
                    top + (ROW_H as f32 - GLYPH_PX) / 2.0,
                    (228, 232, 240, 255),
                );
            }
        }
    }

    pixmap.data().to_vec()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
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

/// Text line-box height in pixels — callers center text vertically
/// inside rows with `(row_h - GLYPH_PX) / 2`.
pub const GLYPH_PX: f32 = 17.0;
/// Rasterization size for panel text.
const FONT_PX: f32 = 14.0;
/// Rasterization size for the panel title.
const TITLE_PX: f32 = 16.0;

fn font() -> &'static fontdue::Font {
    static FONT: std::sync::OnceLock<fontdue::Font> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(
            include_bytes!("../../assets/fonts/AdwaitaSans-Regular.subset.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("embedded UI font must parse")
    })
}

/// Measured advance width of `text` at the panel text size.
pub fn text_width(text: &str) -> f32 {
    text_width_px(text, FONT_PX)
}

/// Measured advance width of a single char at the panel text size.
pub fn char_width(c: char) -> f32 {
    font().metrics(c, FONT_PX).advance_width
}

/// Measured advance width of `text` at `px` size.
pub fn text_width_px(text: &str, px: f32) -> f32 {
    let f = font();
    text.chars().map(|c| f.metrics(c, px).advance_width).sum()
}

/// Rasterizes a text line with the embedded sans font.
/// `(x, y)` is the top-left corner of a `GLYPH_PX`-tall line box;
/// the baseline is centered inside it.
pub fn draw_text(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, color: (u8, u8, u8, u8)) {
    draw_text_px(pixmap, text, x, y, FONT_PX, color);
}

/// `draw_text` at an explicit rasterization size (panel title).
pub fn draw_text_px(
    pixmap: &mut Pixmap,
    text: &str,
    x: f32,
    y: f32,
    px: f32,
    color: (u8, u8, u8, u8),
) {
    let f = font();
    let Some(lm) = f.horizontal_line_metrics(px) else {
        return;
    };
    let baseline = y + GLYPH_PX / 2.0 + (lm.ascent + lm.descent) / 2.0;
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let mut pen_x = x;
    for ch in text.chars() {
        let (m, bmp) = f.rasterize(ch, px);
        let gx0 = (pen_x + m.xmin as f32) as i32;
        let gy0 = (baseline - m.ymin as f32 - m.height as f32) as i32;
        for row in 0..m.height {
            for col in 0..m.width {
                let cov = bmp[row * m.width + col] as u32;
                if cov == 0 {
                    continue;
                }
                let (pxx, pyy) = (gx0 + col as i32, gy0 + row as i32);
                if pxx < 0 || pyy < 0 || pxx >= w || pyy >= h {
                    continue;
                }
                // Coverage-modulated source-over on premultiplied pixels.
                let sa = color.3 as u32 * cov / 255;
                let inv = 255 - sa;
                let idx = (pyy as usize * pixmap.width() as usize + pxx as usize) * 4;
                let data = pixmap.data_mut();
                data[idx] = ((color.0 as u32 * sa + data[idx] as u32 * inv) / 255) as u8;
                data[idx + 1] = ((color.1 as u32 * sa + data[idx + 1] as u32 * inv) / 255) as u8;
                data[idx + 2] = ((color.2 as u32 * sa + data[idx + 2] as u32 * inv) / 255) as u8;
                data[idx + 3] = (sa + data[idx + 3] as u32 * inv / 255) as u8;
            }
        }
        pen_x += m.advance_width;
    }
}

/// `draw_text` at the title rasterization size.
pub fn draw_title(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, color: (u8, u8, u8, u8)) {
    draw_text_px(pixmap, text, x, y, TITLE_PX, color);
}
