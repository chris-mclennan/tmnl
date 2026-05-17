// Tab-strip background. One quad covering the top `strip_h` pixels of
// the viewport, painted in `strip_color`. The cell pipeline draws on
// top of this in the same render pass so the strip ends up behind the
// (empty) grid cells above the actual content area.

struct Globals {
    viewport: vec2<f32>,   // window pixels
    strip_h: f32,           // pixels (height of the strip from top)
    _pad0: f32,
    strip_color: vec4<f32>, // sRGB straight (no premultiply)
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // 4-vertex triangle strip covering (0, 0) — (viewport.x, strip_h)
    // in pixel coords, mapped into NDC.
    //   0 ── 1
    //   │  ╱ │
    //   2 ── 3
    let xs = array<f32, 4>(0.0, g.viewport.x, 0.0, g.viewport.x);
    let ys = array<f32, 4>(0.0, 0.0, g.strip_h, g.strip_h);
    let x_px = xs[idx];
    let y_px = ys[idx];
    // Pixel → NDC: x = 2x/w - 1, y = 1 - 2y/h (y inverted).
    let ndc_x = 2.0 * x_px / g.viewport.x - 1.0;
    let ndc_y = 1.0 - 2.0 * y_px / g.viewport.y;
    var out: VsOut;
    out.pos = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(_in: VsOut) -> @location(0) vec4<f32> {
    return g.strip_color;
}
