# linux-circle-to-search

Android-style "Circle to Search" for Linux (X11). Press a shortcut, drag a box around anything on screen (Google Lens style: drag corners/edges to resize, drag inside to move), then:

- **Search** (Enter): opens the selection in Google Lens in your browser
- **Copy text** (C): reads the text with Tesseract OCR and copies it to the clipboard

Esc or right-click cancels. No API keys, no third-party image host.

## Why it's light

It's a ~750 KB Rust binary that runs only when you press the shortcut (nothing in the background). The frozen screen, dimming, glow animation and text are all drawn by the X server through XRender (GPU-accelerated on most drivers), so the process itself stays around **12 MB** and opens its overlay in **about a millisecond**, even at 4K.

## Requirements

- An X11 session (tested on Linux Mint Cinnamon, 4K at 2x scale). Wayland is not supported.
- `xdg-open`, `notify-send`, `xclip`, and for OCR: `sudo apt install tesseract-ocr`
- Rust (to build)

## Install

```bash
cargo install --path .
```

Then bind `~/.cargo/bin/circle-search` to a keyboard shortcut (e.g. Ctrl+Alt+S) in your desktop's keyboard settings.

## Develop

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Notes

Lens search opens a small local page (in `$XDG_RUNTIME_DIR`) that makes your browser post the image to Google's undocumented `lens.google.com/v3/upload` endpoint. It may break if Google changes that endpoint.
