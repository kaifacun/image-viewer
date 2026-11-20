use std::path::PathBuf;

pub const USAGE: &str = "사용법: iv [파일|폴더] [옵션]
  --renderer=auto|gpu|fallback|cpu   렌더러 선택 (기본 auto: GPU → 폴백 어댑터 → CPU)
  --render-to=<out.png>              창 없이 그려서 PNG 로 저장 (시험용)
  --size=<W>x<H>                     --render-to 크기 (기본 1280x800)
  --zoom=<배율>                      --render-to 에서 배율 지정 (기본 화면 맞춤)
  -h, --help";

/// 렌더러 시작 단계 — auto 는 GPU 부터, 실패하면 아래 단계로 내려간다
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RendererLevel {
    Gpu,
    Fallback,
    Cpu,
}

#[derive(Clone, Debug)]
pub struct Args {
    pub path: Option<PathBuf>,
    pub renderer: RendererLevel,
    pub render_to: Option<PathBuf>,
    pub size: [u32; 2],
    pub zoom: Option<f32>,
    pub help: bool,
}

impl Args {
    pub fn parse(it: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut a = Args { path: None, renderer: RendererLevel::Gpu, render_to: None, size: [1280, 800], zoom: None, help: false };
        for s in it {
            if s == "-h" || s == "--help" {
                a.help = true;
            } else if let Some(v) = s.strip_prefix("--renderer=") {
                a.renderer = match v {
                    "auto" | "gpu" => RendererLevel::Gpu,
                    "fallback" => RendererLevel::Fallback,
                    "cpu" => RendererLevel::Cpu,
                    _ => return Err(format!("알 수 없는 렌더러: {v}")),
                };
            } else if let Some(v) = s.strip_prefix("--render-to=") {
                a.render_to = Some(PathBuf::from(v));
            } else if let Some(v) = s.strip_prefix("--size=") {
                let (w, h) = v.split_once('x').ok_or("--size=WxH")?;
                a.size = [w.parse().map_err(|_| "--size 너비")?, h.parse().map_err(|_| "--size 높이")?];
            } else if let Some(v) = s.strip_prefix("--zoom=") {
                a.zoom = Some(v.parse().map_err(|_| "--zoom 숫자")?);
            } else if s.starts_with("--") {
                return Err(format!("알 수 없는 옵션: {s}"));
            } else {
                a.path = Some(PathBuf::from(s));
            }
        }
        Ok(a)
    }
}
