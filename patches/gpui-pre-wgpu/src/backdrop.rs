//! Draws [`gpui::Backdrop`] regions, CSS `backdrop-filter: blur() saturate()`.
//!
//! The renderer ends its pass when it reaches a backdrop batch. For each
//! backdrop it copies the region beneath it out of the frame, averages it down
//! by a power of two, and blurs it with two Gaussian passes whose combined
//! spread matches the CSS standard deviation. It then resumes the frame and
//! draws the result over the region, clipped to the rounded border box. A
//! frame without backdrops creates nothing here.

use bytemuck::{Pod, Zeroable};
use gpui::{Backdrop, DevicePixels, Size};
use std::num::NonZeroU64;

const SHADER: &str = include_str!("shaders_backdrop.wgsl");

/// Intermediate textures grow in steps of this many pixels, so a region that
/// changes size by a few pixels each frame does not reallocate them.
const TEXTURE_SIZE_STEP: u32 = 128;

/// One backdrop's parameters, laid out as `BackdropParams` in `shaders_backdrop.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct BackdropParams {
    corner_radii: [f32; 4],
    viewport_size: [f32; 2],
    region_origin: [f32; 2],
    region_size: [f32; 2],
    down_size: [f32; 2],
    draw_origin: [f32; 2],
    draw_size: [f32; 2],
    bounds_origin: [f32; 2],
    bounds_size: [f32; 2],
    sigma: f32,
    radius: i32,
    factor: u32,
    saturation: f32,
    opacity: f32,
    premultiplied_alpha: u32,
    pad: [u32; 2],
}

/// How one backdrop is blurred: the region it reads and the blur in
/// downsampled texels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BackdropPlan {
    region_origin: [u32; 2],
    region_size: [u32; 2],
    down_size: [u32; 2],
    params: BackdropParams,
}

impl BackdropPlan {
    /// Plans `backdrop` in a frame of `viewport` pixels, or returns `None`
    /// when nothing of it is visible.
    pub(crate) fn new(
        backdrop: &Backdrop,
        viewport: Size<DevicePixels>,
        premultiplied_alpha: bool,
    ) -> Option<Self> {
        let viewport = [viewport.width.0 as f32, viewport.height.0 as f32];
        let sample = backdrop.sample_bounds();
        let left = sample.origin.x.0.floor().max(0.);
        let top = sample.origin.y.0.floor().max(0.);
        let right = (sample.origin.x.0 + sample.size.width.0).ceil().min(viewport[0]);
        let bottom = (sample.origin.y.0 + sample.size.height.0).ceil().min(viewport[1]);
        if right <= left || bottom <= top {
            return None;
        }
        let draw = backdrop.bounds.intersect(&backdrop.content_mask.bounds);
        let draw_left = draw.origin.x.0.max(0.);
        let draw_top = draw.origin.y.0.max(0.);
        let draw_right = (draw.origin.x.0 + draw.size.width.0).min(viewport[0]);
        let draw_bottom = (draw.origin.y.0 + draw.size.height.0).min(viewport[1]);
        if draw_right <= draw_left || draw_bottom <= draw_top {
            return None;
        }

        let region_size = [(right - left) as u32, (bottom - top) as u32];
        let (factor, sigma) = downsampled_gaussian(backdrop.blur_radius.0);
        let down_size = region_size.map(|size| size.div_ceil(factor));
        let corner_radii = backdrop.corner_radii;
        let params = BackdropParams {
            corner_radii: [
                corner_radii.top_left.0,
                corner_radii.top_right.0,
                corner_radii.bottom_right.0,
                corner_radii.bottom_left.0,
            ],
            viewport_size: viewport,
            region_origin: [left, top],
            region_size: region_size.map(|size| size as f32),
            down_size: down_size.map(|size| size as f32),
            draw_origin: [draw_left, draw_top],
            draw_size: [draw_right - draw_left, draw_bottom - draw_top],
            bounds_origin: [backdrop.bounds.origin.x.0, backdrop.bounds.origin.y.0],
            bounds_size: [backdrop.bounds.size.width.0, backdrop.bounds.size.height.0],
            sigma: sigma.max(1e-3),
            radius: if sigma > 0. { (3. * sigma).ceil() as i32 } else { 0 },
            factor,
            saturation: backdrop.saturation,
            opacity: backdrop.opacity,
            premultiplied_alpha: premultiplied_alpha as u32,
            pad: [0; 2],
        };
        Some(Self {
            region_origin: [left as u32, top as u32],
            region_size,
            down_size,
            params,
        })
    }
}

