use std::{fs::File, io::Write};

use glam::UVec2;
use skia_safe::{
    surfaces, AlphaType, Color, Color4f, ColorSpace, ColorType, ImageInfo, Paint, PixelGeometry,
    Rect, SurfaceProps, SurfacePropsFlags,
};

use super::buffer::{FrameBuffer, FrameParameters};

/*
#include <vector>
#include "include/core/SkSurface.h"
std::vector<char> raster_direct(int width, int height,
                                void (*draw)(SkCanvas*)) {
    SkImageInfo info = SkImageInfo::MakeN32Premul(width, height);
    size_t rowBytes = info.minRowBytes();
    size_t size = info.getSafeSize(rowBytes);
    std::vector<char> pixelMemory(size);  // allocate memory
    sk_sp<SkSurface> surface =
            SkSurface::MakeRasterDirect(
                    info, &pixelMemory[0], rowBytes);
    SkCanvas* canvas = surface->getCanvas();
    draw(canvas);
    return pixelMemory;
}
 */

//https://skia.org/docs/user/api/skcanvas_creation/
pub fn draw(buffer: &mut FrameBuffer, params: FrameParameters) -> Result<(), crate::ClunkyError> {
    let size = params.dimensions;

    let info =
        ImageInfo::new_n32_premul((size.x as i32, size.y as i32), Some(ColorSpace::new_srgb()))
            .with_color_type(ColorType::RGBA8888);
    let props = SurfaceProps::new(SurfacePropsFlags::empty(), PixelGeometry::RGBH);
    let mut surface = surfaces::wrap_pixels(
        &info,
        buffer.as_mut_slice(),
        Some(size.x as usize * 4),
        Some(&props),
    )
    .unwrap();
    let canvas = surface.canvas();

    let color = Color4f::new(0.8, 0.2, 0.7, 1.0);
    let paint = Paint::new(color, &ColorSpace::new_srgb());
    canvas.clear(Color::TRANSPARENT);

    canvas.draw_rect(
        Rect {
            left: 0.0,
            top: 0.0,
            right: size.x as f32,
            bottom: size.y as f32,
        },
        &paint,
    );
    canvas.draw_circle((50, 50), 50.0, &paint);

    Ok(())
}
