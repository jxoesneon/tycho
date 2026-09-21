//! Wayland `zwlr_layer_shell_v1` overlay presenting the status orb.
//!
//! The orb runs on a dedicated thread: the Wayland event queue is
//! synchronous, while pipeline events arrive via the `broadcast` channel
//! (drained non-blockingly each frame). The surface lives on the Overlay
//! layer, takes no keyboard focus, and accepts pointer input: drag to
//! reposition, left-click to trigger a manual listen, right-click for
//! the settings menu. Settings are relayed to the pipeline through a
//! `UiCommand` channel and persisted by the coordinator.

use super::chat;
use super::menu;
use super::settings;
use super::{
    render_frame, rgba_to_argb8888, ActivationMode, OrbPosition, OrbVisual, SettingsSnapshot,
    UiCommand, UiState,
};
use crate::pipeline::events::PipelineEvent;
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsFd;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry, wl_seat,
    wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Orb surface edge length in pixels.
pub const ORB_SIZE: u32 = 96;
/// Default distance from the anchored vertical screen edge.
pub const MARGIN_V: i32 = 48;
/// Default distance from the anchored horizontal screen edge.
pub const MARGIN_H: i32 = 32;
/// Animation frame interval.
const FRAME: Duration = Duration::from_millis(33);
/// Pointer movement (px) separating a click from a drag.
const DRAG_THRESHOLD: f64 = 6.0;
/// linux-input button codes reported by `wl_pointer`.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// Maximum gap between two left-clicks that still counts as a
/// double-click (opens the chat panel).
const DBLCLICK: Duration = Duration::from_millis(400);

/// Resolved startup placement handed to the orb at spawn time.
#[derive(Debug, Clone, Copy)]
pub struct StartupPlacement {
    /// Named preset or `Custom`.
    pub position: OrbPosition,
    /// Custom left-edge margin in px (`-1` when unset).
    pub margin_x: i32,
    /// Custom top-edge margin in px (`-1` when unset).
    pub margin_y: i32,
}

/// Owns the running overlay. Dropping it stops the thread and destroys
/// the layer surface.
pub struct OrbHandle {
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl OrbHandle {
    /// Spawns the overlay when a Wayland session is reachable.
    /// Returns `None` in headless environments or if the thread cannot
    /// be created — the assistant continues without a visual indicator.
    ///
    /// `commands` receives `UiCommand`s produced by pointer input;
    /// `mode` is the shared activation-mode flag. `placement` is the
    /// resolved startup position; `snapshot` feeds the settings window.
    pub fn spawn(
        events: broadcast::Receiver<PipelineEvent>,
        commands: mpsc::Sender<UiCommand>,
        mode: Arc<AtomicU8>,
        placement: StartupPlacement,
        snapshot: SettingsSnapshot,
    ) -> Option<Self> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("WAYLAND_SOCKET").is_none()
        {
            return None;
        }
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let join = std::thread::Builder::new()
            .name("tycho-orb".to_string())
            .spawn(move || {
                if let Err(e) = run_orb(events, commands, mode, placement, snapshot, flag) {
                    tracing::warn!("orb overlay exited: {}", e);
                }
            })
            .ok()?;
        Some(Self {
            running,
            join: Some(join),
        })
    }
}

impl Drop for OrbHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Anchored placement: `mx`/`my` are margins along the anchored edges.
#[derive(Debug, Clone, Copy)]
struct Placement {
    anchor: zwlr_layer_surface_v1::Anchor,
    /// Margin on the anchored horizontal edge (left or right).
    mx: i32,
    /// Margin on the anchored vertical edge (top or bottom).
    my: i32,
}

impl Placement {
    fn named(pos: OrbPosition) -> Self {
        Self {
            anchor: anchor_of(pos),
            mx: MARGIN_H,
            my: MARGIN_V,
        }
    }

    fn custom(x: i32, y: i32) -> Self {
        Self {
            anchor: zwlr_layer_surface_v1::Anchor::Left | zwlr_layer_surface_v1::Anchor::Top,
            mx: x.max(0),
            my: y.max(0),
        }
    }

    /// `(top, right, bottom, left)` margins for `set_margin`.
    fn margins(&self) -> (i32, i32, i32, i32) {
        let a = self.anchor;
        (
            if a.contains(zwlr_layer_surface_v1::Anchor::Top) {
                self.my
            } else {
                0
            },
            if a.contains(zwlr_layer_surface_v1::Anchor::Right) {
                self.mx
            } else {
                0
            },
            if a.contains(zwlr_layer_surface_v1::Anchor::Bottom) {
                self.my
            } else {
                0
            },
            if a.contains(zwlr_layer_surface_v1::Anchor::Left) {
                self.mx
            } else {
                0
            },
        )
    }
}

/// Layer-shell anchor flags for a named position.
fn anchor_of(pos: OrbPosition) -> zwlr_layer_surface_v1::Anchor {
    use zwlr_layer_surface_v1::Anchor as A;
    let (l, r, t, b) = pos.anchor();
    let mut a = A::empty();
    if l {
        a |= A::Left;
    }
    if r {
        a |= A::Right;
    }
    if t {
        a |= A::Top;
    }
    if b {
        a |= A::Bottom;
    }
    a
}

/// Which of our layer surfaces currently holds pointer focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// Pointer is over none of our surfaces.
    None,
    Orb,
    Menu,
    Settings,
    Chat,
    /// Pointer is over the chat's dismiss scrim.
    Scrim,
}

/// State of an in-progress pointer press on one of our surfaces.
#[derive(Debug, Clone)]
struct Press {
    button: u32,
    /// Surface the press landed on.
    target: Focus,
    /// Surface-local coordinates of the last motion event seen
    /// (initialized to the press position).
    last_local: (f64, f64),
    /// Accumulated pointer delta, summed from per-event local deltas.
    /// Equals the true cursor delta whenever the compositor has not yet
    /// applied our own margin moves (the dominant case mid-drag), and
    /// stays bounded when it has — unlike reconstructing from the
    /// committed position, which ratchets.
    accum: (f64, f64),
    dragging: bool,
    /// Absolute orb position when the press began (None ⇒ drag disabled
    /// until output geometry is known).
    origin: Option<(i32, i32)>,
}

/// Live settings-menu surface state. Field order matters for drop:
/// the layer surface is destroyed before the backing objects.
struct MenuState {
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
    mem: memfd::Memfd,
    rows: Vec<menu::MenuRow>,
    hover: Option<usize>,
    height: u32,
    configured: bool,
    /// Set when the buffer contents need to be repainted.
    redraw: bool,
}

