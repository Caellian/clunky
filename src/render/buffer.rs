use std::{fs::File, os::fd::AsFd};

use glam::UVec2;
use memmap2::{MmapMut, RemapOptions};
use skia_safe::ColorType;
use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer,
        wl_shm::{Format as WlFormat, WlShm},
        wl_shm_pool::WlShmPool,
    },
    QueueHandle,
};

use super::wayland::WaylandState;

/// List of supported formats.
///
/// All format must be supported by both Skia and Wayland.
///
/// Formats with lower values will be favored over those with greater values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
#[repr(u8)]
pub enum ColorFormat {
    ARGB8888,
}

impl ColorFormat {
    pub const fn from_wl_format(format: WlFormat) -> Option<Self> {
        match format {
            WlFormat::Rgba8888 => Some(ColorFormat::ARGB8888),
            _ => None,
        }
    }

    #[allow(unreachable_patterns)]
    pub fn as_wl_format(&self) -> WlFormat {
        match self {
            ColorFormat::ARGB8888 => WlFormat::Argb8888,
            _ => unreachable!("frame color format not supported by Wayland"),
        }
    }

    #[allow(unreachable_patterns)]
    pub fn as_skia_format(&self) -> ColorType {
        match self {
            ColorFormat::ARGB8888 => ColorType::BGRA8888,
            _ => unreachable!("frame color format not supported by Skia"),
        }
    }

    pub fn pixel_size(&self) -> usize {
        match self {
            ColorFormat::ARGB8888 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameParameters {
    pub dimensions: UVec2,
    pub format: ColorFormat,
}

impl FrameParameters {
    pub fn new(dimensions: UVec2, format: ColorFormat) -> Self {
        FrameParameters { dimensions, format }
    }

    #[inline]
    pub fn stride(&self) -> i32 {
        self.dimensions.x as i32 * self.format.pixel_size() as i32
    }

    #[inline]
    pub fn len(&self) -> usize {
        (self.dimensions.x * self.dimensions.y) as usize * self.format.pixel_size()
    }
}

pub struct FrameBuffer {
    source: File,
    mmap: MmapMut,

    wl_pool: WlShmPool,
    wl_buffer: WlBuffer,
}

impl FrameBuffer {
    pub fn new(
        shm: &WlShm,
        params: FrameParameters,
        qh: &QueueHandle<WaylandState>,
    ) -> Result<Self, std::io::Error> {
        let source = tempfile::tempfile()?;
        source.set_len(params.len() as u64)?;
        let mmap = unsafe { MmapMut::map_mut(&source)? };

        let pool = shm.create_pool(source.as_fd(), params.len() as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            params.dimensions.x as i32,
            params.dimensions.y as i32,
            params.stride(),
            params.format.as_wl_format(),
            qh,
            (),
        );

        Ok(FrameBuffer {
            source,
            mmap,
            wl_pool: pool,
            wl_buffer: buffer,
        })
    }

    pub fn switch_params(
        &mut self,
        mut params: FrameParameters,
        qh: QueueHandle<WaylandState>,
    ) -> Result<(), std::io::Error> {
        // Neither Skia nor Wayland allow these to be 0
        params.dimensions.x = params.dimensions.x.max(1);
        params.dimensions.y = params.dimensions.y.max(1);

        let new_len = params.len();

        self.wl_buffer.destroy();

        if self.mmap.len() < new_len {
            self.source.set_len(new_len as u64)?;
            self.wl_pool.resize(new_len as i32);
            unsafe {
                // Render is blocked by compositor polling
                self.mmap
                    .remap(new_len, RemapOptions::new().may_move(true))?;
            }
        }
        self.wl_buffer = self.wl_pool.create_buffer(
            0,
            params.dimensions.x as i32,
            params.dimensions.y as i32,
            params.stride(),
            params.format.as_wl_format(),
            &qh,
            (),
        );
        Ok(())
    }

    pub fn buffer(&self) -> &WlBuffer {
        &self.wl_buffer
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.mmap
    }
}

impl Drop for FrameBuffer {
    fn drop(&mut self) {
        self.wl_buffer.destroy();
        self.wl_pool.destroy();
    }
}
