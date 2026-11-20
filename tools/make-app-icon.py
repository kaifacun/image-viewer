#!/usr/bin/env python3
"""KaiIV 앱 아이콘 원본(1024px PNG)을 그린다 → assets/icon-1024.png

macOS 아이콘 격자: 1024 캔버스 안 824px 둥근 사각형(여백 100), 모서리 반지름 약 185.
그림: 어두운 바탕 + 파란 사진 틀(산·해) — 앱 강조색(theme::ACCENT 64,140,255)과 맞춘다.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

S = 1024
OUT = Path(__file__).resolve().parent.parent / "assets" / "icon-1024.png"


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def main():
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # 그림자
    sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(sh).rounded_rectangle((100, 112, 924, 936), 185, fill=(0, 0, 0, 110))
    img.alpha_composite(sh.filter(ImageFilter.GaussianBlur(18)))
    # 바탕: 위→아래 그라데이션
    bg = Image.new("RGBA", (S, S))
    d = ImageDraw.Draw(bg)
    for y in range(S):
        d.line((0, y, S, y), fill=lerp((44, 47, 56), (18, 19, 23), y / S) + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((100, 100, 924, 924), 185, fill=255)
    img.paste(bg, (0, 0), mask)
    d = ImageDraw.Draw(img)
    # 사진 틀
    accent = (64, 140, 255, 255)
    fx0, fy0, fx1, fy1 = 236, 286, 788, 738
    d.rounded_rectangle((fx0, fy0, fx1, fy1), 56, outline=accent, width=34)
    # 해
    d.ellipse((560, 362, 664, 466), fill=(255, 196, 92, 255))
    # 산 두 개 (틀 안쪽으로 잘라 그림)
    inner = Image.new("L", (S, S), 0)
    ImageDraw.Draw(inner).rounded_rectangle((fx0 + 17, fy0 + 17, fx1 - 17, fy1 - 17), 40, fill=255)
    mt = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    m = ImageDraw.Draw(mt)
    m.polygon([(250, 724), (430, 470), (610, 724)], fill=(64, 140, 255, 255))
    m.polygon([(470, 724), (630, 540), (790, 724)], fill=(120, 175, 255, 255))
    img.paste(mt, (0, 0), Image.composite(mt, Image.new("RGBA", (S, S)), inner).split()[3])
    OUT.parent.mkdir(parents=True, exist_ok=True)
    img.save(OUT)
    print(OUT)


if __name__ == "__main__":
    main()
