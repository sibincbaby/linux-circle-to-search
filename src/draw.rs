//! The overlay scene: dimmed frozen screen, rounded selection with a Google-coloured sweep
//! glow, Lens-style corner brackets, action buttons and a hint pill.
//!
//! Every shape is a small A8 mask rasterised once at startup and placed with plain XRender
//! composites (the GPU fast path); per frame only positions and gradient colours change.

use x11rb::protocol::render::{Color, Picture};

use crate::geom::{self, Rect};
use crate::x::{rgba, straight, Res, X};

pub const BUTTONS: [&str; 2] = ["Search", "Copy text"];
const HINT: &str = "Drag to select  ·  Enter to search  ·  C to copy text  ·  Esc to cancel";
// Google blue, red, yellow, green
const GOOGLE: [(f64, f64, f64); 4] = [(0.259, 0.522, 0.957), (0.918, 0.263, 0.208), (0.984, 0.737, 0.020), (0.204, 0.659, 0.325)];
/// Corners in order top-left, top-right, bottom-right, bottom-left as (mirror x, mirror y).
const CORNERS: [(bool, bool); 4] = [(false, false), (true, false), (true, true), (false, true)];

struct Mask {
    pic: Picture,
    w: i32,
    h: i32,
}

pub struct Scene {
    white: Picture,
    dark: Picture,
    buttons: Vec<Mask>,
    hint: Option<Mask>,
    btn_pill: Mask,
    hint_pill: Option<Mask>,
    /// Selection corner radius, glow reach, and bracket padding/size, in device pixels.
    r: i32,
    g: i32,
    bracket_pad: i32,
    bracket_size: i32,
    /// Glow profile across an edge: top, bottom, left, right (1 px wide/tall, tiled).
    glow_edges: [Picture; 4],
    glow_corners: [Picture; 4],
    fill_corners: [Picture; 4],
    brackets: [Picture; 4],
    pub btn_w: i32,
    pub btn_h: i32,
    pub gap: i32,
}

/// Per-frame state the scene is drawn from.
pub struct View {
    pub sel: Option<Rect>,
    /// 0..1 fade-in of the dim layer.
    pub fade: f64,
    /// 0..1 strength of the coloured border (0 while dragging).
    pub glow: f64,
    /// Sweep rotation in degrees.
    pub angle: f64,
    pub show_buttons: bool,
}

/// Coverage bitmap: `f(x, y)` gives 0..1 at each pixel centre.
fn raster(w: usize, h: usize, f: impl Fn(f64, f64) -> f64) -> Vec<u8> {
    (0..w * h)
        .map(|i| (f((i % w) as f64 + 0.5, (i / w) as f64 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// Flip a w×h bitmap so a top-left shape becomes another corner's.
fn mirror(src: &[u8], w: usize, h: usize, (fx, fy): (bool, bool)) -> Vec<u8> {
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            src[if fy { h - 1 - y } else { y } * w + if fx { w - 1 - x } else { x }]
        })
        .collect()
}

/// Glow strength `d` px outside the box edge: a crisp 3 px ring easing into a soft halo.
fn glow_alpha(d: f64, s: f64) -> f64 {
    let ring = 3.0 * s;
    let halo = 0.4 * (-(d - ring).max(0.0) / (5.0 * s)).exp();
    let t = (d - ring + 0.5).clamp(0.0, 1.0); // 1 px blend from ring to halo
    (1.0 - t + t * halo) * (d + 0.5).clamp(0.0, 1.0) // inner edge anti-aliasing
}

/// Distance from (x, y) to a Lens-style bracket path for the top-left corner, in box-corner
/// coordinates: quarter arc of radius r centred (r, r), arms of length `arm` along both edges.
fn bracket_dist(x: f64, y: f64, r: f64, arm: f64) -> f64 {
    match (x < r, y < r) {
        (true, true) => ((x - r).hypot(y - r) - r).abs(),
        (false, true) if x <= r + arm => y.abs(),
        (false, true) => (x - r - arm).hypot(y), // round cap
        (true, false) if y <= r + arm => x.abs(),
        (true, false) => x.hypot(y - r - arm),
        (false, false) => f64::MAX,
    }
}

/// Stadium (pill) shape coverage.
fn pill(w: usize, h: usize) -> Vec<u8> {
    let r = h as f64 / 2.0;
    raster(w, h, |x, y| r - (x.clamp(r, w as f64 - r) - x).hypot(y - r) + 0.5)
}

fn load_font() -> Option<fontdue::Font> {
    const CANDIDATES: [&str; 4] = [
        "/usr/share/fonts/truetype/noto/NotoSans-Medium.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
    ];
    let data = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()).or_else(|| {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans:medium"]).output().ok()?;
        std::fs::read(String::from_utf8(out.stdout).ok()?).ok()
    })?;
    fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).ok()
}