/// Splits a Gaussian of standard deviation `sigma` pixels into a box average
/// over `factor` pixels and a Gaussian in the averaged texels, returning both.
///
/// The box average, the Gaussian and the bilinear upsample add their
/// variances: `(f² - 1) / 12 + f²σ'² + f² / 6 = σ²`. A factor of at most a
/// third of `sigma` keeps the box well inside the Gaussian, where the result
/// differs from a full-resolution blur by less than one level in 255.
fn downsampled_gaussian(sigma: f32) -> (u32, f32) {
    if sigma <= 0. {
        return (1, 0.);
    }
    let factor = ((sigma / 3.).max(1.).log2().floor().exp2() as u32).clamp(1, 8);
    let f = factor as f32;
    let variance = sigma * sigma - (f * f - 1.) / 12. - if factor > 1 { f * f / 6. } else { 0. };
    (factor, (variance / (f * f)).max(0.25).sqrt())
}

/// A texture an intermediate image is drawn into or copied into.
struct Intermediate {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Intermediate {
    fn new(
        device: &wgpu::Device,
        label: &str,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }

    fn fits(&self, size: [u32; 2]) -> bool {
        self.texture.width() >= size[0] && self.texture.height() >= size[1]
    }

    fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
}

/// The textures one backdrop of a batch is blurred in, kept across frames:
/// the copied region, then the downsampled image in `first`, blurred across
/// into `second` and down back into `first`.
struct Slot {
    region: Intermediate,
    first: Intermediate,
    second: Intermediate,
    /// Bind groups reading `region`, `first` and `second`, rebuilt when a
    /// texture or the uniform buffer changes. The composite reads `first`.
    bind_groups: Option<[wgpu::BindGroup; 3]>,
}

struct Pipelines {
    format: wgpu::TextureFormat,
    premultiplied_alpha: bool,
    downsample: wgpu::RenderPipeline,
    blur_horizontal: wgpu::RenderPipeline,
    blur_vertical: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
}

/// The pipelines, textures and parameter buffer backdrops are drawn with,
/// created at the first backdrop and reused by every frame after it.
#[derive(Default)]
pub(crate) struct BackdropRenderer {
    state: Option<State>,
    /// The frame's parameters, staged for one buffer write.
    staged: Vec<u8>,
}

struct State {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipelines: Pipelines,
    uniforms: wgpu::Buffer,
    uniform_stride: u64,
    uniform_capacity: usize,
    slots: Vec<Slot>,
}

/// A batch of backdrops ready to encode: each plan with the slot it uses and
/// the index of its parameters in the frame's buffer.
pub(crate) struct PreparedBatch<'a> {
    plans: &'a [(usize, BackdropPlan)],
}

