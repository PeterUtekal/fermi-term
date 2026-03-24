//! GPU-accelerated terminal renderer using wgpu.
//!
//! # Rendering approach
//!
//! Each terminal cell is rendered as a textured quad in a single instanced
//! draw call. The pipeline consists of two passes per frame:
//!
//! 1. **Background pass** — fills every cell rect with its background colour.
//!    Done as solid-colour quads (no texture lookup needed).
//! 2. **Glyph pass** — blends the glyph alpha mask from the atlas texture
//!    over the background using the foreground colour.
//!
//! In practice both passes are batched into one draw call by encoding the
//! background colour into the instance data and handling it in the shader.
//!
//! # Glyph atlas
//!
//! fontdue rasterises glyphs into an R8Unorm CPU-side buffer. Each new
//! glyph is packed into a 2048×2048 texture (row-major bin packing).
//! The atlas is uploaded lazily and dirtied when new glyphs are added.

use std::sync::Arc;

use fontdue::{Font, FontSettings};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::config::Config;
use crate::terminal::Grid;

const ATLAS_W: u32 = 2048;
const ATLAS_H: u32 = 2048;

const SHADER: &str = r#"
struct Uniforms {
    screen_w: f32,
    screen_h: f32,
    cell_w: f32,
    cell_h: f32,
}

struct Instance {
    @location(0) col: f32,
    @location(1) row: f32,
    @location(2) u0: f32,
    @location(3) v0: f32,
    @location(4) u1: f32,
    @location(5) v1: f32,
    @location(6) fg_r: f32,
    @location(7) fg_g: f32,
    @location(8) fg_b: f32,
    @location(9) bg_r: f32,
    @location(10) bg_g: f32,
    @location(11) bg_b: f32,
    @location(12) glyph_x: f32,
    @location(13) glyph_y: f32,
    @location(14) glyph_w: f32,
    @location(15) glyph_h: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec3<f32>,
    @location(2) bg: vec3<f32>,
    @location(3) has_glyph: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // Unit quad: vi 0=(0,0) 1=(1,0) 2=(0,1) 3=(1,1)
    let qx = f32(vi & 1u);
    let qy = f32((vi >> 1u) & 1u);

    // Cell pixel rect
    let cx = inst.col * u.cell_w;
    let cy = inst.row * u.cell_h;
    let px = cx + qx * u.cell_w;
    let py = cy + qy * u.cell_h;

    // NDC (y flipped: screen top = NDC +1)
    let ndcx = px / u.screen_w * 2.0 - 1.0;
    let ndcy = 1.0 - py / u.screen_h * 2.0;

    // UV interpolates across the glyph rect inside the atlas
    let uv = vec2(mix(inst.u0, inst.u1, qx), mix(inst.v0, inst.v1, qy));

