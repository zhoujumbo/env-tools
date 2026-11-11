from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"
ROOT.mkdir(parents=True, exist_ok=True)


def make(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    m = max(1, size // 16)
    d.rounded_rectangle(
        [m, m, size - m - 1, size - m - 1],
        radius=max(4, size // 6),
        fill=(22, 78, 166, 255),
    )
    # inner tile
    d.rounded_rectangle(
        [size * 0.22, size * 0.22, size * 0.78, size * 0.78],
        radius=max(3, size // 10),
        fill=(61, 139, 253, 255),
    )
    try:
        font = ImageFont.truetype("segoeui.ttf", max(10, int(size * 0.42)))
    except OSError:
        font = ImageFont.load_default()
    text = "E"
    bbox = d.textbbox((0, 0), text, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    d.text(((size - tw) / 2 - bbox[0], (size - th) / 2 - bbox[1] - size * 0.03), text, fill="white", font=font)
    return img


def to_ico(path: Path, sizes=(16, 24, 32, 48, 64, 128, 256)):
    imgs = [make(s) for s in sizes]
    imgs[0].save(path, format="ICO", sizes=[(s, s) for s in sizes], append_images=imgs[1:])


if __name__ == "__main__":
    make(32).save(ROOT / "32x32.png")
    make(128).save(ROOT / "128x128.png")
    make(256).save(ROOT / "128x128@2x.png")
    make(128).save(ROOT / "icon.png")
    to_ico(ROOT / "icon.ico")
    print("icons written to", ROOT)
