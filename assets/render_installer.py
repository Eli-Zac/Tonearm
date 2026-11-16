"""Draws the installer's pictures from the logo.

Run it after changing the logo: `python assets/render_installer.py`. Needs
Pillow and Windows (for Segoe UI). It writes two bitmaps next to itself:

* installer-side.bmp, 164 x 314, on the welcome and finish pages
* installer-header.bmp, 150 x 57, at the top of the other pages

NSIS wants 24 bit BMPs at these exact sizes. Each is drawn at three times
the size and scaled down, so edges and text stay smooth.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

from render_logo import render

HERE = Path(__file__).resolve().parent
FONTS = Path(r"C:\Windows\Fonts")
SS = 3
BG = (0x0C, 0x0C, 0x0E)
GLOW = (0xEE, 0x3A, 0x57)
TEXT = (0xF4, 0xF4, 0xF6)
DIM = (0x9A, 0x9A, 0xA4)


def font(name: str, px: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / name), px * SS)


def centred(draw, y, text, face, fill, width):
    left, _, right, _ = draw.textbbox((0, 0), text, font=face)
    draw.text(((width - (right - left)) / 2 - left, y), text, font=face, fill=fill)


def side() -> Image.Image:
    w, h = 164 * SS, 314 * SS
    image = Image.new("RGB", (w, h), BG)
    # A soft accent glow behind the logo, fading into the dark.
    glow = Image.new("L", (w, h), 0)
    g = glow.load()
    cx, cy, reach = w / 2, h * 0.36, w * 0.9
    for y in range(h):
        for x in range(w):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5 / reach
            g[x, y] = round(max(0.0, 1 - d) ** 2 * 70)
    image.paste(Image.new("RGB", (w, h), GLOW), mask=glow)

    logo = render(256).resize((84 * SS, 84 * SS), Image.LANCZOS)
    image.paste(logo, ((w - logo.width) // 2, round(cy - logo.height / 2)), logo)

    draw = ImageDraw.Draw(image)
    centred(draw, cy + 56 * SS, "Tonearm", font("segoeuib.ttf", 22), TEXT, w)
    centred(draw, cy + 86 * SS, "Music for Windows", font("segoeui.ttf", 11), DIM, w)
    return image.resize((164, 314), Image.LANCZOS)


def header() -> Image.Image:
    # The header sits on the page's white band, so it is white too.
    w, h = 150 * SS, 57 * SS
    image = Image.new("RGB", (w, h), (255, 255, 255))
    logo = render(256).resize((34 * SS, 34 * SS), Image.LANCZOS)
    x = 12 * SS
    image.paste(logo, (x, (h - logo.height) // 2), logo)
    draw = ImageDraw.Draw(image)
    face = font("segoeuib.ttf", 17)
    _, top, _, bottom = draw.textbbox((0, 0), "Tonearm", font=face)
    draw.text((x + logo.width + 9 * SS, (h - (bottom - top)) / 2 - top), "Tonearm",
              font=face, fill=BG)
    return image.resize((150, 57), Image.LANCZOS)


def main():
    side().save(HERE / "installer-side.bmp")
    header().save(HERE / "installer-header.bmp")


if __name__ == "__main__":
    main()
