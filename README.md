# linux-circle-to-search

Android-style "Circle to Search" for Linux (X11). Press a shortcut, drag a box around anything on screen (Google Lens style: drag corners/edges to resize, drag inside to move), then:

- **Search** (Enter): opens the selection in Google Lens in your browser
- **Copy text** (C): reads the text with Tesseract OCR and copies it to the clipboard

Esc or right-click cancels. A single Python script, no API keys, no third-party image host.

## Requirements

Python 3 with GTK 3 bindings (`python3-gi`), `xclip`, `xdg-open`, `notify-send`, and for OCR:

```bash
sudo apt install tesseract-ocr
```

Works on X11 sessions (tested on Linux Mint Cinnamon, HiDPI). Wayland is not supported.

## Setup

```bash
chmod +x circle_search.py
```

Bind `/path/to/circle_search.py` to a keyboard shortcut (e.g. Super+S) in your desktop's keyboard settings.

## Test

```bash
python3 test_circle_search.py
```

## Notes

Lens search opens a small local page (in `$XDG_RUNTIME_DIR`) that makes your browser post the image to Google's undocumented `lens.google.com/v3/upload` endpoint. It may break if Google changes that endpoint.