    var out: VsOut;
    out.pos = vec4(ndcx, ndcy, 0.0, 1.0);
    out.uv = uv;
    out.fg = vec3(inst.fg_r, inst.fg_g, inst.fg_b);
    out.bg = vec3(inst.bg_r, inst.bg_g, inst.bg_b);
    out.has_glyph = select(0.0, 1.0, inst.u0 != inst.u1 || inst.v0 != inst.v1);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if in.has_glyph < 0.5 {
        return vec4(in.bg, 1.0);
    }
    let alpha = textureSample(atlas, atlas_sampler, in.uv).r;
    let color = mix(in.bg, in.fg, alpha);
    return vec4(color, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    col: f32,
    row: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    fg: [f32; 3],
    bg: [f32; 3],
    glyph_x: f32,
    glyph_y: f32,
    glyph_w: f32,
    glyph_h: f32,
}

#[derive(Clone, Copy)]
struct GlyphInfo {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    #[allow(dead_code)]
    offset_x: f32,
    #[allow(dead_code)]
    offset_y: f32,
    #[allow(dead_code)]
    width: f32,
    #[allow(dead_code)]
    height: f32,
}

pub struct Renderer {
    pub cell_w: usize,
    pub cell_h: usize,
    _window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_cpu: Vec<u8>,
    atlas_cursor_x: u32,
    atlas_cursor_y: u32,
    atlas_row_h: u32,
    atlas_dirty: bool,
    glyph_cache: std::collections::HashMap<char, GlyphInfo>,
    font: fontdue::Font,
    font_size: f32,
    ascent: f32,
    bg_color: [f32; 3],
}

impl Renderer {
    pub async fn new(window: Arc<Window>, config: &Config) -> Self {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // ── wgpu init ──────────────────────────────────────────────────
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Safety: surface must not outlive window. We keep Arc<Window> alive in struct.
        let surface = unsafe {
            let surf = instance
                .create_surface(window.as_ref())
                .expect("Failed to create surface");
            std::mem::transmute::<wgpu::Surface<'_>, wgpu::Surface<'static>>(surf)
        };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find wgpu adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("fermi-term device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("Failed to create wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // ── Font ───────────────────────────────────────────────────────
        let font = Self::load_font();
        let font_size = config.font_size;

        let line_metrics = font.horizontal_line_metrics(font_size);
        let ascent = line_metrics.map(|lm| lm.ascent).unwrap_or(font_size * 0.8);

        let (m_metrics, _) = font.rasterize('M', font_size);
        let cell_w = m_metrics.advance_width.ceil() as usize;
        let cell_h = if let Some(lm) = line_metrics {
            (lm.ascent - lm.descent + lm.line_gap).ceil() as usize
        } else {
            (font_size * 1.2).ceil() as usize
        };
        let cell_w = cell_w.max(8);
        let cell_h = cell_h.max(14);

        // ── Atlas texture ──────────────────────────────────────────────
        let atlas_cpu = vec![0u8; (ATLAS_W * ATLAS_H) as usize];

        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // ── Uniform buffer ─────────────────────────────────────────────
        let uniform_data: [f32; 4] = [
            width as f32,
            height as f32,
            cell_w as f32,
            cell_h as f32,
        ];
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniforms"),
            contents: bytemuck::cast_slice(&uniform_data),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // ── Bind group layouts ─────────────────────────────────────────
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bg"),
            layout: &atlas_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        // ── Shader & pipeline ──────────────────────────────────────────
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fermi shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        // Instance buffer attributes — 16 f32 fields
        let instance_attribs: Vec<wgpu::VertexAttribute> = (0..16u32)
            .map(|i| wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: (i * 4) as u64,
                shader_location: i,
            })
            .collect();

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fermi pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CellInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attribs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let bg = config.bg;
        let bg_color = [bg[0] as f32 / 255.0, bg[1] as f32 / 255.0, bg[2] as f32 / 255.0];

        let mut renderer = Self {
            cell_w,
            cell_h,
            _window: window,
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            uniform_buf,
            uniform_bind_group,
            atlas_texture,
            atlas_bind_group,
            atlas_bind_group_layout: atlas_bgl,
            atlas_cpu,
            atlas_cursor_x: 0,
            atlas_cursor_y: 0,
            atlas_row_h: 0,
            atlas_dirty: false,
            glyph_cache: std::collections::HashMap::new(),
            font,
            font_size,
            ascent,
            bg_color,
        };

        // Pre-rasterise ASCII printable chars
        for c in ' '..='~' {
            renderer.ensure_glyph(c);
        }
        if renderer.atlas_dirty {
            renderer.upload_atlas();
        }

        // Rebuild atlas bind group after upload (texture view stays the same, no rebuild needed)

        renderer
    }

    fn load_font() -> Font {
        let font_paths: &[&str] = &[
            "/System/Library/Fonts/Menlo.ttc",
            "/Library/Fonts/Courier New.ttf",
            "/System/Library/Fonts/Monaco.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
        ];

        for path in font_paths {
            if let Ok(bytes) = std::fs::read(path) {
                let settings = FontSettings {
                    collection_index: 0,
                    scale: 40.0,
                    ..FontSettings::default()
                };
                if let Ok(font) = Font::from_bytes(bytes.as_slice(), settings) {
                    eprintln!("[fermi-term] Loaded font: {}", path);
                    return font;
                }
            }
        }
        panic!(
            "fermi-term: Could not load any font. Tried:\n{}\n\
             Please install a monospace font (e.g. DejaVu Sans Mono on Linux).",
            font_paths.join("\n")
        );
    }

