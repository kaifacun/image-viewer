#!/usr/bin/env python3
"""시험용 이미지 묶음 생성 (scratch/fixtures, git 제외).
맥 기본 배경화면 HEIC 를 원본으로 JPEG/PNG/EXIF 회전/투명/대형/다수 파일을 만든다."""
import glob, os, subprocess, sys
from PIL import Image, ImageDraw

out = sys.argv[1] if len(sys.argv) > 1 else "scratch/fixtures"
os.makedirs(out, exist_ok=True)
heics = sorted(f for f in glob.glob("/System/Library/Desktop Pictures/*.heic") if os.path.getsize(f) > 500_000)[:12]

# 1) HEIC 원본 몇 장 그대로 복사
for i, h in enumerate(heics[:4]):
    subprocess.run(["cp", h, f"{out}/h{i:02d}.heic"], check=True)

# 2) HEIC → JPEG (sips), 폴더 넘기기용
for i, h in enumerate(heics):
    subprocess.run(["sips", "-s", "format", "jpeg", "-s", "formatOptions", "85", h, "--out", f"{out}/p{i:02d}.jpg"],
                   check=True, capture_output=True)

# 3) 방향 표식 이미지 (왼쪽 위 빨강, 오른쪽 위 초록, 아래 파랑 띠) + EXIF 방향 1~8
base = Image.new("RGB", (600, 400), "white")
d = ImageDraw.Draw(base)
d.rectangle([0, 0, 150, 100], fill="red")
d.rectangle([450, 0, 600, 100], fill="green")
d.rectangle([0, 350, 600, 400], fill="blue")
# 저장 픽셀을 방향 값의 역변환으로 만들어, 올바른 뷰어에서는 모두 같은 그림으로 보이게 한다
inverse = {1: [], 2: [Image.Transpose.FLIP_LEFT_RIGHT], 3: [Image.Transpose.ROTATE_180],
           4: [Image.Transpose.FLIP_TOP_BOTTOM], 5: [Image.Transpose.TRANSPOSE],
           6: [Image.Transpose.ROTATE_90], 7: [Image.Transpose.TRANSVERSE], 8: [Image.Transpose.ROTATE_270]}
for o, ops in inverse.items():
    im = base
    for op in ops:
        im = im.transpose(op)
    ex = Image.Exif(); ex[0x0112] = o
    im.save(f"{out}/orient{o}.jpg", quality=92, exif=ex)

# 4) 투명 PNG
im = Image.new("RGBA", (512, 512), (0, 0, 0, 0))
ImageDraw.Draw(im).ellipse([56, 56, 456, 456], fill=(255, 128, 0, 160))
im.save(f"{out}/alpha.png")

# 5) 대형 JPEG (12000x8000)
Image.open(f"{out}/p00.jpg").resize((12000, 8000)).save(f"{out}/huge.jpg", quality=85)

# 6) 깨진 파일 / 확장자만 그럴듯한 파일
open(f"{out}/broken.jpg", "wb").write(b"\xff\xd8\xff\xe0garbage" * 10)
open(f"{out}/notes.txt", "w").write("not an image")
print("fixtures ->", out, len(os.listdir(out)), "files")
