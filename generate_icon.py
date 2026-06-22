"""Genere l'icone de l'application (icon.ico)."""

from PIL import Image, ImageDraw

SIZES = [16, 32, 48, 64, 128, 256]


def make_icon(size):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    mid = size // 2
    r = int(size * 0.35)
    draw.ellipse([mid - r, mid - r, mid + r, mid + r], fill=(255, 68, 68, 255))
    return img


def main():
    images = [make_icon(s) for s in SIZES]
    images[0].save("icon.ico", format="ICO", append_images=images[1:])
    print("icon.ico genere.")


if __name__ == "__main__":
    main()