    fn ensure_glyph(&mut self, c: char) -> GlyphInfo {
        if let Some(&info) = self.glyph_cache.get(&c) {
            return info;
        }

        let (metrics, bitmap) = self.font.rasterize(c, self.font_size);

        if bitmap.is_empty() || metrics.width == 0 || metrics.height == 0 {
            let info = GlyphInfo {
                u0: 0.0,
                v0: 0.0,
                u1: 0.0,
                v1: 0.0,
                offset_x: 0.0,
                offset_y: 0.0,
                width: 0.0,
                height: 0.0,
            };
            self.glyph_cache.insert(c, info);
            return info;
        }

        let gw = metrics.width as u32;
        let gh = metrics.height as u32;

        // Bin-packing: advance cursor, wrap row
        if self.atlas_cursor_x + gw > ATLAS_W {
            self.atlas_cursor_x = 0;
            self.atlas_cursor_y += self.atlas_row_h + 1;
            self.atlas_row_h = 0;
        }

        if self.atlas_cursor_y + gh > ATLAS_H {
            // Atlas full — return empty (shouldn't happen with ASCII + sane font sizes)
            let info = GlyphInfo {
                u0: 0.0,
                v0: 0.0,
                u1: 0.0,
                v1: 0.0,
                offset_x: 0.0,
                offset_y: 0.0,
                width: 0.0,
                height: 0.0,
            };
            self.glyph_cache.insert(c, info);
            return info;
        }

        // Copy bitmap into atlas CPU buffer
        let ax = self.atlas_cursor_x;
        let ay = self.atlas_cursor_y;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let atlas_idx = ((ay + row as u32) * ATLAS_W + (ax + col as u32)) as usize;
                let bm_idx = row * metrics.width + col;
                if atlas_idx < self.atlas_cpu.len() {
                    self.atlas_cpu[atlas_idx] = bitmap[bm_idx];
                }
            }
        }

        self.atlas_row_h = self.atlas_row_h.max(gh);
        self.atlas_cursor_x += gw + 1;
        self.atlas_dirty = true;

        let u0 = ax as f32 / ATLAS_W as f32;
        let v0 = ay as f32 / ATLAS_H as f32;
        let u1 = (ax + gw) as f32 / ATLAS_W as f32;
        let v1 = (ay + gh) as f32 / ATLAS_H as f32;

        let info = GlyphInfo {
            u0,
            v0,
            u1,
            v1,
            offset_x: metrics.xmin as f32,
            offset_y: self.ascent - metrics.ymin as f32 - metrics.height as f32,
            width: metrics.width as f32,
            height: metrics.height as f32,
        };
        self.glyph_cache.insert(c, info);
        info
    }

    fn upload_atlas(&mut self) {
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.atlas_cpu,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W),
                rows_per_image: Some(ATLAS_H),
            },
            wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
        );
        self.atlas_dirty = false;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[
                width as f32,
                height as f32,
                self.cell_w as f32,
                self.cell_h as f32,
            ]),
        );
    }

    pub fn render(&mut self, grid: &Grid) {
        // Upload atlas if dirty
        if self.atlas_dirty {
            self.upload_atlas();
        }

        let visible_start = grid.visible_start();
        let rows_to_render = grid.rows.min(grid.cells.len().saturating_sub(visible_start));

        // Build instance list
        let mut instances: Vec<CellInstance> = Vec::with_capacity(grid.rows * grid.cols);

        for row in 0..rows_to_render {
            let abs_row = visible_start + row;
            if abs_row >= grid.cells.len() {
                break;
            }
            let cell_row = &grid.cells[abs_row];
            for col in 0..grid.cols {
                if col >= cell_row.len() {
                    break;
                }
                let cell = &cell_row[col];

                // Cursor: invert fg/bg
                let (fg, bg) = if row == grid.cursor_y && col == grid.cursor_x && grid.scroll_offset == 0 {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };

                let glyph = self.ensure_glyph(cell.c);

                instances.push(CellInstance {
                    col: col as f32,
                    row: row as f32,
                    u0: glyph.u0,
                    v0: glyph.v0,
                    u1: glyph.u1,
                    v1: glyph.v1,
                    fg: [fg[0] as f32 / 255.0, fg[1] as f32 / 255.0, fg[2] as f32 / 255.0],
                    bg: [bg[0] as f32 / 255.0, bg[1] as f32 / 255.0, bg[2] as f32 / 255.0],
                    glyph_x: 0.0,
                    glyph_y: 0.0,
                    glyph_w: glyph.width,
                    glyph_h: glyph.height,
                });
            }
        }

        // Upload atlas again if ensure_glyph dirtied it
        if self.atlas_dirty {
            self.upload_atlas();
        }

        if instances.is_empty() {
            return;
        }

        let instance_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instance buf"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            Err(e) => {
                eprintln!("[fermi-term] Surface error: {e}");
                return;
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render encoder"),
            });

        {
            let bg = self.bg_color;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            pass.set_vertex_buffer(0, instance_buf.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}
