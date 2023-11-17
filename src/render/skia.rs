use std::{fs::File, io::Write, ptr::addr_of};

use glam::UVec2;
use rlua::Function as LuaFunction;
use skia_safe::{
    surfaces, AlphaType, Color, Color4f, ColorSpace, ColorType, ImageInfo, Paint, PixelGeometry,
    Rect, SurfaceProps, SurfacePropsFlags,
};

use crate::skia_bindings::LuaCanvas;

use super::buffer::{FrameBuffer, FrameParameters};

fn reorder_rgba_to_argb(over: &mut [u8]) {
    assert!(over.len() % 4 == 0);
    let over_cast =
        unsafe { std::slice::from_raw_parts_mut(over.as_mut_ptr() as *mut u32, over.len() >> 2) };
    for pixel in over_cast {
        let a = (*pixel & 0xff) << 3;
        *pixel = (*pixel >> 1) | a;
    }
}

//https://skia.org/docs/user/api/skcanvas_creation/
pub fn draw(
    buffer: &mut FrameBuffer,
    params: FrameParameters,
    script_fn: LuaFunction,
) -> Result<(), crate::ClunkyError> {
    let size = params.dimensions;

    let info =
        ImageInfo::new_n32_premul((size.x as i32, size.y as i32), Some(ColorSpace::new_srgb()))
            .with_color_type(ColorType::BGRA8888);

    let mut surface = surfaces::wrap_pixels(
        &info,
        buffer.as_mut_slice(),
        Some(size.x as usize * 4),
        None,
    )
    .unwrap();

    let canvas = unsafe {
        // FIXME: Canvas will outlive script_fn call
        addr_of!(*surface.canvas()).as_ref().unwrap_unchecked()
    };

    script_fn
        .call(LuaCanvas(canvas))
        .map_err(crate::error::ClunkyError::Lua)?;

    Ok(())
}
