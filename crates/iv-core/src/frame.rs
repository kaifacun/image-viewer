use std::sync::Arc;

/// 픽셀 형식. 1차 구현은 `Rgba8Srgb` 만 쓰고 나머지는 HDR·16비트용 자리.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8비트 RGBA, sRGB 감마, straight alpha
    Rgba8Srgb,
    Rgba16,
    Rgba16F,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8Srgb => 4,
            Self::Rgba16 | Self::Rgba16F => 8,
        }
    }
}

/// 색공간 정보. 색 관리(iv-color)는 이후 단계에서 이 값을 읽는다.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ColorInfo {
    #[default]
    Srgb,
    DisplayP3,
    Icc(Arc<[u8]>),
}

/// EXIF 방향(1~8). 디코더는 픽셀을 회전하지 않고 이 값만 돌려주며, 렌더러가 적용한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Normal = 1,
    FlipH = 2,
    Rotate180 = 3,
    FlipV = 4,
    Transpose = 5,
    Rotate90 = 6,
    Transverse = 7,
    Rotate270 = 8,
}

impl Orientation {
    pub fn from_exif(v: u32) -> Self {
        match v {
            2 => Self::FlipH,
            3 => Self::Rotate180,
            4 => Self::FlipV,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Normal,
        }
    }

    /// 90°/270° 계열이면 가로·세로가 바뀐다
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270)
    }

    /// 저장된(회전 전) 크기 → 화면에 보이는 크기
    pub fn display_size(self, stored: [u32; 2]) -> [u32; 2] {
        if self.swaps_axes() { [stored[1], stored[0]] } else { stored }
    }

    /// 화면 UV(0..1, 표시 방향 기준) → 저장 픽셀 UV 로 바꾸는 2x2 행렬 + 이동.
    /// 반환: [a, b, c, d, tx, ty] — src = (a*u + b*v + tx, c*u + d*v + ty)
    pub fn uv_transform(self) -> [f32; 6] {
        match self {
            Self::Normal => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            Self::FlipH => [-1.0, 0.0, 0.0, 1.0, 1.0, 0.0],
            Self::Rotate180 => [-1.0, 0.0, 0.0, -1.0, 1.0, 1.0],
            Self::FlipV => [1.0, 0.0, 0.0, -1.0, 0.0, 1.0],
            Self::Transpose => [0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
            Self::Rotate90 => [0.0, 1.0, -1.0, 0.0, 0.0, 1.0],
            Self::Transverse => [0.0, -1.0, -1.0, 0.0, 1.0, 1.0],
            Self::Rotate270 => [0.0, -1.0, 1.0, 0.0, 1.0, 0.0],
        }
    }

    pub const ALL: [Self; 8] = [
        Self::Normal,
        Self::FlipH,
        Self::Rotate180,
        Self::FlipV,
        Self::Transpose,
        Self::Rotate90,
        Self::Transverse,
        Self::Rotate270,
    ];

    /// EXIF 값 (1~8)
    pub fn exif(self) -> u32 {
        self as u32
    }

    //#region 사용자 회전 — 화면에 보이는 그림을 90° 단위로 더 돌린 방향
    /// 보이는 그림을 시계 방향으로 90° 더 돌렸을 때의 방향.
    /// 새 화면 UV (u', v') 는 돌리기 전 화면 UV (v', 1-u') 이므로 그 뒤에 원래 변환을 잇는다.
    pub fn rotated_cw(self) -> Self {
        let [a, b, c, d, tx, ty] = self.uv_transform();
        // src = M·(v', 1-u') + t  =  (-b)u' + a v' + (b + tx),  (-d)u' + c v' + (d + ty)
        let want = [-b, a, -d, c, b + tx, d + ty];
        Self::ALL.into_iter().find(|o| o.uv_transform() == want).unwrap_or(self)
    }

    pub fn rotated_ccw(self) -> Self {
        self.rotated_cw().rotated_cw().rotated_cw()
    }

    /// 시계 방향 90° 를 `quarters` 번 (0~3, 넘치면 4 로 나눈 나머지)
    pub fn rotated_quarters(self, quarters: u8) -> Self {
        (0..quarters % 4).fold(self, |o, _| o.rotated_cw())
    }
    //#endregion
}

/// 디코드 전에 알 수 있는 메타데이터
#[derive(Clone, Debug, Default)]
pub struct ImageInfo {
    /// 저장된(회전 전) 원본 크기
    pub size: [u32; 2],
    pub orientation: Orientation,
    pub frame_count: u32,
    pub color: ColorInfo,
    pub has_preview: bool,
    /// 사람이 읽는 포맷 이름 (상태 표시줄)
    pub format_name: String,
}

