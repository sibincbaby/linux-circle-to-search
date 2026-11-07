# circle-search

Hotkey-launched X11 overlay: freeze screen → drag a Lens-style box → Google Lens search or OCR-to-clipboard.

## Layout
- `src/main.rs` event loop + selection state; `src/x.rs` X11/XRender plumbing; `src/draw.rs` scene; `src/geom.rs` box maths (unit-tested); `src/actions.rs` Lens page, tesseract, xclip, notify-send.

## Decisions (measured on the dev laptop: Intel Iris Xe, 3840x2160, Xft.dpi 192)
- **XRender, not OpenGL.** EGL/Mesa alone costs ~64 MB RSS and ~50 ms init; XRender keeps the frozen screen server-side: ~12 MB RSS, first frame <1 ms, ~300 full-4K frames/s. Python+GTK was 140 MB / ~150 ms.
- **No winit.** Override-redirect window + our own keyboard/pointer grab (with retry; the WM may still hold the keyboard right after the hotkey).
- **Lens must be posted by the browser.** A server-side POST returns a results URL bound to that client's session (opens empty in Chrome). The page uses `<meta name=referrer content=no-referrer>` so it sends `Origin: null`; a cross-site Origin gets 403.
- **Draw only with Composite + small A8 masks.** XRender anti-aliased triangles/trapezoids fall back to the CPU in glamor (10-60 ms each at 4K → ~117 ms/frame, visibly laggy). Shapes are masks rasterised once at startup (corner arcs, brackets, pills, 1-px tiled glow profiles); frames now take ~1 ms (p95 ~2.5 ms, max ~5 ms).
- **Linear gradients, not conical:** conical runs on the CPU (~2.7 ms per 3000x40 strip vs 0.4 ms linear). The rotating sweep is sampled into one linear gradient per edge/corner. Gradients need `Repeat::PAD`.
- The first gradient composite compiles a shader (~30 ms); `Scene::warm_up` does it off-screen right after the first frame.
- XRender colours: solid fills / FillRectangles are premultiplied (`x::rgba`), gradient stops are not (`x::straight`).
- Like Lens in Chrome, the coloured glow is hidden while dragging (white brackets only); on release it fades in (350 ms), spins, and eases to a stop after 2.5 s so an idle overlay does no work.
- Cursor theme/size come from XSETTINGS (`Gtk/CursorThemeName`/`Size`), as GTK does; x11rb alone falls back to the `default` theme (Bibata here vs the desktop's DMZ-White).

## Gotchas
- Cinnamon only reloads custom shortcuts when `org.cinnamon.desktop.keybindings custom-list` changes; editing `customN/binding` alone does nothing until then.
- Testing: drive it with `xdotool` (mousemove/mousedown/mouseup, key c/Escape); `import -window root` captures the overlay but stalls the X server (~200 ms), so never screenshot while timing frames.