/// Live settings-window surface state.
struct SettingsState {
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
    mem: memfd::Memfd,
    model: settings::SettingsModel,
    hover: Option<settings::HoverMark>,
    height: u32,
    configured: bool,
    redraw: bool,
}

/// Live chat-panel surface state.
struct ChatState {
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
    mem: memfd::Memfd,
    model: chat::ChatModel,
    /// Pointer is hovering the close button.
    close_hover: bool,
    /// Pointer is hovering the verbosity chip.
    verb_hover: bool,
    configured: bool,
    redraw: bool,
}

/// Transparent click-catcher behind the chat panel (Top layer, so the
/// Overlay chat/orb/menu/settings stay above it). A click anywhere
/// that isn't one of our overlay surfaces lands here and dismisses
/// the chat — standard popover semantics, the click is consumed.
struct ScrimState {
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    surface: wl_surface::WlSurface,
    /// Transparent backing buffer, created once the compositor tells
    /// us the stretched size via `configure`.
    buffer: Option<wl_buffer::WlBuffer>,
    mem: Option<memfd::Memfd>,
    configured: bool,
}

/// XKB keyboard state for the chat panel's text input. All pointers
/// live and die on the orb thread. `state` stays null until the
/// compositor delivers a keymap (or libxkbcommon is absent — typing
/// then degrades to a transcript viewer, voice still works).
struct Kbd {
    xkb: Option<&'static xkbcommon_dl::XkbCommon>,
    ctx: *mut xkbcommon_dl::xkb_context,
    keymap: *mut xkbcommon_dl::xkb_keymap,
    state: *mut xkbcommon_dl::xkb_state,
    /// True while the chat surface holds keyboard focus.
    chat_focused: bool,
}

impl Default for Kbd {
    fn default() -> Self {
        let xkb = xkbcommon_dl::xkbcommon_option();
        let ctx = xkb
            .map(|x| unsafe {
                (x.xkb_context_new)(xkbcommon_dl::xkb_context_flags::XKB_CONTEXT_NO_FLAGS)
            })
            .unwrap_or(std::ptr::null_mut());
        Self {
            xkb,
            ctx,
            keymap: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            chat_focused: false,
        }
    }
}

impl Kbd {
    /// Installs a keymap received from the compositor (text-format
    /// V1 buffer) and builds a fresh state from it.
    fn set_keymap(&mut self, buf: &[u8]) {
        let Some(xkb) = self.xkb else { return };
        if self.ctx.is_null() {
            return;
        }
        unsafe {
            let map = (xkb.xkb_keymap_new_from_buffer)(
                self.ctx,
                buf.as_ptr() as *const std::ffi::c_char,
                buf.len(),
                xkbcommon_dl::xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
                xkbcommon_dl::xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
            );
            if map.is_null() {
                return;
            }
            let st = (xkb.xkb_state_new)(map);
            if !self.state.is_null() {
                (xkb.xkb_state_unref)(self.state);
            }
            if !self.keymap.is_null() {
                (xkb.xkb_keymap_unref)(self.keymap);
            }
            self.keymap = map;
            self.state = st;
        }
    }

    /// Updates modifier state from a wl_keyboard `modifiers` event.
    fn update_modifiers(&mut self, dep: u32, lat: u32, lock: u32, group: u32) {
        let Some(xkb) = self.xkb else { return };
        if self.state.is_null() {
            return;
        }
        unsafe {
            (xkb.xkb_state_update_mask)(self.state, dep, lat, lock, 0, 0, group);
        }
    }

    /// Handles one key press/release. `key` is the evdev scancode.
    /// Returns the decoded press outcome (before the state update,
    /// so modifier keys themselves translate correctly).
    fn key(&mut self, key: u32, pressed: bool) -> Option<ChatKey> {
        let xkb = self.xkb.as_ref()?;
        if self.state.is_null() {
            return None;
        }
        let kc = key + 8; // evdev → xkb keycode offset
        unsafe {
            let out = if pressed {
                let sym = (xkb.xkb_state_key_get_one_sym)(self.state, kc);
                match sym {
                    xkbcommon_dl::keysyms::Return | xkbcommon_dl::keysyms::KP_Enter => {
                        Some(ChatKey::Submit)
                    }
                    xkbcommon_dl::keysyms::Escape => Some(ChatKey::Close),
                    xkbcommon_dl::keysyms::BackSpace => Some(ChatKey::Backspace),
                    _ => {
                        let mut buf = [0u8; 16];
                        let n = (xkb.xkb_state_key_get_utf8)(
                            self.state,
                            kc,
                            buf.as_mut_ptr() as *mut std::ffi::c_char,
                            buf.len(),
                        );
                        // xkb_state_key_get_utf8 returns the *required*
                        // length when the buffer is too small — only
                        // slice when the result actually fit.
                        if n > 0 && (n as usize) <= buf.len() {
                            Some(ChatKey::Text(
                                String::from_utf8_lossy(&buf[..n as usize]).into_owned(),
                            ))
                        } else {
                            None
                        }
                    }
                }
            } else {
                None
            };
            (xkb.xkb_state_update_key)(
                self.state,
                kc,
                if pressed {
                    xkbcommon_dl::xkb_key_direction::XKB_KEY_DOWN
                } else {
                    xkbcommon_dl::xkb_key_direction::XKB_KEY_UP
                },
            );
            out
        }
    }
}

/// What a key press means for the chat input field.
enum ChatKey {
    Text(String),
    Submit,
    Close,
    Backspace,
}

impl Drop for Kbd {
    fn drop(&mut self) {
        if let Some(xkb) = self.xkb {
            unsafe {
                if !self.state.is_null() {
                    (xkb.xkb_state_unref)(self.state);
                }
                if !self.keymap.is_null() {
                    (xkb.xkb_keymap_unref)(self.keymap);
                }
                if !self.ctx.is_null() {
                    (xkb.xkb_context_unref)(self.ctx);
                }
            }
        }
    }
}

