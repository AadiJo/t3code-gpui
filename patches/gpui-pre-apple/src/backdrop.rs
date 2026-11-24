//! Draws [`gpui::Backdrop`] regions, CSS `backdrop-filter: blur() saturate()`.
//!
//! This mirrors `backdrop.rs` in gpui-pre-wgpu step for step, and the shaders
//! at the end of `shaders.metal` mirror its `shaders_backdrop.wgsl`. The
//! renderer ends its render encoder when it reaches a backdrop batch. For each
//! backdrop a blit copies the region beneath it out of the frame, and three
//! render passes average it down and blur it across and down. The renderer
//! then resumes the frame and draws the result over the region, clipped to
//! the rounded border box. A frame without backdrops creates nothing here.

use anyhow::{Context as _, Result};
use gpui::{Backdrop, DevicePixels, Size};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLLibrary,
    MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSize,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLViewport,
};
use std::{mem, ptr};

/// Intermediate textures grow in steps of this many pixels, so a region that
/// changes size by a few pixels each frame does not reallocate them.
const TEXTURE_SIZE_STEP: usize = 128;

/// One backdrop's parameters, read by the backdrop shaders in `shaders.metal`
/// through the generated `scene.h`. Same fields as gpui-pre-wgpu's.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct BackdropParams {
    pub corner_radii: [f32; 4],
    pub viewport_size: [f32; 2],
    pub region_origin: [f32; 2],
    pub region_size: [f32; 2],
    pub down_size: [f32; 2],
    pub draw_origin: [f32; 2],
    pub draw_size: [f32; 2],
    pub bounds_origin: [f32; 2],
    pub bounds_size: [f32; 2],
    pub sigma: f32,
    pub radius: i32,
    pub factor: u32,
    pub saturation: f32,
    pub opacity: f32,
    pub premultiplied_alpha: u32,
    pub pad: [u32; 2],
}

/// The backdrop shaders' buffer and texture slots. Buffers start after
/// `GlobalInputIndex`, which the composite shares with the frame.
#[repr(C)]
pub enum BackdropInputIndex {
    Params = 2,
    Source = 3,
}

/// One backdrop's plan with the parameters its shaders read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BackdropPlan {
    plan: gpui::BackdropPlan,
    params: BackdropParams,
}

impl BackdropPlan {
    /// Plans `backdrop` in a frame of `viewport` pixels, or returns `None`
    /// when nothing of it is visible.
    pub(crate) fn new(backdrop: &Backdrop, viewport: Size<DevicePixels>) -> Option<Self> {
        let plan = backdrop.plan(viewport)?;
        let corner_radii = backdrop.corner_radii;
        let params = BackdropParams {
            corner_radii: [
                corner_radii.top_left.0,
                corner_radii.top_right.0,
                corner_radii.bottom_right.0,
                corner_radii.bottom_left.0,
            ],
            viewport_size: [viewport.width.0 as f32, viewport.height.0 as f32],
            region_origin: plan.region_origin.map(|value| value as f32),
            region_size: plan.region_size.map(|value| value as f32),
            down_size: plan.down_size.map(|value| value as f32),
            draw_origin: plan.draw_origin,
            draw_size: plan.draw_size,
            bounds_origin: [backdrop.bounds.origin.x.0, backdrop.bounds.origin.y.0],
            bounds_size: [backdrop.bounds.size.width.0, backdrop.bounds.size.height.0],
            sigma: plan.sigma,
            radius: plan.radius as i32,
            factor: plan.factor,
            saturation: backdrop.saturation,
            opacity: backdrop.opacity,
            // The Metal pipelines blend straight alpha.
            premultiplied_alpha: 0,
            pad: [0; 2],
        };
        Some(Self { plan, params })
    }
}

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// The textures one backdrop of a batch is blurred in, kept across frames:
/// the copied region, then the downsampled image in `first`, blurred across
/// into `second` and down back into `first`, which the composite reads.
struct Slot {
    region: Texture,
    first: Texture,
    second: Texture,
}

struct Pipelines {
    downsample: Pipeline,
    blur_horizontal: Pipeline,
    blur_vertical: Pipeline,
    composite: Pipeline,
}

/// The pipelines and textures backdrops are drawn with, created at the first
/// backdrop and reused by every frame after it.
#[derive(Default)]
pub(crate) struct BackdropRenderer {
    state: Option<State>,
}

struct State {
    pipelines: Pipelines,
    slots: Vec<Slot>,
}

