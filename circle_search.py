#!/usr/bin/env python3
"""Circle to Search for Linux (X11): drag a box around anything on screen (Google Lens style),
adjust it by its corners/edges, then search it with Google Lens (Enter) or copy its text
via Tesseract OCR (C).

Esc / right-click cancels. Bind this script to a keyboard shortcut.
"""
import base64
import os
import subprocess
import sys
import tempfile

import gi

gi.require_version("Gdk", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402

MIN_SIZE = 12  # smaller selections are treated as an accidental click
HANDLE = 14  # logical px around corners/edges that grab for resizing
BTN_W, BTN_H, BTN_GAP = 150, 40, 12
ACTIONS = (("search", "Search  (Enter)"), ("text", "Copy text  (C)"))
CURSORS = {
    "n": "n-resize", "s": "s-resize", "e": "e-resize", "w": "w-resize",
    "ne": "ne-resize", "nw": "nw-resize", "se": "se-resize", "sw": "sw-resize",
    "move": "move", "button": "pointer", None: "crosshair",
}
# Opened in the browser, this page posts the image to Lens itself so the results session
# belongs to the browser. no-referrer makes it send Origin: null, which Lens accepts.
# shortcut: undocumented Lens endpoint, may change; if it breaks, check what lens.google.com's own upload posts.
LENS_PAGE = """<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>
<title>Searching with Google Lens…</title>
<form id=f method=post enctype=multipart/form-data><input type=file name=encoded_image id=i></form>
<script>
const b = atob("%s"), a = new Uint8Array(b.length);
for (let k = 0; k < b.length; k++) a[k] = b.charCodeAt(k);
const dt = new DataTransfer();
dt.items.add(new File([a], "s.png", {type: "image/png"}));
i.files = dt.files;
f.action = "https://lens.google.com/v3/upload?ep=ccm&s=&st=" + Date.now();
f.submit();
</script>"""


def norm(x0, y0, x1, y1, width, height):
    """Rectangle (x, y, w, h) spanning two corners in any order, clamped to the screen."""
    x0, x1 = sorted((min(max(x0, 0), width), min(max(x1, 0), width)))
    y0, y1 = sorted((min(max(y0, 0), height), min(max(y1, 0), height)))
    return int(x0), int(y0), int(x1 - x0), int(y1 - y0)


def hit(sel, px, py):
    """Which part of the selection is under the pointer: 'n'/'se'/... edge or corner, 'move', or None."""
    x, y, w, h = sel
    if not (x - HANDLE <= px <= x + w + HANDLE and y - HANDLE <= py <= y + h + HANDLE):
        return None
    v = "n" if abs(py - y) <= HANDLE else "s" if abs(py - (y + h)) <= HANDLE else ""
    hz = "w" if abs(px - x) <= HANDLE else "e" if abs(px - (x + w)) <= HANDLE else ""
    return v + hz or "move"


def adjust(mode, sel, dx, dy, width, height):
    """Selection after dragging `mode` (from hit()) by (dx, dy)."""
    x, y, w, h = sel
    if mode == "move":
        return min(max(x + dx, 0), width - w), min(max(y + dy, 0), height - h), w, h
    x0, y0, x1, y1 = x, y, x + w, y + h
    if "w" in mode:
        x0 += dx
    if "e" in mode:
        x1 += dx
    if "n" in mode:
        y0 += dy
    if "s" in mode:
        y1 += dy
    return norm(x0, y0, x1, y1, width, height)


def button_rects(sel, width, height):
    """Action buttons centred under the selection, or above it / inside it when there's no room."""
    x, y, w, h = sel
    total = len(ACTIONS) * BTN_W + (len(ACTIONS) - 1) * BTN_GAP
    bx = min(max(0, x + (w - total) / 2), width - total)
    if y + h + BTN_GAP + BTN_H <= height:
        by = y + h + BTN_GAP
    elif y - BTN_GAP - BTN_H >= 0:
        by = y - BTN_GAP - BTN_H
    else:
        by = y + h - BTN_GAP - BTN_H
    return [(bx + i * (BTN_W + BTN_GAP), by, BTN_W, BTN_H) for i in range(len(ACTIONS))]


def ocr_text(png):
    """Run Tesseract on PNG bytes and return the recognised text."""
    out = subprocess.run(["tesseract", "stdin", "stdout"], input=png, capture_output=True, check=True)
    return out.stdout.decode().strip()


def lens_page(png):
    """Write the auto-uploading Lens page for PNG bytes to a private file and return its path."""
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        # Fixed name in the private per-user runtime dir (mode 0700): overwritten each run, nothing piles up.
        path = os.path.join(runtime, "circle-search-lens.html")
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    else:  # shared /tmp: unique O_EXCL file so nobody can pre-plant a symlink
        fd, path = tempfile.mkstemp(prefix="circle-search-", suffix=".html")
    with os.fdopen(fd, "w") as f:
        f.write(LENS_PAGE % base64.b64encode(png).decode())
    return path


def rounded_rect(cr, x, y, w, h, r):
    cr.new_sub_path()
    cr.arc(x + w - r, y + r, r, -1.5708, 0)
    cr.arc(x + w - r, y + h - r, r, 0, 1.5708)
    cr.arc(x + r, y + h - r, r, 1.5708, 3.1416)
    cr.arc(x + r, y + r, r, 3.1416, 4.7124)
    cr.close_path()


class Overlay(Gtk.Window):
    def __init__(self):
        super().__init__(type=Gtk.WindowType.POPUP)
        root = Gdk.get_default_root_window()
        geo = root.get_geometry()
        self.w, self.h = geo.width, geo.height
        self.scale = root.get_scale_factor()
        # Grab the screen before our window appears; pixbuf is in device pixels.
        self.shot = Gdk.pixbuf_get_from_window(root, 0, 0, self.w, self.h)
        self.surface = Gdk.cairo_surface_create_from_pixbuf(self.shot, self.scale, None)
        self.sel = None  # (x, y, w, h)
        self.drag = None  # (mode, start x, start y, selection at drag start)
        self.result = None  # (action, png bytes)

        self.move(0, 0)
        self.set_default_size(self.w, self.h)
        self.set_app_paintable(True)
        self.add_events(
            Gdk.EventMask.BUTTON_PRESS_MASK
            | Gdk.EventMask.BUTTON_RELEASE_MASK
            | Gdk.EventMask.POINTER_MOTION_MASK
            | Gdk.EventMask.KEY_PRESS_MASK
        )
        self.connect("draw", self.on_draw)
        self.connect("button-press-event", self.on_press)
        self.connect("motion-notify-event", self.on_motion)
        self.connect("button-release-event", self.on_release)
        self.connect("key-press-event", self.on_key)
        self.connect("map-event", self.on_map)

    def on_map(self, *_):
        self.set_cursor(None)
        # POPUP windows get no focus from the WM; grab pointer + keyboard ourselves.
        seat = Gdk.Display.get_default().get_default_seat()
        seat.grab(self.get_window(), Gdk.SeatCapabilities.ALL, True, None, None, None, None)

    def set_cursor(self, mode):
        self.get_window().set_cursor(Gdk.Cursor.new_from_name(self.get_display(), CURSORS[mode]))

    def on_draw(self, _w, cr):
        cr.set_source_surface(self.surface, 0, 0)
        cr.paint()
        cr.set_source_rgba(0, 0, 0, 0.4)
        cr.rectangle(0, 0, self.w, self.h)
        if self.sel:
            cr.rectangle(*self.sel)  # even-odd fill leaves the selection undimmed
            cr.set_fill_rule(1)  # cairo.FILL_RULE_EVEN_ODD
        cr.fill()
        cr.select_font_face("Sans")
        cr.set_font_size(18)
        msg = "Drag to select  ·  drag corners to adjust  ·  Esc to cancel"
        ext = cr.text_extents(msg)
        cr.set_source_rgba(1, 1, 1, 0.9)
        cr.move_to((self.w - ext.width) / 2, 48)
        cr.show_text(msg)
        if not self.sel:
            return
        x, y, w, h = self.sel
        cr.set_source_rgba(1, 1, 1, 0.6)
        cr.set_line_width(1)
        cr.rectangle(x + 0.5, y + 0.5, w - 1, h - 1)
        cr.stroke()
        # Lens-style corner brackets
        arm = min(28, w / 2, h / 2)
        cr.set_source_rgba(1, 1, 1, 1)
        cr.set_line_width(5)
        cr.set_line_cap(1)  # round
        cr.set_line_join(1)
        for cx, cy, sx, sy in ((x, y, 1, 1), (x + w, y, -1, 1), (x, y + h, 1, -1), (x + w, y + h, -1, -1)):
            cr.move_to(cx, cy + sy * arm)
            cr.line_to(cx, cy)
            cr.line_to(cx + sx * arm, cy)
            cr.stroke()
        if self.drag:
            return
        cr.set_font_size(15)
        for (bx, by, bw, bh), (_, label) in zip(button_rects(self.sel, self.w, self.h), ACTIONS):
            rounded_rect(cr, bx, by, bw, bh, bh / 2)
            cr.set_source_rgba(0.12, 0.12, 0.14, 0.92)
            cr.fill()
            ext = cr.text_extents(label)
            cr.set_source_rgba(1, 1, 1, 1)
            cr.move_to(bx + (bw - ext.width) / 2, by + (bh + ext.height) / 2)
            cr.show_text(label)

    def on_press(self, _w, ev):
        if ev.button == 3:
            return self.close_overlay()
        if ev.button != 1:
            return
        if self.sel:
            for (bx, by, bw, bh), (action, _) in zip(button_rects(self.sel, self.w, self.h), ACTIONS):
                if bx <= ev.x <= bx + bw and by <= ev.y <= by + bh:
                    return self.finish(action)
        mode = hit(self.sel, ev.x, ev.y) if self.sel else None
        if mode is None:  # outside the box: start a new one from this point
            mode, self.sel = "se", (int(ev.x), int(ev.y), 0, 0)
        self.drag = (mode, ev.x, ev.y, self.sel)
        self.queue_draw()

    def on_motion(self, _w, ev):
        if not self.drag:
            if not self.sel:
                return self.set_cursor(None)
            on_button = any(
                bx <= ev.x <= bx + bw and by <= ev.y <= by + bh
                for bx, by, bw, bh in button_rects(self.sel, self.w, self.h)
            )
            # outside the box: normal hand pointer (dragging there still starts a new box)
            self.set_cursor("button" if on_button else hit(self.sel, ev.x, ev.y) or "button")
            return
        mode, sx, sy, orig = self.drag
        self.sel = adjust(mode, orig, int(ev.x - sx), int(ev.y - sy), self.w, self.h)
        self.queue_draw()

    def on_release(self, _w, ev):
        if ev.button != 1 or not self.drag:
            return
        self.drag = None
        if self.sel[2] < MIN_SIZE or self.sel[3] < MIN_SIZE:
            self.sel = None
        self.queue_draw()

    def on_key(self, _w, ev):
        if ev.keyval == Gdk.KEY_Escape:
            self.close_overlay()
        elif self.sel and not self.drag and ev.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter):
            self.finish("search")
        elif self.sel and not self.drag and ev.keyval in (Gdk.KEY_c, Gdk.KEY_C):
            self.finish("text")

    def finish(self, action):
        x, y, w, h = self.sel
        s = self.scale
        crop = self.shot.new_subpixbuf(x * s, y * s, w * s, h * s)
        self.result = (action, crop.save_to_bufferv("png", [], [])[1])
        self.close_overlay()

    def close_overlay(self):
        Gdk.Display.get_default().get_default_seat().ungrab()
        self.hide()
        Gtk.main_quit()


def notify(msg, icon="edit-find"):
    subprocess.run(["notify-send", "-i", icon, "Circle to Search", msg])


def main():
    win = Overlay()
    win.show_all()
    Gtk.main()
    if win.result is None:
        return 0
    action, png = win.result
    if action == "text":
        try:
            text = ocr_text(png)
        except FileNotFoundError:
            notify("OCR needs Tesseract: sudo apt install tesseract-ocr", "dialog-error")
            return 1
        except subprocess.CalledProcessError as e:
            notify(f"OCR failed: {e.stderr.decode().strip()[:200]}", "dialog-error")
            return 1
        if not text:
            notify("No text found in the selection", "dialog-warning")
            return 0
        # xclip forks to keep serving the clipboard after we exit.
        subprocess.run(["xclip", "-selection", "clipboard"], input=text.encode())
        notify(f"Copied text:\n{text[:200]}", "edit-copy")
        return 0
    subprocess.Popen(["xdg-open", lens_page(png)])
    return 0


if __name__ == "__main__":
    sys.exit(main())
