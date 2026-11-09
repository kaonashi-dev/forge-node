use std::ops::{Deref, DerefMut};

use crate::terminal::shaping::Shaper;
pub use crate::terminal::shaping::{
    ATLAS_SIZE, BOLD, ITALIC, MAX_GLYPHS_PER_CELL, NO_RUN, REGULAR,
};

/// The GPU side of the glyph cache: an R8 texture that [`Shaper`]'s queued uploads are written into.
pub struct Atlas {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    shaper: Shaper,
}

impl Deref for Atlas {
    type Target = Shaper;
    fn deref(&self) -> &Shaper {
        &self.shaper
    }
}

impl DerefMut for Atlas {
    fn deref_mut(&mut self) -> &mut Shaper {
        &mut self.shaper
    }
}

impl Atlas {
    pub fn new(device: &wgpu::Device, cell_pt: (f32, f32), scale: f32) -> Atlas {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Atlas {
            texture,
            view,
            shaper: Shaper::new(cell_pt, scale),
        }
    }

    /// Writes the glyphs rasterized since the last frame; empty (and free) in a steady frame.
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        for up in self.shaper.pending.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: up.x,
                        y: up.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &up.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(up.w),
                    rows_per_image: Some(up.h),
                },
                wgpu::Extent3d {
                    width: up.w,
                    height: up.h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
}
