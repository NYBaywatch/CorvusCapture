#!/usr/bin/env python3
"""Generate the placeholder Corvus Capture crow icon.

Draws a simple flat black crow silhouette (head, body, beak, tail wedge) on a
transparent background at 256x256, then downsamples with a high-quality
filter to produce the 16/32/48/256 px frames baked into resources/corvus.ico.

Re-run this script any time the placeholder icon needs to be regenerated
(e.g. after tweaking the silhouette). Phase 5 replaces this with final
branding — likely a new script or a hand-authored .ico, per D-05 (file swap
only, no code change expected in build.rs/app.rc).

Requires: Pillow (`pip install pillow`)
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SIZES = [16, 32, 48, 256]
CANVAS = 256
OUT_PATH = Path(__file__).resolve().parent.parent / "resources" / "corvus.ico"


def draw_crow(size: int) -> Image.Image:
    """Draw a flat black crow silhouette on a transparent canvas."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    black = (0, 0, 0, 255)

    # Body: rounded torso, slightly tilted, sitting pose.
    draw.ellipse([50, 110, 200, 220], fill=black)

    # Tail: wedge trailing behind/below the body (screen-left, pointing down-left).
    draw.polygon(
        [(70, 190), (10, 245), (95, 215)],
        fill=black,
    )

    # Head: round, overlapping the top of the body.
    draw.ellipse([130, 40, 220, 130], fill=black)

    # Beak: triangular wedge pointing right from the head.
    draw.polygon(
        [(212, 78), (250, 92), (212, 108)],
        fill=black,
    )

    return img


def add_white_outline(img: Image.Image) -> Image.Image:
    """Composites a white ring around `img`'s silhouette so the crow reads
    clearly against a dark taskbar at real 16x16 tray size (RESEARCH.md
    Pattern 2). Dilates the alpha channel with a MaxFilter sized to the
    frame (ring ~= size/8 px) to build the ring mask, then alpha-composites
    the original black crow back on top. Applied per-frame AFTER resizing —
    outlining only the 256px master and downsampling shrinks the ring to a
    fraction of a pixel at 16x16, making it invisible."""
    size = img.size[0]
    kernel = 2 * max(2, size // 8) + 1  # odd; ring thickness = (kernel-1)/2
    alpha = img.split()[3]
    dilated_alpha = alpha.filter(ImageFilter.MaxFilter(kernel))

    white_ring = Image.new("RGBA", img.size, (255, 255, 255, 0))
    white_ring.putalpha(dilated_alpha)

    return Image.alpha_composite(white_ring, img)


def main() -> None:
    crow = draw_crow(CANVAS)
    frames = []
    for size in SIZES:
        if size == CANVAS:
            frame = add_white_outline(crow)
        else:
            frame = add_white_outline(crow.resize((size, size), Image.LANCZOS))
        frames.append(frame)
    base = frames[SIZES.index(CANVAS)]

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    base.save(
        OUT_PATH,
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=[f for f in frames if f is not base],
    )
    print(f"Wrote {OUT_PATH} with frames: {SIZES}")


if __name__ == "__main__":
    main()
