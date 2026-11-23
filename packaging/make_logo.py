"""Draws packaging/logo.png (the app icon's artwork) and packaging/menubar.png: a wooden sandbox full of sand with a
shell prompt pressed into it. Run `uv run --with pillow packaging/make_logo.py`, then `just icon`."""

import random
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

HERE = Path(__file__).parent
S = 1024


def rounded(size: int, inset: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((inset, inset, size - inset - 1, size - inset - 1), radius, fill=255)
    return mask


def wood() -> Image.Image:
    rng = random.Random(7)
    img = Image.new("RGB", (S, S), (150, 94, 52))
    d = ImageDraw.Draw(img)
    for y in range(0, S, 3):
        shade = rng.randint(-14, 14)
        d.line((0, y, S, y), fill=(150 + shade, 94 + shade, 52 + shade), width=3)
    return img


def sand() -> Image.Image:
    rng = random.Random(11)
    img = Image.new("RGB", (S, S), (226, 196, 140))
    d = ImageDraw.Draw(img)
    for _ in range(9000):
        x, y = rng.randrange(S), rng.randrange(S)
        shade = rng.randint(-26, 22)
        d.ellipse((x, y, x + 3, y + 3), fill=(226 + shade, 196 + shade, 140 + shade))
    return img


def logo() -> Image.Image:
    base = wood()
    inner_mask = rounded(S, 72, 150)
    inner = sand()
    # The sand sits below the rim: a soft shadow along the inside edge of the frame.
    shadow = ImageChops.invert(inner_mask).filter(ImageFilter.GaussianBlur(26))
    inner.paste((120, 84, 40), mask=shadow.point(lambda v: int(v * 0.55)))
    base.paste(inner, mask=inner_mask)
    d = ImageDraw.Draw(base)
    # A prompt, ">_", pressed into the sand.
    ink = (52, 64, 96)
    d.line((300, 380, 470, 520, 300, 660), fill=ink, width=70, joint="curve")
    for p in ((300, 380), (470, 520), (300, 660)):
        d.ellipse((p[0] - 35, p[1] - 35, p[0] + 35, p[1] + 35), fill=ink)
    d.rounded_rectangle((540, 640, 730, 690), 25, fill=ink)
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.paste(base, mask=rounded(S, 0, 220))
    return out


def menubar() -> Image.Image:
    # Black on clear at 36 px, which macOS tints: an open box and a chevron.
    img = Image.new("RGBA", (144, 144), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((12, 28, 132, 128), 20, outline=(0, 0, 0, 255), width=12)
    d.line((44, 56, 68, 78, 44, 100), fill=(0, 0, 0, 255), width=11, joint="curve")
    d.line((78, 100, 104, 100), fill=(0, 0, 0, 255), width=11)
    return img.resize((36, 36), Image.LANCZOS)


if __name__ == "__main__":
    logo().save(HERE / "logo.png")
    menubar().save(HERE / "menubar.png")
