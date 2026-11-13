"""Draws the Clip Recorder logo (a red record dot on a dark rounded tile) and writes every
image the app and the installer use.

Run from the repo root: python assets/make_icons.py
Outputs are committed, so building never needs Python.
"""

from pathlib import Path

from PIL import Image, ImageDraw

TILE = (25, 28, 27)  # #191C1B, the app's BG
RING = (47, 53, 50)  # #2F3532, BG3: a rim that keeps the tile visible on dark taskbars
RED = (255, 74, 61)  # #FF4A3D, the app's ACCENT
WHITE = (255, 255, 255)
SUPERSAMPLE = 4
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
ROOT = Path(__file__).resolve().parent.parent

# Geometry on a 16-unit grid (1 unit = 1 px in the 16 px icon).
TILE_BOX = (0.5, 0.5, 15.5, 15.5, 3.5)  # x0, y0, x1, y1, corner radius
DOT = (8, 8, 4.25)  # centre x, centre y, radius


def mark(draw: ImageDraw.ImageDraw, unit: float, ox: float = 0, oy: float = 0) -> None:
    x0, y0, x1, y1, r = TILE_BOX
    draw.rounded_rectangle([ox + x0 * unit, oy + y0 * unit, ox + x1 * unit, oy + y1 * unit], r * unit,
                           fill=TILE, outline=RING, width=max(1, round(0.5 * unit)))
    cx, cy, rr = DOT
    draw.ellipse([ox + (cx - rr) * unit, oy + (cy - rr) * unit, ox + (cx + rr) * unit, oy + (cy + rr) * unit], fill=RED)


def logo(size: int) -> Image.Image:
    big = size * SUPERSAMPLE
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    mark(ImageDraw.Draw(img), big / 16)
    return img.resize((size, size), Image.LANCZOS)


def on_white(width: int, height: int, size: int, at: tuple[int, int]) -> Image.Image:
    canvas = Image.new("RGB", (width, height), WHITE)
    img = logo(size)
    canvas.paste(img, at, img)
    return canvas


def banner() -> Image.Image:
    # WiX draws the page title in black at the top left: keep that side white.
    size = 44
    return on_white(493, 58, size, (493 - size - 10, (58 - size) // 2))


def dialog() -> Image.Image:
    # WiX draws welcome/finish text from x=135 on white: dark panel only on the left.
    panel_w, size = 124, 96
    img = Image.new("RGB", (493, 312), WHITE)
    panel = Image.new("RGB", (panel_w, 312), TILE)
    mark_img = logo(size)
    panel.paste(mark_img, ((panel_w - size) // 2, 28), mark_img)
    img.paste(panel, (0, 0))
    return img


def main() -> None:
    assets, installer = ROOT / "assets", ROOT / "installer"
    installer.mkdir(exist_ok=True)
    images = [logo(s) for s in ICO_SIZES]
    images[-1].save(assets / "clip-recorder.ico", sizes=[(s, s) for s in ICO_SIZES], append_images=images[:-1])
    banner().save(installer / "banner.bmp")
    dialog().save(installer / "dialog.bmp")

    with Image.open(assets / "clip-recorder.ico") as ico:
        assert sorted(ico.info["sizes"]) == [(s, s) for s in ICO_SIZES], ico.info["sizes"]
    assert Image.open(installer / "banner.bmp").size == (493, 58)
    assert Image.open(installer / "dialog.bmp").size == (493, 312)
    print("wrote clip-recorder.ico, banner.bmp, dialog.bmp")


if __name__ == "__main__":
    main()
