/// 디코드 오류 분류. 레지스트리의 폴백 여부가 이 분류로 결정된다.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// 이 디코더는 이 포맷/변형을 지원하지 않음 → 다음 디코더 시도
    #[error("지원하지 않는 형식: {0}")]
    Unsupported(String),
    /// 데이터가 손상됐거나 이 디코더가 해석하지 못함 → 다음 디코더 시도
    #[error("잘못된 데이터: {0}")]
    InvalidData(String),
    /// 파일 읽기 실패 → 즉시 실패 (다른 디코더도 같은 결과)
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),
    /// 취소됨 → 즉시 중단
    #[error("취소됨")]
    Cancelled,
    /// 크기·메모리 상한 초과
    #[error("자원 상한 초과: {0}")]
    ResourceLimit(String),
}

impl DecodeError {
    /// 다음 디코더로 넘겨볼 가치가 있는 오류인가
    pub fn allows_fallback(&self) -> bool {
        matches!(self, Self::Unsupported(_) | Self::InvalidData(_))
    }
}

pub type Result<T> = std::result::Result<T, DecodeError>;
