struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
}

struct Material {
    color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world = in.position;
    out.normal = in.normal;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Zero-length normals give 0 rather than NaN.
    let n = in.normal / max(length(in.normal), 1e-6);
    let to_eye = normalize(globals.eye.xyz - in.world);
    // Headlight; abs() lights back faces too since culling is off.
    let light = 0.25 + 0.75 * abs(dot(n, to_eye));
    let albedo = material.color.rgb * textureSample(base_texture, base_sampler, in.uv).rgb;
    return vec4<f32>(albedo * light, 1.0);
}
