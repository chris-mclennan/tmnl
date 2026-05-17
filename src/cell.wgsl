struct Globals {
    viewport: vec2<f32>,
    cell_size: vec2<f32>,
    inset_px: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) cell_pos: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
    @location(5) glyph_offset: vec2<f32>,
    @location(6) glyph_size: vec2<f32>,
    @location(7) attrs: u32,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) cell_local: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) uv_min: vec2<f32>,
    @location(4) uv_max: vec2<f32>,
    @location(5) glyph_offset: vec2<f32>,
    @location(6) glyph_size: vec2<f32>,
    @location(7) @interpolate(flat) attrs: u32,
};

const ATTR_DIM: u32 = 2u;
const ATTR_UNDERLINE: u32 = 8u;
const ATTR_REVERSED: u32 = 16u;
const ATTR_CROSSED_OUT: u32 = 32u;
const ATTR_CURSOR_BLOCK: u32 = 1u << 16u;
const ATTR_CURSOR_UNDERLINE: u32 = 1u << 17u;
const ATTR_CURSOR_BAR: u32 = 1u << 18u;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let corner = vec2<f32>(f32(in.vi & 1u), f32((in.vi >> 1u) & 1u));
    let pix = g.inset_px + in.cell_pos * g.cell_size + corner * g.cell_size;
    let clip = vec2<f32>(
        (pix.x / g.viewport.x) * 2.0 - 1.0,
        1.0 - (pix.y / g.viewport.y) * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.cell_local = corner * g.cell_size;
    out.fg = in.fg;
    out.bg = in.bg;
    out.uv_min = in.uv_min;
    out.uv_max = in.uv_max;
    out.glyph_offset = in.glyph_offset;
    out.glyph_size = in.glyph_size;
    out.attrs = in.attrs;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var fg = in.fg;
    var bg = in.bg;

    // Block cursor and Reverse both invert fg/bg across the cell. Underline
    // and bar cursors don't — they overlay a strip after the rest renders.
    if ((in.attrs & (ATTR_CURSOR_BLOCK | ATTR_REVERSED)) != 0u) {
        let tmp = fg;
        fg = bg;
        bg = tmp;
    }
    if ((in.attrs & ATTR_DIM) != 0u) {
        fg = vec4<f32>(fg.rgb * 0.65, fg.a);
    }

    var color = bg;
    if (in.glyph_size.x > 0.0 && in.glyph_size.y > 0.0) {
        let local = in.cell_local - in.glyph_offset;
        if (local.x >= 0.0 && local.y >= 0.0
            && local.x < in.glyph_size.x && local.y < in.glyph_size.y) {
            let t = local / in.glyph_size;
            let uv = mix(in.uv_min, in.uv_max, t);
            let sample = textureSample(atlas_tex, atlas_samp, uv);
            // For monochrome glyphs (RGB == 1,1,1) the .rgb factor is identity
            // so this collapses to the prior `mix(bg, fg, alpha)`. For color
            // glyphs (sbix / COLR), sample.rgb carries the glyph's own color
            // and we composite it over bg using sample.a as coverage.
            let glyph = vec4<f32>(sample.rgb * fg.rgb, sample.a);
            color = mix(bg, glyph, sample.a);
        }
    }

    if ((in.attrs & ATTR_UNDERLINE) != 0u && in.cell_local.y >= g.cell_size.y - 1.5) {
        color = fg;
    }
    if ((in.attrs & ATTR_CROSSED_OUT) != 0u) {
        let mid = g.cell_size.y * 0.5;
        if (in.cell_local.y >= mid - 0.5 && in.cell_local.y < mid + 0.5) {
            color = fg;
        }
    }

    // Replace-mode cursor — solid underline along the bottom of the cell.
    if ((in.attrs & ATTR_CURSOR_UNDERLINE) != 0u && in.cell_local.y >= g.cell_size.y - 2.5) {
        color = in.fg;
    }
    // Insert-mode cursor — vertical bar on the left edge of the cell.
    if ((in.attrs & ATTR_CURSOR_BAR) != 0u && in.cell_local.x < 2.0) {
        color = in.fg;
    }

    return color;
}
