// 밉맵 한 단계 생성: 윗 단계를 선형 샘플해 절반 크기로 (sRGB 텍스처라 선형 공간 평균)
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VO {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VO {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    var o: VO;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return o;
}

@fragment
fn fs(in: VO) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