impl BackdropRenderer {
    /// Stages the parameters of every backdrop in the frame, in scene order,
    /// so each batch binds its own. Call once per frame that has backdrops.
    pub(crate) fn prepare_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        premultiplied_alpha: bool,
        depth_format: wgpu::TextureFormat,
        plans: &[Option<BackdropPlan>],
    ) {
        let state = self.state.get_or_insert_with(|| {
            State::new(device, format, premultiplied_alpha, depth_format)
        });
        if state.pipelines.format != format || state.pipelines.premultiplied_alpha != premultiplied_alpha {
            state.pipelines =
                Pipelines::new(device, &state.layout, format, premultiplied_alpha, depth_format);
        }
        if plans.len() > state.uniform_capacity {
            state.grow_uniforms(device, plans.len());
        }
        let stride = state.uniform_stride as usize;
        self.staged.clear();
        self.staged.resize(stride * plans.len(), 0);
        for (index, plan) in plans.iter().enumerate() {
            if let Some(plan) = plan {
                let bytes = bytemuck::bytes_of(&plan.params);
                self.staged[index * stride..index * stride + bytes.len()].copy_from_slice(bytes);
            }
        }
        queue.write_buffer(&state.uniforms, 0, &self.staged);
    }

    /// Copies, downsamples and blurs each backdrop of a batch. `plans` pairs
    /// each with its index among the frame's backdrops. Call between render
    /// passes, after everything beneath the batch was drawn into `target`.
    pub(crate) fn encode_blurs<'a>(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        plans: &'a [(usize, BackdropPlan)],
    ) -> PreparedBatch<'a> {
        let state = self
            .state
            .as_mut()
            .expect("prepare_frame runs before encode_blurs");
        let resources = SlotResources {
            layout: &state.layout,
            sampler: &state.sampler,
            uniforms: &state.uniforms,
        };
        for (slot_index, (uniform_index, plan)) in plans.iter().enumerate() {
            let slot = resources.slot(&mut state.slots, device, slot_index, target.format(), plan);
            let offset = (*uniform_index as u64 * state.uniform_stride) as u32;
            let [downsample_group, horizontal_group, vertical_group] = slot
                .bind_groups
                .as_ref()
                .expect("slot() builds the bind groups");

            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: target,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: plan.region_origin[0],
                        y: plan.region_origin[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &slot.region.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: plan.region_size[0],
                    height: plan.region_size[1],
                    depth_or_array_layers: 1,
                },
            );
            let passes = [
                (&state.pipelines.downsample, downsample_group, &slot.first.view),
                (&state.pipelines.blur_horizontal, horizontal_group, &slot.second.view),
                (&state.pipelines.blur_vertical, vertical_group, &slot.first.view),
            ];
            for (pipeline, bind_group, target_view) in passes {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("backdrop_blur_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });
                pass.set_viewport(
                    0.,
                    0.,
                    plan.down_size[0] as f32,
                    plan.down_size[1] as f32,
                    0.,
                    1.,
                );
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[offset]);
                pass.draw(0..3, 0..1);
            }
        }
        PreparedBatch { plans }
    }

    /// Draws a blurred batch over the frame, in the resumed main pass whose
    /// viewport already places the batch at its depth.
    pub(crate) fn draw_composites(&self, pass: &mut wgpu::RenderPass<'_>, batch: PreparedBatch<'_>) {
        let Some(state) = &self.state else {
            return;
        };
        pass.set_pipeline(&state.pipelines.composite);
        for (slot, (uniform_index, _)) in state.slots.iter().zip(batch.plans) {
            let Some([_, reads_first, _]) = &slot.bind_groups else {
                continue;
            };
            let offset = (*uniform_index as u64 * state.uniform_stride) as u32;
            pass.set_bind_group(0, reads_first, &[offset]);
            pass.draw(0..4, 0..1);
        }
    }

    /// The GPU memory held by intermediate textures, in bytes.
    #[allow(dead_code)]
    pub(crate) fn texture_bytes(&self) -> u64 {
        let Some(state) = &self.state else {
            return 0;
        };
        state
            .slots
            .iter()
            .flat_map(|slot| [&slot.region, &slot.first, &slot.second])
            .map(|texture| {
                let [width, height] = texture.size();
                width as u64 * height as u64 * 4
            })
            .sum()
    }
}

impl State {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        premultiplied_alpha: bool,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(size_of::<BackdropParams>() as u64),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("backdrop_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipelines = Pipelines::new(device, &layout, format, premultiplied_alpha, depth_format);
        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let uniform_stride = (size_of::<BackdropParams>() as u64).next_multiple_of(alignment);
        let mut state = Self {
            layout,
            sampler,
            pipelines,
            uniforms: Self::uniform_buffer(device, uniform_stride, 1),
            uniform_stride,
            uniform_capacity: 1,
            slots: Vec::new(),
        };
        state.grow_uniforms(device, 4);
        state
    }

