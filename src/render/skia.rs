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
    use std::{
        io::Cursor,
        ptr::{addr_of, addr_of_mut},
    };

    use skia_safe::{Matrix, M44};
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("invalid number of matrix values, expected {expected} values; found: {found}")]
    pub struct BadSize {
        expected: usize,
        found: usize,
    }

    pub trait MatrixExt: Sized {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize>;
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize>;
        fn as_slice(&self) -> &[f32];
        fn as_slice_mut(&mut self) -> &mut [f32];
        fn to_vec(&self) -> Vec<f32> {
            self.as_slice().to_vec()
        }
    }

    impl MatrixExt for Matrix {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize> {
            if values.len() != 9 {
                return Err(BadSize {
                    expected: 9,
                    found: values.len(),
                });
            }
            let mut result = Matrix::new_identity();

            result.as_slice_mut().copy_from_slice(&values);
            Ok(result)
        }

        #[inline]
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize> {
            let values: Vec<f32> = iter.into_iter().take(9).collect();
            Self::from_vec(values)
        }

        #[inline]
        fn as_slice(&self) -> &[f32] {
            unsafe {
                (addr_of!(*self) as *mut [f32; 9])
                    .as_ref()
                    .unwrap_unchecked()
            }
        }

        #[inline]
        fn as_slice_mut(&mut self) -> &mut [f32] {
            unsafe {
                (addr_of_mut!(*self) as *mut [f32; 9])
                    .as_mut()
                    .unwrap_unchecked()
            }
        }
    }

    impl MatrixExt for M44 {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize> {
            if values.len() != 16 {
                return Err(BadSize {
                    expected: 16,
                    found: values.len(),
                });
            }
            let mut result = M44::new_identity();
            result.as_slice_mut().copy_from_slice(&values);
            Ok(result)
        }

        #[inline]
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize> {
            let values: Vec<f32> = iter.into_iter().take(16).collect();
            Self::from_vec(values)
        }

        #[inline]
        fn as_slice(&self) -> &[f32] {
            unsafe {
                (addr_of!(*self) as *mut [f32; 16])
                    .as_ref()
                    .unwrap_unchecked()
            }
        }

        #[inline]
        fn as_slice_mut(&mut self) -> &mut [f32] {
            unsafe {
                (addr_of_mut!(*self) as *mut [f32; 16])
                    .as_mut()
                    .unwrap_unchecked()
            }
        }
    }
}
#[path = "skia_bindings.rs"]
pub mod bindings;
