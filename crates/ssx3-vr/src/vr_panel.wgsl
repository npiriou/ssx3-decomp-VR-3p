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
@group(0) @binding(2) var hud_image: texture_2d<f32>;

struct Panel {
    game_from_eye: mat4x4<f32>,
    clip: vec4<f32>,
    size_distance_height: vec4<f32>,
    world_valid: vec4<f32>,
};
@group(0) @binding(3) var<uniform> panel: Panel;

@fragment
fn fragment(input: VertexOut) -> @location(0) vec4<f32> {
    var world_color = vec3<f32>(0.0);
    if panel.world_valid.x > 0.5 {
        world_color = textureSample(game_image, game_sampler, input.uv).rgb;
    }
    let ndc = vec2<f32>(2.0 * input.uv.x - 1.0, 1.0 - 2.0 * input.uv.y);
    let ray_eye = vec3<f32>(
        (ndc.x - panel.clip.z) / panel.clip.x,
        (ndc.y - panel.clip.w) / panel.clip.y,
        1.0,
    );
    let origin = (panel.game_from_eye * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let ray = (panel.game_from_eye * vec4<f32>(ray_eye, 0.0)).xyz;
    if ray.z <= 0.001 {
        return vec4<f32>(world_color, 1.0);
    }
    let t = (panel.size_distance_height.z - origin.z) / ray.z;
    if t <= 0.0 {
        return vec4<f32>(world_color, 1.0);
    }
    let point = origin + t * ray;
    let hud_uv = vec2<f32>(
        0.5 + point.x / panel.size_distance_height.x,
        0.5 - (point.y - panel.size_distance_height.w) / panel.size_distance_height.y,
    );
    if any(hud_uv < vec2<f32>(0.0)) || any(hud_uv > vec2<f32>(1.0)) {
        return vec4<f32>(world_color, 1.0);
    }
    let hud = textureSample(hud_image, game_sampler, hud_uv);
    // Some original GS HUD draws mask alpha writes while changing RGB.
    let rgb_only = hud.a < 0.001 && any(hud.rgb > vec3<f32>(0.001));
    let coverage = select(hud.a, 1.0, rgb_only);
    return vec4<f32>(mix(world_color, hud.rgb, clamp(coverage, 0.0, 1.0)), 1.0);
}
