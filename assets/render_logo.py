"""Draws the Tonearm logo (assets/logo.svg) into the Windows icon file.

Run it after changing the logo: `python assets/render_logo.py`. Needs
Pillow. Each size is drawn on its own, four times over and scaled down, so
small icons stay sharp.
"""
from pathlib import Path
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
TOP, BOTTOM = (0xFF, 0x4A, 0x4A), (0xD9, 0x1E, 0x2B)
WHITE = (255, 255, 255, 255)
# The sound wave, in the logo's 64 units: five bars, mirrored about the middle.
HEIGHTS = [14, 26, 38, 26, 14]
WIDTH, GAP = 6.0, 3.2


def render(px: int) -> Image.Image:
    ss = 4
    size = px * ss
    k = size / 64  # grid units to pixels

    # The disc's diagonal gradient.
    grad = Image.new("RGBA", (size, size))
    g = grad.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * (size - 1))
            g[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM)) + (255,)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).ellipse([2 * k, 2 * k, 62 * k, 62 * k], fill=255)
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    total = len(HEIGHTS) * WIDTH + (len(HEIGHTS) - 1) * GAP
    x = 32 - total / 2
    for h in HEIGHTS:
        d.rounded_rectangle([x * k, (32 - h / 2) * k, (x + WIDTH) * k, (32 + h / 2) * k],
                            radius=WIDTH / 2 * k, fill=WHITE)
        x += WIDTH + GAP

    return img.resize((px, px), Image.LANCZOS)


def main():
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = {px: render(px) for px in sizes}
    images[256].save(HERE / "tonearm.ico", sizes=[(p, p) for p in sizes],
                     append_images=[images[p] for p in sizes if p != 256])
    print("wrote assets/tonearm.ico")


if __name__ == "__main__":
    main()
