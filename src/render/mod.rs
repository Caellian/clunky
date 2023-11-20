#[cfg(feature = "wayland")]
pub mod wayland;

pub mod buffer;
pub mod skia;

pub use skia as frontend;

use glam::{IVec2, UVec2};
use parking_lot::Condvar;
use wayland_client::{Connection, QueueHandle};
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

use crate::error::Result;

use self::{
    buffer::{FrameBuffer, FrameParameters},
    wayland::WaylandState,
};

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
    type QH;

    fn create(config: TargetConfig) -> Result<(Self, Connection, Q)>;
    fn reposition(&mut self, new_position: IVec2) -> Result<()>;
    fn resize(&mut self, new_size: UVec2, qh: Self::QH) -> Result<()>;
    fn push_frame(&mut self, qh: Self::QH);
    fn destroy(&mut self) -> Result<()>;

    fn frame_parameters(&self) -> FrameParameters;
    fn buffer(&mut self) -> &mut FrameBuffer;

    fn running(&self) -> bool;

    fn can_render(&self) -> bool;
}

#[cfg(feature = "wayland")]
pub type RenderTargetImpl = WaylandState;
