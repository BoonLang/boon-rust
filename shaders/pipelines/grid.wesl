import package::common::fullscreen::{VertexOut, fullscreen_vertex};
import package::common::palette::boon_color;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    return fullscreen_vertex(vertex_index);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return boon_color(3.0, in.uv);
}
