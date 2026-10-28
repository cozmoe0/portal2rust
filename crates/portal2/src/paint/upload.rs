//! Paint map uploads: only the faces that changed are written into the GPU
//! texture (the atlas can be thousands of texels wide), straight through
//! the render queue.

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

use super::PaintRect;

pub struct PaintUploadPlugin;

impl Plugin for PaintUploadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaintUploads>().add_systems(First, clear_sent);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<PendingWrites>()
            .add_systems(ExtractSchedule, extract_uploads)
            .add_systems(Render, write_rects.in_set(RenderSystems::PrepareResources));
    }
}

/// Rectangles produced this frame; the render world copies them out at
/// extraction and they are dropped at the start of the next frame.
#[derive(Resource, Default)]
pub struct PaintUploads {
    rects: Vec<PaintRect>,
}

impl PaintUploads {
    pub fn push(&mut self, rects: Vec<PaintRect>) {
        self.rects.extend(rects);
    }
}

fn clear_sent(mut uploads: ResMut<PaintUploads>) {
    uploads.rects.clear();
}

/// Writes waiting for their texture to reach the GPU, with the frames waited.
#[derive(Resource, Default)]
struct PendingWrites(Vec<(PaintRect, u32)>);

/// Writes for a texture that never arrives (the map changed) are dropped.
const MAX_WAIT_FRAMES: u32 = 600;

fn extract_uploads(mut pending: ResMut<PendingWrites>, uploads: Extract<Res<PaintUploads>>) {
    pending.0.extend(uploads.rects.iter().map(|r| {
        let rect = PaintRect { image: r.image, x: r.x, y: r.y, width: r.width, height: r.height, data: r.data.clone() };
        (rect, 0)
    }));
}

fn write_rects(mut pending: ResMut<PendingWrites>, images: Res<RenderAssets<GpuImage>>, queue: Res<RenderQueue>) {
    if pending.0.is_empty() {
        return;
    }
    let mut waiting = Vec::new();
    for (rect, age) in pending.0.drain(..) {
        let Some(gpu) = images.get(rect.image) else {
            if age < MAX_WAIT_FRAMES {
                waiting.push((rect, age + 1));
            }
            continue;
        };
        let size = gpu.texture_descriptor.size;
        if rect.x + rect.width > size.width || rect.y + rect.height > size.height {
            continue;
        }
        queue.write_texture(
            TexelCopyTextureInfo { texture: &gpu.texture, mip_level: 0, origin: Origin3d { x: rect.x, y: rect.y, z: 0 }, aspect: TextureAspect::All },
            &rect.data,
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(rect.width * 4), rows_per_image: None },
            Extent3d { width: rect.width, height: rect.height, depth_or_array_layers: 1 },
        );
    }
    pending.0 = waiting;
}
