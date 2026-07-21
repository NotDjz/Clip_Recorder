"""Genere l'icone de l'application (assets/icon.ico)."""

import os

from PIL import Image, ImageDraw

SIZES = [16, 32, 48, 64, 128, 256]
# This script sits in scripts/; the icon belongs in assets/ at the repo root.
REPO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON_PATH = os.path.join(REPO_DIR, "assets", "icon.ico")


def make_icon(size):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    mid = size // 2
    r = int(size * 0.35)
    draw.ellipse([mid - r, mid - r, mid + r, mid + r], fill=(255, 68, 68, 255))
    return img


def main():
    images = [make_icon(s) for s in SIZES]
    os.makedirs(os.path.dirname(ICON_PATH), exist_ok=True)
    images[0].save(ICON_PATH, format="ICO", append_images=images[1:])
    print(f"{ICON_PATH} genere.")


if __name__ == "__main__":
    main()