/// Per-connection application state carried by the event queue.
struct OrbApp {
    visual: OrbVisual,
    configured: bool,
    closed: bool,
    mode: Arc<AtomicU8>,
    commands: mpsc::Sender<UiCommand>,
    compositor: wl_compositor::WlCompositor,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    position: OrbPosition,
    placement: Placement,
    /// Logical output size `(w, h)` from `wl_output` mode/scale events.
    output_size: Option<(i32, i32)>,
    out_mode: (i32, i32),
    out_scale: i32,
    /// Latest surface-local pointer coordinates.
    pointer: (f64, f64),
    /// Which of our surfaces the pointer hotspot is over.
    focus: Focus,
    press: Option<Press>,
    menu: Option<MenuState>,
    settings: Option<SettingsState>,
    chat: Option<ChatState>,
    /// Click-outside catcher for the chat panel (lives exactly as
    /// long as the chat surface).
    scrim: Option<ScrimState>,
    /// Timestamp of the last orb left-click release — the
    /// double-click window for opening the chat panel.
    last_left: Option<Instant>,
    /// Keyboard decode state for the chat input field.
    kbd: Kbd,
    /// Editable config snapshot used to build the settings window.
    snapshot: SettingsSnapshot,
    last_mode: ActivationMode,
}

impl OrbApp {
    /// Absolute position of the orb's top-left corner in output coords.
    fn abs_pos(&self) -> Option<(i32, i32)> {
        let (ow, oh) = self.output_size?;
        let a = self.placement.anchor;
        let size = ORB_SIZE as i32;
        let x = if a.contains(zwlr_layer_surface_v1::Anchor::Left) {
            self.placement.mx
        } else if a.contains(zwlr_layer_surface_v1::Anchor::Right) {
            ow - self.placement.mx - size
        } else {
            (ow - size) / 2
        };
        let y = if a.contains(zwlr_layer_surface_v1::Anchor::Top) {
            self.placement.my
        } else if a.contains(zwlr_layer_surface_v1::Anchor::Bottom) {
            oh - self.placement.my - size
        } else {
            (oh - size) / 2
        };
        Some((x, y))
    }

    /// Re-anchor and re-margin the orb surface.
    fn apply_placement(&mut self) {
        self.layer.set_anchor(self.placement.anchor);
        let (t, r, b, l) = self.placement.margins();
        self.layer.set_margin(t, r, b, l);
        self.surface.commit();
    }

    /// Move the orb to an absolute top-left position during a drag.
    fn drag_to(&mut self, x: i32, y: i32) {
        let (x, y) = match self.output_size {
            Some((ow, oh)) => (
                x.clamp(0, (ow - ORB_SIZE as i32).max(0)),
                y.clamp(0, (oh - ORB_SIZE as i32).max(0)),
            ),
            None => (x.max(0), y.max(0)),
        };
        self.placement = Placement::custom(x, y);
        self.position = OrbPosition::Custom;
        self.apply_placement();
    }

    /// Where the menu panel goes: beside/below the orb when output
    /// geometry is known, otherwise offset inward past the orb along
    /// the same anchor.
    fn menu_placement(&self, height: u32) -> Placement {
        if let (Some((ow, oh)), Some((ox, oy))) = (self.output_size, self.abs_pos()) {
            let x = ox.clamp(0, (ow - menu::MENU_WIDTH as i32).max(0));
            let fits_below = oy + ORB_SIZE as i32 + 8 + height as i32 <= oh;
            let y = if fits_below {
                oy + ORB_SIZE as i32 + 8
            } else {
                (oy - height as i32 - 8).max(0)
            };
            Placement::custom(x, y)
        } else {
            let mut p = self.placement;
            p.my += ORB_SIZE as i32 + 8;
            p
        }
    }

    fn open_menu(&mut self, qh: &QueueHandle<Self>) {
        if self.menu.is_some() {
            return;
        }
        tracing::info!("orb: opening settings menu");
        let mode = ActivationMode::load(&self.mode);
        let rows = menu::menu_rows(mode, self.position);
        let height = menu::menu_height(&rows);
        let buf_len = (menu::MENU_WIDTH * height * 4) as u64;

        let surface = self.compositor.create_surface(qh, ());
        let region = self.compositor.create_region(qh, ());
        region.add(0, 0, menu::MENU_WIDTH as i32, height as i32);
        surface.set_input_region(Some(&region));
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "tycho-menu".to_string(),
            qh,
            (),
        );
        let place = self.menu_placement(height);
        layer.set_anchor(place.anchor);
        layer.set_size(menu::MENU_WIDTH, height);
        let (t, r, b, l) = place.margins();
        layer.set_margin(t, r, b, l);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();

