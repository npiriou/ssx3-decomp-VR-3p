struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    var points = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    let p = points[index];
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

@group(0) @binding(0) var game_image: texture_2d<f32>;
@group(0) @binding(1) var game_sampler: sampler;

@fragment
fn fragment(input: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(game_image));
    let aspect = size.x / size.y;
    let fit = select(vec2<f32>(aspect, 1.0), vec2<f32>(1.0, 1.0 / aspect), aspect >= 1.0) * 0.92;
    let source_uv = (input.uv - vec2<f32>(0.5)) / fit + vec2<f32>(0.5);
    if (any(source_uv < vec2<f32>(0.0)) || any(source_uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let color = textureSample(game_image, game_sampler, source_uv);
    return vec4<f32>(color.rgb, 1.0);
}
