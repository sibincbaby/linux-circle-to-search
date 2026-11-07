//! X11 plumbing: frozen screenshot, override-redirect overlay, grabs, and XRender helpers.
//! All drawing runs inside the X server (GPU-accelerated via glamor), so this process never
//! holds the full-screen image.

use std::error::Error;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::cursor;
use x11rb::protocol::render::{self, ConnectionExt as _, Color, CreatePictureAux, Fixed, PictOp, Pictformat, Picture, Pointfix, Repeat};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::{resource_manager, COPY_DEPTH_FROM_PARENT, NONE};

pub type Res<T> = Result<T, Box<dyn Error>>;

pub struct X {
    pub conn: RustConnection,
    pub w: i32,
    pub h: i32,
    /// HiDPI factor from Xft.dpi (2.0 at 192 dpi); UI sizes are multiplied by it.
    pub scale: f64,
    pub win: Window,
    shot_pix: Pixmap,
    /// The frozen screen.
    pub shot: Picture,
    /// Off-screen frame buffer, copied to the window in one go to avoid flicker.
    pub back: Picture,
    win_pic: Picture,
    pub a8: Pictformat,
    cursors: cursor::Handle,
    keymap: GetKeyboardMappingReply,
    min_keycode: u8,
}

impl X {
    pub fn open() -> Res<X> {
        let (conn, sn) = RustConnection::connect(None)?;
        let scr = conn.setup().roots[sn].clone();
        let (w, h, root) = (scr.width_in_pixels, scr.height_in_pixels, scr.root);

        // Freeze the screen into a server-side pixmap before anything of ours appears.
        let shot_pix = conn.generate_id()?;
        conn.create_pixmap(scr.root_depth, shot_pix, root, w, h)?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, root, &CreateGCAux::new().subwindow_mode(SubwindowMode::INCLUDE_INFERIORS))?;
        conn.copy_area(root, shot_pix, gc, 0, 0, 0, 0, w, h)?;
        conn.free_gc(gc)?;

        let db = resource_manager::new_from_default(&conn)?;
        let dpi = db.get_value::<u32>("Xft.dpi", "").ok().flatten().unwrap_or(96);
        // x11rb only reads XCURSOR_* / Xcursor.*; desktops like Cinnamon publish the theme via
        // XSETTINGS instead, so without this the overlay shows a different cursor than every app.
        let xs = xsettings(&conn, sn).unwrap_or_default();
        for (key, env) in [("Gtk/CursorThemeName", "XCURSOR_THEME"), ("Gtk/CursorThemeSize", "XCURSOR_SIZE")] {
            if let (Some(v), None) = (xs.get(key), std::env::var_os(env)) {
                std::env::set_var(env, v); // single-threaded here, before anything reads the env
            }
        }
        let cursors = cursor::Handle::new(&conn, sn, &db)?.reply()?;

        let formats = conn.render_query_pict_formats()?.reply()?;
        let visual_fmt = formats
            .screens
            .iter()
            .flat_map(|s| &s.depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual == scr.root_visual)
            .ok_or("no XRender format for the root visual")?
            .format;
        let a8 = formats
            .formats
            .iter()
            .find(|f| f.depth == 8 && f.type_ == render::PictType::DIRECT && f.direct.alpha_mask == 0xff)
            .ok_or("no A8 XRender format")?
            .id;