impl BackdropRenderer {
    /// Copies, downsamples and blurs each backdrop of a batch into its slot.
    /// Call between render encoders, after everything beneath the batch was
    /// drawn into `target`.
    pub(crate) fn encode_blurs(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        library: &ProtocolObject<dyn MTLLibrary>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        target: &ProtocolObject<dyn MTLTexture>,
        plans: &[BackdropPlan],
    ) -> Result<()> {
        if self.state.is_none() {
            self.state = Some(State {
                pipelines: Pipelines::new(device, library)?,
                slots: Vec::new(),
            });
        }
        let state = self.state.as_mut().context("backdrop state")?;
        for (index, plan) in plans.iter().enumerate() {
            state.ensure_slot(device, index, plan)?;
        }

        let blit = command_buffer
            .blitCommandEncoder()
            .context("creating backdrop blit encoder")?;
        for (slot, plan) in state.slots.iter().zip(plans) {
            let [x, y] = plan.plan.region_origin.map(|value| value as usize);
            let [width, height] = plan.plan.region_size.map(|value| value as usize);
            // SAFETY: The region lies within `target`, which `plan` was made for,
            // and `ensure_slot` made `slot.region` at least as large.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    target,
                    0,
                    0,
                    MTLOrigin { x, y, z: 0 },
                    MTLSize { width, height, depth: 1 },
                    &slot.region,
                    0,
                    0,
                    MTLOrigin { x: 0, y: 0, z: 0 },
                );
            }
        }
        blit.endEncoding();

        for (slot, plan) in state.slots.iter().zip(plans) {
            let passes = [
                (&state.pipelines.downsample, &slot.region, &slot.first),
                (&state.pipelines.blur_horizontal, &slot.first, &slot.second),
                (&state.pipelines.blur_vertical, &slot.second, &slot.first),
            ];
            for (pipeline, source, destination) in passes {
                let descriptor = MTLRenderPassDescriptor::new();
                // SAFETY: Index zero is within the render pass's color attachment array.
                let color_attachment =
                    unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
                color_attachment.setTexture(Some(destination));
                color_attachment.setLoadAction(MTLLoadAction::DontCare);
                color_attachment.setStoreAction(MTLStoreAction::Store);
                let encoder = command_buffer
                    .renderCommandEncoderWithDescriptor(&descriptor)
                    .context("creating backdrop blur encoder")?;
                encoder.setViewport(MTLViewport {
                    originX: 0.,
                    originY: 0.,
                    width: plan.plan.down_size[0] as f64,
                    height: plan.plan.down_size[1] as f64,
                    znear: 0.,
                    zfar: 1.,
                });
                encoder.setRenderPipelineState(pipeline);
                // SAFETY: Metal copies the parameters, which match the shaders'
                // `BackdropParams`, and retains the texture until the pass completes.
                unsafe {
                    encoder.setFragmentBytes_length_atIndex(
                        ptr::NonNull::from(&plan.params).cast(),
                        mem::size_of::<BackdropParams>(),
                        BackdropInputIndex::Params as usize,
                    );
                    encoder.setFragmentTexture_atIndex(
                        Some(source),
                        BackdropInputIndex::Source as usize,
                    );
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        3,
                    );
                }
                encoder.endEncoding();
            }
        }
        Ok(())
    }

    /// Draws a blurred batch over the frame, in the resumed render encoder
    /// whose viewport already places the batch at its depth.
    pub(crate) fn draw_composites(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        plans: &[BackdropPlan],
    ) {
        let Some(state) = &self.state else {
            return;
        };
        encoder.setRenderPipelineState(&state.pipelines.composite);
        for (slot, plan) in state.slots.iter().zip(plans) {
            // SAFETY: As in `encode_blurs`; the composite reads `first`.
            unsafe {
                encoder.setVertexBytes_length_atIndex(
                    ptr::NonNull::from(&plan.params).cast(),
                    mem::size_of::<BackdropParams>(),
                    BackdropInputIndex::Params as usize,
                );
                encoder.setFragmentBytes_length_atIndex(
                    ptr::NonNull::from(&plan.params).cast(),
                    mem::size_of::<BackdropParams>(),
                    BackdropInputIndex::Params as usize,
                );
                encoder.setFragmentTexture_atIndex(
                    Some(&slot.first),
                    BackdropInputIndex::Source as usize,
                );
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    4,
                );
            }
        }
    }
}

