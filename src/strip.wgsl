// Chrome backgrounds. Two quads:
//   * instance 0 — the top strip (`[0,0]..[viewport.x, strip_h]`)
//   * instance 1 — the left sidebar (`[0, strip_h]..[sidebar_w, viewport.y]`)
// Both painted in `strip_color`. The cell pipeline draws on top in
// the same render pass so chrome ends up behind the (empty) grid
// cells above + left of the actual content area. When `sidebar_w`
// is `0.0` (horizontal layout mode), the sidebar quad collapses to
// zero area and emits no pixels.

struct Globals {
    viewport: vec2<f32>,    // window pixels
    strip_h: f32,            // pixels — top strip height
    sidebar_w: f32,          // pixels — left sidebar width (0 in horizontal mode)
    strip_color: vec4<f32>,  // sRGB straight (no premultiply)
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) idx: u32,
    @builtin(instance_index) inst: u32,
) -> VsOut {
    // 4-vertex triangle strip per quad. Instance 0 = top strip,
    // instance 1 = left sidebar.
    //   0 ── 1
    //   │  ╱ │
    //   2 ── 3
    var x0 = 0.0;
    var x1 = g.viewport.x;
    var y0 = 0.0;
    var y1 = g.strip_h;
    if (inst == 1u) {
        // Left sidebar — only paints when the strip's sidebar_w > 0.
        x0 = 0.0;
        x1 = g.sidebar_w;
        y0 = g.strip_h;
        y1 = g.viewport.y;
    }
    let xs = array<f32, 4>(x0, x1, x0, x1);
    let ys = array<f32, 4>(y0, y0, y1, y1);
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