/// 디코드된 한 장. `pixels` 는 Arc 라 캐시·렌더러가 복사 없이 공유한다.
#[derive(Clone, Debug)]
pub struct Frame {
    /// 이 프레임 픽셀 크기 (저장 방향, 축소 디코딩이면 원본보다 작음)
    pub width: u32,
    pub height: u32,
    /// 한 줄 바이트 수
    pub stride: usize,
    pub format: PixelFormat,
    pub color: ColorInfo,
    pub orientation: Orientation,
    /// 원본 크기 (저장 방향) — 확대 배율은 이 기준
    pub full_size: [u32; 2],
    /// 내장 미리보기에서 왔는가 (본 디코드로 교체 대상)
    pub is_preview: bool,
    pub pixels: Arc<[u8]>,
}

impl Frame {
    /// 꽉 찬(stride = width*4) RGBA8 sRGB 프레임
    pub fn rgba8(width: u32, height: u32, pixels: Vec<u8>, full_size: [u32; 2]) -> Self {
        debug_assert_eq!(pixels.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            stride: width as usize * 4,
            format: PixelFormat::Rgba8Srgb,
            color: ColorInfo::Srgb,
            orientation: Orientation::Normal,
            full_size,
            is_preview: false,
            pixels: pixels.into(),
        }
    }

    pub fn with_orientation(mut self, o: Orientation) -> Self {
        self.orientation = o;
        self
    }

    pub fn byte_size(&self) -> usize {
        self.pixels.len()
    }

    /// 화면에 보이는 원본 크기 (방향 적용)
    pub fn display_full_size(&self) -> [u32; 2] {
        self.orientation.display_size(self.full_size)
    }

    /// 긴 변 (축소 등급 비교용)
    pub fn max_side(&self) -> u32 {
        self.width.max(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(o: Orientation, u: f32, v: f32) -> (f32, f32) {
        let [a, b, c, d, tx, ty] = o.uv_transform();
        (a * u + b * v + tx, c * u + d * v + ty)
    }

    #[test]
    fn rotate90_cw_maps_corners() {
        // EXIF 6: 저장 이미지의 왼쪽 아래가 화면 왼쪽 위로 온다
        assert_eq!(map(Orientation::Rotate90, 0.0, 0.0), (0.0, 1.0));
        // 화면 오른쪽 위 = 저장 왼쪽 위
        assert_eq!(map(Orientation::Rotate90, 1.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn rotate270_maps_corners() {
        // EXIF 8: 저장 오른쪽 위가 화면 왼쪽 위로 온다
        assert_eq!(map(Orientation::Rotate270, 0.0, 0.0), (1.0, 0.0));
        assert_eq!(map(Orientation::Rotate270, 0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn rotation_composes_like_turning_the_picture() {
        assert_eq!(Orientation::Normal.rotated_cw(), Orientation::Rotate90);
        assert_eq!(Orientation::Rotate90.rotated_cw(), Orientation::Rotate180);
        assert_eq!(Orientation::Rotate180.rotated_cw(), Orientation::Rotate270);
        assert_eq!(Orientation::Rotate270.rotated_cw(), Orientation::Normal);
        for o in Orientation::ALL {
            // 네 번 돌리면 제자리, 시계·반시계는 서로 되돌림, 축 교환은 한 번마다 뒤집힘
            assert_eq!(o.rotated_quarters(4), o);
            assert_eq!(o.rotated_cw().rotated_ccw(), o);
            assert_ne!(o.rotated_cw().swaps_axes(), o.swaps_axes());
            // 거울 상태는 회전으로 바뀌지 않는다 (FlipH 계열은 FlipH 계열로)
            let mirrored = |o: Orientation| matches!(o, Orientation::FlipH | Orientation::FlipV | Orientation::Transpose | Orientation::Transverse);
            assert_eq!(mirrored(o.rotated_cw()), mirrored(o));
            // 저장 픽셀 기준으로 확인: 새 화면의 왼쪽 위 모서리 = 돌리기 전 화면의 왼쪽 아래 모서리
            assert_eq!(map(o.rotated_cw(), 0.0, 0.0), map(o, 0.0, 1.0));
            assert_eq!(map(o.rotated_cw(), 1.0, 0.0), map(o, 0.0, 0.0));
            assert_eq!(map(o.rotated_cw(), 0.0, 1.0), map(o, 1.0, 1.0));
        }
    }

    #[test]
    fn all_orientations_stay_in_unit_square() {
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            for (u, w) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let (x, y) = map(o, u, w);
                assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "{o:?}");
            }
        }
    }
}
