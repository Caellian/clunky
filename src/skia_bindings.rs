use std::rc::Rc;

use phf::phf_map;
use rlua::{prelude::*, Context as LuaContext, Table as LuaTable, UserData};
use skia_safe::{
    canvas,
    image_filters::{self, CropRect},
    paint, BlendMode, Borrows, Canvas, Color, Color4f, ColorSpace, Data, Image, ImageFilter,
    Matrix, Paint, Path, Point, Rect, SamplingOptions, Shader, Surface, TileMode,
};

#[inline]
fn read_table_color(table: LuaTable) -> Color4f {
    let r = table.get("r").unwrap_or_default();
    let g = table.get("g").unwrap_or_default();
    let b = table.get("b").unwrap_or_default();
    let a = table.get("a").unwrap_or(1.0);
    Color4f { r, g, b, a }
}

#[inline]
fn read_table_rect(table: LuaTable) -> Result<Rect, LuaError> {
    let left = table.get("left").unwrap_or_default();
    let top = table.get("top").unwrap_or_default();
    let right = table
        .get("right")
        .map_err(|_| LuaError::RuntimeError("Rect missing 'right' field".to_string()))?;
    let bottom = table
        .get("bottom")
        .map_err(|_| LuaError::RuntimeError("Rect missing 'bottom' field".to_string()))?;
    Ok(Rect {
        left,
        top,
        right,
        bottom,
    })
}

static BLEND_MODE_NAMES: phf::Map<&'static str, BlendMode> = phf_map! {
    "clear" => BlendMode::Clear,
    "src" => BlendMode::Src,
    "dst" => BlendMode::Dst,
    "srcover" => BlendMode::SrcOver,
    "src_over" => BlendMode::SrcOver,
    "dstover" => BlendMode::DstOver,
    "dst_over" => BlendMode::DstOver,
    "srcin" => BlendMode::SrcIn,
    "src_in" => BlendMode::SrcIn,
    "dstin" => BlendMode::DstIn,
    "dst_in" => BlendMode::DstIn,
    "srcout" => BlendMode::SrcOut,
    "src_out" => BlendMode::SrcOut,
    "dstout" => BlendMode::DstOut,
    "dst_out" => BlendMode::DstOut,
    "srcatop" => BlendMode::SrcATop,
    "src_a_top" => BlendMode::SrcATop,
    "dstatop" => BlendMode::DstATop,
    "dst_a_top" => BlendMode::DstATop,
    "xor" => BlendMode::Xor,
    "plus" => BlendMode::Plus,
    "modulate" => BlendMode::Modulate,
    "screen" => BlendMode::Screen,
    "overlay" => BlendMode::Overlay,
    "darken" => BlendMode::Darken,
    "lighten" => BlendMode::Lighten,
    "colordodge" => BlendMode::ColorDodge,
    "color_dodge" => BlendMode::ColorDodge,
    "colorburn" => BlendMode::ColorBurn,
    "color_burn" => BlendMode::ColorBurn,
    "hardlight" => BlendMode::HardLight,
    "hard_light" => BlendMode::HardLight,
    "softlight" => BlendMode::SoftLight,
    "soft_light" => BlendMode::SoftLight,
    "difference" => BlendMode::Difference,
    "exclusion" => BlendMode::Exclusion,
    "multiply" => BlendMode::Multiply,
    "hue" => BlendMode::Hue,
    "saturation" => BlendMode::Saturation,
    "color" => BlendMode::Color,
    "luminosity" => BlendMode::Luminosity,
};

#[inline]
fn read_blend_mode(name: impl AsRef<str>) -> Result<BlendMode, LuaError> {
    Ok(*BLEND_MODE_NAMES
        .get(name.as_ref().to_ascii_lowercase().as_str())
        .ok_or(LuaError::RuntimeError(format!(
            "unknown blend mode: {}",
            name.as_ref()
        )))?)
}

#[derive(Clone)]
pub struct LuaSkShader(pub Shader);

impl UserData for LuaSkShader {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isOpaque", |_, this, ()| Ok(this.0.is_opaque()));
        methods.add_method("isAImage", |_, this, ()| Ok(this.0.is_a_image()));
        // TODO: asAGradient
    }
}

#[derive(Clone)]
pub struct LuaSkImage(pub Image);

impl UserData for LuaSkImage {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("width", |_, this, ()| Ok(this.0.width()));
        methods.add_method("height", |_, this, ()| Ok(this.0.height()));
        methods.add_method("newShader", |_, this, ()| {
            this.0
                .to_shader(
                    Some((TileMode::Clamp, TileMode::Clamp)),
                    SamplingOptions::default(),
                    None,
                )
                .map(LuaSkShader)
                .ok_or(LuaError::RuntimeError(
                    "can't create shader from image".to_string(),
                ))
        });
    }
}

#[derive(Clone)]
pub struct LuaSkImageFilter(pub ImageFilter);

impl UserData for LuaSkImageFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

pub struct LuaSkMatrix(pub Matrix);

