// Chrome backgrounds. Three quads:
//   * instance 0 — top strip (`[0,0]..[viewport.x, strip_h]`)
//   * instance 1 — left sidebar
//                  (`[0, strip_h]..[sidebar_w, viewport.y]`)
//   * instance 2 — right-edge border of the sidebar
//                  (`[sidebar_w-1, strip_h]..[sidebar_w, viewport.y]`)
// Quads 0+1 painted in `strip_color`; quad 2 painted in
// `border_color` (slightly lighter than strip_color so the column
// reads as a distinct region). The cell pipeline draws on top in
// the same render pass so chrome ends up behind the (empty) grid
// cells above + left of the actual content area. When `sidebar_w`
// is `0.0` (horizontal layout mode), quads 1 + 2 collapse to zero
// area and emit no pixels.

struct Globals {
    viewport: vec2<f32>,     // window pixels
    strip_h: f32,            // pixels — top strip height
    sidebar_w: f32,          // pixels — left sidebar width (0 in horizontal mode)
    strip_color: vec4<f32>,  // sRGB straight (no premultiply)
    border_color: vec4<f32>, // sRGB straight; used by instance 2 only
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) inst_idx: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) idx: u32,
    @builtin(instance_index) inst: u32,
) -> VsOut {
    // 4-vertex triangle strip per quad.
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
    } else if (inst == 2u) {
        // 1-pixel border on the right edge of the sidebar — visible
        // separator between the sidebar column and the body grid.
        x0 = max(g.sidebar_w - 1.0, 0.0);
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
    out.inst_idx = inst;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (in.inst_idx == 2u) {
        return g.border_color;
    }
    return g.strip_color;
}
