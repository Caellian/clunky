#[cfg(feature = "wayland")]
pub mod wayland;

use glam::{IVec2, UVec2};
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
    fn create(config: TargetConfig) -> Result<(Self, Q)>;
    fn reposition(&mut self, new_position: IVec2) -> Result<()>;
    fn resize(&mut self, new_size: UVec2) -> Result<()>;
    fn destroy(&mut self) -> Result<()>;

    fn active(&self) -> bool;
}

#[cfg(feature = "wayland")]
pub type RenderTargetImpl = WaylandState;
