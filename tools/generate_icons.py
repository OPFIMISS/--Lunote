from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]


def build_icon(size: int) -> Image.Image:
    scale = 4
    px = size * scale
    image = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    def box(values):
        return tuple(round(value * scale) for value in values)

    draw.rounded_rectangle(box((0, 0, size, size)), radius=round(size * 0.23 * scale), fill="#163A4A")
    draw.ellipse(box((size * 0.125, size * 0.22, size * 0.72, size * 0.815)), fill="#F0B94D")
    draw.ellipse(box((size * 0.305, size * 0.10, size * 0.79, size * 0.585)), fill="#163A4A")

    paper = [
        (size * 0.39, size * 0.28),
        (size * 0.67, size * 0.28),
        (size * 0.80, size * 0.41),
        (size * 0.80, size * 0.77),
        (size * 0.39, size * 0.77),
    ]
    draw.polygon([(round(x * scale), round(y * scale)) for x, y in paper], fill="#FFFFFF")
    draw.polygon(
        [
            (round(size * 0.67 * scale), round(size * 0.28 * scale)),
            (round(size * 0.67 * scale), round(size * 0.41 * scale)),
            (round(size * 0.80 * scale), round(size * 0.41 * scale)),
        ],
        fill="#D8EEF0",
    )
    line_width = max(1, round(size * 0.035 * scale))
    draw.line(box((size * 0.48, size * 0.54, size * 0.70, size * 0.54)), fill="#3E7080", width=line_width)
    draw.line(box((size * 0.48, size * 0.64, size * 0.66, size * 0.64)), fill="#3E7080", width=line_width)
    return image.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    android = ROOT / "app/android/app/src/main/res"
    for folder, size in {
        "mipmap-mdpi": 48,
        "mipmap-hdpi": 72,
        "mipmap-xhdpi": 96,
        "mipmap-xxhdpi": 144,
        "mipmap-xxxhdpi": 192,
    }.items():
        build_icon(size).save(android / folder / "ic_launcher.png")

    icon_256 = build_icon(256)
    icon_256.save(
        ROOT / "app/windows/runner/resources/app_icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    preview = ROOT / "design/lunote-icon-preview.png"
    icon_256.save(preview)
    print(preview)


if __name__ == "__main__":
    main()