        let mem = match memfd::MemfdOptions::default().create("tycho-menu") {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("menu buffer alloc failed: {}", e);
                layer.destroy();
                surface.destroy();
                return;
            }
        };
        if let Err(e) = mem.as_file().set_len(buf_len) {
            tracing::warn!("menu buffer resize failed: {}", e);
            layer.destroy();
            surface.destroy();
            return;
        }
        let pool = self
            .shm
            .create_pool(mem.as_file().as_fd(), buf_len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            menu::MENU_WIDTH as i32,
            height as i32,
            (menu::MENU_WIDTH * 4) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        self.menu = Some(MenuState {
            layer,
            surface,
            buffer,
            mem,
            rows,
            hover: None,
            height,
            configured: false,
            redraw: true,
        });
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            m.layer.destroy();
            m.surface.destroy();
        }
    }

    /// Paints the menu into its shm buffer and commits it.
    fn render_menu_frame(&mut self) {
        let Some(m) = self.menu.as_mut() else {
            return;
        };
        let rgba = menu::render_menu(&m.rows, m.hover);
        if let Err(e) = m.mem.as_file().write_all_at(&rgba_to_argb8888(&rgba), 0) {
            tracing::warn!("menu buffer write failed: {}", e);
            return;
        }
        m.surface.attach(Some(&m.buffer), 0, 0);
        m.surface
            .damage_buffer(0, 0, menu::MENU_WIDTH as i32, m.height as i32);
        m.surface.commit();
        m.redraw = false;
    }

    /// Updates hover highlight from a menu-local y coordinate.
    fn menu_hover(&mut self, y: f64) {
        if let Some(m) = self.menu.as_mut() {
            let hit = menu::hit_row(&m.rows, y as f32);
            if hit != m.hover {
                m.hover = hit;
                m.redraw = true;
            }
        }
    }

    /// Applies a `UiCommand` to the orb's own state so the visual
    /// reflects the change before the coordinator confirms it.
    fn apply_command_local(&mut self, cmd: &UiCommand) {
        match cmd {
            UiCommand::SetMode(mode) => {
                self.mode.store(mode.as_u8(), Ordering::Relaxed);
            }
            UiCommand::SetPlacement {
                position,
                margin_x,
                margin_y,
            } => {
                if *position != OrbPosition::Custom {
                    self.position = *position;
                    self.placement = Placement::named(*position);
                    self.apply_placement();
                } else if *margin_x >= 0 && *margin_y >= 0 {
                    self.position = OrbPosition::Custom;
                    self.placement = Placement::custom(*margin_x, *margin_y);
                    self.apply_placement();
                }
            }
            _ => {}
        }
    }

    /// Executes the action for the row under `y`.
    fn menu_click(&mut self, y: f64, qh: &QueueHandle<Self>) {
        let Some(m) = &self.menu else { return };
        let Some(idx) = menu::hit_row(&m.rows, y as f32) else {
            return;
        };
        let rows = m.rows.clone();
        let Some(cmd) = menu::row_action(idx, &rows) else {
            return;
        };
        if cmd == UiCommand::OpenSettings {
            self.close_menu();
            self.open_settings(qh);
            return;
        }
        if cmd == UiCommand::ToggleChat {
            self.close_menu();
            self.toggle_chat(qh);
            return;
        }
        if cmd == UiCommand::Shutdown {
            self.close_settings();
        }
        self.apply_command_local(&cmd);
        let _ = self.commands.send(cmd);
        self.close_menu();
    }

    /// Centered-window placement for the settings surface: screen
    /// center when output geometry is known, otherwise beside the orb.
    /// The surface includes a transparent shadow margin, so it is
    /// shifted up-left of the panel's visual center.
    fn settings_placement(&self, height: u32) -> Placement {
        let pad = settings::SURFACE_PAD as i32;
        let width = settings::surface_width() as i32;
        if let Some((ow, oh)) = self.output_size {
            let x = ((ow - width) / 2).max(0);
            let y = ((oh - height as i32) / 2).max(0);
            Placement::custom(x, y)
        } else {
            let mut p = self.placement;
            p.mx += ORB_SIZE as i32 + 12 - pad;
            p.my -= pad;
            p
        }
    }

    fn open_settings(&mut self, qh: &QueueHandle<Self>) {
        if self.settings.is_some() {
            return;
        }
        tracing::info!("orb: opening settings window");
        let model = settings::build_model(&self.snapshot);
        let height = settings::surface_height(&model);
        let width = settings::surface_width();
        let buf_len = (width * height * 4) as u64;

        let surface = self.compositor.create_surface(qh, ());
        let region = self.compositor.create_region(qh, ());
        region.add(0, 0, width as i32, height as i32);
        surface.set_input_region(Some(&region));
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "tycho-settings".to_string(),
            qh,
            (),
        );
        let place = self.settings_placement(height);
        layer.set_anchor(place.anchor);
        layer.set_size(width, height);
        let (t, r, b, l) = place.margins();
        layer.set_margin(t, r, b, l);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();

        let mem = match memfd::MemfdOptions::default().create("tycho-settings") {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("settings buffer alloc failed: {}", e);
                layer.destroy();
                surface.destroy();
                return;
            }
        };
        if let Err(e) = mem.as_file().set_len(buf_len) {
            tracing::warn!("settings buffer resize failed: {}", e);
            layer.destroy();
            surface.destroy();
            return;
        }
        let pool = self
            .shm
            .create_pool(mem.as_file().as_fd(), buf_len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        self.settings = Some(SettingsState {
            layer,
            surface,
            buffer,
            mem,
            model,
            hover: None,
            height,
            configured: false,
            redraw: true,
        });
    }

    fn close_settings(&mut self) {
        if let Some(s) = self.settings.take() {
            s.layer.destroy();
            s.surface.destroy();
        }
    }

    /// Paints the settings panel into its shm buffer and commits it.
    fn render_settings_frame(&mut self) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let rgba = settings::render_settings(&s.model, s.hover);
        if let Err(e) = s.mem.as_file().write_all_at(&rgba_to_argb8888(&rgba), 0) {
            tracing::warn!("settings buffer write failed: {}", e);
            return;
        }
        s.surface.attach(Some(&s.buffer), 0, 0);
        s.surface
            .damage_buffer(0, 0, settings::surface_width() as i32, s.height as i32);
        s.surface.commit();
        s.redraw = false;
    }

    /// Updates the settings hover state from surface-local coords.
    fn settings_hover(&mut self, x: f64, y: f64) {
        if let Some(s) = self.settings.as_mut() {
            let pad = settings::SURFACE_PAD as f32;
            let hit = settings::hover_at(&s.model, x as f32 - pad, y as f32 - pad);
            if hit != s.hover {
                s.hover = hit;
                s.redraw = true;
            }
        }
    }

    /// Handles a click inside the settings window: cycles the setting
    /// under `(x, y)`, closes via the title-band `[x]`, or runs a
    /// footer action.
    fn settings_click(&mut self, x: f64, y: f64) {
        let pad = settings::SURFACE_PAD as f32;
        let (px, py) = (x as f32 - pad, y as f32 - pad);
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        match settings::hit_row(&s.model, px, py) {
            Some(settings::SettingsHit::Toggle(idx)) => {
                settings::toggle_dropdown(&mut s.model, idx);
                s.redraw = true;
            }
            Some(settings::SettingsHit::Select(opt)) => {
                let idx = s.model.open.expect("Select implies open dropdown");
                let value = s.model.open_opts[opt].clone();
                s.model.entries[idx].current = value;
                s.model.open = None;
                let cmd = settings::command_for_entry(&s.model.entries[idx]);
                s.redraw = true;
                self.apply_command_local(&cmd);
                let _ = self.commands.send(cmd);
            }
            Some(settings::SettingsHit::Dismiss) => {
                s.model.open = None;
                s.redraw = true;
            }
            Some(settings::SettingsHit::Action(settings::SettingsAction::OpenConfigFile)) => {
                let _ = self.commands.send(UiCommand::OpenConfig);
                self.close_settings();
            }
            Some(
                settings::SettingsHit::Action(settings::SettingsAction::Done)
                | settings::SettingsHit::Close,
            ) => {
                self.close_settings();
            }
            None => {}
        }
    }

    /// Position-aware chat placement: the panel docks beside the orb,
    /// preferring the side with room (right, then left, then clamped
    /// on top). Vertically it hugs the orb's edge — bottoms aligned
    /// when the orb sits in the lower half of the output, tops
    /// aligned in the upper half. The surface carries a transparent
    /// shadow margin, so placement coordinates subtract the pad.
    fn chat_placement(&self) -> Placement {
        let pad = chat::SURFACE_PAD as i32;
        let (pw, ph) = (chat::CHAT_WIDTH as i32, chat::CHAT_HEIGHT as i32);
        let gap = 10;
        if let (Some((ow, oh)), Some((ox, oy))) = (self.output_size, self.abs_pos()) {
            // Panel coordinates (inside the transparent shadow margin):
            // prefer right of the orb, then left, then clamped on top.
            let right_fits = ox + ORB_SIZE as i32 + gap + pw <= ow;
            let left_fits = ox - gap - pw >= 0;
            let panel_x = if right_fits {
                ox + ORB_SIZE as i32 + gap
            } else if left_fits {
                ox - gap - pw
            } else {
                ox.clamp(0, (ow - pw).max(0))
            };
            // Bottoms aligned when the orb sits low, tops when high.
            let panel_y = if oy + ORB_SIZE as i32 / 2 > oh / 2 {
                oy + ORB_SIZE as i32 - ph
            } else {
                oy
            };
            let panel_y = panel_y.clamp(0, (oh - ph).max(0));
            Placement::custom((panel_x - pad).max(0), (panel_y - pad).max(0))
        } else {
            // Geometry unknown: offset right past the orb along the
            // same anchor, keeping the vertical margin.
            let mut p = self.placement;
            p.mx += ORB_SIZE as i32 + gap;
            p
        }
    }

    fn open_chat(&mut self, qh: &QueueHandle<Self>) {
        if self.chat.is_some() {
            return;
        }
        tracing::info!("orb: opening chat panel");

        // Scrim first: a fullscreen transparent surface in the Top
        // layer (the chat itself sits in Overlay, so it always paints
        // above). Clicks that land on it dismiss the chat.
        let scrim_surface = self.compositor.create_surface(qh, ());
        let scrim_layer = self.layer_shell.get_layer_surface(
            &scrim_surface,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "tycho-chat-scrim".to_string(),
            qh,
            (),
        );
        scrim_layer.set_anchor(
            zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right
                | zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Bottom,
        );
        scrim_layer.set_size(0, 0);
        scrim_layer.set_exclusive_zone(-1);
        scrim_layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        scrim_surface.commit();
        self.scrim = Some(ScrimState {
            layer: scrim_layer,
            surface: scrim_surface,
            buffer: None,
            mem: None,
            configured: false,
        });

        let width = chat::surface_width();
        let height = chat::surface_height();
        let buf_len = (width * height * 4) as u64;

        let surface = self.compositor.create_surface(qh, ());
        let region = self.compositor.create_region(qh, ());
        region.add(0, 0, width as i32, height as i32);
        surface.set_input_region(Some(&region));
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "tycho-chat".to_string(),
            qh,
            (),
        );
        let place = self.chat_placement();
        layer.set_anchor(place.anchor);
        layer.set_size(width, height);
        let (t, r, b, l) = place.margins();
        layer.set_margin(t, r, b, l);
        layer.set_exclusive_zone(-1);
        // Exclusive so the input field actually receives keys — the
        // panel yields focus (and closes) the moment anything else is
        // clicked.
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
        surface.commit();

        let mem = match memfd::MemfdOptions::default().create("tycho-chat") {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("chat buffer alloc failed: {}", e);
                layer.destroy();
                surface.destroy();
                return;
            }
        };
        if let Err(e) = mem.as_file().set_len(buf_len) {
            tracing::warn!("chat buffer resize failed: {}", e);
            layer.destroy();
            surface.destroy();
            return;
        }
        let pool = self
            .shm
            .create_pool(mem.as_file().as_fd(), buf_len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );

        self.chat = Some(ChatState {
            layer,
            surface,
            buffer,
            mem,
            model: chat::ChatModel::with_verbosity(chat::ChatVerbosity::parse(
                &self.snapshot.chat_verbosity,
            )),
            close_hover: false,
            verb_hover: false,
            configured: false,
            redraw: true,
        });
    }

    fn close_chat(&mut self) {
        if let Some(c) = self.chat.take() {
            c.layer.destroy();
            c.surface.destroy();
        }
        if let Some(s) = self.scrim.take() {
            s.layer.destroy();
            s.surface.destroy();
        }
        self.kbd.chat_focused = false;
    }

    fn toggle_chat(&mut self, qh: &QueueHandle<Self>) {
        if self.chat.is_some() {
            self.close_chat();
        } else {
            self.open_chat(qh);
        }
    }

    /// Paints the chat panel into its shm buffer and commits it.
    fn render_chat_frame(&mut self) {
        let Some(c) = self.chat.as_mut() else {
            return;
        };
        let rgba = chat::render_chat(&c.model, c.close_hover, c.verb_hover);
        if let Err(e) = c.mem.as_file().write_all_at(&rgba_to_argb8888(&rgba), 0) {
            tracing::warn!("chat buffer write failed: {}", e);
            return;
        }
        c.surface.attach(Some(&c.buffer), 0, 0);
        c.surface.damage_buffer(
            0,
            0,
            chat::surface_width() as i32,
            chat::surface_height() as i32,
        );
        c.surface.commit();
        c.redraw = false;
    }

    /// Pointer moved over the chat panel: the close button and the
    /// verbosity chip highlight.
    fn chat_hover(&mut self, x: f64, y: f64) {
        let pad = chat::SURFACE_PAD as f32;
        if let Some(c) = self.chat.as_mut() {
            let hit = chat::hit_at(&c.model, x as f32 - pad, y as f32 - pad);
            let over_close = hit == chat::ChatHit::Close;
            let over_verb = hit == chat::ChatHit::Verbosity;
            if over_close != c.close_hover || over_verb != c.verb_hover {
                c.close_hover = over_close;
                c.verb_hover = over_verb;
                c.redraw = true;
            }
        }
    }

    /// Click inside the chat panel: the close button dismisses, the
    /// verbosity chip cycles and persists; any other click just holds
    /// keyboard focus.
    fn chat_click(&mut self, x: f64, y: f64) {
        let pad = chat::SURFACE_PAD as f32;
        let hit = {
            let Some(c) = self.chat.as_ref() else { return };
            chat::hit_at(&c.model, x as f32 - pad, y as f32 - pad)
        };
        match hit {
            chat::ChatHit::Close => self.close_chat(),
            chat::ChatHit::Verbosity => {
                if let Some(c) = self.chat.as_mut() {
                    c.model.verbosity = c.model.verbosity.next();
                    c.redraw = true;
                    let v = c.model.verbosity.label().to_lowercase();
                    // Keep the settings snapshot in sync so the
                    // settings window shows the same level.
                    self.snapshot.chat_verbosity = v.clone();
                    if let Some(e) = self
                        .snapshot
                        .entries
                        .iter_mut()
                        .find(|e| e.key == "ui.chat_verbosity")
                    {
                        e.current = v.clone();
                    }
                    let _ = self.commands.send(UiCommand::SetConfig {
                        key: "ui.chat_verbosity".to_string(),
                        value: v,
                    });
                }
            }
            chat::ChatHit::Body => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for OrbApp {
    // Required bound for `registry_queue_init`; registry events are
    // handled internally by its own object data, never dispatched here.
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for OrbApp {
    fn event(
        app: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let is_menu = app
            .menu
            .as_ref()
            .map(|m| m.layer.id() == surface.id())
            .unwrap_or(false);
        let is_settings = app
            .settings
            .as_ref()
            .map(|s| s.layer.id() == surface.id())
            .unwrap_or(false);
        let is_chat = app
            .chat
            .as_ref()
            .map(|c| c.layer.id() == surface.id())
            .unwrap_or(false);
        let is_scrim = app
            .scrim
            .as_ref()
            .map(|s| s.layer.id() == surface.id())
            .unwrap_or(false);
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                surface.ack_configure(serial);
                if is_scrim {
                    // The scrim stretched to the output size: give it
                    // a fully transparent buffer and a matching input
                    // region so clicks land on it for dismissal.
                    if let Some(s) = app.scrim.as_mut() {
                        s.configured = true;
                        if width > 0 && height > 0 && s.buffer.is_none() {
                            // Widen before multiplying: width*height*4
                            // overflows u32 on high-res outputs, and cap
                            // the pool so a malicious/buggy size can't
                            // demand an absurd shm allocation.
                            let len = u64::from(width) * u64::from(height) * 4;
                            if len > (1 << 30) {
                                return;
                            }
                            if let Ok(mem) = memfd::MemfdOptions::default().create("tycho-scrim") {
                                if mem.as_file().set_len(len).is_ok() {
                                    let pool = app.shm.create_pool(
                                        mem.as_file().as_fd(),
                                        len as i32,
                                        qh,
                                        (),
                                    );
                                    let buf = pool.create_buffer(
                                        0,
                                        width as i32,
                                        height as i32,
                                        (width * 4) as i32,
                                        wl_shm::Format::Argb8888,
                                        qh,
                                        (),
                                    );
                                    // memfd is zero-filled: fully
                                    // transparent, nothing to paint.
                                    let region = app.compositor.create_region(qh, ());
                                    region.add(0, 0, width as i32, height as i32);
                                    s.surface.set_input_region(Some(&region));
                                    s.surface.attach(Some(&buf), 0, 0);
                                    s.surface.commit();
                                    s.buffer = Some(buf);
                                    s.mem = Some(mem);
                                }
                            }
                        }
                    }
                } else if is_menu {
                    if let Some(m) = app.menu.as_mut() {
                        m.configured = true;
                        m.redraw = true;
                    }
                } else if is_settings {
                    if let Some(s) = app.settings.as_mut() {
                        s.configured = true;
                        s.redraw = true;
                    }
                } else if is_chat {
                    if let Some(c) = app.chat.as_mut() {
                        c.configured = true;
                        c.redraw = true;
                    }
                } else {
                    app.configured = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if is_menu {
                    app.menu = None;
                } else if is_settings {
                    app.settings = None;
                } else if is_chat {
                    app.chat = None;
                    app.kbd.chat_focused = false;
                } else if is_scrim {
                    app.scrim = None;
                } else {
                    app.closed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for OrbApp {
    fn event(
        app: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wl_pointer::Event as E;
        match event {
            E::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer = (surface_x, surface_y);
                app.focus = if app
                    .menu
                    .as_ref()
                    .map(|m| m.surface.id() == surface.id())
                    .unwrap_or(false)
                {
                    Focus::Menu
                } else if app
                    .settings
                    .as_ref()
                    .map(|s| s.surface.id() == surface.id())
                    .unwrap_or(false)
                {
                    Focus::Settings
                } else if app
                    .chat
                    .as_ref()
                    .map(|c| c.surface.id() == surface.id())
                    .unwrap_or(false)
                {
                    Focus::Chat
                } else if app
                    .scrim
                    .as_ref()
                    .map(|s| s.surface.id() == surface.id())
                    .unwrap_or(false)
                {
                    Focus::Scrim
                } else {
                    Focus::Orb
                };
                match app.focus {
                    Focus::Menu => app.menu_hover(surface_y),
                    Focus::Settings => app.settings_hover(surface_x, surface_y),
                    Focus::Chat => app.chat_hover(surface_x, surface_y),
                    _ => {}
                }
            }
            E::Leave { .. } => {
                app.focus = Focus::None;
                if let Some(m) = app.menu.as_mut() {
                    if m.hover.is_some() {
                        m.hover = None;
                        m.redraw = true;
                    }
                }
                if let Some(s) = app.settings.as_mut() {
                    if s.hover.is_some() {
                        s.hover = None;
                        s.redraw = true;
                    }
                }
                if let Some(c) = app.chat.as_mut() {
                    if c.close_hover {
                        c.close_hover = false;
                        c.redraw = true;
                    }
                }
            }
            E::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer = (surface_x, surface_y);
                let dragging_orb = app
                    .press
                    .as_ref()
                    .map(|p| p.target == Focus::Orb && p.button == BTN_LEFT)
                    .unwrap_or(false);
                if dragging_orb {
                    // Motion coords are surface-local and the surface itself
                    // tracks the cursor, so absolute deltas collapse. Sum the
                    // per-event local deltas instead: between two events the
                    // surface is (mostly) uncommitted, so Δlocal ≈ Δcursor.
                    let press = app.press.as_mut().expect("checked above");
                    let dx = surface_x - press.last_local.0;
                    let dy = surface_y - press.last_local.1;
                    press.last_local = (surface_x, surface_y);
                    press.accum.0 += dx;
                    press.accum.1 += dy;
                    let (ax, ay) = press.accum;
                    if !press.dragging
                        && (ax * ax + ay * ay).sqrt() > DRAG_THRESHOLD
                        && press.origin.is_some()
                    {
                        press.dragging = true;
                    }
                    if press.dragging {
                        let (ox, oy) = press.origin.unwrap_or((0, 0));
                        app.drag_to(ox + ax as i32, oy + ay as i32);
                    }
                } else {
                    match app.focus {
                        Focus::Menu => app.menu_hover(surface_y),
                        Focus::Settings => app.settings_hover(surface_x, surface_y),
                        Focus::Chat => app.chat_hover(surface_x, surface_y),
                        _ => {}
                    }
                }
            }
            E::Button { button, state, .. } => {
                let pressed = matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                if pressed {
                    // A press landing on the scrim — anywhere that is
                    // not the chat or another overlay surface —
                    // dismisses the chat and is consumed.
                    if app.focus == Focus::Scrim {
                        app.close_chat();
                        let _ = conn.flush();
                        return;
                    }
                    app.press = Some(Press {
                        button,
                        target: app.focus,
                        last_local: app.pointer,
                        accum: (0.0, 0.0),
                        dragging: false,
                        origin: app.abs_pos(),
                    });
                } else if let Some(press) = app.press.take() {
                    if press.target == Focus::Menu {
                        if button == BTN_LEFT {
                            let (_, y) = app.pointer;
                            app.menu_click(y, qh);
                        } else {
                            app.close_menu();
                        }
                    } else if press.target == Focus::Settings {
                        if button == BTN_LEFT {
                            let (x, y) = app.pointer;
                            app.settings_click(x, y);
                        } else {
                            app.close_settings();
                        }
                    } else if press.target == Focus::Chat {
                        if button == BTN_LEFT {
                            let (x, y) = app.pointer;
                            app.chat_click(x, y);
                        }
                    } else if press.dragging {
                        let _ = app.commands.send(UiCommand::SetPlacement {
                            position: OrbPosition::Custom,
                            margin_x: app.placement.mx,
                            margin_y: app.placement.my,
                        });
                    } else {
                        match button {
                            BTN_LEFT => {
                                // Double-click toggles the chat panel.
                                // The first click already started a
                                // listen; the second cancels it so the
                                // panel opens without a stray reprompt.
                                let now = Instant::now();
                                let double = app
                                    .last_left
                                    .map(|t| now.duration_since(t) < DBLCLICK)
                                    .unwrap_or(false);
                                app.close_menu();
                                app.close_settings();
                                if double {
                                    app.last_left = None;
                                    let _ = app.commands.send(UiCommand::CancelListen);
                                    app.toggle_chat(qh);
                                } else {
                                    app.last_left = Some(now);
                                    let _ = app.commands.send(UiCommand::ListenNow);
                                }
                            }
                            BTN_RIGHT => {
                                if app.menu.is_some() {
                                    app.close_menu();
                                } else {
                                    app.open_menu(qh);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            E::Axis { axis, value, .. } => {
                let vertical = matches!(axis, WEnum::Value(wl_pointer::Axis::VerticalScroll));
                if vertical && app.focus == Focus::Settings {
                    if let Some(s) = app.settings.as_mut() {
                        settings::scroll_by(&mut s.model, value as f32 * 4.0);
                        s.redraw = true;
                    }
                } else if vertical && app.focus == Focus::Chat {
                    // Wheel-down scrolls toward the newest message.
                    if let Some(c) = app.chat.as_mut() {
                        chat::scroll_by(&mut c.model, -value as f32 * 4.0);
                        c.redraw = true;
                    }
                }
            }
            _ => {}
        }
        let _ = conn.flush();
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for OrbApp {
    fn event(
        app: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_keyboard::Event as E;
        match event {
            E::Keymap { format, fd, size } => {
                let is_xkb = matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1));
                if is_xkb {
                    // Keymap fds arrive positioned at end-of-file and
                    // may be sealed — read absolutely from offset 0.
                    // Real keymaps are tens of KB; cap the compositor-
                    // supplied size so a bogus value can't force a huge
                    // allocation.
                    if size == 0 || size > 1024 * 1024 {
                        return;
                    }
                    let f = std::fs::File::from(fd);
                    let mut buf = vec![0u8; size as usize];
                    if f.read_exact_at(&mut buf, 0).is_ok() {
                        app.kbd.set_keymap(&buf);
                    }
                }
            }
            E::Enter { surface, .. } => {
                app.kbd.chat_focused = app
                    .chat
                    .as_ref()
                    .map(|c| c.surface.id() == surface.id())
                    .unwrap_or(false);
            }
            E::Leave { surface, .. } => {
                // Keyboard focus left the chat — clicking anywhere else
                // dismisses the panel like a transient popover.
                let was_chat = app
                    .chat
                    .as_ref()
                    .map(|c| c.surface.id() == surface.id())
                    .unwrap_or(false);
                if was_chat && app.kbd.chat_focused {
                    app.kbd.chat_focused = false;
                    app.close_chat();
                }
            }
            E::Key { key, state, .. } => {
                let pressed = matches!(state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                // Releases always update xkb state even when the chat
                // is not focused — modifier tracking must not drift.
                let out = app.kbd.key(key, pressed);
                if !pressed || !app.kbd.chat_focused || app.chat.is_none() {
                    let _ = conn.flush();
                    return;
                }
                match out {
                    Some(ChatKey::Text(t)) => {
                        if let Some(c) = app.chat.as_mut() {
                            c.model.input_insert(&t);
                            c.redraw = true;
                        }
                    }
                    Some(ChatKey::Backspace) => {
                        if let Some(c) = app.chat.as_mut() {
                            c.model.input_backspace();
                            c.redraw = true;
                        }
                    }
                    Some(ChatKey::Submit) => {
                        if let Some(c) = app.chat.as_mut() {
                            if let Some(q) = c.model.submit() {
                                let _ = app.commands.send(UiCommand::SubmitQuery(q));
                            }
                            c.redraw = true;
                        }
                    }
                    Some(ChatKey::Close) => app.close_chat(),
                    None => {}
                }
            }
            E::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                app.kbd
                    .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
            }
            _ => {}
        }
        let _ = conn.flush();
    }
}

impl Dispatch<wl_output::WlOutput, ()> for OrbApp {
    fn event(
        app: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Mode { width, height, .. } => {
                app.out_mode = (width, height);
            }
            wl_output::Event::Scale { factor } => {
                app.out_scale = factor.max(1);
            }
            wl_output::Event::Done => {
                let (w, h) = app.out_mode;
                if w > 0 && h > 0 {
                    app.output_size = Some((w / app.out_scale, h / app.out_scale));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for OrbApp {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Acquire pointer and keyboard as soon as the seat advertises
        // them — the keyboard feeds the chat panel's input field.
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let caps = match capabilities {
                WEnum::Value(c) => c,
                _ => wl_seat::Capability::empty(),
            };
            if caps.contains(wl_seat::Capability::Pointer) {
                let _ = seat.get_pointer(qh, ());
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                let _ = seat.get_keyboard(qh, ());
            }
        }
    }
}

delegate_noop!(OrbApp: ignore wl_compositor::WlCompositor);
delegate_noop!(OrbApp: ignore wl_shm::WlShm);
delegate_noop!(OrbApp: ignore wl_shm_pool::WlShmPool);
delegate_noop!(OrbApp: ignore wl_buffer::WlBuffer);
delegate_noop!(OrbApp: ignore wl_surface::WlSurface);
delegate_noop!(OrbApp: ignore wl_region::WlRegion);
delegate_noop!(OrbApp: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);

fn run_orb(
    mut events: broadcast::Receiver<PipelineEvent>,
    commands: mpsc::Sender<UiCommand>,
    mode: Arc<AtomicU8>,
    startup: StartupPlacement,
    snapshot: SettingsSnapshot,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot reach Wayland display: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<OrbApp>(&conn).map_err(|e| format!("registry init failed: {e}"))?;
    let qh = queue.handle();

    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=4, ())
        .map_err(|e| format!("wl_compositor unavailable: {e}"))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm unavailable: {e}"))?;
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals
        .bind(&qh, 2..=4, ())
        .map_err(|e| format!("zwlr_layer_shell_v1 unavailable: {e}"))?;
    // Optional globals: missing seat/output degrades interaction, not
    // the overlay itself.
    let _seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    let _output: Option<wl_output::WlOutput> = globals.bind(&qh, 1..=4, ()).ok();

    let surface = compositor.create_surface(&qh, ());
    let input_region = compositor.create_region(&qh, ());
    input_region.add(0, 0, ORB_SIZE as i32, ORB_SIZE as i32);
    surface.set_input_region(Some(&input_region));

    let layer = layer_shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "tycho-orb".to_string(),
        &qh,
        (),
    );
    let placement = match startup.position {
        OrbPosition::Custom if startup.margin_x >= 0 && startup.margin_y >= 0 => {
            Placement::custom(startup.margin_x, startup.margin_y)
        }
        p => Placement::named(p),
    };
    layer.set_anchor(placement.anchor);
    layer.set_size(ORB_SIZE, ORB_SIZE);
    let (t, r, b, l) = placement.margins();
    layer.set_margin(t, r, b, l);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    surface.commit();

    let mem = memfd::MemfdOptions::default()
        .create("tycho-orb")
        .map_err(|e| format!("memfd create failed: {e}"))?;
    let file = mem.as_file();
    let buf_len = (ORB_SIZE * ORB_SIZE * 4) as u64;
    file.set_len(buf_len)
        .map_err(|e| format!("memfd resize failed: {e}"))?;
    let pool = shm.create_pool(file.as_fd(), buf_len as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        ORB_SIZE as i32,
        ORB_SIZE as i32,
        (ORB_SIZE * 4) as i32,
        wl_shm::Format::Argb8888,
        &qh,
        (),
    );

    let mut app = OrbApp {
        visual: OrbVisual::default(),
        configured: false,
        closed: false,
        mode,
        commands,
        compositor,
        layer_shell,
        shm,
        surface,
        layer,
        position: startup.position,
        placement,
        output_size: None,
        out_mode: (0, 0),
        out_scale: 1,
        pointer: (0.0, 0.0),
        focus: Focus::None,
        press: None,
        menu: None,
        settings: None,
        chat: None,
        scrim: None,
        last_left: None,
        kbd: Kbd::default(),
        snapshot,
        last_mode: ActivationMode::Auto,
    };

    while !app.closed && running.load(Ordering::Relaxed) {
        loop {
            match events.try_recv() {
                Ok(ev) => {
                    app.visual.apply_event(&ev);
                    // The chat transcript rides the same broadcast:
                    // transcriptions become user turns, responses and
                    // action outputs become Tycho turns.
                    if let Some(c) = app.chat.as_mut() {
                        if c.model.observe(&ev) {
                            c.redraw = true;
                        }
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        queue
            .roundtrip(&mut app)
            .map_err(|e| format!("wayland dispatch failed: {e}"))?;
        if !app.configured {
            std::thread::sleep(FRAME);
            continue;
        }

        let mode_now = ActivationMode::load(&app.mode);
        if mode_now != app.last_mode {
            app.last_mode = mode_now;
            app.visual.dirty = true;
        }
        let shown = if mode_now == ActivationMode::Off {
            UiState::Off
        } else {
            app.visual.state
        };

        if app.visual.animating() && mode_now != ActivationMode::Off || app.visual.dirty {
            app.visual.phase += 0.09;
            app.visual.amplitude *= 0.92;
            let rgba = render_frame(ORB_SIZE, shown, app.visual.phase, app.visual.amplitude);
            file.write_all_at(&rgba_to_argb8888(&rgba), 0)
                .map_err(|e| format!("buffer write failed: {e}"))?;
            app.surface.attach(Some(&buffer), 0, 0);
            app.surface
                .damage_buffer(0, 0, ORB_SIZE as i32, ORB_SIZE as i32);
            app.surface.commit();
            app.visual.dirty = false;
        }

        if app
            .menu
            .as_ref()
            .map(|m| m.configured && m.redraw)
            .unwrap_or(false)
        {
            app.render_menu_frame();
        }
        if app
            .settings
            .as_ref()
            .map(|s| s.configured && s.redraw)
            .unwrap_or(false)
        {
            app.render_settings_frame();
        }
        if app
            .chat
            .as_ref()
            .map(|c| c.configured && c.redraw)
            .unwrap_or(false)
        {
            app.render_chat_frame();
        }
        conn.flush()
            .map_err(|e| format!("wayland flush failed: {e}"))?;
        std::thread::sleep(FRAME);
    }
    Ok(())
}
