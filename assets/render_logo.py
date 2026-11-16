"""Draws the Tonearm logo (assets/logo.svg) into the Windows icon file.

Run it after changing the logo: `python assets/render_logo.py`. Needs
Pillow. Each size is drawn on its own, four times over and scaled down, so
small icons stay sharp. Below 32 px the faint groove ring goes and the arm
gets heavier, so the mark still reads on the taskbar.
"""
from pathlib import Path
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
TOP, BOTTOM = (0xFF, 0x6F, 0x88), (0xEE, 0x3A, 0x57)
INK = (0x0C, 0x0C, 0x0E, 255)
ARM = [(49, 13), (45, 29.5), (38.5, 33)]


def render(px: int) -> Image.Image:
    small = px < 32
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

    def ring(r, width, color, draw=d):
        # Pillow draws an outline inward from the box. SVG centres the
        # stroke on the radius, so grow the box by half the width.
        r += width / 2
        draw.ellipse([(32 - r) * k, (32 - r) * k, (32 + r) * k, (32 + r) * k],
                     outline=color, width=max(1, round(width * k)))

    def dot(cx, cy, r, color=INK):
        d.ellipse([(cx - r) * k, (cy - r) * k, (cx + r) * k, (cy + r) * k], fill=color)

    if not small:
        overlay = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        ring(22, 1.2, (12, 12, 14, 41), ImageDraw.Draw(overlay))
        img.alpha_composite(overlay)
    ring(9, 3.4 if not small else 4.2, INK)
    dot(32, 32, 2.2 if not small else 2.6)

    arm = 4.0 if not small else 5.2
    pts = [(x * k, y * k) for x, y in ARM]
    d.line(pts, fill=INK, width=round(arm * k))
    for x, y in ARM:  # round caps and joins
        dot(x, y, arm / 2)
    dot(49, 13, 3.8 if not small else 4.4)

    return img.resize((px, px), Image.LANCZOS)


def main():
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = {px: render(px) for px in sizes}
    images[256].save(HERE / "tonearm.ico", sizes=[(p, p) for p in sizes],
                     append_images=[images[p] for p in sizes if p != 256])
    print("wrote assets/tonearm.ico")


if __name__ == "__main__":
    main()
