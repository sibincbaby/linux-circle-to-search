# linux-circle-to-search

Android-style "Circle to Search" for Linux (X11). Press a shortcut, draw around anything on screen, then:

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

Lens search uses Google's undocumented `lens.google.com/v3/upload` endpoint and may break if Google changes it.