    fn uniform_buffer(device: &wgpu::Device, stride: u64, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop_uniforms"),
            size: stride * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn grow_uniforms(&mut self, device: &wgpu::Device, capacity: usize) {
        let capacity = capacity.next_power_of_two();
        self.uniforms = Self::uniform_buffer(device, self.uniform_stride, capacity);
        self.uniform_capacity = capacity;
        for slot in &mut self.slots {
            slot.bind_groups = None;
        }
    }

}

/// What a slot's bind groups refer to besides its textures.
struct SlotResources<'a> {
    layout: &'a wgpu::BindGroupLayout,
    sampler: &'a wgpu::Sampler,
    uniforms: &'a wgpu::Buffer,
}

impl SlotResources<'_> {
    /// The slot for the `index`th backdrop of a batch, with textures large
    /// enough for `plan`.
    fn slot<'s>(
        &self,
        slots: &'s mut Vec<Slot>,
        device: &wgpu::Device,
        index: usize,
        format: wgpu::TextureFormat,
        plan: &BackdropPlan,
    ) -> &'s Slot {
        let step = |size: [u32; 2]| size.map(|size| size.next_multiple_of(TEXTURE_SIZE_STEP));
        let region_usage = wgpu::TextureUsages::COPY_DST;
        let blur_usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if index == slots.len() {
            let region = step(plan.region_size);
            let down = step(plan.down_size);
            slots.push(Slot {
                region: Intermediate::new(device, "backdrop_region", format, region, region_usage),
                first: Intermediate::new(device, "backdrop_first", format, down, blur_usage),
                second: Intermediate::new(device, "backdrop_second", format, down, blur_usage),
                bind_groups: None,
            });
        }
        let slot = &mut slots[index];
        let grown = |texture: &Intermediate, size: [u32; 2]| {
            let current = texture.size();
            step([current[0].max(size[0]), current[1].max(size[1])])
        };
        if !slot.region.fits(plan.region_size) || slot.region.texture.format() != format {
            let size = grown(&slot.region, plan.region_size);
            slot.region = Intermediate::new(device, "backdrop_region", format, size, region_usage);
            slot.bind_groups = None;
        }
        if !slot.first.fits(plan.down_size) || slot.first.texture.format() != format {
            let size = grown(&slot.first, plan.down_size);
            slot.first = Intermediate::new(device, "backdrop_first", format, size, blur_usage);
            slot.second = Intermediate::new(device, "backdrop_second", format, size, blur_usage);
            slot.bind_groups = None;
        }
        if slot.bind_groups.is_none() {
            let bind_group = |view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("backdrop_bind_group"),
                    layout: self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: self.uniforms,
                                offset: 0,
                                size: NonZeroU64::new(size_of::<BackdropParams>() as u64),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(self.sampler),
                        },
                    ],
                })
            };
            slot.bind_groups = Some([
                bind_group(&slot.region.view),
                bind_group(&slot.first.view),
                bind_group(&slot.second.view),
            ]);
        }
        slot
    }
}

impl Pipelines {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        premultiplied_alpha: bool,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop_shaders"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        let pipeline = |name: &str,
                        vertex: &str,
                        fragment: &str,
                        topology: wgpu::PrimitiveTopology,
                        blend: Option<wgpu::BlendState>,
                        depth_stencil: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vertex),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let list = wgpu::PrimitiveTopology::TriangleList;
        let blend = if premultiplied_alpha {
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
        } else {
            wgpu::BlendState::ALPHA_BLENDING
        };
        // Drawn in the main pass, so it is tested against the opaque quads
        // that belong above it, like every other primitive.
        let depth = wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        Self {
            format,
            premultiplied_alpha,
            downsample: pipeline("backdrop_downsample", "vs_fullscreen", "fs_downsample", list, None, None),
            blur_horizontal: pipeline(
                "backdrop_blur_horizontal",
                "vs_fullscreen",
                "fs_blur_horizontal",
                list,
                None,
                None,
            ),
            blur_vertical: pipeline(
                "backdrop_blur_vertical",
                "vs_fullscreen",
                "fs_blur_vertical",
                list,
                None,
                None,
            ),
            composite: pipeline(
                "backdrop_composite",
                "vs_composite",
                "fs_composite",
                wgpu::PrimitiveTopology::TriangleStrip,
                Some(blend),
                Some(depth),
            ),
        }
    }
}
