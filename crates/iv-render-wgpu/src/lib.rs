//! wgpu 렌더러.
//!
//! - 이미지: sRGB 텍스처 + 밉맵(직접 blit), 확대·이동·EXIF 방향은 uniform 만 바꿔 재업로드 없음
//! - UI: egui-wgpu 로 같은 패스 위에 오버레이
//! - 어댑터: 하드웨어 → (요청 시) 폴백 어댑터(WARP/llvmpipe)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iv_core::Frame;
use iv_render::{EguiPaint, RenderOutcome, Renderer, Scene, UiCapability, texture_magnification};
use iv_viewport::Viewport;

/// 어댑터 종류
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterMode {
    Hardware,
    /// `force_fallback_adapter` — 소프트웨어 래스터 어댑터
    Fallback,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    rect: [f32; 4],
    uvm: [f32; 4],
    uvt: [f32; 4],
}

struct GpuImage {
    key: u64,
    /// 같은 키라도 다른 해상도 프레임이면 다시 올린다
    frame_ptr: *const u8,
    bind_linear: wgpu::BindGroup,
    bind_nearest: wgpu::BindGroup,
    orientation: iv_core::Orientation,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 표면 기본 포맷 (비-sRGB) — egui 가 그린다
    format: wgpu::TextureFormat,
    /// 같은 표면의 sRGB 뷰 포맷 — 이미지가 그린다 (선형 공간 필터링·블렌딩)
    srgb_format: wgpu::TextureFormat,
    image_pipeline: wgpu::RenderPipeline,
    image_bgl: wgpu::BindGroupLayout,
    mip_pipeline: wgpu::RenderPipeline,
    mip_bgl: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    egui: egui_wgpu::Renderer,
    image: Option<GpuImage>,
    max_side: u32,
    /// 잡히지 않은 wgpu 오류·장치 손실 발생 — 다음 render 에서 DeviceLost 로 알린다
    broken: Arc<AtomicBool>,
}

pub struct WgpuRenderer {
    gpu: Gpu,
    surface: Option<(wgpu::Surface<'static>, wgpu::SurfaceConfiguration)>,
    size: [u32; 2],
    name: String,
}

fn adapter_name(info: &wgpu::AdapterInfo) -> String {
    let backend = format!("{:?}", info.backend);
    match info.device_type {
        wgpu::DeviceType::Cpu => format!("{backend} 소프트웨어 ({})", info.name),
        _ => format!("{backend} ({})", info.name),
    }
}

impl WgpuRenderer {
    /// 창 표면용 렌더러. `target` 은 winit 창 등 (Arc 로 'static 보장)
    pub fn new_surface(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: [u32; 2],
        mode: AdapterMode,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(target).map_err(|e| format!("표면 생성 실패: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: mode == AdapterMode::Fallback,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("어댑터 없음 ({mode:?}): {e}"))?;
        let info = adapter.get_info();
        let caps = surface.get_capabilities(&adapter);
        // egui 는 비-sRGB 표면을 원하고, 이미지는 sRGB 뷰로 그려야 정확하다 → 비-sRGB 표면 + sRGB 뷰
        // 구성 포맷은 반드시 caps.formats 안에서 고른다. 비-sRGB 가 없으면 sRGB 하나로 (egui 색만 약간 다름)
        let base = caps.formats.first().copied().ok_or("표면 포맷 없음")?;
        let (format, srgb_format) = match caps.formats.iter().copied().find(|f| !f.is_srgb()) {
            Some(f) => (f, f.add_srgb_suffix()),
            None => (base, base),
        };
        let gpu = Gpu::new(&adapter, format, srgb_format)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size[0].max(1),
            height: size[1].max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if srgb_format != format { vec![srgb_format] } else { vec![] },
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&gpu.device, &config);
        log::info!("렌더러: {} / 표면 {format:?}", adapter_name(&info));
        Ok(Self { gpu, surface: Some((surface, config)), size, name: adapter_name(&info) })
    }

    /// 창 없는 렌더러 (--render-to, 시험)
    pub fn new_offscreen(size: [u32; 2], mode: AdapterMode) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: mode == AdapterMode::Fallback,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| format!("어댑터 없음 ({mode:?}): {e}"))?;
        let info = adapter.get_info();
        let gpu = Gpu::new(&adapter, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Rgba8UnormSrgb)?;
        Ok(Self { gpu, surface: None, size, name: adapter_name(&info) })
    }
}

