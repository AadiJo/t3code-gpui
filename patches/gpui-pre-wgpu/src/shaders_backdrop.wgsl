// Backdrop blur: CSS `backdrop-filter: blur() saturate()`.
//
// A region of the frame is copied out, box-downsampled, blurred with a
// separable Gaussian, then drawn back clipped to a rounded rectangle with its
// saturation scaled. Nothing outside the region is read: the blur mirrors the
// region at its edges, which matches Chromium more closely than clamping. `backdrop.rs` sets the parameters, and the
// Metal shader mirrors this file.

struct BackdropParams {
    // Top left, top right, bottom right, bottom left.
    corner_radii: vec4<f32>,
    viewport_size: vec2<f32>,
    // The copied region, in frame pixels. Its origin is whole.
    region_origin: vec2<f32>,
    region_size: vec2<f32>,
    // The downsampled region, in texels.
    down_size: vec2<f32>,
    // Where the result is drawn: the border box within the content mask.
    draw_origin: vec2<f32>,
    draw_size: vec2<f32>,
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    // The Gaussian's standard deviation and reach, in downsampled texels.
    sigma: f32,
    radius: i32,
    factor: u32,
    saturation: f32,
    opacity: f32,
    premultiplied_alpha: u32,
    pad: vec2<u32>,
}

@group(0) @binding(0) var<uniform> params: BackdropParams;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

// One triangle covering the viewport, which each pass sets to its target.
@vertex
fn vs_fullscreen(@builtin(vertex_index) vertex_id: u32) -> @builtin(position) vec4<f32> {
    let unit = vec2<f32>(f32((vertex_id << 1u) & 2u), f32(vertex_id & 2u));
    return vec4<f32>(unit * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

// Averages each factor x factor block of the copied region.
@fragment
fn fs_downsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let factor = i32(params.factor);
    let origin = vec2<i32>(position.xy) * factor;
    let last = vec2<i32>(params.region_size) - vec2<i32>(1);
    var sum = vec4<f32>(0.0);
    for (var y = 0; y < factor; y++) {
        for (var x = 0; x < factor; x++) {
            sum += textureLoad(source, min(origin + vec2<i32>(x, y), last), 0);
        }
    }
    return sum / f32(factor * factor);
}

// Reflects a texel index that falls outside 0..=last back inside.
fn mirror(index: vec2<i32>, last: vec2<i32>) -> vec2<i32> {
    let reflected = select(index, -index - vec2<i32>(1), index < vec2<i32>(0));
    return clamp(select(reflected, 2 * last - reflected + vec2<i32>(1), reflected > last),
                 vec2<i32>(0), last);
}

fn gaussian_blur(position: vec2<f32>, direction: vec2<i32>) -> vec4<f32> {
    let texel = vec2<i32>(position);
    let last = vec2<i32>(params.down_size) - vec2<i32>(1);
    if (params.radius == 0) {
        return textureLoad(source, min(texel, last), 0);
    }
    let exponent = -0.5 / (params.sigma * params.sigma);
    var sum = vec4<f32>(0.0);
    var weights = 0.0;
    for (var i = -params.radius; i <= params.radius; i++) {
        let weight = exp(f32(i * i) * exponent);
        sum += weight * textureLoad(source, mirror(texel + direction * i, last), 0);
        weights += weight;
    }
    return sum / weights;
}

@fragment
fn fs_blur_horizontal(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return gaussian_blur(position.xy, vec2<i32>(1, 0));
}

@fragment
fn fs_blur_vertical(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return gaussian_blur(position.xy, vec2<i32>(0, 1));
}

@vertex
fn vs_composite(@builtin(vertex_index) vertex_id: u32) -> @builtin(position) vec4<f32> {
    let unit = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let position = params.draw_origin + unit * params.draw_size;
    let device = position / params.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(device, 0.0, 1.0);
}

// The CSS `saturate()` matrix.
fn saturate_color(color: vec3<f32>, amount: f32) -> vec3<f32> {
    let r = vec3<f32>(0.213 + 0.787 * amount, 0.715 - 0.715 * amount, 0.072 - 0.072 * amount);
    let g = vec3<f32>(0.213 - 0.213 * amount, 0.715 + 0.285 * amount, 0.072 - 0.072 * amount);
    let b = vec3<f32>(0.213 - 0.213 * amount, 0.715 - 0.715 * amount, 0.072 + 0.928 * amount);
    return vec3<f32>(dot(r, color), dot(g, color), dot(b, color));
}

// How much of the pixel at `point` the rounded border box covers, with the
// same half-pixel antialiasing as quads.
fn rounded_rect_coverage(point: vec2<f32>) -> f32 {
    let half_size = params.bounds_size * 0.5;
    let center_to_point = point - (params.bounds_origin + half_size);
    var radius: f32;
    if (center_to_point.x < 0.0) {
        radius = select(params.corner_radii.w, params.corner_radii.x, center_to_point.y < 0.0);
    } else {
        radius = select(params.corner_radii.z, params.corner_radii.y, center_to_point.y < 0.0);
    }
    let corner_to_point = abs(center_to_point) - half_size + vec2<f32>(radius);
    let distance = min(max(corner_to_point.x, corner_to_point.y), 0.0)
        + length(max(corner_to_point, vec2<f32>(0.0))) - radius;
    return saturate(0.5 - distance);
}

@fragment
fn fs_composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = clamp((position.xy - params.region_origin) / f32(params.factor),
                      vec2<f32>(0.5), params.down_size - vec2<f32>(0.5));
    let blurred = textureSampleLevel(source, source_sampler, texel / vec2<f32>(textureDimensions(source)), 0.0);
    let color = saturate(saturate_color(blurred.rgb, params.saturation));
    let coverage = rounded_rect_coverage(position.xy) * params.opacity;
    if (params.premultiplied_alpha != 0u) {
        return vec4<f32>(color * coverage, coverage);
    }
    return vec4<f32>(color, coverage);
}
