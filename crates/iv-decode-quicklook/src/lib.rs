//! macOS 훑어보기(QuickLook) 썸네일 — Finder 가 보여 주는 바로 그 미리보기.
//! 문서 종류(텍스트·코드·오피스·iWork 등)의 썸네일·미리보기에 쓴다. 여러 쪽 문서는 첫 쪽만.
//!
//! 비동기 API 를 채널로 기다린다 (썸네일·파이프라인 워커 스레드에서 불린다 — 메인 스레드 아님).

/// 다루는 확장자 (`iv_core::formats` 의 문서 묶음 중 PDF 를 뺀 것)
pub const EXTENSIONS: &[&str] = &[
    // 텍스트
    "txt", "md", "markdown", "log", "csv", "tsv", "json", "xml", "yaml", "yml", "toml", "ini",
    // 코드
    "rs", "py", "js", "ts", "tsx", "jsx", "java", "kt", "swift", "c", "h", "cpp", "hpp", "cs", "go", "rb", "php", "sh", "html", "css", "sql",
    // 워드·스프레드시트·프레젠테이션 (MS·오픈·iWork)
    "doc", "docx", "odt", "rtf", "pages", "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key",
];

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::QuickLookDecoder;

pub fn register(_registry: &mut iv_core::DecoderRegistry) {
    #[cfg(target_os = "macos")]
    _registry.register(QuickLookDecoder);
}