/// Rasterise one line of text into an A8 mask on the server.
fn label(x: &X, font: &fontdue::Font, text: &str, px: f32) -> Res<Mask> {
    let lm = font.horizontal_line_metrics(px).ok_or("font has no horizontal metrics")?;
    let h = (lm.ascent - lm.descent).ceil() as usize;
    let mut pen = 0.0;
    let mut prev = None;
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        if let Some(p) = prev {
            pen += font.horizontal_kern(p, ch, px).unwrap_or(0.0);
        }
        let (m, bitmap) = font.rasterize(ch, px);
        glyphs.push((pen, m, bitmap));
        pen += m.advance_width;
        prev = Some(ch);
    }
    let w = pen.ceil() as usize + 2;
    let mut buf = vec![0u8; w * h];
    for (pen, m, bitmap) in glyphs {
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (lm.ascent - m.ymin as f32 - m.height as f32).round() as i32;
        for (i, &v) in bitmap.iter().enumerate() {
            let (px, py) = (gx + (i % m.width) as i32, gy + (i / m.width) as i32);
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                let d = &mut buf[py as usize * w + px as usize];
                *d = (*d).max(v);
            }
        }
    }
    Ok(Mask { pic: x.upload_mask(&buf, w, h, false)?, w: w as i32, h: h as i32 })
}

/// Colour of the rotating sweep at screen angle `deg` (degrees) around the box centre.
fn sweep(deg: f64, alpha: f64) -> Color {
    let t = (deg / 360.0).rem_euclid(1.0) * GOOGLE.len() as f64;
    let (i, f) = (t.floor() as usize % GOOGLE.len(), t.fract());
    let (a, b) = (GOOGLE[i], GOOGLE[(i + 1) % GOOGLE.len()]);
    straight(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f, alpha)
}

impl Scene {
    pub fn new(x: &X) -> Res<Scene> {
        let s = x.scale;
        let r = (14.0 * s).round() as i32;
        let g = (24.0 * s).ceil() as i32;
        let (rf, gf) = (r as f64, g as f64);

        let up = |data: &[u8], w: i32, h: i32, repeat: bool| x.upload_mask(data, w as usize, h as usize, repeat);
        let four = |base: &[u8], w: i32, h: i32| -> Res<[Picture; 4]> {
            let m = |c| up(&mirror(base, w as usize, h as usize, c), w, h, false);
            Ok([m(CORNERS[0])?, m(CORNERS[1])?, m(CORNERS[2])?, m(CORNERS[3])?])
        };

        // glow across the top edge (row 0 = farthest out), mirrored for the other edges
        let top = raster(1, g as usize, |_, py| glow_alpha(gf - py, s));
        let left = raster(g as usize, 1, |px, _| glow_alpha(gf - px, s));
        let glow_edges = [
            up(&top, 1, g, true)?,
            up(&mirror(&top, 1, g as usize, (false, true)), 1, g, true)?,
            up(&left, g, 1, true)?,
            up(&mirror(&left, g as usize, 1, (true, false)), g, 1, true)?,
        ];
        // glow around the top-left arc, centred on the mask's bottom-right corner
        let c = r + g;
        let corner = raster(c as usize, c as usize, |px, py| glow_alpha((c as f64 - px).hypot(c as f64 - py) - rf, s));
        let glow_corners = four(&corner, c, c)?;
        // inside of the rounded top-left corner, to un-dim it
        let fill = raster(r as usize, r as usize, |px, py| rf - (rf - px).hypot(rf - py) + 0.5);
        let fill_corners = four(&fill, r, r)?;
        // white bracket straddling the edge
        let (half, arm) = (2.5 * s, (22.0 * s).round());
        let pad = half.ceil() as i32 + 1;
        let bsize = pad + r + arm as i32 + pad;
        let bracket = raster(bsize as usize, bsize as usize, |px, py| {
            half - bracket_dist(px - pad as f64, py - pad as f64, rf, arm) + 0.5
        });
        let brackets = four(&bracket, bsize, bsize)?;

        let (btn_w, btn_h) = ((132.0 * s) as i32, (40.0 * s) as i32);
        let btn_pill = Mask { pic: up(&pill(btn_w as usize, btn_h as usize), btn_w, btn_h, false)?, w: btn_w, h: btn_h };
        let font = load_font(); // no font: the overlay still works, just without text or buttons
        let mut buttons = Vec::new();
        let (mut hint, mut hint_pill) = (None, None);
        if let Some(f) = &font {
            for b in BUTTONS {
                buttons.push(label(x, f, b, (15.0 * s) as f32)?);
            }
            let h = label(x, f, HINT, (15.0 * s) as f32)?;
            let (pw, ph) = (h.w + (36.0 * s) as i32, h.h + (18.0 * s) as i32);
            hint_pill = Some(Mask { pic: up(&pill(pw as usize, ph as usize), pw, ph, false)?, w: pw, h: ph });
            hint = Some(h);
        }
        Ok(Scene {
            white: x.solid(rgba(1.0, 1.0, 1.0, 1.0))?,
            dark: x.solid(rgba(0.11, 0.11, 0.13, 0.95))?,
            buttons,
            hint,
            btn_pill,
            hint_pill,
            r,
            g,
            bracket_pad: pad,
            bracket_size: bsize,
            glow_edges,
            glow_corners,
            fill_corners,
            brackets,
            btn_w,
            btn_h,
            gap: (24.0 * s) as i32, // clears the glow halo
        })
    }

