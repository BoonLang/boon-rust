fn boon_color(seed: f32, uv: vec2<f32>) -> vec4<f32> {
    let base = vec3<f32>(
        fract(seed * 0.173 + uv.x * 0.25),
        fract(seed * 0.317 + uv.y * 0.20),
        fract(seed * 0.419 + (uv.x + uv.y) * 0.15),
    );
    return vec4<f32>(base * 0.65 + vec3<f32>(0.2, 0.25, 0.18), 1.0);
}
