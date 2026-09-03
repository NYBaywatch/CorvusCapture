#!/usr/bin/env python3
"""Generate the Corvus Capture tray icon from the logo artwork.

Cuts the crow-on-photo-frame motif out of ``resources/corvus_logo.png``
(white background made transparent), composes it at 88% scale on a light
grey (#D9D9D9) disc, and emits 16/32/48/256 px frames into
``resources/corvus.ico``. The disc keeps the near-black crow visible on
the Windows 11 dark taskbar without needing an outline ring.

Re-run this script any time the icon needs to be regenerated (e.g. after
replacing the logo art). Design chosen by the user from generated samples
(2026-09-03): "crow + frame on a light grey circle, bigger crow".

Requires: Pillow (`pip install pillow`)
"""

from pathlib import Path

from PIL import Image, ImageDraw

SIZES = [16, 32, 48, 256]
CANVAS = 512
DISC_COLOR = (217, 217, 217)  # light grey #D9D9D9
INNER_FRAC = 0.88             # crow+frame size relative to the disc canvas

ROOT = Path(__file__).resolve().parent.parent
SRC_PATH = ROOT / "resources" / "corvus_logo.png"
OUT_PATH = ROOT / "resources" / "corvus.ico"

# Crop box for the crow + photo frame, in the logo's original 1233px
# coordinate space (scaled to the actual source size at runtime).
CROP_REF = 1233.0
CROP_BOX = (280, 150, 880, 900)


def white_to_alpha(img: Image.Image) -> Image.Image:
    """Near-white background -> transparent, with a soft feather edge."""
    img = img.convert("RGBA")
    px = img.load()
    w, h = img.size
    for y in range(h):
        for x in range(w):
            r, g, b, a = px[x, y]
            m = (r + g + b) / 3
            if m > 240:
                px[x, y] = (r, g, b, 0)
            elif m > 210:
                px[x, y] = (r, g, b, int(255 * (240 - m) / 30))
    return img


def autocrop(img: Image.Image, pad: int = 12) -> Image.Image:
    l, t, r, b = img.split()[3].getbbox()
    return img.crop((
        max(0, l - pad), max(0, t - pad),
        min(img.width, r + pad), min(img.height, b + pad),
    ))


def squareize(img: Image.Image) -> Image.Image:
    s = max(img.size)
    base = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    base.paste(img, ((s - img.width) // 2, (s - img.height) // 2))
    return base


def build_master() -> Image.Image:
    src = Image.open(SRC_PATH)
    f = src.width / CROP_REF
    box = tuple(int(v * f) for v in CROP_BOX)
    motif = squareize(autocrop(white_to_alpha(src.crop(box))))

    base = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    d = ImageDraw.Draw(base)
    d.ellipse([4, 4, CANVAS - 4, CANVAS - 4], fill=DISC_COLOR + (255,))
    n = int(CANVAS * INNER_FRAC)
    inner = motif.resize((n, n), Image.LANCZOS)
    base.alpha_composite(inner, ((CANVAS - n) // 2, (CANVAS - n) // 2))
    return base


def main() -> None:
    master = build_master()
    frames = [master.resize((s, s), Image.LANCZOS) for s in SIZES]

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    frames[-1].save(
        OUT_PATH,
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=frames[:-1],
    )
    print(f"Wrote {OUT_PATH} with frames: {SIZES}")


if __name__ == "__main__":
    main()
