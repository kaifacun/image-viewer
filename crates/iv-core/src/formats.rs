//! 형식 묶음 — 확장자 필터와 폴더 스캔이 함께 쓰는 단위 (jpg·jpeg·jpe 는 한 묶음 "JPEG").
//!
//! 묶음의 순서가 스캔 비트 번호다. 나중에 비디오·문서 모드는 [`MediaKind`] 가 다른 묶음을 덧붙인다
//! (모드 = 그 종류 묶음들의 비트 합). 비트는 64개 미만.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    /// 화면 모드 순서 (모드 전환 버튼) — 문서 모드는 여기 덧붙인다
    pub const ALL: [Self; 2] = [Self::Image, Self::Video];

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "사진",
            Self::Video => "비디오",
        }
    }

    /// 세는 단위 ("사진 3장", "비디오 3개")
    pub fn unit(self) -> &'static str {
        match self {
            Self::Image => "장",
            Self::Video => "개",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatGroup {
    pub name: &'static str,
    pub kind: MediaKind,
    /// 소문자 확장자
    pub exts: Vec<&'static str>,
}

/// 이름 붙은 사진 묶음 (실제로 디코더가 아는 확장자만 남긴다)
const IMAGE_TABLE: &[(&str, &[&str])] = &[
    ("JPEG", &["jpg", "jpeg", "jpe", "jfif"]),
    ("PNG", &["png"]),
    ("HEIC", &["heic", "heif", "hif"]),
    ("AVIF", &["avif"]),
    ("GIF", &["gif"]),
    ("WebP", &["webp"]),
    ("TIFF", &["tif", "tiff"]),
    ("BMP", &["bmp"]),
    ("PSD", &["psd"]),
    (
        "RAW",
        &[
            "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "srw", "x3f", "3fr", "fff",
            "iiq", "mos", "mrw", "erf", "dcr", "kdc",
        ],
    ),
];

/// 비디오 묶음 (AVFoundation 이 재생하는 것)
const VIDEO_TABLE: &[(&str, &[&str])] = &[("MP4", &["mp4", "m4v"]), ("MOV", &["mov", "qt"]), ("3GP", &["3gp", "3g2"])];

/// 비디오 확장자 전부 (디코더가 알든 모르든 — 사진 쪽 "기타" 로 새지 않게 먼저 뺀다)
pub fn video_extensions() -> impl Iterator<Item = &'static str> {
    VIDEO_TABLE.iter().flat_map(|(_, e)| e.iter().copied())
}

/// 모든 종류의 묶음: 사진 묶음 다음 비디오 묶음. 비디오 확장자는 사진 "기타" 에 들어가지 않는다.
pub fn media_groups(known: &HashSet<&'static str>) -> Vec<FormatGroup> {
    let video: HashSet<&'static str> = video_extensions().collect();
    let images: HashSet<&'static str> = known.iter().copied().filter(|e| !video.contains(e)).collect();
    let mut out = image_groups(&images);
    for (name, exts) in VIDEO_TABLE {
        let mut exts: Vec<&'static str> = exts.iter().copied().filter(|e| known.contains(e)).collect();
        exts.sort_unstable();
        if !exts.is_empty() {
            out.push(FormatGroup { name, kind: MediaKind::Video, exts });
        }
    }
    out
}

/// 한 종류의 확장자 전부
pub fn kind_extensions(groups: &[FormatGroup], kind: MediaKind) -> HashSet<&'static str> {
    groups.iter().filter(|g| g.kind == kind).flat_map(|g| g.exts.iter().copied()).collect()
}

/// 확장자의 종류 (모르는 확장자면 None)
pub fn kind_of(groups: &[FormatGroup], ext: &str) -> Option<MediaKind> {
    let ext = ext.to_ascii_lowercase();
    groups.iter().find(|g| g.exts.iter().any(|e| *e == ext)).map(|g| g.kind)
}

/// 디코더가 아는 확장자(`known`)로 사진 묶음을 만든다. 표에 없는 확장자는 마지막 "기타" 묶음.
pub fn image_groups(known: &HashSet<&'static str>) -> Vec<FormatGroup> {
    let mut used = HashSet::new();
    let mut out: Vec<FormatGroup> = IMAGE_TABLE
        .iter()
        .filter_map(|(name, exts)| {
            let exts: Vec<&'static str> = known.iter().copied().filter(|k| exts.contains(k)).collect();
            used.extend(exts.iter().copied());
            let mut exts = exts;
            exts.sort_unstable();
            (!exts.is_empty()).then_some(FormatGroup { name, kind: MediaKind::Image, exts })
        })
        .collect();
    let mut rest: Vec<&'static str> = known.iter().copied().filter(|k| !used.contains(k)).collect();
    rest.sort_unstable();
    if !rest.is_empty() {
        out.push(FormatGroup { name: "기타", kind: MediaKind::Image, exts: rest });
    }
    out
}

/// 묶음 비트 합 (`pick` 이 고른 묶음들)
pub fn group_mask(groups: &[FormatGroup], pick: impl Fn(&FormatGroup) -> bool) -> u64 {
    groups.iter().enumerate().filter(|(_, g)| pick(g)).fold(0, |m, (i, _)| m | (1u64 << i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_cover_every_known_extension_once() {
        let known: HashSet<&'static str> = ["jpg", "jpeg", "png", "cr2", "dng", "exr", "tga"].into_iter().collect();
        let g = image_groups(&known);
        let names: Vec<&str> = g.iter().map(|g| g.name).collect();
        assert_eq!(names, ["JPEG", "PNG", "RAW", "기타"]);
        assert_eq!(g[0].exts, ["jpeg", "jpg"]);
        assert_eq!(g[3].exts, ["exr", "tga"]);
        let total: usize = g.iter().map(|g| g.exts.len()).sum();
        assert_eq!(total, known.len());
        assert_eq!(group_mask(&g, |g| g.name == "PNG" || g.name == "기타"), 0b1010);
    }

    #[test]
    fn video_extensions_never_leak_into_images() {
        let known: HashSet<&'static str> = ["jpg", "mp4", "mov", "exr"].into_iter().collect();
        let g = media_groups(&known);
        let names: Vec<(&str, MediaKind)> = g.iter().map(|g| (g.name, g.kind)).collect();
        assert_eq!(names, [("JPEG", MediaKind::Image), ("기타", MediaKind::Image), ("MP4", MediaKind::Video), ("MOV", MediaKind::Video)]);
        assert_eq!(kind_extensions(&g, MediaKind::Image), ["jpg", "exr"].into_iter().collect());
        assert_eq!(kind_of(&g, "MOV"), Some(MediaKind::Video));
        assert_eq!(kind_of(&g, "txt"), None);
    }
}
