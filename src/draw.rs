//! The overlay scene: dimmed frozen screen, rounded selection with a Google-coloured sweep
//! glow, Lens-style corner brackets, action buttons and a hint pill.

use x11rb::protocol::render::Picture;

use crate::geom::{self, Rect};
use crate::x::{rgba, straight, Res, X};

const SEG: usize = 8; // arc segments per rounded corner
pub const BUTTONS: [&str; 2] = ["Search", "Copy text"];
const HINT: &str = "Drag to select  ·  Enter to search  ·  C to copy text  ·  Esc to cancel";
// Google blue, red, yellow, green
const GOOGLE: [(f64, f64, f64); 4] = [(0.259, 0.522, 0.957), (0.918, 0.263, 0.208), (0.984, 0.737, 0.020), (0.204, 0.659, 0.325)];

pub struct Label {
    mask: Picture,
    w: usize,
    h: usize,
}

pub struct Scene {
    white: Picture,
    dark: Picture,
    buttons: Vec<Label>,
    hint: Option<Label>,
    pub btn_w: i32,
    pub btn_h: i32,
    pub gap: i32,
}

/// Per-frame state the scene is drawn from.
pub struct View {
    pub sel: Option<Rect>,
    /// 0..1 fade-in of the dim layer.
    pub fade: f64,
    /// Sweep-gradient rotation in degrees.
    pub angle: f64,
    pub show_buttons: bool,
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
fn label(x: &X, font: &fontdue::Font, text: &str, px: f32) -> Res<Label> {
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
    Ok(Label { mask: x.upload_mask(&buf, w, h)?, w, h })
}

impl Scene {
    pub fn new(x: &X) -> Res<Scene> {
        let s = x.scale;
        let font = load_font(); // no font: the overlay still works, just without text
        let mut buttons = Vec::new();
        let mut hint = None;
        if let Some(f) = &font {
            for b in BUTTONS {
                buttons.push(label(x, f, b, (15.0 * s) as f32)?);
            }
            hint = Some(label(x, f, HINT, (15.0 * s) as f32)?);
        }
        Ok(Scene {
            white: x.solid(rgba(1.0, 1.0, 1.0, 1.0))?,
            dark: x.solid(rgba(0.11, 0.11, 0.13, 0.88))?,
            buttons,
            hint,
            btn_w: (132.0 * s) as i32,
            btn_h: (40.0 * s) as i32,
            gap: (24.0 * s) as i32, // clears the glow halo
        })
    }

    /// Empty when no font loaded: nothing is drawn, so nothing should be clickable.
    pub fn button_rects(&self, x: &X, sel: Rect) -> Vec<Rect> {
        geom::button_rects(sel, x.w, x.h, self.buttons.len() as i32, self.btn_w, self.btn_h, self.gap)
    }

    fn pill(&self, x: &X, r: Rect, text: Option<&Label>) -> Res<()> {
        x.fan(self.dark, (r.0 + r.2 / 2, r.1 + r.3 / 2), &geom::rounded_rect(r, r.3 as f64 / 2.0, 0.0, SEG))?;
        if let Some(l) = text {
            x.masked(self.white, l.mask, r.0 + (r.2 - l.w as i32) / 2, r.1 + (r.3 - l.h as i32) / 2, l.w, l.h)?;
        }
        Ok(())
    }

    pub fn draw(&self, x: &X, v: &View) -> Res<()> {
        let s = x.scale;
        x.paint_shot()?;
        x.fill_rects(rgba(0.0, 0.0, 0.0, 0.5 * v.fade), &[(0, 0, x.w, x.h)])?;

        if let Some(sel) = v.sel {
            let r = 14.0 * s;
            let center = (sel.0 + sel.2 / 2, sel.1 + sel.3 / 2);
            // un-dim the selection by repainting the frozen screen inside its rounded outline
            x.fan(x.shot, center, &geom::rounded_rect(sel, r, 0.0, SEG))?;

            // glow: crisp sweep ring plus two soft halos, Google colours rotating around the box
            for (d, width, a) in [(0.0, 3.0, 1.0), (3.0, 6.0, 0.35), (9.0, 10.0, 0.12)] {
                let stops: Vec<_> = (0..=GOOGLE.len())
                    .map(|i| {
                        let (cr, cg, cb) = GOOGLE[i % GOOGLE.len()];
                        (i as f64 / GOOGLE.len() as f64, straight(cr, cg, cb, a))
                    })
                    .collect();
                let grad = x.sweep(center.0 as f64, center.1 as f64, v.angle, &stops)?;
                let outer = geom::rounded_rect(sel, r, (d + width) * s, SEG);
                let inner = geom::rounded_rect(sel, r, d * s, SEG);
                x.strip(grad, &geom::strip(&outer, &inner, true))?;
                x.free(grad)?;
            }

            // white Lens-style corner brackets straddling the edge
            let t = 2.5 * s;
            let arm = (22.0 * s).min(sel.2 as f64 / 2.0 - r).min(sel.3 as f64 / 2.0 - r).max(0.0);
            for c in 0..4 {
                let outer = geom::bracket(sel, r, t, c, arm, SEG);
                let inner = geom::bracket(sel, r, -t, c, arm, SEG);
                x.strip(self.white, &geom::strip(&outer, &inner, false))?;
            }

            if v.show_buttons {
                for (rect, l) in self.button_rects(x, sel).into_iter().zip(&self.buttons) {
                    self.pill(x, rect, Some(l))?;
                }
            }
        }

        if let Some(h) = &self.hint {
            let (pw, ph) = (h.w as i32 + (36.0 * s) as i32, h.h as i32 + (18.0 * s) as i32);
            self.pill(x, ((x.w - pw) / 2, (20.0 * s) as i32, pw, ph), Some(h))?;
        }
        x.present()
    }
}
