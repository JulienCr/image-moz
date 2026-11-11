//! wgpu drawing of a `Scene`. Nothing here owns the device, so the same
//! types serve the offscreen thumbnail device and eframe's own.

use std::ops::Range;
use std::sync::mpsc;
use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use eframe::wgpu;
use image::RgbaImage;

use super::camera::OrbitCamera;
use super::{Aabb, DEFAULT_COLOR, Material, Scene, Vertex};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const GAMMA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLE_COUNT: u32 = 4;
const READBACK_TIMEOUT: Duration = Duration::from_secs(60);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    color: [f32; 4],
}

pub struct ModelRenderer {
    pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn uniform_binding() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

impl ModelRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model globals layout"),
            entries: &[layout_entry(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                uniform_binding(),
            )],
        });
        let frag = wgpu::ShaderStages::FRAGMENT;
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model material layout"),
            entries: &[
                layout_entry(0, frag, uniform_binding()),
                layout_entry(
                    1,
                    frag,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    2,
                    frag,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x2,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("model sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            globals_layout,
            material_layout,
            sampler,
        }
    }

    /// Draws `scene` into `target` (cleared to transparent) and submits the work.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &RenderTarget,
        scene: &GpuScene,
        camera: &OrbitCamera,
        bounds: &Aabb,
    ) {
        let [w, h] = target.size();
        let eye = camera.eye();
        let globals = Globals {
            view_proj: camera
                .view_proj(w as f32 / h as f32, bounds)
                .to_cols_array_2d(),
            eye: [eye.x, eye.y, eye.z, 1.0],
        };
        queue.write_buffer(&scene.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("model render"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&target.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !scene.draws.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &scene.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, scene.vertex_buffer.slice(..));
                pass.set_index_buffer(scene.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for draw in &scene.draws {
                    pass.set_bind_group(1, &scene.materials[draw.material], &[]);
                    pass.draw_indexed(draw.indices.clone(), 0, 0..1);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
}

struct Draw {
    indices: Range<u32>,
    material: usize,
}

/// Scene geometry, materials and per-scene uniforms living on one device.
pub struct GpuScene {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    materials: Vec<wgpu::BindGroup>,
    draws: Vec<Draw>,
}

fn buffer_with_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    usage: wgpu::BufferUsages,
    data: &[u8],
) -> wgpu::Buffer {
    // write_buffer instead of mapped_at_creation: an over-limit size then
    // surfaces as a device error rather than a panic on an invalid buffer.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: data.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, data);
    buffer
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &RgbaImage,
) -> wgpu::TextureView {
    let (width, height) = image.dimensions();
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl GpuScene {
    pub fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &ModelRenderer,
        scene: &Scene,
    ) -> Self {
        use wgpu::BufferUsages as Usage;
        let vertex_buffer = buffer_with_data(
            device,
            queue,
            "model vertices",
            Usage::VERTEX,
            bytemuck::cast_slice(&scene.vertices),
        );
        let index_buffer = buffer_with_data(
            device,
            queue,
            "model indices",
            Usage::INDEX,
            bytemuck::cast_slice(&scene.indices),
        );
        let globals_buffer = buffer_with_data(
            device,
            queue,
            "model globals",
            Usage::UNIFORM,
            bytemuck::bytes_of(&Globals::zeroed()),
        );
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model globals"),
            layout: &renderer.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let white = RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]));
        let white_view = upload_texture(device, queue, &white);
        let views: Vec<Option<wgpu::TextureView>> = scene
            .textures
            .iter()
            .map(|img| {
                let empty = img.width() == 0 || img.height() == 0;
                (!empty).then(|| upload_texture(device, queue, img))
            })
            .collect();
        let fallback = [Material {
            base_color: DEFAULT_COLOR,
            texture: None,
        }];
        let sources = if scene.materials.is_empty() {
            &fallback[..]
        } else {
            &scene.materials[..]
        };
        let materials: Vec<wgpu::BindGroup> = sources
            .iter()
            .map(|m| {
                let uniform = buffer_with_data(
                    device,
                    queue,
                    "model material",
                    Usage::UNIFORM,
                    bytemuck::bytes_of(&MaterialUniform {
                        color: m.base_color,
                    }),
                );
                let view = m
                    .texture
                    .and_then(|i| views.get(i)?.as_ref())
                    .unwrap_or(&white_view);
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("model material"),
                    layout: &renderer.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&renderer.sampler),
                        },
                    ],
                })
            })
            .collect();

        let index_count = scene.indices.len() as u32;
        let draws = if scene.vertices.is_empty() {
            Vec::new()
        } else {
            scene
                .primitives
                .iter()
                .filter(|p| p.indices.start < p.indices.end && p.indices.end <= index_count)
                .map(|p| Draw {
                    indices: p.indices.clone(),
                    material: p.material.min(materials.len() - 1),
                })
                .collect()
        };
        Self {
            vertex_buffer,
            index_buffer,
            globals_buffer,
            globals_bind_group,
            materials,
            draws,
        }
    }
}

/// Multisampled color and depth plus the resolved texture callers read or sample.
pub struct RenderTarget {
    size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
}

impl RenderTarget {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make = |label, format, sample_count, usage, view_formats: &[wgpu::TextureFormat]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa = make("model msaa color", COLOR_FORMAT, SAMPLE_COUNT, attachment, &[]);
        let depth = make("model depth", DEPTH_FORMAT, SAMPLE_COUNT, attachment, &[]);
        let resolve = make(
            "model resolved color",
            COLOR_FORMAT,
            1,
            attachment | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            &[GAMMA_FORMAT],
        );
        Self {
            size,
            msaa_view: view(&msaa),
            depth_view: view(&depth),
            resolve_view: view(&resolve),
            resolve,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The resolved single-sample texture.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.resolve
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.resolve_view
    }

    /// The resolved texture read as raw sRGB bytes, the form egui samples user textures in.
    pub fn gamma_view(&self) -> wgpu::TextureView {
        self.resolve.create_view(&wgpu::TextureViewDescriptor {
            format: Some(GAMMA_FORMAT),
            ..Default::default()
        })
    }
}

/// Copies a `COLOR_FORMAT` texture (needs `COPY_SRC`) to the CPU, blocking on the GPU.
pub fn try_read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<RgbaImage, String> {
    if texture.format() != COLOR_FORMAT || !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
    {
        return Err("texture cannot be read back".into());
    }
    let (width, height) = (texture.width(), texture.height());
    let row_bytes = 4 * width;
    let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model readback"),
        size: padded_row_bytes as u64 * height as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("model readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit([encoder.finish()]);

    let (tx, rx) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(READBACK_TIMEOUT),
        })
        .map_err(|e| format!("waiting for the GPU failed: {e}"))?;
    // Another thread's poll may be the one that runs our callback, so block on the channel.
    rx.recv_timeout(READBACK_TIMEOUT)
        .map_err(|e| format!("readback never completed: {e}"))?
        .map_err(|e| format!("mapping the readback buffer failed: {e}"))?;

    let mapped = buffer.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
    for row in mapped.chunks_exact(padded_row_bytes as usize) {
        pixels.extend_from_slice(&row[..row_bytes as usize]);
    }
    drop(mapped);
    buffer.unmap();
    RgbaImage::from_raw(width, height, pixels).ok_or_else(|| "readback size mismatch".into())
}
