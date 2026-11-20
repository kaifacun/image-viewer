#!/bin/sh
# 비디오 시험 파일 → scratch/videos (git 제외). ffmpeg 필요 (brew install ffmpeg).
# - counter.mp4  1280x720 30fps 4초, testsrc2 (프레임마다 그림·시각이 바뀜) + 440Hz 소리
# - portrait.mov 같은 영상에 90° 회전 표시 정보만 (휴대폰 세로 영상 흉내)
# - big4k.mp4    3840x2160 2초 (복사 비용 측정용)
# - silent.m4v   소리 없는 짧은 영상
set -e
cd "$(dirname "$0")/.."
OUT=scratch/videos
mkdir -p "$OUT"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
ff -f lavfi -i testsrc2=size=1280x720:rate=30 -f lavfi -i sine=frequency=440:sample_rate=48000 -t 4 \
   -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$OUT/counter.mp4"
ff -display_rotation:v:0 90 -i "$OUT/counter.mp4" -c copy "$OUT/portrait.mov"
ff -f lavfi -i testsrc2=size=3840x2160:rate=30 -t 2 -c:v libx264 -pix_fmt yuv420p "$OUT/big4k.mp4"
ff -f lavfi -i testsrc2=size=640x360:rate=25 -t 2 -c:v libx264 -pix_fmt yuv420p "$OUT/silent.m4v"
cp scratch/fixtures/orient1.jpg "$OUT/still.jpg" 2>/dev/null || true
ls -la "$OUT"
