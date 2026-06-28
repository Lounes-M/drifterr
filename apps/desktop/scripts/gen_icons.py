#!/usr/bin/env python3
"""Generate Drifterr's tray + app icons as PNGs using only the stdlib.

Tray icons are filled colored circles on a transparent background (one per
state). The app icon is a rounded dark tile with the brand dot. Run from
anywhere; writes into ../src-tauri/icons.

    python3 apps/desktop/scripts/gen_icons.py
"""
import os
import struct
import zlib

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")

STATES = {
    "green": (0x34, 0xC7, 0x59),
    "amber": (0xFF, 0x9F, 0x0A),
    "red": (0xFF, 0x45, 0x3A),
    "unknown": (0x6B, 0x72, 0x80),
}


def write_png(path, width, height, pixels):
    """pixels: list of (r,g,b,a) row-major."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter type 0
        for x in range(width):
            raw.extend(pixels[y * width + x])
    compressor = zlib.compressobj(level=9)
    data = compressor.compress(bytes(raw)) + compressor.flush()

    def chunk(tag, payload):
        return (
            struct.pack(">I", len(payload))
            + tag
            + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
        )

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)  # 8-bit RGBA
    with open(path, "wb") as f:
        f.write(sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", data) + chunk(b"IEND", b""))


def circle(size, color, radius_frac=0.82, ss=4):
    """Anti-aliased filled circle via supersampling. Transparent background."""
    px = []
    cx = cy = (size - 1) / 2.0
    r = (size / 2.0) * radius_frac
    for y in range(size):
        for x in range(size):
            covered = 0
            for sy in range(ss):
                for sx in range(ss):
                    fx = x + (sx + 0.5) / ss
                    fy = y + (sy + 0.5) / ss
                    if (fx - cx - 0.5) ** 2 + (fy - cy - 0.5) ** 2 <= r * r:
                        covered += 1
            a = int(255 * covered / (ss * ss))
            px.append((color[0], color[1], color[2], a))
    return px


def rounded_tile(size, bg, dot, radius_frac=0.22, ss=4):
    """Rounded-rect dark tile with a centered brand dot — the app icon."""
    px = []
    rad = size * radius_frac
    cx = cy = (size - 1) / 2.0
    dot_r = size * 0.22
    for y in range(size):
        for x in range(size):
            # rounded-rect coverage
            cov = 0
            din = 0
            for sy in range(ss):
                for sx in range(ss):
                    fx = x + (sx + 0.5) / ss
                    fy = y + (sy + 0.5) / ss
                    if inside_round_rect(fx, fy, size, rad):
                        cov += 1
                    if (fx - cx - 0.5) ** 2 + (fy - cy - 0.5) ** 2 <= dot_r * dot_r:
                        din += 1
            a = cov / (ss * ss)
            d = din / (ss * ss)
            r = int(bg[0] * (1 - d) + dot[0] * d)
            g = int(bg[1] * (1 - d) + dot[1] * d)
            b = int(bg[2] * (1 - d) + dot[2] * d)
            px.append((r, g, b, int(255 * a)))
    return px


def inside_round_rect(fx, fy, size, rad):
    x = fx - 0.5
    y = fy - 0.5
    lo, hi = rad, size - rad
    cxp = min(max(x, lo), hi)
    cyp = min(max(y, lo), hi)
    return (x - cxp) ** 2 + (y - cyp) ** 2 <= rad * rad


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, color in STATES.items():
        write_png(os.path.join(OUT, f"tray-{name}.png"), 32, 32, circle(32, color))
    # App icon (brand: blue dot on dark tile) at a few common sizes.
    for size in (32, 128, 512):
        px = rounded_tile(size, (0x16, 0x18, 0x1D), (0x0A, 0x84, 0xFF))
        name = "icon.png" if size == 512 else f"{size}x{size}.png"
        write_png(os.path.join(OUT, name), size, size, px)
    print("icons written to", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
