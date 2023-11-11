#[cfg(feature = "wayland")]
pub mod wayland;

pub mod skia;

use std::{
    fs::File,
    os::fd::{AsFd, BorrowedFd},
};

use glam::{IVec2, UVec2};
use memmap::MmapMut;
use wayland_client::QueueHandle;
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

use crate::error::Result;

use self::wayland::WaylandState;

pub trait Drawable<Q, S: RenderTarget<Q>> {
    fn draw(&self, surface: &mut S);
}

#[derive(Debug)]
pub struct TargetConfig {
    pub position: IVec2,
    pub size: UVec2,
    pub anchor: Anchor,
}

impl Default for TargetConfig {
    fn default() -> Self {
        TargetConfig {
            position: IVec2::ZERO,
            size: UVec2::ZERO,
            anchor: Anchor::Top | Anchor::Left,
        }
    }
}

pub trait RenderTarget<Q>: Sized {
    fn create(config: TargetConfig, buffer: RenderBuffer) -> Result<(Self, Q)>;
    fn reposition(&mut self, new_position: IVec2) -> Result<()>;
    fn resize(&mut self, new_size: UVec2, qh: &QueueHandle<Self>) -> Result<()>;
    fn destroy(&mut self) -> Result<()>;
    fn buffer(&mut self) -> &mut RenderBuffer;

    fn active(&self) -> bool;
}

#[cfg(feature = "wayland")]
pub type RenderTargetImpl = WaylandState;

pub struct RenderBuffer {
    length: usize,
    size: UVec2,
    source: File,
    mmap: MmapMut,
}

impl RenderBuffer {
    pub fn new() -> Self {
        let source = tempfile::tempfile().unwrap();
        source.set_len(4).unwrap();

        let buffer = unsafe { MmapMut::map_mut(&source).expect("unable to memory map file") };

        RenderBuffer {
            length: 4,
            size: UVec2::new(1, 1),
            source,
            mmap: buffer,
        }
    }

    pub fn as_fd(&self) -> BorrowedFd {
        self.source.as_fd()
    }

    pub fn ensure_capacity(&mut self, size: UVec2, bpp: usize) {
        let new_length = (size.x * size.y) as usize * bpp;
        if self.length < new_length {
            self.source
                .set_len(new_length as u64)
                .expect("unable to grow the buffer");
            self.mmap = unsafe { MmapMut::map_mut(&self.source).unwrap() };
            self.length = new_length;
        }
        self.size = size;
    }

    pub fn size(&self) -> UVec2 {
        self.size
    }
}