        let win = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixmap(NONE)
                .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS),
        )?;

        let shot = conn.generate_id()?;
        conn.render_create_picture(shot, shot_pix, visual_fmt, &Default::default())?;
        let back_pix = conn.generate_id()?;
        conn.create_pixmap(scr.root_depth, back_pix, root, w, h)?;
        let back = conn.generate_id()?;
        conn.render_create_picture(back, back_pix, visual_fmt, &Default::default())?;
        conn.free_pixmap(back_pix)?; // the picture keeps it alive
        let win_pic = conn.generate_id()?;
        conn.render_create_picture(win_pic, win, visual_fmt, &Default::default())?;

        let min_keycode = conn.setup().min_keycode;
        let keymap = conn.get_keyboard_mapping(min_keycode, conn.setup().max_keycode - min_keycode + 1)?.reply()?;

        Ok(X {
            w: w as i32,
            h: h as i32,
            scale: (dpi as f64 / 96.0).max(1.0),
            conn,
            win,
            shot_pix,
            shot,
            back,
            win_pic,
            a8,
            cursors,
            keymap,
            min_keycode,
        })
    }

    /// Map the overlay and take pointer + keyboard (override-redirect windows get no focus
    /// from the WM). Retries briefly: right after the hotkey the WM may still hold the keyboard.
    pub fn show(&self) -> Res<()> {
        self.conn.map_window(self.win)?;
        self.set_cursor("crosshair")?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let kb = self
                .conn
                .grab_keyboard(true, self.win, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?
                .reply()?;
            let ptr = self
                .conn
                .grab_pointer(
                    false,
                    self.win,
                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    NONE,
                    NONE,
                    x11rb::CURRENT_TIME,
                )?
                .reply()?;
            if kb.status == GrabStatus::SUCCESS && ptr.status == GrabStatus::SUCCESS {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("could not grab keyboard/pointer".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn hide(&self) -> Res<()> {
        self.conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
        self.conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        self.conn.unmap_window(self.win)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Themed cursor by name ("crosshair", "pointer", "move", "nw-resize", ...).
    pub fn set_cursor(&self, name: &str) -> Res<()> {
        let c = self.cursors.load_cursor(&self.conn, name)?;
        self.conn.change_window_attributes(self.win, &ChangeWindowAttributesAux::new().cursor(c))?;
        self.conn.free_cursor(c)?;
        Ok(())
    }

    /// All keysyms on a keycode (every layout group/level), e.g. 0xff1b for Escape.
    pub fn keysyms(&self, keycode: u8) -> &[u32] {
        let per = self.keymap.keysyms_per_keycode as usize;
        let i = keycode.saturating_sub(self.min_keycode) as usize * per;
        self.keymap.keysyms.get(i..i + per).unwrap_or(&[])
    }

    /// Copy the finished frame to the window and wait until the server has drawn it.
    pub fn present(&self) -> Res<()> {
        let (w, h) = (self.w as u16, self.h as u16);
        self.conn.render_composite(PictOp::SRC, self.back, NONE, self.win_pic, 0, 0, 0, 0, 0, 0, w, h)?;
        self.conn.get_input_focus()?.reply()?; // round-trip = frame done, paces the animation
        Ok(())
    }

    /// RGB bytes of a region of the frozen screen.
    pub fn crop_rgb(&self, x: i32, y: i32, w: i32, h: i32) -> Res<Vec<u8>> {
        let img = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.shot_pix, x as i16, y as i16, w as u16, h as u16, !0)?
            .reply()?;
        // shortcut: assumes 32-bpp little-endian BGRX (every desktop X server today); handle others if needed.
        let ok = img.depth == 24 && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        if !ok || img.data.len() != (w * h * 4) as usize {
            return Err("unsupported screen pixel format".into());
        }
        Ok(img.data.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0]]).collect())
    }

    pub fn solid(&self, c: Color) -> Res<Picture> {
        let p = self.conn.generate_id()?;
        self.conn.render_create_solid_fill(p, c)?;
        Ok(p)
    }

    /// Linear gradient from p1 to p2 in screen coordinates (use with `composite`'s identity alignment).
    /// Linear, not conical: the X server draws conical gradients on the CPU (~7x slower here).
    pub fn linear(&self, p1: (f64, f64), p2: (f64, f64), stops: &[(f64, Color)]) -> Res<Picture> {
        let p = self.conn.generate_id()?;
        let (s, c): (Vec<Fixed>, Vec<Color>) = stops.iter().map(|(s, c)| (fixed(*s), *c)).unzip();
        self.conn.render_create_linear_gradient(p, pf(p1.0, p1.1), pf(p2.0, p2.1), &s, &c)?;
        // extend the end colours beyond p1/p2 (the default leaves those pixels transparent)
        self.conn.render_change_picture(p, &render::ChangePictureAux::new().repeat(Repeat::PAD))?;
        Ok(p)
    }

    pub fn free(&self, p: Picture) -> Res<()> {
        self.conn.render_free_picture(p)?;
        Ok(())
    }

    pub fn fill_rects(&self, c: Color, rects: &[(i32, i32, i32, i32)]) -> Res<()> {
        let r: Vec<_> = rects
            .iter()
            .filter(|r| r.2 > 0 && r.3 > 0)
            .map(|&(x, y, w, h)| Rectangle { x: x as i16, y: y as i16, width: w as u16, height: h as u16 })
            .collect();
        self.conn.render_fill_rectangles(PictOp::OVER, self.back, c, &r)?;
        Ok(())
    }

    /// Paint `src` over `dst` rect (x, y, w, h) through an optional A8 `mask` whose pixel
    /// (mx, my) lands at (x, y). The source is aligned 1:1 with the screen. Only plain
    /// composites are used for drawing: X's anti-aliased triangles fall back to the CPU
    /// and cost 10-60 ms each at 4K.
    pub fn composite(&self, src: Picture, mask: Option<Picture>, (mx, my): (i32, i32), (x, y, w, h): (i32, i32, i32, i32)) -> Res<()> {
        if w > 0 && h > 0 {
            let (x16, y16) = (x as i16, y as i16);
            let m = mask.unwrap_or(NONE);
            self.conn.render_composite(PictOp::OVER, src, m, self.back, x16, y16, mx as i16, my as i16, x16, y16, w as u16, h as u16)?;
        }
        Ok(())
    }

    /// Upload an 8-bit coverage bitmap (text, shape) as an A8 mask picture; `repeat` tiles it.
    pub fn upload_mask(&self, data: &[u8], w: usize, h: usize, repeat: bool) -> Res<Picture> {
        let stride = w.div_ceil(4) * 4; // X scanlines are padded to 32 bits
        let mut padded = vec![0u8; stride * h];
        for (dst, src) in padded.chunks_exact_mut(stride).zip(data.chunks_exact(w)) {
            dst[..w].copy_from_slice(src);
        }
        let pix = self.conn.generate_id()?;
        self.conn.create_pixmap(8, pix, self.win, w as u16, h as u16)?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, pix, &Default::default())?;
        self.conn.put_image(ImageFormat::Z_PIXMAP, pix, gc, w as u16, h as u16, 0, 0, 0, 8, &padded)?;
        self.conn.free_gc(gc)?;
        let pic = self.conn.generate_id()?;
        let aux = CreatePictureAux::new().repeat(if repeat { Repeat::NORMAL } else { Repeat::NONE });
        self.conn.render_create_picture(pic, pix, self.a8, &aux)?;
        self.conn.free_pixmap(pix)?;
        Ok(pic)
    }

    /// Copy the frozen screen into the frame at the same place.
    pub fn copy_shot(&self, (x, y, w, h): (i32, i32, i32, i32)) -> Res<()> {
        if w > 0 && h > 0 {
            let (x16, y16) = (x as i16, y as i16);
            self.conn.render_composite(PictOp::SRC, self.shot, NONE, self.back, x16, y16, 0, 0, x16, y16, w as u16, h as u16)?;
        }
        Ok(())
    }
}

