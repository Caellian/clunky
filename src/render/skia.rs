use std::{fs::File, io::Write};

use glam::UVec2;
use skia_safe::{
    surfaces, AlphaType, Color4f, ColorSpace, ImageInfo, Paint, PixelGeometry, SurfaceProps,
    SurfacePropsFlags,
};

use super::RenderBuffer;

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
pub fn draw(buffer: &mut RenderBuffer) {
    let size = buffer.size();
    let info =
        ImageInfo::new_n32_premul((size.x as i32, size.y as i32), Some(ColorSpace::new_srgb()));
    let props = SurfaceProps::new(SurfacePropsFlags::empty(), PixelGeometry::RGBH);
    let mut surface = surfaces::wrap_pixels(
        &info,
        buffer.mmap.as_mut(),
        Some(size.x as usize * 4),
        Some(&props),
    )
    .unwrap();
    let canvas = surface.canvas();

    
    let color = Color4f::new(0.8, 0.2, 0.7, 1.0);
    let paint = Paint::new(color, &ColorSpace::new_srgb());
    canvas.draw_circle((50, 50), 50.0, &paint);
}
