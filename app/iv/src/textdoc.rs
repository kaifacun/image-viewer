//! 텍스트·코드 문서 읽기 — 전체 보기에서 글로 보여 준다 (선택·복사 가능).
//! UTF-8 (BOM 포함)·UTF-16(BOM)을 먼저, 아니면 한국어 Windows 문서에 흔한 EUC-KR(CP949)로 읽는다.

use std::path::Path;

/// 이 크기까지만 읽는다 (그 이상은 앞부분만 + 알림)
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub struct TextDoc {
    pub text: String,
    /// 코드면 줄 번호·고정폭
    pub code: bool,
    /// 앞부분만 읽었음
    pub truncated: bool,
    /// 사람이 읽는 인코딩 이름
    pub encoding: &'static str,
}

/// 확장자로: Some(코드인가) — 글로 보여 줄 문서가 아니면 None
pub fn text_kind(path: &Path) -> Option<bool> {
    const PROSE: &[&str] = &["txt", "md", "markdown", "log", "csv", "tsv", "json", "xml", "yaml", "yml", "toml", "ini"];
    const CODE: &[&str] = &["rs", "py", "js", "ts", "tsx", "jsx", "java", "kt", "swift", "c", "h", "cpp", "hpp", "cs", "go", "rb", "php", "sh", "html", "css", "sql"];
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if CODE.contains(&ext.as_str()) {
        Some(true)
    } else if PROSE.contains(&ext.as_str()) {
        // 표 형식(csv·tsv·json·xml…)은 고정폭이 읽기 좋다
        Some(matches!(ext.as_str(), "csv" | "tsv" | "json" | "xml" | "yaml" | "yml" | "toml" | "ini" | "log"))
    } else {
        None
    }
}

pub fn load(path: &Path) -> Result<TextDoc, String> {
    use std::io::Read;
    let code = text_kind(path).ok_or("글 문서가 아님")?;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut bytes = Vec::new();
    f.take(MAX_BYTES).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let (text, encoding) = decode(&bytes);
    // 탭은 4칸 (egui 는 탭 폭이 들쭉날쭉)
    let text = text.replace('\t', "    ");
    Ok(TextDoc { text, code, truncated: len > MAX_BYTES, encoding })
}

fn decode(bytes: &[u8]) -> (String, &'static str) {
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(bytes) {
        let (t, _, _) = enc.decode(bytes);
        return (t.into_owned(), enc.name());
    }
    // 잘린 끝의 반쪽 글자는 UTF-8 판정에서 봐준다
    let valid = match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && e.valid_up_to() + 4 > bytes.len(),
    };
    if valid {
        return (String::from_utf8_lossy(bytes).into_owned(), "UTF-8");
    }
    let (t, _, bad) = encoding_rs::EUC_KR.decode(bytes);
    if !bad {
        return (t.into_owned(), "EUC-KR");
    }
    (String::from_utf8_lossy(bytes).into_owned(), "UTF-8 (깨진 글자 있음)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_and_euc_kr() {
        assert_eq!(decode("안녕 KaiIV".as_bytes()), ("안녕 KaiIV".to_string(), "UTF-8"));
        let (euc, _, _) = encoding_rs::EUC_KR.encode("한글 문서");
        assert_eq!(decode(&euc), ("한글 문서".to_string(), "EUC-KR"));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("가".as_bytes());
        assert_eq!(decode(&bom).0, "가");
        assert_eq!(text_kind(Path::new("a.RS")), Some(true));
        assert_eq!(text_kind(Path::new("a.md")), Some(false));
        assert_eq!(text_kind(Path::new("a.pdf")), None);
    }
}