/// String and integer XSETTINGS (Gtk/CursorThemeName, ...) from the settings manager.
fn xsettings(conn: &RustConnection, screen: usize) -> Res<std::collections::HashMap<String, String>> {
    let atom = |name: &str| -> Res<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let owner = conn.get_selection_owner(atom(&format!("_XSETTINGS_S{screen}"))?)?.reply()?.owner;
    if owner == NONE {
        return Ok(Default::default());
    }
    let prop = conn.get_property(false, owner, atom("_XSETTINGS_SETTINGS")?, AtomEnum::ANY, 0, 1 << 16)?.reply()?;
    Ok(parse_xsettings(&prop.value).unwrap_or_default())
}

/// Parse the XSETTINGS wire format; colours are skipped. None on malformed data.
fn parse_xsettings(d: &[u8]) -> Option<std::collections::HashMap<String, String>> {
    let le = *d.first()? == 0;
    let u16_at = |i: usize| d.get(i..i + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |i: usize| {
        d.get(i..i + 4).map(|b| {
            let a = [b[0], b[1], b[2], b[3]];
            if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
        })
    };
    let pad = |n: usize| n.div_ceil(4) * 4;
    let mut out = std::collections::HashMap::new();
    let mut i = 12;
    for _ in 0..u32_at(8)? {
        let kind = *d.get(i)?;
        let name_len = u16_at(i + 2)? as usize;
        let name = String::from_utf8_lossy(d.get(i + 4..i + 4 + name_len)?).into_owned();
        i += 4 + pad(name_len) + 4; // header, name, last-change serial
        match kind {
            0 => {
                out.insert(name, (u32_at(i)? as i32).to_string());
                i += 4;
            }
            1 => {
                let len = u32_at(i)? as usize;
                out.insert(name, String::from_utf8_lossy(d.get(i + 4..i + 4 + len)?).into_owned());
                i += 4 + pad(len);
            }
            2 => i += 8,
            _ => return None,
        }
    }
    Some(out)
}

/// 16.16 fixed point.
pub fn fixed(v: f64) -> Fixed {
    (v * 65536.0).round() as Fixed
}

fn pf(x: f64, y: f64) -> Pointfix {
    Pointfix { x: fixed(x), y: fixed(y) }
}

/// Premultiplied RGBA, as solid fills and FillRectangles expect.
pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Color {
    straight(r * a, g * a, b * a, a)
}

/// Non-premultiplied RGBA, as gradient stops expect.
pub fn straight(r: f64, g: f64, b: f64, a: f64) -> Color {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    Color { red: c(r), green: c(g), blue: c(b), alpha: c(a) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_xsettings_strings_and_ints() {
        let mut d = vec![0u8, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0]; // LSB, serial 1, 3 settings
        let name = |d: &mut Vec<u8>, kind: u8, n: &str| {
            d.extend([kind, 0]);
            d.extend((n.len() as u16).to_le_bytes());
            d.extend(n.as_bytes());
            d.resize(d.len().div_ceil(4) * 4, 0);
            d.extend(0u32.to_le_bytes());
        };
        name(&mut d, 1, "Gtk/CursorThemeName");
        d.extend(9u32.to_le_bytes());
        d.extend(b"DMZ-White\0\0\0");
        name(&mut d, 2, "Net/Color"); // colours are skipped
        d.extend([0u8; 8]);
        name(&mut d, 0, "Gtk/CursorThemeSize");
        d.extend(68i32.to_le_bytes());
        let s = super::parse_xsettings(&d).unwrap();
        assert_eq!(s["Gtk/CursorThemeName"], "DMZ-White");
        assert_eq!(s["Gtk/CursorThemeSize"], "68");
        assert!(super::parse_xsettings(&d[..20]).is_none()); // truncated
    }
}
