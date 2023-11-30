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

pub mod ext {
    use std::ptr::{addr_of, addr_of_mut};

    use skia_safe::{Matrix, M44};

    #[inline]
    pub fn matrix_as_slice(mx: &Matrix) -> &[f32; 9] {
        unsafe { (addr_of!(*mx) as *mut [f32; 9]).as_ref().unwrap_unchecked() }
    }

    #[inline]
    pub fn matrix_as_slice_mut(mx: &mut Matrix) -> &mut [f32; 9] {
        unsafe {
            (addr_of_mut!(*mx) as *mut [f32; 9])
                .as_mut()
                .unwrap_unchecked()
        }
    }

    #[inline]
    pub fn m44_as_slice(mx: &M44) -> &[f32; 16] {
        unsafe {
            (addr_of!(*mx) as *mut [f32; 16])
                .as_ref()
                .unwrap_unchecked()
        }
    }

    #[inline]
    pub fn m44_as_slice_mut(mx: &mut M44) -> &mut [f32; 16] {
        unsafe {
            (addr_of_mut!(*mx) as *mut [f32; 16])
                .as_mut()
                .unwrap_unchecked()
        }
    }
}
#[path = "skia_bindings.rs"]
pub mod bindings;
