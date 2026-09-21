//! Injects synthetic pointer input via `zwlr_virtual_pointer_v1` to
//! exercise the orb's interactions on a live compositor (wlr/Hyprland).
//!
//! Usage: `cargo run --example orb_input_demo -- <drag|click|menu> [x y]`
//! `x y` is the pointer start point in logical output coordinates
//! (the orb's on-screen location). Waits are built in so `grim` can be
//! run between actions from another shell.

use std::time::Duration;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

struct VpApp;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for VpApp {
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

delegate_noop!(VpApp: ignore wl_seat::WlSeat);
delegate_noop!(VpApp: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(VpApp: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);

fn move_abs(
    vp: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
    t: &mut u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) {
    vp.motion_absolute(*t, x, y, w, h);
    vp.frame();
    *t += 30;
}

fn click(vp: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, t: &mut u32, button: u32) {
    vp.button(*t, button, wl_pointer::ButtonState::Pressed);
    vp.frame();
    *t += 40;
    vp.button(*t, button, wl_pointer::ButtonState::Released);
    vp.frame();
    *t += 40;
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let action = args.get(1).map(|s| s.as_str()).unwrap_or("click");
    let x: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(80);
    let y: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(704);
    let (w, h): (u32, u32) = (1280, 800); // logical output extents

    let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<VpApp>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();

    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
    let manager: zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| format!("zwlr_virtual_pointer_manager_v1 unavailable: {e}"))?;
    let vp = manager.create_virtual_pointer(Some(&seat), &qh, ());

    queue.roundtrip(&mut VpApp).map_err(|e| e.to_string())?;
    let mut t = 1u32;

    match action {
        "drag" => {
            // Optional 4th/5th args: destination (x2, y2). Default is the
            // legacy right-220/up-140 path.
            let x2: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(x + 220);
            let y2: u32 = args
                .get(5)
                .and_then(|s| s.parse().ok())
                .unwrap_or(y.saturating_sub(140));
            move_abs(&vp, &mut t, x, y, w, h);
            vp.button(t, BTN_LEFT, wl_pointer::ButtonState::Pressed);
            vp.frame();
            t += 60;
            for i in 1..=10u32 {
                let ix = x as i64 + (x2 as i64 - x as i64) * i as i64 / 10;
                let iy = y as i64 + (y2 as i64 - y as i64) * i as i64 / 10;
                move_abs(&vp, &mut t, ix.max(0) as u32, iy.max(0) as u32, w, h);
            }
            std::thread::sleep(Duration::from_millis(120));
            vp.button(t, BTN_LEFT, wl_pointer::ButtonState::Released);
            vp.frame();
            eprintln!("drag complete");
        }
        "menu" => {
            // Optional 4th arg: menu row index to left-click after opening.
            let row: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
            move_abs(&vp, &mut t, x, y, w, h);
            click(&vp, &mut t, BTN_RIGHT);
            eprintln!("right-click sent; menu should be visible for 4s");
            std::thread::sleep(Duration::from_secs(4));
            // Menu opens above the orb: top-left = (orb_x, orb_y - 206).
            // Row centers: items 0,1,2 at +15/+45/+75; item 4 at +114;
            // items 6,7 at +153/+183 (separators are inert).
            let row_off = match row {
                0 => 15,
                1 => 45,
                2 => 75,
                4 => 114,
                6 => 153,
                _ => 183,
            };
            move_abs(&vp, &mut t, x - 48 + 60, y - 48 - 206 + row_off, w, h);
            click(&vp, &mut t, BTN_LEFT);
            eprintln!("menu row {} click sent", row);
        }
        "click" => {
            move_abs(&vp, &mut t, x, y, w, h);
            click(&vp, &mut t, BTN_LEFT);
            eprintln!("left-click sent");
        }
        "doubleclick" => {
            // Two left clicks inside the orb's 400ms window — the
            // second cancels the first's listen and opens chat.
            move_abs(&vp, &mut t, x, y, w, h);
            click(&vp, &mut t, BTN_LEFT);
            std::thread::sleep(Duration::from_millis(120));
            click(&vp, &mut t, BTN_LEFT);
            eprintln!("double-click sent; chat should open");
        }
        "open" => {
            // Right-click only: leaves the menu open for follow-up runs.
            move_abs(&vp, &mut t, x, y, w, h);
            click(&vp, &mut t, BTN_RIGHT);
            eprintln!("right-click sent; menu left open");
        }
        "hover" => {
            // Motion only, no buttons.
            move_abs(&vp, &mut t, x, y, w, h);
            eprintln!("hovered {} {}", x, y);
            std::thread::sleep(Duration::from_millis(800));
        }
        other => {
            eprintln!("unknown action {other}; use drag|click|menu");
            std::process::exit(2);
        }
    }

    conn.flush().map_err(|e| e.to_string())?;
    queue.roundtrip(&mut VpApp).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(400));
    Ok(())
}
