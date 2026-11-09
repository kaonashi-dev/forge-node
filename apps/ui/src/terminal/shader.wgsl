struct Params {
    origin: vec2f,
    frame: vec2f,
    atlas: vec2f,
    srgb_blend: f32,
    pad: f32,
}
var<immediate> p: Params;

@group(0) @binding(0) var atlas: texture_2d<f32>;

struct Out {
    @builtin(position) pos: vec4f,
    @location(0) color: vec4f,
    @location(1) local: vec2f,
    @location(2) @interpolate(flat) uv: vec4u,
}

fn decode(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

@vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) xy: vec2f,
    @location(1) wh: vec2f,
    @location(2) uv: vec4u,
    @location(3) color: u32,
) -> Out {
    let corner = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let px = p.origin + xy + corner * wh;
    let ndc = px / p.frame * 2.0 - 1.0;
    var c = unpack4x8unorm(color);
    if (p.srgb_blend < 0.5) { c = vec4f(decode(c.rgb), c.a); }
    var o: Out;
    o.pos = vec4f(ndc.x, -ndc.y, 0.0, 1.0);
    o.color = c;
    o.local = corner * wh;
    o.uv = uv;
    return o;
}

@fragment fn fs(v: Out) -> @location(0) vec4f {
    var a = v.color.a;
    if (v.uv.z != 0u) {
        let t = vec2i(vec2u(v.uv.xy)) + min(vec2i(floor(v.local)), vec2i(vec2u(v.uv.zw)) - 1);
        a = a * textureLoad(atlas, t, 0).r;
    }
    return vec4f(v.color.rgb * a, a);
}
