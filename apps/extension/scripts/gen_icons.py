#!/usr/bin/env python3
"""Generate the browser-extension icons (16/32/48/128) from the brand mark.

Resizes the shared app icon into apps/extension/icons/. Run from anywhere:

    python3 apps/extension/scripts/gen_icons.py
"""
import os

from PIL import Image

HERE = os.path.dirname(__file__)
SRC = os.path.normpath(
    os.path.join(HERE, "..", "..", "desktop", "src-tauri", "icons", "icon.png")
)
OUT = os.path.normpath(os.path.join(HERE, "..", "icons"))
SIZES = [16, 32, 48, 128]


def main():
    os.makedirs(OUT, exist_ok=True)
    src = Image.open(SRC).convert("RGBA")
    for s in SIZES:
        img = src.resize((s, s), Image.LANCZOS)
        path = os.path.join(OUT, f"icon{s}.png")
        img.save(path, "PNG")
        print(f"✓ {path}")


if __name__ == "__main__":
    main()
