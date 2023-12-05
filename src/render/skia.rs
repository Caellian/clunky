use skia_safe::{surfaces, Borrows, ColorSpace, ColorType, ImageInfo, Surface};

use super::buffer::FrameBuffer;

pub trait FrameBufferSurface {
    fn to_surface<'a>(&'a mut self) -> Borrows<'a, Surface>;
}

impl FrameBufferSurface for FrameBuffer {
    fn to_surface<'a>(&'a mut self) -> Borrows<'a, Surface> {
        let size = self.frame_parameters().dimensions;

        let info =
            ImageInfo::new_n32_premul((size.x as i32, size.y as i32), Some(ColorSpace::new_srgb()))
                .with_color_type(ColorType::BGRA8888);

        surfaces::wrap_pixels(&info, self.as_mut_slice(), Some(size.x as usize * 4), None).unwrap()
    }
}

pub use rlua_skia as bindings;
