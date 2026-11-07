# circle-search

Hotkey-launched X11 overlay: freeze screen → drag a Lens-style box → Google Lens search or OCR-to-clipboard.

## Layout
- `src/main.rs` event loop + selection state; `src/x.rs` X11/XRender plumbing; `src/draw.rs` scene; `src/geom.rs` box maths (unit-tested); `src/actions.rs` Lens page, tesseract, xclip, notify-send.

## Decisions (measured on the dev laptop: Intel Iris Xe, 3840x2160, Xft.dpi 192)
- **XRender, not OpenGL.** EGL/Mesa alone costs ~64 MB RSS and ~50 ms init; XRender keeps the frozen screen server-side: ~12 MB RSS, first frame <1 ms, ~300 full-4K frames/s. Python+GTK was 140 MB / ~150 ms.
- **No winit.** Override-redirect window + our own keyboard/pointer grab (with retry; the WM may still hold the keyboard right after the hotkey).
- **Lens must be posted by the browser.** A server-side POST returns a results URL bound to that client's session (opens empty in Chrome). The page uses `<meta name=referrer content=no-referrer>` so it sends `Origin: null`; a cross-site Origin gets 403.
- XRender colours: solid fills / FillRectangles are premultiplied (`x::rgba`), gradient stops are not (`x::straight`).
- Tri-fan/strip source alignment: pass the first point (truncated) as src_x/src_y for identity mapping.
- The glow spins only while the box changes, then eases to a stop so an idle overlay does no work.

## Gotchas
- Cinnamon only reloads custom shortcuts when `org.cinnamon.desktop.keybindings custom-list` changes; editing `customN/binding` alone does nothing until then.
- Testing: drive it with `xdotool` (mousemove/mousedown/mouseup, key c/Escape); `import -window root` captures the overlay.
