//! iv — 빠른 이미지 뷰어.
//!
//! 이 파일은 조립만 한다: 디코더 등록 → 파이프라인 → 뷰어 상태 → 렌더러(자동 폴백) → winit 이벤트 루프.
//! 기능 로직은 각 crate 에 있다.

mod args;
mod decoders;
mod headless;
mod input;
mod renderer;
mod selftest;
mod textdoc;
mod window_app;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
        .format_timestamp_millis()
        .init();
    let args = match args::Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    };
    if args.help {
        println!("{}", args::USAGE);
        return;
    }
    let registry = decoders::build_registry();
    log::info!("KaiIV {} 시작", env!("CARGO_PKG_VERSION"));
    log::info!("디코더: {:?}", registry.names());

    if let Some(out) = &args.render_to {
        std::process::exit(headless::run(&args, &registry, out));
    }
    window_app::run(args, registry);
}
