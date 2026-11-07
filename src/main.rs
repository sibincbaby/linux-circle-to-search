//! Circle to Search for Linux (X11): drag a box around anything on screen (Google Lens style),
//! adjust it by its corners/edges, then search it with Google Lens (Enter) or copy its text
//! via Tesseract OCR (C). Esc / right-click cancels. Bind the binary to a keyboard shortcut.

mod actions;
mod draw;
mod geom;
mod x;

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;

use draw::{Scene, View};
use geom::Rect;
use x::{Res, X};

const XK_ESCAPE: u32 = 0xff1b;
const XK_RETURN: u32 = 0xff0d;
const XK_KP_ENTER: u32 = 0xff8d;
const XK_C: u32 = 0x43;
const XK_LOWER_C: u32 = 0x63;
const FADE: Duration = Duration::from_millis(180);
/// The glow spins while the box changes, then eases to a stop so an idle overlay costs nothing.
const SPIN: Duration = Duration::from_millis(1600);
const SPIN_SPEED: f64 = 220.0; // degrees per second at full speed

enum Action {
    Search,
    Text,
}

fn main() {
    if let Err(e) = run() {
        actions::notify(&format!("Circle to Search failed: {e}"), "dialog-error");
        eprintln!("circle-search: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let x = X::open()?;
    let scene = Scene::new(&x)?;
    x.show()?;

    let s = x.scale;
    let handle = (12.0 * s) as i32;
    let min_size = (12.0 * s) as i32;
    let opened = Instant::now();
    let mut last_frame = opened;
    let mut changed = opened.checked_sub(SPIN).unwrap_or(opened); // last selection change; drives the spin
    let mut angle = 0.0;
    let mut sel: Option<Rect> = None;
    let mut drag: Option<(&'static str, i32, i32, Rect)> = None; // (mode, start x, start y, box at start)
    let mut cursor = String::from("crosshair");

    let action = 'run: loop {
        let animating = opened.elapsed() < FADE || changed.elapsed() < SPIN;
        let mut next = if animating { x.conn.poll_for_event()? } else { Some(x.conn.wait_for_event()?) };
        let mut dirty = animating;
        while let Some(event) = next {
            match event {
                Event::Expose(_) => dirty = true,
                Event::KeyPress(k) => {
                    let syms = x.keysyms(k.detail);
                    let ready = sel.is_some() && drag.is_none();
                    if syms.contains(&XK_ESCAPE) {
                        break 'run None;
                    }
                    if ready && (syms.contains(&XK_RETURN) || syms.contains(&XK_KP_ENTER)) {
                        break 'run Some(Action::Search);
                    }
                    if ready && (syms.contains(&XK_C) || syms.contains(&XK_LOWER_C)) {
                        break 'run Some(Action::Text);
                    }
                }
                Event::ButtonPress(b) if b.detail == 3 => break 'run None,
                Event::ButtonPress(b) if b.detail == 1 => {
                    let (px, py) = (b.event_x as i32, b.event_y as i32);
                    if let Some(cur) = sel {
                        match scene.button_rects(&x, cur).into_iter().position(|r| geom::contains(r, px, py)) {
                            Some(0) => break 'run Some(Action::Search),
                            Some(_) => break 'run Some(Action::Text),
                            None => {}
                        }
                    }
                    let start = match sel.and_then(|cur| geom::hit(cur, px, py, handle).map(|m| (m, cur))) {
                        Some(found) => found,
                        None => ("se", (px, py, 0, 0)), // outside the box: start a new one here
                    };
                    sel = Some(start.1);
                    drag = Some((start.0, px, py, start.1));
                    changed = Instant::now();
                    dirty = true;
                }
                Event::MotionNotify(m) => {
                    let (px, py) = (m.event_x as i32, m.event_y as i32);
                    if let Some((mode, sx, sy, orig)) = drag {
                        sel = Some(geom::adjust(mode, orig, px - sx, py - sy, x.w, x.h));
                        changed = Instant::now();
                        dirty = true;
                    } else {
                        let want = match sel {
                            None => "crosshair".to_string(),
                            Some(cur) if scene.button_rects(&x, cur).into_iter().any(|r| geom::contains(r, px, py)) => {
                                "pointer".to_string()
                            }
                            Some(cur) => match geom::hit(cur, px, py, handle) {
                                Some("move") => "move".to_string(),
                                Some(edge) => format!("{edge}-resize"),
                                None => "pointer".to_string(), // outside: normal hand; dragging still starts a new box
                            },
                        };
                        if want != cursor {
                            x.set_cursor(&want)?;
                            cursor = want;
                        }
                    }
                }
                Event::ButtonRelease(b) if b.detail == 1 && drag.is_some() => {
                    drag = None;
                    if sel.is_some_and(|(_, _, w, h)| w < min_size || h < min_size) {
                        sel = None;
                    }
                    dirty = true;
                }
                Event::Error(e) => eprintln!("circle-search: X error {e:?}"),
                _ => {}
            }
            next = x.conn.poll_for_event()?;
        }

        if dirty {
            let now = Instant::now();
            let slowdown = 1.0 - (changed.elapsed().as_secs_f64() / SPIN.as_secs_f64()).min(1.0);
            angle = (angle + (now - last_frame).as_secs_f64() * SPIN_SPEED * slowdown) % 360.0;
            last_frame = now;
            let fade = (opened.elapsed().as_secs_f64() / FADE.as_secs_f64()).min(1.0);
            let view = View { sel, fade: 1.0 - (1.0 - fade).powi(3), angle, show_buttons: drag.is_none() };
            scene.draw(&x, &view)?;
        }
        if animating {
            std::thread::sleep(Duration::from_millis(8)); // present() round-trips; this caps at ~60 fps
        }
    };

    x.hide()?;
    let (Some(action), Some((sx, sy, sw, sh))) = (action, sel) else { return Ok(()) };
    let png = actions::png(&x.crop_rgb(sx, sy, sw, sh)?, sw as u32, sh as u32);
    drop(x); // release the frozen screen before the slow part
    match action {
        Action::Search => actions::search(&png)?,
        Action::Text => match actions::copy_text(&png) {
            Ok(text) if text.is_empty() => actions::notify("No text found in the selection", "dialog-warning"),
            Ok(text) => actions::notify(&format!("Copied text:\n{}", text.chars().take(200).collect::<String>()), "edit-copy"),
            Err(e) => actions::notify(&e, "dialog-error"),
        },
    }
    Ok(())
}
