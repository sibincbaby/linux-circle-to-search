#!/usr/bin/env python3
"""Circle to Search for Linux (X11): draw around anything on screen, then search it
with Google Lens (Enter) or copy its text via Tesseract OCR (C).

Esc / right-click cancels. Bind this script to a keyboard shortcut.
"""
import http.client
import subprocess
import sys
import time
import uuid

import gi

gi.require_version("Gdk", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402

PAD = 8  # logical px added around the drawn shape
MIN_SIZE = 12  # smaller selections are treated as an accidental click
BTN_W, BTN_H, BTN_GAP = 150, 40, 12
ACTIONS = (("search", "Search  (Enter)"), ("text", "Copy text  (C)"))


def bbox(points, width, height, pad=PAD):
    """Bounding box (x, y, w, h) of the stroke, padded and clamped to the screen."""
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    x0, y0 = max(0, int(min(xs)) - pad), max(0, int(min(ys)) - pad)
    x1, y1 = min(width, int(max(xs)) + pad), min(height, int(max(ys)) + pad)
    return x0, y0, x1 - x0, y1 - y0


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


def lens_url(png):
    """Upload PNG bytes to Google Lens and return the results URL from its redirect."""
    boundary = uuid.uuid4().hex
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="encoded_image"; filename="s.png"\r\n'
        "Content-Type: image/png\r\n\r\n"
    ).encode() + png + f"\r\n--{boundary}--\r\n".encode()
    # shortcut: undocumented Lens endpoint, may change; fall back to a browser form post if it breaks.
    conn = http.client.HTTPSConnection("lens.google.com", timeout=30)
    conn.request(
        "POST",
        f"/v3/upload?ep=ccm&s=&st={int(time.time() * 1000)}",
        body,
        {
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/130.0 Safari/537.36",
        },
    )
    resp = conn.getresponse()
    location = resp.getheader("Location")
    if resp.status not in (302, 303) or not location:
        raise RuntimeError(f"Google Lens returned HTTP {resp.status}")
    return location


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
        self.points = []
        self.sel = None  # (x, y, w, h) once a shape is drawn
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
        # POPUP windows get no focus from the WM; grab pointer + keyboard ourselves.
        seat = Gdk.Display.get_default().get_default_seat()
        cursor = Gdk.Cursor.new_from_name(self.get_display(), "crosshair")
        seat.grab(self.get_window(), Gdk.SeatCapabilities.ALL, True, cursor, None, None, None)

    def on_draw(self, _w, cr):
        cr.set_source_surface(self.surface, 0, 0)
        cr.paint()
        cr.set_source_rgba(0, 0, 0, 0.35)
        cr.rectangle(0, 0, self.w, self.h)
        if self.sel:
            cr.rectangle(*self.sel)  # even-odd fill leaves the selection undimmed
            cr.set_fill_rule(1)  # cairo.FILL_RULE_EVEN_ODD
        cr.fill()
        if len(self.points) > 1:
            cr.set_line_cap(1)  # round
            cr.set_line_join(1)
            for width, rgba in ((14, (0.4, 0.6, 1, 0.35)), (5, (1, 1, 1, 0.95))):
                cr.set_source_rgba(*rgba)
                cr.set_line_width(width)
                cr.move_to(*self.points[0])
                for p in self.points[1:]:
                    cr.line_to(*p)
                cr.stroke()
        cr.select_font_face("Sans")
        cr.set_font_size(18)
        msg = "Circle anything to search or copy its text  ·  Esc to cancel"
        ext = cr.text_extents(msg)
        cr.set_source_rgba(1, 1, 1, 0.9)
        cr.move_to((self.w - ext.width) / 2, 48)
        cr.show_text(msg)
        if self.sel:
            cr.set_font_size(15)
            for (bx, by, bw, bh), (_, label) in zip(button_rects(self.sel, self.w, self.h), ACTIONS):
                r = bh / 2  # pill shape
                cr.new_sub_path()
                cr.arc(bx + r, by + r, r, 1.5708, 4.7124)
                cr.arc(bx + bw - r, by + r, r, 4.7124, 1.5708)
                cr.close_path()
                cr.set_source_rgba(0.12, 0.12, 0.14, 0.92)
                cr.fill()
                ext = cr.text_extents(label)
                cr.set_source_rgba(1, 1, 1, 1)
                cr.move_to(bx + (bw - ext.width) / 2, by + (bh + ext.height) / 2)
                cr.show_text(label)

    def on_press(self, _w, ev):
        if ev.button == 3:
            self.close_overlay()
        elif ev.button == 1:
            if self.sel:
                for (bx, by, bw, bh), (action, _) in zip(button_rects(self.sel, self.w, self.h), ACTIONS):
                    if bx <= ev.x <= bx + bw and by <= ev.y <= by + bh:
                        return self.finish(action)
            self.sel = None  # clicking elsewhere starts a new shape
            self.points = [(ev.x, ev.y)]
            self.queue_draw()

    def on_motion(self, _w, ev):
        if self.points and ev.state & Gdk.ModifierType.BUTTON1_MASK:
            self.points.append((ev.x, ev.y))
            self.queue_draw()

    def on_release(self, _w, ev):
        if ev.button != 1 or not self.points:
            return
        sel = bbox(self.points, self.w, self.h)
        if sel[2] < MIN_SIZE or sel[3] < MIN_SIZE:
            self.points = []
        else:
            self.sel = sel
        self.queue_draw()

    def on_key(self, _w, ev):
        if ev.keyval == Gdk.KEY_Escape:
            self.close_overlay()
        elif self.sel and ev.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter):
            self.finish("search")
        elif self.sel and ev.keyval in (Gdk.KEY_c, Gdk.KEY_C):
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
    try:
        url = lens_url(png)
    except Exception as e:  # network / endpoint failure: tell the user, don't crash silently
        notify(f"Lens upload failed: {e}", "dialog-error")
        return 1
    subprocess.Popen(["xdg-open", url])
    return 0


if __name__ == "__main__":
    sys.exit(main())
