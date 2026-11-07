//! Selection-box maths, all in device pixels.

/// (x, y, w, h)
pub type Rect = (i32, i32, i32, i32);

/// Rectangle spanning two corners in any order, clamped to the screen.
pub fn norm(x0: i32, y0: i32, x1: i32, y1: i32, width: i32, height: i32) -> Rect {
    let (ax, bx) = (x0.clamp(0, width), x1.clamp(0, width));
    let (ay, by) = (y0.clamp(0, height), y1.clamp(0, height));
    (ax.min(bx), ay.min(by), (ax - bx).abs(), (ay - by).abs())
}

/// Part of the selection under the pointer: a compass edge/corner ("n", "se", ...), "move", or None.
pub fn hit(sel: Rect, px: i32, py: i32, handle: i32) -> Option<&'static str> {
    let (x, y, w, h) = sel;
    if px < x - handle || px > x + w + handle || py < y - handle || py > y + h + handle {
        return None;
    }
    let v = if (py - y).abs() <= handle { "n" } else if (py - (y + h)).abs() <= handle { "s" } else { "" };
    let hz = if (px - x).abs() <= handle { "w" } else if (px - (x + w)).abs() <= handle { "e" } else { "" };
    Some(match (v, hz) {
        ("", "") => "move",
        ("n", "w") => "nw",
        ("n", "e") => "ne",
        ("s", "w") => "sw",
        ("s", "e") => "se",
        (v, "") => v,
        (_, hz) => hz,
    })
}

/// Selection after dragging `mode` (from `hit`) by (dx, dy).
pub fn adjust(mode: &str, sel: Rect, dx: i32, dy: i32, width: i32, height: i32) -> Rect {
    let (x, y, w, h) = sel;
    if mode == "move" {
        return ((x + dx).clamp(0, width - w), (y + dy).clamp(0, height - h), w, h);
    }
    let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
    if mode.contains('w') {
        x0 += dx;
    }
    if mode.contains('e') {
        x1 += dx;
    }
    if mode.contains('n') {
        y0 += dy;
    }
    if mode.contains('s') {
        y1 += dy;
    }
    norm(x0, y0, x1, y1, width, height)
}

/// `n` buttons of size (bw, bh) centred under the selection, or above / inside it when there's no room.
pub fn button_rects(sel: Rect, width: i32, height: i32, n: i32, bw: i32, bh: i32, gap: i32) -> Vec<Rect> {
    let (x, y, w, h) = sel;
    let total = n * bw + (n - 1) * gap;
    let bx = (x + (w - total) / 2).max(0).min(width - total);
    let by = if y + h + gap + bh <= height {
        y + h + gap
    } else if y - gap - bh >= 0 {
        y - gap - bh
    } else {
        y + h - gap - bh
    };
    (0..n).map(|i| (bx + i * (bw + gap), by, bw, bh)).collect()
}

pub fn contains(r: Rect, px: i32, py: i32) -> bool {
    px >= r.0 && px <= r.0 + r.2 && py >= r.1 && py <= r.1 + r.3
}

/// Outline of a rounded rectangle grown by `d` on every side, clockwise from the top-left arc.
/// Every call with the same `seg` returns the same number of points, so outlines at two
/// offsets pair up point-for-point into a ring.
pub fn rounded_rect(sel: Rect, r: f64, d: f64, seg: usize) -> Vec<(f64, f64)> {
    (0..4).flat_map(|c| corner_arc(sel, r, d, c, seg)).collect()
}

/// Arc points of corner `c` (0 = top-left, clockwise) of `rounded_rect`.
pub fn corner_arc(sel: Rect, r: f64, d: f64, c: usize, seg: usize) -> Vec<(f64, f64)> {
    let (x, y, w, h) = (sel.0 as f64, sel.1 as f64, sel.2 as f64, sel.3 as f64);
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let (cx, cy) = [(x + r, y + r), (x + w - r, y + r), (x + w - r, y + h - r), (x + r, y + h - r)][c];
    let start = std::f64::consts::PI * (1.0 + c as f64 * 0.5);
    (0..=seg)
        .map(|i| {
            let a = start + std::f64::consts::FRAC_PI_2 * i as f64 / seg as f64;
            (cx + (r + d) * a.cos(), cy + (r + d) * a.sin())
        })
        .collect()
}