impl State {
    /// Makes the slot for the `index`th backdrop of a batch large enough for `plan`.
    fn ensure_slot(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        index: usize,
        plan: &BackdropPlan,
    ) -> Result<()> {
        let region_size = plan.plan.region_size.map(|value| value as usize);
        let down_size = plan.plan.down_size.map(|value| value as usize);
        let fits = |texture: &Texture, size: [usize; 2]| {
            texture.width() >= size[0] && texture.height() >= size[1]
        };
        let grown = |texture: Option<&Texture>, size: [usize; 2]| {
            let current = texture.map_or([0, 0], |texture| [texture.width(), texture.height()]);
            [current[0].max(size[0]), current[1].max(size[1])]
                .map(|value| value.next_multiple_of(TEXTURE_SIZE_STEP))
        };
        let new_texture = |size: [usize; 2], usage: MTLTextureUsage| {
            intermediate_texture(device, size, usage)
        };
        if index == self.slots.len() {
            let down = grown(None, down_size);
            self.slots.push(Slot {
                region: new_texture(grown(None, region_size), MTLTextureUsage::ShaderRead)?,
                first: new_texture(down, MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget)?,
                second: new_texture(down, MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget)?,
            });
        }
        let slot = &mut self.slots[index];
        if !fits(&slot.region, region_size) {
            slot.region = new_texture(grown(Some(&slot.region), region_size), MTLTextureUsage::ShaderRead)?;
        }
        if !fits(&slot.first, down_size) {
            let down = grown(Some(&slot.first), down_size);
            slot.first = new_texture(down, MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget)?;
            slot.second = new_texture(down, MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget)?;
        }
        Ok(())
    }
}

/// A private BGRA texture, the frame's format, for one intermediate image.
fn intermediate_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    size: [usize; 2],
    usage: MTLTextureUsage,
) -> Result<Texture> {
    let descriptor = MTLTextureDescriptor::new();
    // SAFETY: The size is a region of an existing render target, rounded up a little.
    unsafe {
        descriptor.setWidth(size[0]);
        descriptor.setHeight(size[1]);
    }
    descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    descriptor.setUsage(usage);
    descriptor.setStorageMode(MTLStorageMode::Private);
    device
        .newTextureWithDescriptor(&descriptor)
        .context("allocating backdrop texture")
}

impl Pipelines {
    fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        library: &ProtocolObject<dyn MTLLibrary>,
    ) -> Result<Self> {
        let blur = |label: &str, fragment: &str| {
            build_pipeline_state(device, library, label, "backdrop_fullscreen_vertex", fragment, false)
        };
        Ok(Self {
            downsample: blur("backdrop_downsample", "backdrop_downsample_fragment")?,
            blur_horizontal: blur("backdrop_blur_horizontal", "backdrop_blur_horizontal_fragment")?,
            blur_vertical: blur("backdrop_blur_vertical", "backdrop_blur_vertical_fragment")?,
            // Drawn in the frame's render encoder, so it has the frame's depth
            // attachment and is tested against the opaque quads above it.
            composite: build_pipeline_state(
                device,
                library,
                "backdrop_composite",
                "backdrop_composite_vertex",
                "backdrop_composite_fragment",
                true,
            )?,
        })
    }
}

/// Like `metal_renderer::build_pipeline_state`, but the blur passes have no
/// depth attachment, so only the composite declares one, and only the
/// composite blends.
fn build_pipeline_state(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    label: &str,
    vertex_fn_name: &str,
    fragment_fn_name: &str,
    is_composite: bool,
) -> Result<Pipeline> {
    let vertex_fn = library
        .newFunctionWithName(&NSString::from_str(vertex_fn_name))
        .with_context(|| format!("locating vertex function {vertex_fn_name}"))?;
    let fragment_fn = library
        .newFunctionWithName(&NSString::from_str(fragment_fn_name))
        .with_context(|| format!("locating fragment function {fragment_fn_name}"))?;
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(label)));
    descriptor.setVertexFunction(Some(vertex_fn.as_ref()));
    descriptor.setFragmentFunction(Some(fragment_fn.as_ref()));
    // SAFETY: Index zero is within the pipeline's color attachment array.
    let color_attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color_attachment.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    if is_composite {
        color_attachment.setBlendingEnabled(true);
        color_attachment.setRgbBlendOperation(objc2_metal::MTLBlendOperation::Add);
        color_attachment.setAlphaBlendOperation(objc2_metal::MTLBlendOperation::Add);
        color_attachment.setSourceRGBBlendFactor(objc2_metal::MTLBlendFactor::SourceAlpha);
        color_attachment.setSourceAlphaBlendFactor(objc2_metal::MTLBlendFactor::One);
        color_attachment
            .setDestinationRGBBlendFactor(objc2_metal::MTLBlendFactor::OneMinusSourceAlpha);
        color_attachment
            .setDestinationAlphaBlendFactor(objc2_metal::MTLBlendFactor::OneMinusSourceAlpha);
        descriptor.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
    }
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .with_context(|| format!("creating {label} render pipeline"))
}