impl UserData for LuaSkMatrix {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        //methods.add_method("getType", |_, this, ()| Ok(()));
        //methods.add_method("getScaleX", |_, this, ()| Ok(()));
        //methods.add_method("getScaleY", |_, this, ()| Ok(()));
        //methods.add_method("getTranslateX", |_, this, ()| Ok(()));
        //methods.add_method("getTranslateY", |_, this, ()| Ok(()));
        //methods.add_method("setRectToRect", |_, this, ()| Ok(()));
        //methods.add_method("invert", |_, this, ()| Ok(()));
        //methods.add_method("mapXY", |_, this, ()| Ok(()));
    }
}

#[derive(Clone)]
pub struct LuaSkPaint(pub Paint);

impl UserData for LuaSkPaint {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isAntiAlias", |_, this, ()| Ok(this.0.is_anti_alias()));
        methods.add_method_mut("setAntiAlias", |_, this, (anti_alias,): (bool,)| {
            this.0.set_anti_alias(anti_alias);
            Ok(())
        });
        methods.add_method("isDither", |_, this, ()| Ok(this.0.is_dither()));
        //TODO: methods.add_method_mut("setDither", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getFilterQuality", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setFilterQuality", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getAlpha", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setAlpha", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getColor", |_, this, ()| Ok(()));
        methods.add_method_mut("setColor", |_, this, (color,): (LuaTable,)| {
            let color = read_table_color(color);
            this.0.set_color4f(color, &ColorSpace::new_srgb());
            Ok(())
        });
        //TODO: methods.add_method("getStroke", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setStroke", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getStrokeCap", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getStrokeJoin", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getStrokeWidth", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setStrokeWidth", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getStrokeMiter", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getEffects", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getColorFilter", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setColorFilter", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getImageFilter", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setImageFilter", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getShader", |_, this, ()| Ok(()));
        //TODO: methods.add_method_mut("setShader", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getPathEffect", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getFillPath", |_, this, ()| Ok(()));
    }
}

#[derive(Clone)]
pub struct LuaSkPath(pub Path);

impl UserData for LuaSkPath {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaSkCanvas<'a>(pub &'a Canvas);

unsafe impl<'a> Send for LuaSkCanvas<'a> {}