/// Lens-style corner bracket: corner `c`'s arc plus straight arms of length `arm` along both edges.
pub fn bracket(sel: Rect, r: f64, d: f64, c: usize, arm: f64, seg: usize) -> Vec<(f64, f64)> {
    let mut pts = corner_arc(sel, r, d, c, seg);
    let start = std::f64::consts::PI * (1.0 + c as f64 * 0.5);
    let end = start + std::f64::consts::FRAC_PI_2;
    // clockwise tangent at angle a is (-sin a, cos a)
    let (f, l) = (pts[0], pts[seg]);
    pts.insert(0, (f.0 + arm * start.sin(), f.1 - arm * start.cos()));
    pts.push((l.0 - arm * end.sin(), l.1 + arm * end.cos()));
    pts
}

/// Interleave two matching outlines into a triangle strip covering the band between them.
pub fn strip(outer: &[(f64, f64)], inner: &[(f64, f64)], closed: bool) -> Vec<(f64, f64)> {
    let mut out: Vec<_> = outer.iter().zip(inner).flat_map(|(a, b)| [*a, *b]).collect();
    if closed {
        out.extend([outer[0], inner[0]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const W: i32 = 1920;
    const H: i32 = 1080;

    #[test]
    fn norm_any_direction_and_clamped() {
        assert_eq!(norm(300, 200, 100, 50, W, H), (100, 50, 200, 150));
        assert_eq!(norm(-20, -5, 2000, 1100, W, H), (0, 0, W, H));
    }

    #[test]
    fn hit_regions() {
        let sel = (100, 100, 200, 100);
        assert_eq!(hit(sel, 100, 100, 14), Some("nw"));
        assert_eq!(hit(sel, 300, 200, 14), Some("se"));
        assert_eq!(hit(sel, 200, 101, 14), Some("n"));
        assert_eq!(hit(sel, 299, 150, 14), Some("e"));
        assert_eq!(hit(sel, 200, 150, 14), Some("move"));
        assert_eq!(hit(sel, 500, 500, 14), None);
    }

    #[test]
    fn adjust_resize_flip_move() {
        let sel = (100, 100, 200, 100);
        assert_eq!(adjust("se", sel, 50, 20, W, H), (100, 100, 250, 120));
        assert_eq!(adjust("w", sel, 250, 0, W, H), (300, 100, 50, 100));
        assert_eq!(adjust("move", sel, 5000, -5000, W, H), (W - 200, 0, 200, 100));
    }

    #[test]
    fn buttons_below_above_inside() {
        let b = |s| button_rects(s, W, H, 2, 150, 40, 12);
        assert_eq!(b((100, 100, 300, 200))[0].1, 312);
        assert_eq!(b((100, 900, 300, 170))[0].1, 900 - 12 - 40);
        let full = b((0, 0, W, H));
        assert_eq!(full[0].1, H - 12 - 40);
        assert!(full[1].0 + full[1].2 <= W);
        let right = b((1800, 100, 100, 100));
        assert!(right[1].0 + right[1].2 <= W);
    }

    #[test]
    fn outlines_pair_up() {
        let sel = (10, 10, 100, 50);
        assert_eq!(rounded_rect(sel, 12.0, 0.0, 6).len(), rounded_rect(sel, 12.0, 8.0, 6).len());
        // top-left arc starts on the left edge and ends on the top edge
        let arc = corner_arc(sel, 12.0, 0.0, 0, 6);
        assert!((arc[0].0 - 10.0).abs() < 1e-9 && (arc[6].1 - 10.0).abs() < 1e-9);
        // bracket arms run down the left edge and along the top edge
        let b = bracket(sel, 12.0, 0.0, 0, 20.0, 6);
        assert!((b[0].0 - 10.0).abs() < 1e-9 && (b[0].1 - 42.0).abs() < 1e-9);
        assert!((b[8].0 - 42.0).abs() < 1e-9 && (b[8].1 - 10.0).abs() < 1e-9);
    }
}