    /// Empty when no font loaded: nothing is drawn, so nothing should be clickable.
    pub fn button_rects(&self, x: &X, sel: Rect) -> Vec<Rect> {
        geom::button_rects(sel, x.w, x.h, self.buttons.len() as i32, self.btn_w, self.btn_h, self.gap)
    }

    fn pill(&self, x: &X, shape: &Mask, (px, py): (i32, i32), text: &Mask) -> Res<()> {
        x.composite(self.dark, Some(shape.pic), (0, 0), (px, py, shape.w, shape.h))?;
        let (tx, ty) = (px + (shape.w - text.w) / 2, py + (shape.h - text.h) / 2);
        x.composite(self.white, Some(text.pic), (0, 0), (tx, ty, text.w, text.h))
    }

    /// Sweep glow: each edge and corner gets a linear gradient sampled from the rotating sweep.
    fn glow(&self, x: &X, (x0, y0, w, h): Rect, angle: f64, alpha: f64) -> Res<()> {
        let (r, g) = (self.r, self.g);
        let (cx, cy) = (x0 as f64 + w as f64 / 2.0, y0 as f64 + h as f64 / 2.0);
        let at = |p: (f64, f64)| sweep((p.1 - cy).atan2(p.0 - cx).to_degrees() + angle, alpha);
        let paint = |p1: (f64, f64), p2: (f64, f64), n: usize, mask: Picture, rect: Rect| -> Res<()> {
            let stops: Vec<_> = (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    (t, at((p1.0 + (p2.0 - p1.0) * t, p1.1 + (p2.1 - p1.1) * t)))
                })
                .collect();
            let grad = x.linear(p1, p2, &stops)?;
            x.composite(grad, Some(mask), (0, 0), rect)?;
            x.free(grad)
        };
        let (l, t, rt, b) = (x0 as f64, y0 as f64, (x0 + w) as f64, (y0 + h) as f64);
        let rf = r as f64;
        paint((l + rf, t), (rt - rf, t), 8, self.glow_edges[0], (x0 + r, y0 - g, w - 2 * r, g))?;
        paint((l + rf, b), (rt - rf, b), 8, self.glow_edges[1], (x0 + r, y0 + h, w - 2 * r, g))?;
        paint((l, t + rf), (l, b - rf), 8, self.glow_edges[2], (x0 - g, y0 + r, g, h - 2 * r))?;
        paint((rt, t + rf), (rt, b - rf), 8, self.glow_edges[3], (x0 + w, y0 + r, g, h - 2 * r))?;
        // corners: gradient from where one edge ends to where the next begins
        let c = r + g;
        for (i, (fx, fy)) in CORNERS.into_iter().enumerate() {
            let (ex, ey) = (if fx { rt } else { l }, if fy { b } else { t }); // the box corner
            let (sx, sy) = (if fx { -rf } else { rf }, if fy { -rf } else { rf });
            let rect = (if fx { x0 + w - r } else { x0 - g }, if fy { y0 + h - r } else { y0 - g }, c, c);
            paint((ex, ey + sy), (ex + sx, ey), 2, self.glow_corners[i], rect)?;
        }
        Ok(())
    }

    /// Draw the glow once off-screen: the driver compiles its gradient shader on first use
    /// (~30 ms here), which would otherwise land on the first frame of a drag.
    pub fn warm_up(&self, x: &X) -> Res<()> {
        self.glow(x, (self.g, self.g, 4 * self.r, 4 * self.r), 0.0, 1.0)
    }

    pub fn draw(&self, x: &X, v: &View) -> Res<()> {
        x.copy_shot((0, 0, x.w, x.h))?;
        x.fill_rects(rgba(0.0, 0.0, 0.0, 0.5 * v.fade), &[(0, 0, x.w, x.h)])?;

        if let Some(sel) = v.sel {
            let (x0, y0, w, h) = sel;
            let r = self.r;
            if w >= 2 * r && h >= 2 * r {
                // un-dim: centre column and side strips, then the rounded corners through masks
                x.copy_shot((x0 + r, y0, w - 2 * r, h))?;
                x.copy_shot((x0, y0 + r, r, h - 2 * r))?;
                x.copy_shot((x0 + w - r, y0 + r, r, h - 2 * r))?;
                for (i, (fx, fy)) in CORNERS.into_iter().enumerate() {
                    let pos = (if fx { x0 + w - r } else { x0 }, if fy { y0 + h - r } else { y0 });
                    x.composite(x.shot, Some(self.fill_corners[i]), (0, 0), (pos.0, pos.1, r, r))?;
                }
                if v.glow > 0.0 {
                    self.glow(x, sel, v.angle, v.glow)?;
                }
                // brackets, clipped at the box's midlines so small boxes don't overlap them
                let (pad, size) = (self.bracket_pad, self.bracket_size);
                let (cw, ch) = (size.min(pad + w / 2), size.min(pad + h / 2));
                for (i, (fx, fy)) in CORNERS.into_iter().enumerate() {
                    let dx = if fx { x0 + w + pad - cw } else { x0 - pad };
                    let dy = if fy { y0 + h + pad - ch } else { y0 - pad };
                    let m = (if fx { size - cw } else { 0 }, if fy { size - ch } else { 0 });
                    x.composite(self.white, Some(self.brackets[i]), m, (dx, dy, cw, ch))?;
                }
            } else {
                x.copy_shot(sel)?; // too small to round yet (start of a drag)
            }

            if v.show_buttons {
                for (rect, l) in self.button_rects(x, sel).into_iter().zip(&self.buttons) {
                    self.pill(x, &self.btn_pill, (rect.0, rect.1), l)?;
                }
            }
        }

        if let (Some(h), Some(p)) = (&self.hint, &self.hint_pill) {
            self.pill(x, p, ((x.w - p.w) / 2, (20.0 * x.scale) as i32), h)?;
        }
        x.present()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_flips() {
        let src = [1, 2, 3, 4, 5, 6]; // 3x2
        assert_eq!(mirror(&src, 3, 2, (true, false)), [3, 2, 1, 6, 5, 4]);
        assert_eq!(mirror(&src, 3, 2, (false, true)), [4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn glow_profile() {
        assert_eq!(glow_alpha(-2.0, 2.0), 0.0); // inside the box
        assert!((glow_alpha(2.0, 2.0) - 1.0).abs() < 1e-9); // crisp ring
        let (near, far) = (glow_alpha(10.0, 2.0), glow_alpha(40.0, 2.0));
        assert!(near < 0.4 && far < near && far > 0.0); // halo fades out
    }

    #[test]
    fn bracket_path() {
        assert!(bracket_dist(0.0, 20.0, 14.0, 22.0) < 1e-9); // on the left arm
        assert!(bracket_dist(20.0, 0.0, 14.0, 22.0) < 1e-9); // on the top arm
        let d = 14.0 - 14.0 / 2f64.sqrt(); // arc midpoint
        assert!(bracket_dist(d, d, 14.0, 22.0) < 1e-9);
        assert!((bracket_dist(40.0, 0.0, 14.0, 22.0) - 4.0).abs() < 1e-9); // past the arm end
    }

    #[test]
    fn sweep_hits_google_colours() {
        let rgb = |c: Color| (c.red, c.green, c.blue, c.alpha); // Color has no PartialEq
        let google = |i: usize| rgb(straight(GOOGLE[i].0, GOOGLE[i].1, GOOGLE[i].2, 1.0));
        assert_eq!(rgb(sweep(0.0, 1.0)), google(0));
        assert_eq!(rgb(sweep(90.0, 1.0)), google(1));
        assert_eq!(rgb(sweep(-90.0, 1.0)), rgb(sweep(270.0, 1.0)));
    }
}
