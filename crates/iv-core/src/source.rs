use std::path::{Path, PathBuf};

/// 이미지 출처. 이후 압축 파일 안 항목(cbz)·메모리 바이트 변형을 추가한다.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    Path(PathBuf),
}

impl ImageSource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(p) => Some(p),
        }
    }

    /// 소문자 확장자 (후보 축소 힌트용)
    pub fn extension(&self) -> Option<String> {
        self.path()?.extension()?.to_str().map(|s| s.to_ascii_lowercase())
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Path(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }

    /// 매직바이트 판별용 앞부분 읽기
    pub fn read_head(&self, n: usize) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        match self {
            Self::Path(p) => {
                let mut f = std::fs::File::open(p)?;
                let mut buf = vec![0u8; n];
                let mut read = 0;
                while read < n {
                    let k = f.read(&mut buf[read..])?;
                    if k == 0 {
                        break;
                    }
                    read += k;
                }
                buf.truncate(read);
                Ok(buf)
            }
        }
    }

    pub fn read_all(&self) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Path(p) => std::fs::read(p),
        }
    }
}

impl From<PathBuf> for ImageSource {
    fn from(p: PathBuf) -> Self {
        Self::Path(p)
    }
}
