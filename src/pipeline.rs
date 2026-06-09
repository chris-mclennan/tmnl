use crate::atlas::{Atlas, style_from_attrs};
use crate::grid::Grid;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Default)]
pub struct Instance {
    pub cell_pos: [f32; 2],
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub glyph_offset: [f32; 2],
    pub glyph_size: [f32; 2],
    pub attrs: u32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Default)]
pub struct Globals {
    pub viewport: [f32; 2],
    pub cell_size: [f32; 2],
    /// Pixel offset added to every cell so the grid floats inside an
    /// equal-width margin (default 1 cell on every side). Lets the text
    /// breathe against the window's chrome.
    pub inset_px: [f32; 2],
    pub _pad: [f32; 2],
}

pub struct CellPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    pub globals_buf: wgpu::Buffer,
    pub instance_buf: wgpu::Buffer,
    pub instance_capacity: u64,
}

impl CellPipeline {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        atlas: &Atlas,
        initial_capacity: u64,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cell.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cell-bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas.sampler),
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cell-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let instance_attrs = [
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 24,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 40,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 48,
                shader_location: 4,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 56,
                shader_location: 5,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 64,
                shader_location: 6,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 72,
                shader_location: 7,
                format: wgpu::VertexFormat::Uint32,
            },
        ];

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attrs,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell-instances"),
            size: initial_capacity * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            globals_buf,
            instance_buf,
            instance_capacity: initial_capacity,
        }
    }

    pub fn write_globals(
        &self,
        queue: &wgpu::Queue,
        viewport: [f32; 2],
        cell: [f32; 2],
        inset_px: [f32; 2],
    ) {
        let g = Globals {
            viewport,
            cell_size: cell,
            inset_px,
            _pad: [0.0, 0.0],
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&g));
    }

    pub fn build_instances(grid: &Grid, atlas: &mut Atlas, queue: &wgpu::Queue) -> Vec<Instance> {
        let mut out = Vec::with_capacity(grid.cells.len());
        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let cell = grid.cells[(row * grid.cols + col) as usize];
                let style = style_from_attrs(cell.attrs);
                let g = atlas.glyph(cell.ch, style, queue);
                out.push(Instance {
                    cell_pos: [col as f32, row as f32],
                    fg: cell.fg,
                    bg: cell.bg,
                    uv_min: g.uv_min,
                    uv_max: g.uv_max,
                    glyph_offset: g.offset,
                    glyph_size: g.size,
                    attrs: cell.attrs,
                    _pad: 0,
                });
            }
        }
        out
    }

    pub fn ensure_capacity(&mut self, device: &wgpu::Device, count: u64) {
        if count <= self.instance_capacity {
            return;
        }
        let mut new_cap = self.instance_capacity.max(1);
        while new_cap < count {
            new_cap *= 2;
        }
        self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell-instances"),
            size: new_cap * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = new_cap;
    }

    pub fn upload(&self, queue: &wgpu::Queue, instances: &[Instance]) {
        if instances.is_empty() {
            return;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
    }
}

// ─── Strip pipeline ──────────────────────────────────────────────
//
// Single colored quad rendered over the top `strip_h` pixels of the
// viewport. Used to paint the tab-strip background — the row where
// the traffic-light buttons + tab chips live. Decoupled from the
// cell pipeline so the strip stays out of the per-cell instance
// math and the grid coordinate system. Drawn BEFORE the cell pass
// so empty cells above the body still show the strip color, not
// `CLEAR_BG`.

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Default)]
pub struct StripGlobals {
    pub viewport: [f32; 2],
    pub strip_h: f32,
    /// Pixel width of the left-edge tab sidebar (when
    /// `tab_layout = Vertical`). `0.0` ⇒ no sidebar — instances 1+2
    /// collapse to zero area.
    pub sidebar_w: f32,
    pub strip_color: [f32; 4],
    /// Color of the 1-px vertical border on the right edge of the
    /// tab sidebar AND the launcher rail. Slightly lighter than
    /// `strip_color` so each chrome region reads as distinct.
    pub border_color: [f32; 4],
    /// Pixel width of the left-edge launcher rail (when
    /// `launcher_icons` is non-empty). Sits left of the tab
    /// sidebar; the sidebar's x offset is `launcher_w` not `0`.
    /// `0.0` ⇒ no rail — instances 3+4 collapse to zero area.
    pub launcher_w: f32,
    /// Padding to 16-byte alignment — WGSL uniform buffers expect
    /// vec4 boundaries.
    pub _pad: [f32; 3],
}

pub struct StripPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    pub globals_buf: wgpu::Buffer,
}

impl StripPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("strip.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("strip.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strip-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strip-globals"),
            size: std::mem::size_of::<StripGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("strip-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strip-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("strip-pipeline"),
            layout: Some(&pl_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group,
            globals_buf,
        }
    }

    // Eight params is over clippy's default-7 threshold, but each
    // is load-bearing (viewport + the four chrome dimensions + two
    // colors + the rail width) and there's no natural grouping that
    // wouldn't just be a tuple-of-arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn write_globals(
        &self,
        queue: &wgpu::Queue,
        viewport: [f32; 2],
        strip_h: f32,
        sidebar_w: f32,
        strip_color: [f32; 4],
        border_color: [f32; 4],
        launcher_w: f32,
    ) {
        let g = StripGlobals {
            viewport,
            strip_h,
            sidebar_w,
            strip_color,
            border_color,
            launcher_w,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&g));
    }
}
