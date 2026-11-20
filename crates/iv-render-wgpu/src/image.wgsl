// 이미지 사각형 그리기: 위치는 NDC 사각형, UV 는 EXIF 방향 변환을 적용해 샘플
struct U {
    rect: vec4<f32>,   // x0, y0, x1, y1 (NDC)
    uvm: vec4<f32>,    // a, b, c, d
    uvt: vec4<f32>,    // tx, ty, -, -
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VO {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VO {
    let c = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var o: VO;
    o.pos = vec4<f32>(mix(u.rect.x, u.rect.z, c.x), mix(u.rect.y, u.rect.w, c.y), 0.0, 1.0);
    o.uv = c;
    return o;
}

@fragment
fn fs(in: VO) -> @location(0) vec4<f32> {
    let su = u.uvm.x * in.uv.x + u.uvm.y * in.uv.y + u.uvt.x;
    let sv = u.uvm.z * in.uv.x + u.uvm.w * in.uv.y + u.uvt.y;
    return textureSample(tex, samp, vec2<f32>(su, sv));
}