impl<'a> UserData for LuaSkCanvas<'a> {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("clear", |_, this, (color,): (Option<LuaTable>,)| {
            let color = color
                .map(read_table_color)
                .unwrap_or(skia_safe::colors::TRANSPARENT);
            this.0.clear(color);
            Ok(())
        });
        methods.add_method(
            "drawColor",
            |_, this, (color, blend_mode): (LuaTable, Option<String>)| {
                let color = read_table_color(color);
                let mode = match blend_mode {
                    Some(it) => Some(read_blend_mode(it)?),
                    None => None,
                };
                this.0.draw_color(color, mode);
                Ok(())
            },
        );
        methods.add_method("drawPaint", |_, this, (paint,): (LuaSkPaint,)| {
            this.0.draw_paint(&paint.0);
            Ok(())
        });
        methods.add_method(
            "drawRect",
            |_, this, (rect, paint): (LuaTable, LuaSkPaint)| {
                let rect = read_table_rect(rect)?;
                this.0.draw_rect(rect, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawOval",
            |_, this, (oval, paint): (LuaTable, LuaSkPaint)| {
                let oval = read_table_rect(oval)?;
                this.0.draw_oval(oval, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawCircle",
            |_, this, (x, y, r, paint): (f32, f32, f32, LuaSkPaint)| {
                this.0.draw_circle((x, y), r, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawImage",
            |_, this, (image, x, y, paint): (LuaSkImage, f32, f32, Option<LuaSkPaint>)| {
                this.0
                    .draw_image(image.0, (x, y), paint.map(|it| it.0).as_ref());
                Ok(())
            },
        );
        methods.add_method(
            "drawImageRect",
            |_,
             this,
             (image, src_rect, dst_rect, paint): (
                LuaSkImage,
                Option<LuaTable>,
                LuaTable,
                Option<LuaSkPaint>,
            )| {
                let paint: Paint = match paint {
                    Some(it) => it.0,
                    None => Paint::default(),
                };
                let src_rect = match src_rect {
                    Some(it) => Some(read_table_rect(it)?),
                    None => None,
                };
                let dst_rect = read_table_rect(dst_rect)?;
                this.0.draw_image_rect(
                    image.0,
                    src_rect
                        .as_ref()
                        .map(|rect| (rect, canvas::SrcRectConstraint::Fast)),
                    dst_rect,
                    &paint,
                );
                Ok(())
            },
        );
        methods.add_method(
            "drawPatch",
            |_,
             this,
             (cubics_table, colors, tex_coords, blend_mode, paint): (
                LuaTable,
                Option<LuaTable>,
                Option<LuaTable>,
                String,
                LuaSkPaint,
            )| {
                if cubics_table.len()? != 24 {
                    return Err(LuaError::RuntimeError(
                        "expected 12 cubic points".to_string(),
                    ));
                }
                let mut cubics = [Point::new(0.0, 0.0); 12];
                for i in 0..12 {
                    let x: f32 = cubics_table.get(i * 2)?;
                    let y: f32 = cubics_table.get(i * 2 + 1)?;
                    cubics[i] = Point::new(x, y);
                }

                let colors = match colors {
                    Some(colors) => {
                        let mut result = [Color::TRANSPARENT; 4];
                        for i in 0..4 {
                            result[i] = read_table_color(colors.get(i)?).to_color();
                        }
                        Some(result)
                    }
                    None => None,
                };

                let tex_coords = match tex_coords {
                    Some(coords) => {
                        let mut result = [Point::new(0.0, 0.0); 4];
                        for i in 0..4 {
                            let x: f32 = coords.get(i * 2)?;
                            let y: f32 = coords.get(i * 2 + 1)?;
                            result[i] = Point::new(x, y);
                        }
                        Some(result)
                    }
                    None => None,
                };

                let mode = read_blend_mode(blend_mode)?;
                this.0.draw_patch(
                    &cubics,
                    colors.as_ref(),
                    tex_coords.as_ref(),
                    mode,
                    &paint.0,
                );
                Ok(())
            },
        );
        methods.add_method(
            "drawPath",
            |_, this, (path, paint): (LuaSkPath, LuaSkPaint)| {
                this.0.draw_path(&path.0, &paint.0);
                Ok(())
            },
        );
        //TODO: methods.add_method("drawPicture", |_, this, ()| Ok(()));
        //TODO: methods.add_method("drawText", |_, this, ()| Ok(()));
        //TODO: methods.add_method("drawTextBlob", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getSaveCount", |_, this, ()| Ok(()));
        //TODO: methods.add_method("getLocalToDevice", |_, this, ()| Ok(()));
        methods.add_method("getLocalToDevice3x3", |_, this, ()| {
            Ok(LuaSkMatrix(this.0.local_to_device_as_3x3()))
        });
        //TODO: methods.add_method("save", |_, this, ()| Ok(()));
        //TODO: methods.add_method("saveLayer", |_, this, ()| Ok(()));
        //TODO: methods.add_method("restore", |_, this, ()| Ok(()));
        methods.add_method("scale", |_, this, (sx, sy): (f32, Option<f32>)| {
            let sy = sy.unwrap_or(sx);
            this.0.scale((sx, sy));
            Ok(())
        });
        //TODO: methods.add_method("translate", |_, this, ()| Ok(()));
        //TODO: methods.add_method("rotate", |_, this, ()| Ok(()));
        //TODO: methods.add_method("concat", |_, this, ()| Ok(()));
        //TODO: methods.add_method("newSurface", |_, this, ()| Ok(()));
    }
}

struct LuaSk;
impl UserData for LuaSk {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("loadImage", |_, _, (name,): (String,)| {
            let handle: Data = Data::new_copy(
                &std::fs::read(name)
                    .map_err(|io_err| rlua::Error::RuntimeError(io_err.to_string()))?,
            );
            Image::from_encoded(handle)
                .map(LuaSkImage)
                .ok_or(LuaError::RuntimeError(
                    "Unsupported encoded image format".to_string(),
                ))
        });
        methods.add_method(
            "newBlurImageFilter",
            |_, _, (sigma_x, sigma_y): (f32, f32)| {
                if !sigma_x.is_finite() || sigma_x < 0f32 {
                    return Err(LuaError::RuntimeError(
                        "x sigma must be a positive, finite scalar".to_string(),
                    ));
                }
                if !sigma_y.is_finite() || sigma_y < 0f32 {
                    return Err(LuaError::RuntimeError(
                        "y sigma must be a positive, finite scalar".to_string(),
                    ));
                }
                image_filters::blur((sigma_x, sigma_y), None, None, CropRect::NO_CROP_RECT)
                    .ok_or(LuaError::RuntimeError(
                        "unable to construct ImageFilter::Blur".to_string(),
                    ))
                    .map(LuaSkImageFilter)
            },
        );
        //TODO: methods.add_method("newLinearGradient", |ctx, this, ()| Ok(()));
        methods.add_method("newMatrix", |_, _, ()| {
            Ok(LuaSkMatrix(Matrix::new_identity()))
        });
        methods.add_method("newPaint", |_, _, ()| Ok(LuaSkPaint(Paint::default())));
        methods.add_method("newPath", |_, _, ()| Ok(LuaSkPath(Path::new())));
        //TODO: methods.add_method("newPictureRecorder", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newRRect", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newRasterSurface", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newTextBlob", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newTypeface", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newFontStyle", |ctx, this, ()| Ok(()));
    }
}

#[allow(non_snake_case)]
pub fn setup<'lua>(ctx: LuaContext<'lua>) -> Result<(), rlua::Error> {
    let gfx = ctx.create_userdata(LuaSk)?;
    ctx.globals().set("Gfx", gfx)?;
    Ok(())
}