impl Gpu {
    fn new(adapter: &wgpu::Adapter, format: wgpu::TextureFormat, srgb_format: wgpu::TextureFormat) -> Result<Self, String> {
        let limits = wgpu::Limits::default().using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("iv"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| format!("장치 생성 실패: {e}"))?;
        let max_side = device.limits().max_texture_dimension_2d;
        // 기본 동작(패닉) 대신 기록 → 앱이 다음 단계 렌더러로 폴백
        let broken = Arc::new(AtomicBool::new(false));
        let b = broken.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            log::error!("wgpu 오류: {e}");
            b.store(true, Ordering::SeqCst);
        }));
        let b = broken.clone();
        device.set_device_lost_callback(move |reason, msg| {
            log::error!("wgpu 장치 손실 {reason:?}: {msg}");
            b.store(true, Ordering::SeqCst);
        });

        let image_shader = device.create_shader_module(wgpu::include_wgsl!("image.wgsl"));
        let mip_shader = device.create_shader_module(wgpu::include_wgsl!("mip.wgsl"));

        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let samp_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let image_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                samp_entry(2),
            ],
        });
        let mip_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
            entries: &[tex_entry(0), samp_entry(1)],
        });

        let pipeline = |label, shader: &wgpu::ShaderModule, bgl: &wgpu::BindGroupLayout, fmt, topo, blend| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState { format: fmt, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { topology: topo, ..Default::default() },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let image_pipeline = pipeline(
            "image",
            &image_shader,
            &image_bgl,
            srgb_format,
            wgpu::PrimitiveTopology::TriangleStrip,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let mip_pipeline = pipeline(
            "mip",
            &mip_shader,
            &mip_bgl,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::PrimitiveTopology::TriangleList,
            None,
        );

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let egui = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        Ok(Self {
            device,
            queue,
            format,
            srgb_format,
            image_pipeline,
            image_bgl,
            mip_pipeline,
            mip_bgl,
            uniform,
            linear,
            nearest,
            egui,
            image: None,
            max_side,
            broken,
        })
    }

    /// 프레임을 텍스처로 올리고 밉맵 생성
    fn upload(&mut self, key: u64, frame: &Arc<Frame>) {
        let ptr = frame.pixels.as_ptr();
        if self.image.as_ref().is_some_and(|i| i.key == key && i.frame_ptr == ptr) {
            return;
        }
        let t = std::time::Instant::now();
        // 한도를 넘는 프레임은 안전하게 축소 (정상 경로에선 디코드 hint 가 막아준다)
        let owned;
        let frame: &Frame = if frame.width.max(frame.height) > self.max_side {
            log::warn!("텍스처 한도 초과 {}x{} → {} 로 축소", frame.width, frame.height, self.max_side);
            owned = iv_imageops::downscale_rgba8(frame, self.max_side);
            &owned
        } else {
            frame
        };
        let (w, h) = (frame.width, frame.height);
        let levels = 32 - w.max(h).leading_zeros();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &frame.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(frame.stride as u32), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.generate_mips(&texture, levels);
        let view = texture.create_view(&Default::default());
        let bind = |sampler: &wgpu::Sampler| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("image"),
                layout: &self.image_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        self.image = Some(GpuImage {
            key,
            frame_ptr: ptr,
            bind_linear: bind(&self.linear),
            bind_nearest: bind(&self.nearest),
            orientation: frame.orientation,
        });
        log::debug!("업로드 {w}x{h} ({levels} 단계) {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    fn generate_mips(&self, texture: &wgpu::Texture, levels: u32) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        for level in 1..levels {
            let src = texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level - 1,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let dst = texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip"),
                layout: &self.mip_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.linear) },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    /// 그리지 못하는 프레임에서도 egui 텍스처 변경은 반드시 반영한다
    /// (글꼴 아틀라스 delta 를 잃으면 이후 "아직 없는 텍스처 갱신"으로 패닉)
    fn apply_ui_textures_only(&mut self, ui: Option<EguiPaint>) {
        if let Some(mut p) = ui {
            for (id, deltas) in &p.textures_delta.set {
                for delta in deltas {
                    self.egui.update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            for id in &p.textures_delta.free {
                self.egui.free_texture(id);
            }
            // 반영했음을 표시 (안 비우면 egui 0.37 디버그 빌드가 drop 에서 패닉)
            p.textures_delta.clear();
        }
    }

    /// 장면을 target 에 그린다
    fn draw(&mut self, target_tex: &wgpu::Texture, size: [u32; 2], scene: Scene<'_>) {
        let srgb_view = target_tex.create_view(&wgpu::TextureViewDescriptor { format: Some(self.srgb_format), ..Default::default() });
        let linear_view = target_tex.create_view(&wgpu::TextureViewDescriptor { format: Some(self.format), ..Default::default() });
        if let Some((key, frame)) = scene.image {
            self.upload(key, frame);
        } else {
            self.image = None;
        }
        let mut enc = self.device.create_command_encoder(&Default::default());

        // egui 텍스처·버퍼 준비
        let mut egui_jobs: Option<(Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor, Vec<egui::TextureId>)> = None;
        if let Some(EguiPaint { primitives, mut textures_delta, pixels_per_point }) = scene.ui {
            for (id, deltas) in &textures_delta.set {
                for delta in deltas {
                    self.egui.update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point };
            let extra = self.egui.update_buffers(&self.device, &self.queue, &mut enc, &primitives, &sd);
            if !extra.is_empty() {
                self.queue.submit(extra);
            }
            egui_jobs = Some((primitives, sd, textures_delta.free.iter().copied().collect()));
            textures_delta.clear(); // set 은 반영했고 free 는 그린 뒤 위에서 모은 목록으로 처리
        }

        let area = iv_render::clamp_area(scene.image_area, size);
        let draw_image = self.image.as_ref().zip(area).map(|(img, area)| {
            let origin = scene.image_area.map_or([0.0, 0.0], |a| [a[0], a[1]]);
            let u = uniforms(scene.viewport, size, img.orientation, origin);
            self.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
            // 텍스처 픽셀이 화면에서 3배 이상 커지면 픽셀을 또렷하게 (확대 검사용)
            let mag = scene.image.map(|(_, f)| texture_magnification(scene.viewport, f)).unwrap_or(1.0);
            (if mag >= 3.0 { &img.bind_nearest } else { &img.bind_linear }, area)
        });

        let [r, g, b] = scene.background;
        let srgb_to_linear = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        // sRGB 뷰에 그리므로 지우기 색은 선형 값
        let clear = wgpu::Color { r: srgb_to_linear(r), g: srgb_to_linear(g), b: srgb_to_linear(b), a: 1.0 };
        {
            // 1) 이미지 — sRGB 뷰
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("image"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &srgb_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            if let Some((bg, [x0, y0, x1, y1])) = draw_image {
                // 영역 밖으로 번지지 않게 (영역은 clamp_area 로 타깃 안임이 보장됨)
                pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..4, 0..1);
            }
        }
        if let Some((prims, sd, _)) = &egui_jobs {
            // 2) UI — 비-sRGB 뷰 (egui 가 감마 공간 출력을 기대)
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &linear_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui.render(&mut pass, prims, sd);
        }
        self.queue.submit([enc.finish()]);
        if let Some((_, _, free)) = egui_jobs {
            for id in free {
                self.egui.free_texture(&id);
            }
        }
    }
}

/// 뷰포트 → 셰이더 uniform (NDC 사각형 + 방향 UV 변환)
fn uniforms(v: &Viewport, size: [u32; 2], o: iv_core::Orientation, origin: [f32; 2]) -> Uniforms {
    let mut r = v.image_rect_on_screen();
    r[0] += origin[0];
    r[2] += origin[0];
    r[1] += origin[1];
    r[3] += origin[1];
    let (w, h) = (size[0] as f32, size[1] as f32);
    let [a, b, c, d, tx, ty] = o.uv_transform();
    Uniforms {
        rect: [r[0] / w * 2.0 - 1.0, 1.0 - r[1] / h * 2.0, r[2] / w * 2.0 - 1.0, 1.0 - r[3] / h * 2.0],
        uvm: [a, b, c, d],
        uvt: [tx, ty, 0.0, 0.0],
    }
}

impl Renderer for WgpuRenderer {
    fn name(&self) -> &str {
        &self.name
    }

    fn ui_capability(&self) -> UiCapability {
        UiCapability::Full
    }

    fn max_texture_side(&self) -> u32 {
        self.gpu.max_side
    }

    fn resize(&mut self, size: [u32; 2]) {
        self.size = [size[0].max(1), size[1].max(1)];
        if let Some((surface, config)) = &mut self.surface {
            config.width = self.size[0];
            config.height = self.size[1];
            surface.configure(&self.gpu.device, config);
        }
    }

    fn render(&mut self, scene: Scene<'_>) -> RenderOutcome {
        if self.gpu.broken.load(Ordering::SeqCst) {
            return RenderOutcome::DeviceLost("wgpu 오류/장치 손실".into());
        }
        let Some((surface, config)) = &self.surface else {
            self.gpu.apply_ui_textures_only(scene.ui);
            return RenderOutcome::Skipped;
        };
        let skip = |gpu: &mut Gpu, ui, outcome| {
            gpu.apply_ui_textures_only(ui);
            outcome
        };
        let tex = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // 이번엔 그리고 다음에 재구성
                self.gpu.draw(&t.texture, [config.width, config.height], scene);
                self.gpu.queue.present(t);
                return RenderOutcome::Reconfigure;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return skip(&mut self.gpu, scene.ui, RenderOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return skip(&mut self.gpu, scene.ui, RenderOutcome::Reconfigure);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return skip(&mut self.gpu, scene.ui, RenderOutcome::DeviceLost("표면 검증 오류".into()));
            }
        };
        let size = [config.width, config.height];
        self.gpu.draw(&tex.texture, size, scene);
        self.gpu.queue.present(tex);
        if self.gpu.broken.load(Ordering::SeqCst) {
            return RenderOutcome::DeviceLost("wgpu 오류/장치 손실".into());
        }
        RenderOutcome::Ok
    }

    fn capture(&mut self, scene: Scene<'_>) -> Option<(u32, u32, Vec<u8>)> {
        let [w, h] = self.size;
        let device = &self.gpu.device;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[self.gpu.srgb_format],
        });
        self.gpu.draw(&target, [w, h], scene);

        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let device = &self.gpu.device;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.gpu.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let bgra = matches!(self.gpu.format, wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let line = &data[y * row as usize..y * row as usize + w as usize * 4];
            if bgra {
                for px in line.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(line);
            }
        }
        Some((w, h, out))
    }
}
