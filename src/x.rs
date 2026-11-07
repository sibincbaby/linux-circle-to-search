//! X11 plumbing: frozen screenshot, override-redirect overlay, grabs, and XRender helpers.
//! All drawing runs inside the X server (GPU-accelerated via glamor), so this process never
//! holds the full-screen image.

use std::error::Error;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::cursor;
use x11rb::protocol::render::{self, ConnectionExt as _, Color, Fixed, PictOp, Pictformat, Picture, Pointfix};
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

    /// Conical (sweep) gradient around (cx, cy), starting at `angle` degrees.
    pub fn sweep(&self, cx: f64, cy: f64, angle: f64, stops: &[(f64, Color)]) -> Res<Picture> {
        let p = self.conn.generate_id()?;
        let (s, c): (Vec<Fixed>, Vec<Color>) = stops.iter().map(|(s, c)| (fixed(*s), *c)).unzip();
        self.conn.render_create_conical_gradient(p, pf(cx, cy), fixed(angle), &s, &c)?;
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

    /// Anti-aliased filled polygon: fan from `center` through `outline`, painted with `src`
    /// at identity alignment (screen pixel (x, y) takes source pixel (x, y)).
    pub fn fan(&self, src: Picture, center: (i32, i32), outline: &[(f64, f64)]) -> Res<()> {
        let mut pts = vec![pf(center.0 as f64, center.1 as f64)];
        pts.extend(outline.iter().map(|p| pf(p.0, p.1)));
        pts.push(pf(outline[0].0, outline[0].1));
        // Render aligns the source so (src_x, src_y) sits at the first point (truncated).
        self.conn.render_tri_fan(PictOp::OVER, src, self.back, self.a8, center.0 as i16, center.1 as i16, &pts)?;
        Ok(())
    }

    /// Anti-aliased triangle strip (from `geom::strip`), painted with `src` at identity alignment.
    pub fn strip(&self, src: Picture, pts: &[(f64, f64)]) -> Res<()> {
        let fx: Vec<_> = pts.iter().map(|p| pf(p.0, p.1)).collect();
        let (sx, sy) = (pts[0].0 as i16, pts[0].1 as i16);
        self.conn.render_tri_strip(PictOp::OVER, src, self.back, self.a8, sx, sy, &fx)?;
        Ok(())
    }

    /// Upload an 8-bit coverage bitmap (e.g. rendered text) as an A8 mask picture.
    pub fn upload_mask(&self, data: &[u8], w: usize, h: usize) -> Res<Picture> {
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
        self.conn.render_create_picture(pic, pix, self.a8, &Default::default())?;
        self.conn.free_pixmap(pix)?;
        Ok(pic)
    }

    /// Paint `src` through an A8 `mask` of size (w, h) at (x, y).
    pub fn masked(&self, src: Picture, mask: Picture, x: i32, y: i32, w: usize, h: usize) -> Res<()> {
        self.conn
            .render_composite(PictOp::OVER, src, mask, self.back, 0, 0, 0, 0, x as i16, y as i16, w as u16, h as u16)?;
        Ok(())
    }

    /// Paint the frozen screen over the whole frame.
    pub fn paint_shot(&self) -> Res<()> {
        let (w, h) = (self.w as u16, self.h as u16);
        self.conn.render_composite(PictOp::SRC, self.shot, NONE, self.back, 0, 0, 0, 0, 0, 0, w, h)?;
        Ok(())
    }
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
