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
/// Like Lens in Chrome, the coloured border appears once a drag ends: it fades in, spins,
/// then eases to a stop so an idle overlay costs nothing.
const GLOW_IN: Duration = Duration::from_millis(350);
const SPIN: Duration = Duration::from_millis(2500);
const SPIN_SPEED: f64 = 260.0; // degrees per second at full speed

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
    let mut settled: Option<Instant> = None; // when the current box was released; drives the glow
    let mut angle = 0.0;
    let mut sel: Option<Rect> = None;
    let mut drag: Option<(&'static str, i32, i32, Rect)> = None; // (mode, start x, start y, box at start)
    let mut cursor = String::from("crosshair");

    let mut warm = false;

    let action = 'run: loop {
        let animating = opened.elapsed() < FADE || settled.is_some_and(|t| t.elapsed() < SPIN);
        let mut next = if animating { x.conn.poll_for_event()? } else { Some(x.conn.wait_for_event()?) };
        let mut dirty = animating;
        let had_input = next.is_some();
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
                    settled = None;
                    dirty = true;
                }
                Event::MotionNotify(m) => {
                    let (px, py) = (m.event_x as i32, m.event_y as i32);
                    if let Some((mode, sx, sy, orig)) = drag {
                        sel = Some(geom::adjust(mode, orig, px - sx, py - sy, x.w, x.h));
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
                    } else {
                        settled = Some(Instant::now());
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
            let ease = |t: f64| 1.0 - (1.0 - t.min(1.0)).powi(3);
            let since = settled.map(|t| t.elapsed().as_secs_f64());
            let slowdown = since.map_or(0.0, |e| 1.0 - (e / SPIN.as_secs_f64()).min(1.0));
            angle = (angle + (now - last_frame).as_secs_f64() * SPIN_SPEED * slowdown) % 360.0;
            last_frame = now;
            let view = View {
                sel,
                fade: ease(opened.elapsed().as_secs_f64() / FADE.as_secs_f64()),
                glow: since.map_or(0.0, |e| ease(e / GLOW_IN.as_secs_f64())),
                angle,
                show_buttons: drag.is_none(),
            };
            scene.draw(&x, &view)?;
            if !warm {
                scene.warm_up(&x)?; // the next frame repaints the whole buffer
                warm = true;
            }
        }
        if animating && !had_input {
            std::thread::sleep(Duration::from_millis(8)); // idle spin/fade: cap at ~60 fps; input renders at once
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
