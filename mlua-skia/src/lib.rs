use std::{
    alloc::Layout,
    collections::HashMap,
    ffi::OsString,
    mem::{align_of, size_of},
    ops::Deref,
    os::unix::ffi::{OsStrExt, OsStringExt},
    ptr::addr_of,
    str::FromStr,
    sync::Arc,
};

use byteorder::WriteBytesExt;
use mlua::{prelude::*, FromLua, Lua as LuaContext, Table as LuaTable};
use mlua_skia_macros::lua_methods;
use skia_safe::{
    canvas::{self, SaveLayerFlags, SaveLayerRec},
    color_filter::color_filters,
    font_style::{Slant, Weight, Width},
    gradient_shader::interpolation::{ColorSpace as InColorSpace, HueMethod, InPremul},
    gradient_shader::Interpolation,
    image_filters::{self, CropRect},
    paint::Style as PaintStyle,
    path::Verb,
    path_effect::DashInfo,
    stroke_rec::InitStyle as StrokeRecInitStyle,
    typeface::FontTableTag,
    *,
};

/// Skia argument packs
pub mod args;
/// Skia enum wrappers
pub mod enums;
pub(crate) mod ext;
pub(crate) mod lua;
pub(crate) mod util;

pub use crate::args::*;
pub use crate::enums::*;
use crate::ext::skia::*;
use crate::lua::{combinators::*, *};

pub trait StructToTable<'lua> {
    fn to_table(&self, lua: &'lua LuaContext) -> LuaResult<LuaTable<'lua>>;
}

macro_rules! struct_to_table {
    ($ty: ident : {$($name: literal: |$this: ident, $lua: tt| $access: expr),+ $(,)?}) => {
        impl<'lua> StructToTable<'lua> for $ty {paste::paste!{
            fn to_table(&self, lua: &'lua LuaContext) -> LuaResult<LuaTable<'lua>> {
                let result = lua.create_table()?;
                $(
                    result.set($name, (|$this: &$ty, $lua: &LuaContext| $access)(self, &lua))?;
                )+
                Ok(result)
            }
        }}
    };
}

struct_to_table! { FontMetrics: {
    "top": |metrics, _| metrics.top,
    "ascent": |metrics, _| metrics.ascent,
    "descent": |metrics, _| metrics.descent,
    "bottom": |metrics, _| metrics.bottom,
    "leading": |metrics, _| metrics.leading,
    "avg_char_width": |metrics, _| metrics.avg_char_width,
    "max_char_width": |metrics, _| metrics.max_char_width,
    "x_min": |metrics, _| metrics.x_min,
    "x_max": |metrics, _| metrics.x_max,
    "x_height": |metrics, _| metrics.x_height,
    "cap_height": |metrics, _| metrics.cap_height,
}}

wrap_skia_handle!(Shader);

#[lua_methods(lua_name: Shader)]
impl LuaShader {
    fn is_opaque(&self) -> bool {
        Ok(self.is_opaque())
    }

    fn is_a_image(&self) -> bool {
        Ok(self.is_a_image())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LuaInterpolation(Interpolation);

impl Default for LuaInterpolation {
    fn default() -> Self {
        LuaInterpolation(Interpolation {
            in_premul: InPremul::No,
            color_space: InColorSpace::Destination,
            hue_method: HueMethod::Shorter,
        })
    }
}

impl<'lua> FromLua<'lua> for LuaInterpolation {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua LuaContext) -> LuaResult<Self> {
        let value = match value {
            LuaValue::Integer(value) => {
                let flags = gradient_shader::Flags::from_bits(value as u32).ok_or(
                    LuaError::FromLuaConversionError {
                        from: "integer",
                        to: "Interpolation",
                        message: Some("invalid flags value".to_string()),
                    },
                )?;
                return Ok(LuaInterpolation(Interpolation::from(flags)));
            }
            LuaValue::Number(value) => {
                let flags = gradient_shader::Flags::from_bits(value as u32).ok_or(
                    LuaError::FromLuaConversionError {
                        from: "integer",
                        to: "Interpolation",
                        message: Some("invalid flags value".to_string()),
                    },
                )?;
                return Ok(LuaInterpolation(Interpolation::from(flags)));
            }
            LuaValue::Table(table) => table,
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Interpolaton",
                    message: None,
                })
            }
        };

        let in_premul = value.try_get_or_t::<_, LuaInPremul>("in_premul", lua, InPremul::No)?;
        let color_space = value.try_get_or_t::<_, LuaInColorSpace>(
            "color_space",
            lua,
            InColorSpace::Destination,
        )?;
        let hue_method =
            value.try_get_or_t::<_, LuaHueMethod>("hue_method", lua, HueMethod::Shorter)?;

        Ok(LuaInterpolation(Interpolation {
            in_premul,
            color_space,
            hue_method,
        }))
    }
}
from_lua_argpack!(LuaInterpolation);

pub struct ColorStops {
    positions: Vec<f32>,
    colors: Vec<Color4f>,
}

/// ## Supported formats
/// - {pos: color, pos: color, ...}
/// - {color...}, nil - uniformly spaced
/// - {color...}, {pos...}
impl<'lua> FromArgPack<'lua> for ColorStops {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        args.assert_next_type(&[LuaType::Table])?;

        let first: LuaTable<'lua> =
            args.pop_typed_or(Some("expected a {position: color} table or a color array"))?;

        let key_out_of_bounds = first
            .clone()
            .pairs::<LuaNumber, LuaValue>()
            .any(|it| match it {
                Err(_) => false, // non-numeric index, ignore and assume one table
                Ok((i, _)) => !(0.0..=1.0).contains(&i),
            });

        if !key_out_of_bounds {
            // if user passes a table like {Color}, we ignore the next argument
            // as well because it doesn't matter

            let count = first.clone().pairs::<f32, LuaColor>().count();
            let stops: Vec<(f32, Color4f)> = first
                .clone()
                .pairs::<f32, LuaColor>()
                .filter_map(|it| match it {
                    Ok((f, c)) => Some((f, c.into())),
                    Err(_) => None,
                })
                .collect();

            if stops.len() < count {
                args.revert(first);
                return Err(LuaError::FromLuaConversionError {
                    from: "table",
                    to: "ColorStops",
                    message: Some("ColorStops expects a table with only Color values".to_string()),
                });
            }

            let (positions, colors) = stops.into_iter().unzip();
            return Ok(ColorStops { positions, colors });
        }

        // TODO: check colors in color stops didn't error
        let colors: Vec<Color4f> = first
            .sequence_values::<LuaColor>()
            .filter_map(|it| match it {
                Ok(it) => Some(it.into()),
                Err(_) => None,
            })
            .collect();

        let positions: LuaTable<'lua> = match args.pop_typed() {
            Some(it) => it,
            None => {
                let step = 1.0 / (colors.len() as f32 - 1.0);
                let positions = (0..colors.len()).map(|it| it as f32 * step).collect();
                return Ok(ColorStops { positions, colors });
            }
        };

        let count = positions.clone().sequence_values::<f32>().count();
        let items: Vec<f32> = positions
            .clone()
            .sequence_values::<f32>()
            .filter_map(Result::ok)
            .collect();

        let positions = if items.len() < count {
            args.revert(positions);
            None
        } else {
            Some(items)
        };

        if let Some(positions) = positions {
            Ok(ColorStops { positions, colors })
        } else {
            let step = 1.0 / (colors.len() as f32 - 1.0);
            let positions = (0..colors.len()).map(|it| it as f32 * step).collect();
            Ok(ColorStops { positions, colors })
        }
    }
}

pub struct GradientShader;

#[lua_methods]
impl GradientShader {
    fn make_linear(
        from: LuaPoint,
        to: LuaPoint,
        stops: ColorStops,
        color_space: LuaFallible<LuaColorSpace>,
        tile_mode: LuaFallible<LuaTileMode>,
        interpolation: LuaFallible<LuaInterpolation>,
        local: LuaFallible<LuaMatrix>,
    ) -> LuaShader {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::linear_gradient_with_interpolation(
            (from, to),
            (
                stops.colors.as_slice(),
                color_space.map(LuaColorSpace::unwrap),
            ),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        )
        .map(LuaShader))
    }
    fn make_radial(
        center: LuaPoint,
        radius: f32,
        stops: ColorStops,
        color_space: LuaFallible<LuaColorSpace>,
        tile_mode: LuaFallible<LuaTileMode>,
        interpolation: LuaFallible<LuaInterpolation>,
        local: LuaFallible<LuaMatrix>,
    ) -> LuaShader {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::radial_gradient_with_interpolation(
            (center, radius),
            (
                stops.colors.as_slice(),
                color_space.map(LuaColorSpace::unwrap),
            ),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        )
        .map(LuaShader))
    }
    fn make_sweep(
        center: LuaPoint,
        stops: ColorStops,
        color_space: LuaFallible<LuaColorSpace>,
        tile_mode: LuaFallible<LuaTileMode>,
        angles: LuaFallible<(f32, f32)>,
        interpolation: LuaFallible<LuaInterpolation>,
        local: LuaFallible<LuaMatrix>,
    ) -> LuaShader {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::sweep_gradient_with_interpolation(
            center,
            (
                stops.colors.as_slice(),
                color_space.map(LuaColorSpace::unwrap),
            ),
            Some(stops.positions.as_slice()),
            tile_mode,
            *angles,
            interpolation,
            local.as_ref(),
        )
        .map(LuaShader))
    }
    fn make_two_point_conical(
        start: LuaPoint,
        start_radius: f32,
        end: LuaPoint,
        end_radius: f32,
        stops: ColorStops,
        color_space: LuaFallible<LuaColorSpace>,
        tile_mode: LuaFallible<LuaTileMode>,
        interpolation: LuaFallible<LuaInterpolation>,
        local: LuaFallible<LuaMatrix>,
    ) -> LuaShader {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::two_point_conical_gradient_with_interpolation(
            (start, start_radius),
            (end, end_radius),
            (
                stops.colors.as_slice(),
                color_space.map(LuaColorSpace::unwrap),
            ),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        )
        .map(LuaShader))
    }
}

wrap_skia_handle!(Image);

#[lua_methods(lua_name: Image)]
impl LuaImage {
    fn load(path: String) -> LuaImage {
        let handle: Data = Data::new_copy(
            &std::fs::read(path).map_err(|io_err| mlua::Error::RuntimeError(io_err.to_string()))?,
        );
        Image::from_encoded(handle)
            .map(LuaImage)
            .ok_or(LuaError::RuntimeError(
                "unsupported encoded image format".to_string(),
            ))
    }
    fn width(&self) -> usize {
        Ok(self.0.width() as usize)
    }
    fn height(&self) -> usize {
        Ok(self.0.height() as usize)
    }
    fn new_shader(
        &self,
        tile_x: LuaFallible<LuaTileMode>,
        tile_y: LuaFallible<LuaTileMode>,
        sampling: LuaFallible<LuaSamplingOptions>,
        local_matrix: LuaFallible<LuaMatrix>,
    ) -> LuaShader {
        let tile_modes = if tile_x.is_none() && tile_y.is_none() {
            None
        } else {
            let n_tile_x = tile_x.unwrap_or_t(TileMode::Clamp);
            let n_tile_y = tile_y.unwrap_or_t(n_tile_x);
            Some((n_tile_x, n_tile_y))
        };
        let local_matrix = local_matrix.map(LuaMatrix::into);

        Ok(self
            .to_shader(
                tile_modes,
                sampling.unwrap_or_default(),
                local_matrix.as_ref(),
            )
            .map(LuaShader))
    }
}

wrap_skia_handle!(ColorSpace);

impl Default for LuaColorSpace {
    fn default() -> Self {
        LuaColorSpace(ColorSpace::new_srgb())
    }
}

#[lua_methods(lua_name: ColorSpace)]
impl LuaColorSpace {
    fn make_srgb() -> LuaColorSpace {
        Ok(LuaColorSpace(ColorSpace::new_srgb()))
    }
    fn make_srgb_linear() -> LuaColorSpace {
        Ok(LuaColorSpace(ColorSpace::new_srgb_linear()))
    }
    fn is_srgb(&self) -> bool {
        Ok(self.0.is_srgb())
    }
    fn to_xyzd50_hash(&self) -> u32 {
        Ok(self.0.to_xyzd50_hash().0)
    }
    fn make_linear_gamma(&self) -> LuaColorSpace {
        Ok(LuaColorSpace(self.with_linear_gamma()))
    }
    fn make_srgb_gamma(&self) -> LuaColorSpace {
        Ok(LuaColorSpace(self.with_srgb_gamma()))
    }
    fn make_color_spin(&self) -> LuaColorSpace {
        Ok(LuaColorSpace(self.with_color_spin()))
    }
}

wrap_skia_handle!(Picture);

#[lua_methods(lua_name: Picture)]
impl LuaPicture {
    fn playback(&self, canvas: &LuaCanvas) {
        self.0.playback(canvas);
        Ok(())
    }
    fn cull_rect(&self) -> LuaRect {
        Ok(LuaRect::from(self.0.cull_rect()))
    }
    fn approximate_op_count(&self, nested: Option<bool>) -> usize {
        Ok(self
            .0
            .approximate_op_count_nested(nested.unwrap_or_default()))
    }
    fn approximate_bytes_used(&self) -> usize {
        Ok(self.0.approximate_bytes_used())
    }
    fn make_shader(
        &self,
        tile_x: Option<LuaTileMode>,
        tile_y: Option<LuaTileMode>,
        mode: Option<LuaFilterMode>,
        local_matrix: Option<LuaMatrix>,
        tile_rect: Option<LuaRect>,
    ) -> LuaShader {
        let tm = if tile_x.is_none() && tile_y.is_none() {
            None
        } else {
            let n_tile_x = tile_x.unwrap_or_t(TileMode::Clamp);
            let n_tile_y = tile_x.unwrap_or_t(n_tile_x);
            Some((n_tile_x, n_tile_y))
        };
        let mode = mode.unwrap_or_t(FilterMode::Nearest);
        let local_matrix: Option<Matrix> = local_matrix.map(LuaMatrix::into);
        let tile_rect: Option<Rect> = tile_rect.map(LuaRect::into);

        Ok(LuaShader(self.0.to_shader(
            tm,
            mode,
            local_matrix.as_ref(),
            tile_rect.as_ref(),
        )))
    }
}

wrap_skia_handle!(ImageFilter);

#[lua_methods(lua_name: ImageFilter)]
impl LuaImageFilter {
    fn arithmetic(
        k1: f32,
        k2: f32,
        k3: f32,
        k4: f32,
        enforce_pm_color: bool,
        background: LuaFallible<LuaImageFilter>,
        foreground: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let background = background.map(LuaImageFilter::unwrap);
        let foreground = foreground.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::arithmetic(
            k1,
            k2,
            k3,
            k4,
            enforce_pm_color,
            background,
            foreground,
            crop_rect,
        )
        .map(LuaImageFilter))
    }

    fn blend(
        mode: LuaBlendMode,
        background: LuaFallible<LuaImageFilter>,
        foreground: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let background = background.map(LuaImageFilter::unwrap);
        let foreground = foreground.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::blend(*mode, background, foreground, crop_rect).map(LuaImageFilter))
    }

    fn blur(
        sigma_x: f32,
        sigma_y: LuaFallible<f32>,
        tile_mode: LuaFallible<LuaTileMode>,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        if !sigma_x.is_finite() || sigma_x < 0f32 {
            return Err(LuaError::RuntimeError(
                "x sigma must be a positive, finite scalar".to_string(),
            ));
        }
        let sigma_y = match *sigma_y {
            Some(sigma_y) if !sigma_y.is_finite() || sigma_y < 0f32 => {
                return Err(LuaError::RuntimeError(
                    "y sigma must be a positive, finite scalar".to_string(),
                ));
            }
            Some(it) => it,
            None => sigma_x,
        };

        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(
            image_filters::blur((sigma_x, sigma_y), tile_mode.map_t(), input, crop_rect)
                .map(LuaImageFilter),
        )
    }

    fn color_filter(
        cf: LuaColorFilter,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::color_filter(cf.unwrap(), input, crop_rect).map(LuaImageFilter))
    }

    fn compose(outer: LuaImageFilter, inner: LuaImageFilter) -> _ {
        Ok(image_filters::compose(outer.unwrap(), inner.unwrap()).map(LuaImageFilter))
    }

    fn crop(
        rect: LuaRect,
        tile_mode: LuaFallible<LuaTileMode>,
        input: LuaFallible<LuaImageFilter>,
    ) -> LuaImageFilter {
        let rect: Rect = rect.into();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::crop(&rect, tile_mode.map_t(), input).map(LuaImageFilter))
    }

    fn dilate(
        radius_x: f32,
        radius_y: LuaFallible<f32>,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        if !radius_x.is_finite() || radius_x < 0f32 {
            return Err(LuaError::RuntimeError(
                "x radius must be a positive, finite scalar".to_string(),
            ));
        }
        let radius_y = match *radius_y {
            Some(radius_y) if !radius_y.is_finite() || radius_y < 0f32 => {
                return Err(LuaError::RuntimeError(
                    "y radius must be a positive, finite scalar".to_string(),
                ));
            }
            Some(it) => it,
            None => radius_x,
        };
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::dilate((radius_x, radius_y), input, crop_rect).map(LuaImageFilter))
    }

    fn displacement_map(
        x_channel_selector: LuaColorChannel,
        y_channel_selector: LuaColorChannel,
        scale: f32,
        displacement: LuaFallible<LuaImageFilter>,
        color: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let displacement = displacement.map(LuaImageFilter::unwrap);
        let color = color.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::displacement_map(
            (x_channel_selector.unwrap(), y_channel_selector.unwrap()),
            scale,
            displacement,
            color,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn distant_lit_diffuse(
        direction: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::distant_lit_diffuse(
            direction,
            light_color,
            surface_scale,
            kd,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn distant_lit_specular(
        direction: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        ks: f32,
        shininess: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::distant_lit_specular(
            direction,
            light_color,
            surface_scale,
            ks,
            shininess,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn drop_shadow(
        offset: LuaPoint,
        sigma_x: f32,
        sigma_y: f32,
        color: LuaColor,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(
            image_filters::drop_shadow(offset, (sigma_x, sigma_y), color, input, crop_rect)
                .map(LuaImageFilter),
        )
    }
    fn drop_shadow_only(
        offset: LuaPoint,
        sigma_x: f32,
        sigma_y: f32,
        color: LuaColor,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(
            image_filters::drop_shadow_only(offset, (sigma_x, sigma_y), color, input, crop_rect)
                .map(LuaImageFilter),
        )
    }
    fn empty() -> LuaImageFilter {
        Ok(LuaImageFilter(image_filters::empty()))
    }
    fn erode(
        radius_x: f32,
        radius_y: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::erode((radius_x, radius_y), input, crop_rect).map(LuaImageFilter))
    }
    fn image(
        image: LuaImage,
        src_rect: LuaFallible<LuaRect>,
        dst_rect: LuaFallible<LuaRect>,
        sampling: LuaFallible<LuaSamplingOptions>,
    ) -> LuaImageFilter {
        let src_rect: Option<Rect> = src_rect.map(LuaRect::into);
        let dst_rect: Option<Rect> = dst_rect.map(LuaRect::into);
        let sampling: SamplingOptions = sampling.unwrap_or_default().into();
        Ok(image_filters::image(
            image.unwrap(),
            src_rect.as_ref(),
            dst_rect.as_ref(),
            sampling,
        )
        .map(LuaImageFilter))
    }
    fn magnifier(
        lens_bounds: LuaRect,
        zoom_amount: f32,
        inset: f32,
        sampling: LuaFallible<LuaSamplingOptions>,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let lens_bounds: Rect = lens_bounds.into();
        let sampling: SamplingOptions = sampling.unwrap_or_default().into();
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(
            image_filters::magnifier(lens_bounds, zoom_amount, inset, sampling, input, crop_rect)
                .map(LuaImageFilter),
        )
    }
    fn matrix_convolution(
        kernel_size: LuaSize,
        kernel: Vec<f32>,
        gain: f32,
        bias: f32,
        kernel_offset: LuaPoint,
        tile_mode: LuaTileMode,
        convolve_alpha: bool,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::matrix_convolution(
            kernel_size,
            &kernel,
            gain,
            bias,
            kernel_offset,
            *tile_mode,
            convolve_alpha,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn matrix_transform(
        matrix: LuaMatrix,
        sampling: LuaFallible<LuaSamplingOptions>,
        input: LuaFallible<LuaImageFilter>,
    ) -> LuaImageFilter {
        let matrix: Matrix = matrix.into();
        let sampling = sampling.unwrap_or_default();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::matrix_transform(&matrix, sampling, input).map(LuaImageFilter))
    }
    fn merge(filters: Vec<LuaImageFilter>, crop_rect: LuaFallible<LuaRect>) -> LuaImageFilter {
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        let filters = filters.into_iter().map(|it| Some(it.unwrap()));
        Ok(image_filters::merge(filters, crop_rect).map(LuaImageFilter))
    }
    fn offset(
        offset: LuaPoint,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::offset(offset, input, crop_rect).map(LuaImageFilter))
    }
    fn picture(pic: LuaPicture, target_rect: LuaFallible<LuaRect>) -> LuaImageFilter {
        let target_rect: Option<Rect> = target_rect.map(LuaRect::into);
        Ok(image_filters::picture(pic.unwrap(), target_rect.as_ref()).map(LuaImageFilter))
    }
    fn point_lit_diffuse(
        location: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::point_lit_diffuse(
            location,
            light_color,
            surface_scale,
            kd,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn point_lit_specular(
        location: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        ks: f32,
        shininess: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::point_lit_specular(
            location,
            light_color,
            surface_scale,
            ks,
            shininess,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn shader(shader: LuaShader, crop_rect: LuaFallible<LuaRect>) -> LuaImageFilter {
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::shader(shader.unwrap(), crop_rect).map(LuaImageFilter))
    }
    fn spot_lit_diffuse(
        location: LuaPoint<3>,
        target: LuaPoint<3>,
        falloff_exponent: f32,
        cutoff_angle: f32,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();

        Ok(image_filters::spot_lit_diffuse(
            location,
            target,
            falloff_exponent,
            cutoff_angle,
            light_color,
            surface_scale,
            kd,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn spot_lit_specular(
        location: LuaPoint<3>,
        target: LuaPoint<3>,
        falloff_exponent: f32,
        cutoff_angle: f32,
        light_color: LuaColor,
        surface_scale: f32,
        ks: f32,
        shininess: f32,
        input: LuaFallible<LuaImageFilter>,
        crop_rect: LuaFallible<LuaRect>,
    ) -> LuaImageFilter {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect
            .map(|it| {
                let it: Rect = it.into();
                CropRect::from(it)
            })
            .unwrap_or_default();
        Ok(image_filters::spot_lit_specular(
            location,
            target,
            falloff_exponent,
            cutoff_angle,
            light_color,
            surface_scale,
            ks,
            shininess,
            input,
            crop_rect,
        )
        .map(LuaImageFilter))
    }
    fn tile(src: LuaRect, dst: LuaRect, input: LuaFallible<LuaImageFilter>) -> LuaImageFilter {
        let src: Rect = src.into();
        let dst: Rect = dst.into();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::tile(&src, &dst, input).map(LuaImageFilter))
    }

    fn filter_bounds(
        &self,
        src: LuaRect,
        ctm: LuaMatrix,
        map_direction: LuaMapDirection,
        input_rect: Option<LuaRect>,
    ) -> LuaRect {
        let src: IRect = src.into();
        let ctm: Matrix = ctm.into();
        let input_rect = input_rect.map(Into::<IRect>::into);
        let filtered = self
            .0
            .filter_bounds(src, &ctm, *map_direction, input_rect.as_ref());
        Ok(LuaRect::from(filtered))
    }
    fn is_color_filter_node(&self) -> LuaColorFilter {
        Ok(self.0.color_filter_node().map(LuaColorFilter))
    }
    fn as_a_color_filter(&self) -> LuaColorFilter {
        Ok(self.0.to_a_color_filter().map(LuaColorFilter))
    }
    fn count_inputs(&self) -> usize {
        Ok(self.0.count_inputs())
    }
    fn get_input(&self, index: usize) -> LuaImageFilter {
        Ok(self.0.get_input(index).map(LuaImageFilter))
    }
    fn compute_fast_bounds(&self, rect: LuaRect) -> LuaRect {
        let rect: Rect = rect.into();
        let bounds = self.0.compute_fast_bounds(rect);
        Ok(LuaRect::from(bounds))
    }
    fn can_compute_fast_bounds(&self) -> bool {
        Ok(self.0.can_compute_fast_bounds())
    }
    fn make_with_local_matrix(&self, matrix: LuaMatrix) -> LuaImageFilter {
        let matrix: Matrix = matrix.into();
        Ok(self.0.with_local_matrix(&matrix).map(LuaImageFilter))
    }
}

wrap_skia_handle!(ColorFilter);

#[lua_methods(lua_name: ColorFilter)]
impl LuaColorFilter {
    fn blend(
        color: LuaColor,
        color_space: LuaFallible<LuaColorSpace>,
        mode: LuaBlendMode,
    ) -> Option<LuaColorFilter> {
        // NYI: blend color filter color_space handling
        let mode = mode.unwrap();
        Ok(color_filters::blend(color, mode).map(LuaColorFilter))
    }
    fn compose(outer: LuaColorFilter, inner: LuaColorFilter) -> Option<LuaColorFilter> {
        Ok(color_filters::compose(outer, inner).map(LuaColorFilter))
    }
    // TODO: ColorFilters::HSLA_matrix(matrix: LuaColorMatrix)
    fn lerp(t: f32, source: LuaColorFilter, destination: LuaColorFilter) -> Option<LuaColorFilter> {
        Ok(color_filters::lerp(t, source, destination).map(LuaColorFilter))
    }
    fn lighting(multiply: LuaColor, add: LuaColor) -> Option<LuaColorFilter> {
        Ok(color_filters::lighting(multiply, add).map(LuaColorFilter))
    }
    fn linear_to_srgb_gamma() -> LuaColorFilter {
        Ok(LuaColorFilter(color_filters::linear_to_srgb_gamma()))
    }
    // TODO: ColorFilters::matrix(matrix: LuaColorMatrix)
    fn srgb_to_linear_gamma() -> LuaColorFilter {
        Ok(LuaColorFilter(color_filters::srgb_to_linear_gamma()))
    }
    // TODO: ColorFilters::table(table: LuaColorTable)
    // TODO: ColorFilters::table_ARGB(table: LuaColorTable)

    fn to_a_color_mode<'lua>(&self, lua: &'lua LuaContext) -> LuaValue<'lua> {
        if let Some((color, mode)) = self.0.to_a_color_mode() {
            let result = lua.create_table()?;
            result.set(0, LuaColor::from(color))?;
            result.set(1, LuaBlendMode(mode))?;
            Ok(LuaValue::Table(result))
        } else {
            Ok(LuaNil)
        }
    }

    fn to_a_color_matrix<'lua>(&self, lua: &'lua LuaContext) -> LuaValue<'lua> {
        if let Some(mx) = self.0.to_a_color_matrix() {
            Ok(LuaValue::Table(
                lua.create_table_from(mx.into_iter().enumerate())?,
            ))
        } else {
            Ok(LuaNil)
        }
    }

    fn is_alpha_unchanged(&self) -> bool {
        Ok(self.is_alpha_unchanged())
    }

    fn filter_color(
        &self,
        color: LuaColor,
        src_cs: Option<LuaColorSpace>,
        dst_cs: Option<LuaColorSpace>,
    ) -> LuaColor {
        Ok(match src_cs {
            None => LuaColor::from(self.0.filter_color(color)),
            Some(src_cs) => {
                let color: Color4f = color.into();
                LuaColor::from(self.filter_color4f(
                    &color,
                    &src_cs,
                    dst_cs.map(LuaColorSpace::unwrap).as_ref(),
                ))
            }
        })
    }

    fn make_composed(&self, inner: LuaColorFilter) -> LuaColorFilter {
        self.composed(inner.unwrap())
            .ok_or(LuaError::RuntimeError(
                "unable to compose filters".to_string(),
            ))
            .map(LuaColorFilter)
    }

    fn make_with_working_color_space(&self, color_space: LuaColorSpace) -> LuaColorFilter {
        self.with_working_color_space(color_space.unwrap())
            .ok_or(LuaError::RuntimeError(
                "unable to apply color space to filter".to_string(),
            ))
            .map(LuaColorFilter)
    }
}

wrap_skia_handle!(MaskFilter);

#[lua_methods(lua_name: MaskFilter)]
impl LuaMaskFilter {
    fn make_blur(style: LuaBlurStyle, sigma: f32, ctm: LuaFallible<bool>) -> Option<LuaMaskFilter> {
        Ok(MaskFilter::blur(style.unwrap(), sigma, *ctm).map(LuaMaskFilter))
    }
    fn approximate_filtered_bounds(&self, src: LuaRect) -> LuaRect {
        let src: Rect = src.into();
        Ok(LuaRect::from(self.0.approximate_filtered_bounds(src)))
    }
}

wrap_skia_handle!(DashInfo);
type_like!(DashInfo);

impl<'lua> TryFrom<LuaTable<'lua>> for LuaDashInfo {
    type Error = LuaError;
    fn try_from(t: LuaTable<'lua>) -> Result<Self, Self::Error> {
        let phase: f32 = t.get("phase").unwrap_or_default();
        if let Ok(intervals) = t.get("intervals") {
            return Ok(LuaDashInfo(DashInfo { intervals, phase }));
        } else {
            let intervals: Vec<f32> = t
                .sequence_values::<f32>()
                .take_while(|it| it.is_ok())
                .map(|it| it.unwrap())
                .collect();

            if intervals.len() > 0 {
                return Ok(LuaDashInfo(DashInfo { intervals, phase }));
            }
        }
        return Err(LuaError::FromLuaConversionError {
            from: "table",
            to: "DashInfo",
            message: Some("not a valid DashInfo".to_string()),
        });
    }
}

/// ## Supported formats
/// - [`LuaDashInfo`]
/// - intervals: {number...}, phase: number
/// - {intervals: {number...}, phase: number}
impl<'lua> FromArgPack<'lua> for LikeDashInfo {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        if let Some(ud) = args.pop_typed::<LuaAnyUserData>() {
            return Ok(LikeDashInfo(ud.borrow::<LuaDashInfo>()?.to_owned()));
        }

        let table = args.pop_typed_or::<LuaTable<'lua>, _>(Some("expected DashInfo or table"))?;

        if table.get::<_, LuaTable<'lua>>("intervals").is_ok() {
            return match LuaDashInfo::try_from(table.clone()) {
                Ok(it) => Ok(LikeDashInfo(it)),
                Err(err) => Err(err),
            };
        }

        let intervals: Vec<f32> = FromLua::from_lua(LuaValue::Table(table), lua)?;
        let phase: f32 = args.pop_typed().unwrap_or_default();

        Ok(LikeDashInfo(LuaDashInfo(DashInfo { intervals, phase })))
    }
}

#[lua_methods(lua_name: DashInfo)]
impl LuaDashInfo {
    fn get_intervals(&self) -> Vec<f32> {
        Ok(self.intervals.clone())
    }
    fn get_phase(&self) -> f32 {
        Ok(self.phase)
    }
}

wrap_skia_handle!(StrokeRec);

impl Default for LuaStrokeRec {
    fn default() -> Self {
        LuaStrokeRec(StrokeRec::new(StrokeRecInitStyle::Fill))
    }
}

#[lua_methods(lua_name: StrokeRec)]
impl LuaStrokeRec {
    fn make<'lua>(lua: &'lua LuaContext, args: LuaMultiValue<'lua>) -> LuaStrokeRec {
        let mut args = args.into_iter();

        let mut stroke_rec = LuaStrokeRec::default();

        let first = match args.next() {
            Some(it) => it,
            None => return Ok(stroke_rec),
        };

        let paint = match first {
            LuaNil => return Ok(stroke_rec),
            LuaValue::String(init_style) => {
                let init_style = LuaStrokeRecInitStyle::try_from(init_style)?;
                return Ok(LuaStrokeRec(StrokeRec::new(*init_style)));
            }
            LuaValue::Table(paint_like) => LuaPaint::try_from((paint_like, lua))?,
            LuaValue::UserData(ud) if ud.is::<LuaPaint>() => ud.borrow::<LuaPaint>()?.to_owned(),
            other => {
                return Err(LuaError::RuntimeError(format!(
                    "StrokeRec constructor requires string or Paint; got: {}",
                    other.type_name()
                )))
            }
        };

        stroke_rec.set_stroke_params(
            paint.stroke_cap(),
            paint.stroke_join(),
            paint.stroke_miter(),
        );

        match args.next() {
            None => {
                return Ok(stroke_rec)
            }
            Some(LuaValue::String(style)) => {
                let stroke_and_fill = *LuaPaintStyle::try_from(style)? != PaintStyle::Stroke;
                let width = stroke_rec.width();
                stroke_rec.0.set_stroke_style(width, stroke_and_fill)
            }
            Some(LuaValue::Number(number)) => {
                stroke_rec.set_res_scale(number as f32);
                return Ok(stroke_rec);
            }
            Some(LuaValue::Integer(number)) => {
                stroke_rec.set_res_scale(number as f32);
                return Ok(stroke_rec);
            }
            Some(other) => return Err(LuaError::RuntimeError(
                format!("StrokeRec constructor requires style (string) or resScale (number) as second argument; got: {}", other.type_name())
            )),
        };

        match args.next() {
            None => return Ok(stroke_rec),
            Some(LuaValue::Number(number)) => {
                stroke_rec.set_res_scale(number as f32);
            }
            Some(LuaValue::Integer(number)) => {
                stroke_rec.set_res_scale(number as f32);
            }
            Some(other) => {
                return Err(LuaError::RuntimeError(format!(
                    "StrokeRec constructor requires resScale (number) as third argument; got: {}",
                    other.type_name()
                )))
            }
        };

        Ok(stroke_rec)
    }

    fn get_style(&self) -> LuaStrokeRecStyle {
        Ok(LuaStrokeRecStyle(self.style()))
    }
    fn get_width(&self) -> f32 {
        Ok(self.width())
    }
    fn get_miter(&self) -> f32 {
        Ok(self.miter())
    }
    fn get_cap(&self) -> LuaPaintCap {
        Ok(LuaPaintCap(self.cap()))
    }
    fn get_join(&self) -> LuaPaintJoin {
        Ok(LuaPaintJoin(self.join()))
    }
    fn is_hairline_style(&self) -> bool {
        Ok(self.is_hairline_style())
    }
    fn is_fill_style(&self) -> bool {
        Ok(self.is_fill_style())
    }
    fn set_fill_style(&mut self) {
        self.set_fill_style();
        Ok(())
    }
    fn set_hairline_style(&mut self) {
        self.set_hairline_style();
        Ok(())
    }
    fn set_stroke_style(&mut self, width: f32, stroke_and_fill: Option<bool>) {
        self.set_stroke_style(width, stroke_and_fill);
        Ok(())
    }
    fn set_stroke_params(&mut self, cap: LuaPaintCap, join: LuaPaintJoin, miter_limit: f32) {
        self.set_stroke_params(*cap, *join, miter_limit);
        Ok(())
    }
    fn get_res_scale(&self) -> f32 {
        Ok(self.res_scale())
    }
    fn set_res_scale(&mut self, scale: f32) {
        self.set_res_scale(scale);
        Ok(())
    }
    fn need_to_apply(&self) -> bool {
        Ok(self.need_to_apply())
    }
    fn apply_to_path(&self, path: LuaPath) -> LuaPath {
        let mut result = Path::new();
        self.0.apply_to_path(&mut result, &path);
        Ok(LuaPath(result))
    }
    fn apply_to_paint(&self, mut paint: LuaPaint) -> LuaPaint {
        self.apply_to_paint(&mut paint);
        Ok(paint)
    }
    fn get_inflation_radius(&self) -> f32 {
        Ok(self.inflation_radius())
    }
    fn has_equal_effect(&self, other: Self) -> bool {
        Ok(self.0.has_equal_effect(&other))
    }
}

wrap_skia_handle!(PathEffect);

#[lua_methods(lua_name: PathEffect)]
impl LuaPathEffect {
    fn make_sum(first: LuaPathEffect, second: LuaPathEffect) -> LuaPathEffect {
        Ok(LuaPathEffect(path_effect::PathEffect::sum(
            first.0, second.0,
        )))
    }
    fn make_compose(outer: LuaPathEffect, inner: LuaPathEffect) -> LuaPathEffect {
        Ok(LuaPathEffect(path_effect::PathEffect::compose(
            outer.0, inner.0,
        )))
    }
    fn make_dash(like_dash: LikeDashInfo) -> Option<LuaPathEffect> {
        Ok(
            skia_safe::dash_path_effect::new(&like_dash.intervals, like_dash.phase)
                .map(LuaPathEffect),
        )
    }
    fn make_trim(start: f32, stop: f32, mode: LuaFallible<LuaTrimMode>) -> Option<LuaPathEffect> {
        Ok(skia_safe::trim_path_effect::new(start, stop, mode.map_t()).map(LuaPathEffect))
    }
    fn make_radius(radius: f32) -> Option<LuaPathEffect> {
        Ok(skia_safe::corner_path_effect::new(radius).map(LuaPathEffect))
    }
    fn make_discrete(length: f32, dev: f32, seed: LuaFallible<u32>) -> Option<LuaPathEffect> {
        Ok(skia_safe::discrete_path_effect::new(length, dev, *seed).map(LuaPathEffect))
    }
    fn make_2d_path(width: f32, mx: LuaMatrix) -> Option<LuaPathEffect> {
        let mx: Matrix = mx.into();
        Ok(skia_safe::line_2d_path_effect::new(width, &mx).map(LuaPathEffect))
    }

    fn as_a_dash(&self) -> Option<LuaDashInfo> {
        Ok(self.as_a_dash().map(LuaDashInfo))
    }

    fn filter_path<'lua>(
        &self,
        lua: &'lua LuaContext,
        src: LuaPath,
        stroke_rec: LuaStrokeRec,
        cull_rect: LuaRect,
        ctm: Option<LuaMatrix>,
    ) -> LuaValue<'lua> {
        let cull_rect: Rect = cull_rect.into();
        let mut dst = Path::new();
        let mut stroke_rec = stroke_rec.unwrap();
        match ctm {
            None => match self.0.filter_path(&src, &stroke_rec, cull_rect) {
                Some((new_dst, new_stroke_rec)) => {
                    dst = new_dst;
                    stroke_rec = new_stroke_rec;
                }
                None => return Ok(LuaNil),
            },
            Some(ctm) => {
                if !self.0.filter_path_inplace_with_matrix(
                    &mut dst,
                    &src,
                    &mut stroke_rec,
                    cull_rect,
                    &ctm.into(),
                ) {
                    return Ok(LuaNil);
                }
            }
        };
        let result = lua.create_table()?;
        result.set(0, LuaPath(dst))?;
        result.set(1, LuaStrokeRec(stroke_rec))?;
        Ok(LuaValue::Table(result))
    }

    fn needs_ctm(&self) -> bool {
        Ok(self.0.needs_ctm())
    }
}

#[derive(Clone)]
pub enum LuaMatrix {
    Three(Matrix),
    Four(M44),
}

impl<'lua> FromClonedUD<'lua> for LuaMatrix {}

impl Into<Matrix> for LuaMatrix {
    fn into(self) -> Matrix {
        match self {
            LuaMatrix::Three(it) => it,
            LuaMatrix::Four(other) => other.to_m33(),
        }
    }
}
impl Into<M44> for LuaMatrix {
    fn into(self) -> M44 {
        match self {
            LuaMatrix::Four(it) => it,
            #[rustfmt::skip]
            LuaMatrix::Three(other) => {
                let m = other.as_slice();
                M44::row_major(&[
                    m[0], m[1], 0., m[2],
                    m[3], m[4], 0., m[5],
                      0.,   0., 1.,   0.,
                    m[6], m[7], 0., m[8],
                ])
            }
        }
    }
}

#[lua_methods(lua_name: Matrix)]
impl LuaMatrix {
    fn new(argument: Option<LuaValue>) -> LuaMatrix {
        let argument = match argument {
            Some(it) => it,
            None => return Ok(LuaMatrix::Three(Matrix::new_identity())),
        };

        let dim = match argument {
            LuaValue::Number(num) => num as usize,
            LuaValue::Integer(num) => num as usize,
            LuaValue::Table(values) => {
                let values: Vec<f32> = values
                    .sequence_values::<f32>()
                    .take_while(Result::is_ok)
                    .filter_map(Result::ok)
                    .collect();

                match values.len() {
                    9 => {
                        return Ok(LuaMatrix::Three(unsafe {
                            Matrix::from_vec(values).unwrap_unchecked()
                        }))
                    }
                    16 => {
                        return Ok(LuaMatrix::Four(unsafe {
                            M44::from_vec(values).unwrap_unchecked()
                        }))
                    }
                    other => {
                        return Err(LuaError::FromLuaConversionError {
                            from: "table",
                            to: "Matrix",
                            message: Some(format!(
                            "expected a table with either 9 or 16 number values; instead got: {}",
                            other
                        )),
                        })
                    }
                }
            }
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Matrix",
                    message: None,
                })
            }
        };

        match dim {
            3 => Ok(LuaMatrix::Three(Matrix::new_identity())),
            4 => Ok(LuaMatrix::Four(M44::new_identity())),
            other => Err(LuaError::RuntimeError(format!(
                "unsupported matrix size ({}); supported sizes are: 3, 4",
                other
            ))),
        }
    }

    fn get_dimensions(&self) -> LuaMatrix {
        match self {
            LuaMatrix::Three(_) => Ok(3),
            LuaMatrix::Four(_) => Ok(4),
        }
    }
    fn get(&self, pos: LuaPoint) -> LuaMatrix {
        let [col, row] = pos.as_array().map(|it| it as usize);
        match self {
            LuaMatrix::Three(it) => {
                let i = col + row * 3;
                if i < 9 && col < 3 {
                    Ok(LuaValue::Number(it.as_slice()[i] as f64))
                } else {
                    Ok(LuaNil)
                }
            }
            LuaMatrix::Four(it) => {
                let i = col + row * 4;
                if i < 16 && col < 4 {
                    Ok(LuaValue::Number(it.as_slice()[i] as f64))
                } else {
                    Ok(LuaNil)
                }
            }
        }
    }
    fn set(&mut self, pos: LuaPoint, value: f32) {
        let [col, row] = pos.as_array().map(|it| it as usize);
        match self {
            LuaMatrix::Three(it) => {
                let i = col + row * 3;
                if i < 9 && col < 3 {
                    it.as_slice_mut()[i] = value;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            LuaMatrix::Four(it) => {
                let i = col + row * 4;
                if i < 16 && col < 4 {
                    it.as_slice_mut()[i] = value;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }
    fn get_type<'lua>(&self, lua: &'lua LuaContext) -> LuaTypeMask {
        match self {
            LuaMatrix::Three(it) => LuaTypeMask(it.get_type())
                .to_table(lua)
                .map(LuaValue::Table),
            LuaMatrix::Four(_) => Ok(LuaNil),
        }
    }
    fn get_scale_x(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.scale_x(),
            LuaMatrix::Four(it) => it.row(0)[0],
        })
    }
    fn set_scale_x(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_scale_x(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[0] = value;
            }
        }
        Ok(true)
    }
    fn get_scale_y(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.scale_y(),
            LuaMatrix::Four(it) => it.row(1)[1],
        })
    }
    fn set_scale_y(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_scale_y(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[5] = value;
            }
        }
        Ok(true)
    }
    fn get_scale_z(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(_) => LuaValue::Number(1.0 as f64),
            LuaMatrix::Four(it) => LuaValue::Number(it.row(2)[2] as f64),
        })
    }
    fn set_scale_z(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(_) => Ok(false),
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[10] = value;
                Ok(true)
            }
        }
    }
    fn get_translate_x(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.translate_x(),
            LuaMatrix::Four(it) => it.row(0)[3],
        })
    }
    fn set_translate_x(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_translate_x(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[3] = value;
            }
        }
        Ok(true)
    }
    fn get_translate_y(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.translate_y(),
            LuaMatrix::Four(it) => it.row(1)[3],
        })
    }
    fn set_translate_y(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_translate_y(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[7] = value;
            }
        }
        Ok(true)
    }
    fn get_translate_z(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(_) => LuaValue::Number(0.0 as f64),
            LuaMatrix::Four(it) => LuaValue::Number(it.row(2)[3] as f64),
        })
    }
    fn set_translate_z(&mut self, value: f32) -> bool {
        match self {
            LuaMatrix::Three(_) => Ok(false),
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[11] = value;
                Ok(true)
            }
        }
    }
    fn get_skew_x(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.skew_x(),
            LuaMatrix::Four(it) => it.row(0)[1],
        })
    }
    fn set_skew_x(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_skew_x(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[1] = value;
            }
        }
        Ok(true)
    }
    fn get_skew_y(&self) -> f32 {
        Ok(match self {
            LuaMatrix::Three(it) => it.skew_y(),
            LuaMatrix::Four(it) => it.row(1)[0],
        })
    }
    fn set_skew_y(&mut self, value: f32) {
        match self {
            LuaMatrix::Three(it) => {
                it.set_skew_y(value);
            }
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[4] = value;
            }
        }
        Ok(true)
    }
    fn set_rect_to_rect(&mut self, from: LuaRect, to: LuaRect, stf: LuaScaleToFit) -> bool {
        let from: Rect = from.into();
        let to: Rect = to.into();
        Ok(match self {
            LuaMatrix::Three(it) => it.set_rect_to_rect(from, to, *stf),
            #[rustfmt::skip]
            LuaMatrix::Four(it) => {
                let mut mat = Matrix::new_identity();
                let result = mat.set_rect_to_rect(from, to, *stf);
                *it = M44::row_major(&[
                    mat[0], 0.0,    0.0, mat[2],
                    0.0,    mat[4], 0.0, mat[5],
                    0.0,    0.0,    1.0, 0.0,
                    0.0,    0.0,    0.0, 1.0,
                ]);
                result
            }
        })
    }
    fn invert(&self) -> LuaMatrix {
        Ok(match self {
            LuaMatrix::Three(mx) => mx.invert().map(LuaMatrix::Three),
            LuaMatrix::Four(mx) => mx.invert().map(LuaMatrix::Four),
        })
    }
    fn transpose(&self) -> LuaMatrix {
        Ok(match self {
            #[rustfmt::skip]
            LuaMatrix::Three(it) => {
                LuaMatrix::Three(Matrix::new_all(
                    it[0], it[3], it[6],
                    it[1], it[4], it[7],
                    it[2], it[5], it[8]
                ))
            },
            LuaMatrix::Four(it) => LuaMatrix::Four(it.transpose()),
        })
    }
    fn map_xy<'lua>(&self, lua: &'lua LuaContext, point: LuaPoint) -> LuaTable<'lua> {
        let result = lua.create_table()?;
        match self {
            LuaMatrix::Three(it) => {
                it.map_xy(point.x(), point.y());
                result.set(0, point.x())?;
                result.set(1, point.y())?;
            }
            LuaMatrix::Four(it) => {
                let out = it.map(point.x(), point.y(), 0.0, 1.0);
                result.set(0, out.x)?;
                result.set(1, out.y)?;
            }
        }
        Ok(result)
    }
    fn map_xyz<'lua>(&self, lua: &'lua LuaContext, point: LuaPoint<3>) -> LuaTable<'lua> {
        let result = lua.create_table()?;
        match self {
            LuaMatrix::Three(it) => {
                it.map_xy(point.x(), point.y());
                result.set(0, point.x())?;
                result.set(1, point.y())?;
                result.set(2, point.z())?;
            }
            LuaMatrix::Four(it) => {
                let out = it.map(point.x(), point.y(), point.z(), 1.0);
                result.set(0, out.x)?;
                result.set(1, out.y)?;
                result.set(2, out.z)?;
            }
        }
        Ok(result)
    }
    fn map_rect(&self, rect: LuaRect) -> LuaRect {
        let rect: Rect = rect.into();
        let mapped = match self {
            LuaMatrix::Three(it) => it.map_rect(rect).0,
            LuaMatrix::Four(it) => {
                let a = it.map(rect.left, rect.top, 0.0, 1.0);
                let b = it.map(rect.right, rect.bottom, 0.0, 1.0);
                Rect::new(a.x, a.y, b.x, b.y)
            }
        };
        Ok(LuaRect::from(mapped))
    }
}

wrap_skia_handle!(Paint);

type_like_table!(Paint: |value: LuaTable, lua: &'lua Lua| {
    let mut paint = Paint::default();

    let color_space = value.try_get_t::<_, LuaColorSpace>("color_space", lua)?;
    if let Ok(color) = LuaColor::from_lua(LuaValue::Table(value.clone()), lua) {
        let color: Color4f = color.into();
        paint.set_color4f(color, color_space.as_ref());
    }

    if let Some(aa) = value.try_get::<_, bool>("anti_alias", lua)? {
        paint.set_anti_alias(aa);
    }

    if let Some(dither) = value.try_get::<_, bool>("dither",lua)? {
        paint.set_dither(dither);
    }

    if let Some(image_filter) = value.try_get_t::<_, LuaImageFilter>("image_filter",lua)? {
        paint.set_image_filter(image_filter);
    }
    if let Some(mask_filter) = value.try_get_t::<_, LuaMaskFilter>("mask_filter",lua)? {
        paint.set_mask_filter(mask_filter);
    }
    if let Some(color_filter) = value.try_get_t::<_, LuaColorFilter>("color_filter", lua)? {
        paint.set_color_filter(color_filter);
    }

    if let Some(style) = value.try_get_t::<_, LuaPaintStyle>("style", lua)? {
        paint.set_style(style);
    }
    if let Some(cap) = value.try_get_t::<_, LuaPaintCap>("stroke_cap", lua)?.or(value.try_get_t::<_, LuaPaintCap>("cap", lua)?) {
        paint.set_stroke_cap(cap);
    }
    if let Some(join) = value.try_get_t::<_, LuaPaintJoin>("stroke_join", lua)?.or(value.try_get_t::<_, LuaPaintJoin>("join", lua)?) {
        paint.set_stroke_join(join);
    }
    if let Some(width) = value.try_get::<_, f32>("stroke_width", lua)?.or(value.try_get::<_, f32>("width", lua)?) {
        paint.set_stroke_width(width);
    }
    if let Some(miter) = value.try_get::<_, f32>("stroke_miter", lua)?.or(value.try_get::<_, f32>("miter", lua)?) {
        paint.set_stroke_miter(miter);
    }
    if let Some(path_effect) = value.try_get_t::<_, LuaPathEffect>("path_effect", lua)? {
        paint.set_path_effect(path_effect);
    }

    if let Some(shader) = value.try_get_t::<_, LuaShader>("shader", lua)? {
        paint.set_shader(Some(shader));
    }

    return Ok(LuaPaint(paint))
});

#[lua_methods(lua_name: Paint)]
impl LuaPaint {
    fn make(color: Option<LuaColor>, color_space: Option<LuaColorSpace>) -> LuaPaint {
        let paint = match (color, color_space) {
            (None, None) => Paint::default(),
            (Some(color), None) => {
                let color: Color4f = color.into();
                Paint::new(color, None)
            }
            (Some(color), Some(color_space)) => {
                let color: Color4f = color.into();
                Paint::new(color, Some(&*color_space))
            }
            (None, Some(color_space)) => {
                let color: Color4f = Color::BLACK.into();
                Paint::new(color, Some(&*color_space))
            }
        };
        Ok(LuaPaint(paint))
    }

    fn is_anti_alias(&self) -> bool {
        Ok(self.is_anti_alias())
    }
    fn set_anti_alias(&mut self, anti_alias: bool) {
        self.set_anti_alias(anti_alias);
        Ok(())
    }
    fn is_dither(&self) -> bool {
        Ok(self.is_dither())
    }
    fn set_dither(&mut self, dither: bool) {
        self.set_dither(dither);
        Ok(())
    }
    fn get_image_filter(&self) -> LuaImageFilter {
        Ok(self.image_filter().map(LuaImageFilter))
    }
    fn set_image_filter(&mut self, image_filter: Option<LuaImageFilter>) {
        self.set_image_filter(image_filter.map(LuaImageFilter::unwrap));
        Ok(())
    }
    fn get_mask_filter(&self) -> LuaMaskFilter {
        Ok(self.mask_filter().map(LuaMaskFilter))
    }
    fn set_mask_filter(&mut self, mask_filter: Option<LuaMaskFilter>) {
        self.0
            .set_mask_filter(mask_filter.map(LuaMaskFilter::unwrap));
        Ok(())
    }
    fn get_color_filter(&self) -> LuaColorFilter {
        Ok(self.color_filter().map(LuaColorFilter))
    }
    fn set_color_filter(&mut self, color_filter: Option<LuaColorFilter>) {
        self.set_color_filter(color_filter.map(LuaColorFilter::unwrap));
        Ok(())
    }
    fn get_alpha(&self) -> f32 {
        Ok(self.alpha_f())
    }
    fn set_alpha(&mut self, alpha: f32) {
        self.set_alpha_f(alpha);
        Ok(())
    }
    fn get_color(&self) -> LuaColor {
        Ok(LuaColor::from(self.color4f()))
    }
    fn set_color(&mut self, color: LuaColor, color_space: Option<LuaColorSpace>) {
        let color: Color4f = color.into();
        self.set_color4f(color, color_space.map(LuaColorSpace::unwrap).as_ref());
        Ok(())
    }
    fn get_style<'lua>(&self, lua: &'lua LuaContext) -> LuaTable {
        let result = lua.create_table()?;
        match self.style() {
            skia_safe::paint::Style::Fill => {
                result.set("fill", true)?;
                result.set("stroke", false)?;
            }
            skia_safe::paint::Style::Stroke => {
                result.set("fill", false)?;
                result.set("stroke", true)?;
            }
            skia_safe::paint::Style::StrokeAndFill => {
                result.set("fill", true)?;
                result.set("stroke", true)?;
            }
        }
        Ok(result)
    }
    fn set_style(&mut self, style: LuaTable) {
        let fill: bool = style.get("fill").unwrap_or_default();
        let stroke: bool = style.get("stroke").unwrap_or_default();
        self.set_style(match (fill, stroke) {
            (true, false) => skia_safe::paint::Style::Fill,
            (false, true) => skia_safe::paint::Style::Stroke,
            (true, true) => skia_safe::paint::Style::StrokeAndFill,
            (false, false) => {
                return Err(LuaError::RuntimeError(
                    "invalid paint style; neither 'fill' nor 'stroke' is true".to_string(),
                ))
            }
        });
        Ok(())
    }
    fn get_stroke_cap(&self) -> LuaPaintCap {
        Ok(LuaPaintCap(self.stroke_cap()))
    }
    fn set_stroke_cap(&mut self, cap: LuaPaintCap) {
        self.set_stroke_cap(*cap);
        Ok(())
    }
    fn get_stroke_join(&self) -> LuaPaintJoin {
        Ok(LuaPaintJoin(self.stroke_join()))
    }
    fn set_stroke_join(&mut self, join: LuaPaintJoin) {
        self.set_stroke_join(*join);
        Ok(())
    }
    fn get_stroke_width(&self) -> f32 {
        Ok(self.stroke_width())
    }
    fn set_stroke_width(&mut self, width: f32) {
        self.set_stroke_width(width);
        Ok(())
    }
    fn get_stroke_miter(&self) -> f32 {
        Ok(self.stroke_miter())
    }
    fn set_stroke_miter(&mut self, miter: f32) {
        self.0.set_stroke_miter(miter);
        Ok(())
    }
    fn get_path_effect(&self) -> LuaPathEffect {
        Ok(self.path_effect().map(LuaPathEffect))
    }
    fn set_path_effect(&mut self, effect: Option<LuaPathEffect>) {
        self.set_path_effect(effect.map(LuaPathEffect::unwrap));
        Ok(())
    }
    fn get_shader(&self) -> LuaShader {
        Ok(self.shader().map(LuaShader))
    }
    fn set_shader(&mut self, shader: Option<LuaShader>) {
        self.set_shader(shader.map(LuaShader::unwrap));
        Ok(())
    }
}

wrap_skia_handle!(Path);

#[lua_methods(lua_name: Path)]
impl LuaPath {
    #[lua(constructor)]
    fn empty() -> LuaPath {
        Ok(LuaPath(Path::default()))
    }
    fn make(
        points: Vec<LuaPoint>,
        verbs: Vec<LuaVerb>,
        conic_weights: Vec<f32>,
        fill_type: LuaPathFillType,
        volatile: LuaFallible<bool>,
    ) -> LuaPath {
        let points: Vec<Point> = points.into_iter().map(LuaPoint::into).collect();
        let verbs: Vec<u8> = verbs.into_iter().map(|it| it.0 as u8).collect();
        Ok(LuaPath(Path::new_from(
            &points,
            &verbs,
            &conic_weights,
            *fill_type,
            *volatile,
        )))
    }

    fn add_arc(&mut self, oval: LuaRect, start_angle: f32, sweep_angle: f32) {
        let oval: Rect = oval.into();
        self.add_arc(&oval, start_angle, sweep_angle);
        Ok(())
    }
    fn add_circle(&mut self, point: LuaPoint, radius: f32, dir: Option<LuaPathDirection>) {
        self.add_circle(point, radius, dir.map_t());
        Ok(())
    }
    fn add_oval(&mut self, oval: LuaRect, dir: Option<LuaPathDirection>, start: Option<usize>) {
        let oval: Rect = oval.into();
        let start = start.unwrap_or(1);
        self.0
            .add_oval(oval, Some((dir.unwrap_or_default_t(), start)));
        Ok(())
    }
    fn add_path(&mut self, other: LuaPath, point: LuaPoint, mode: Option<LuaAddPathMode>) {
        self.add_path(&other, point, mode.map_t());
        Ok(())
    }
    fn add_poly(&mut self, points: Vec<LuaPoint>, close: bool) {
        let points: Vec<_> = points.into_iter().map(LuaPoint::into).collect();
        self.add_poly(&points, close);
        Ok(())
    }
    fn add_rect(&mut self, rect: LuaRect, dir: Option<LuaPathDirection>, start: Option<usize>) {
        let rect: Rect = rect.into();
        let start = start.unwrap_or(1);
        self.0
            .add_rect(rect, Some((dir.unwrap_or_default_t(), start)));
        Ok(())
    }
    fn add_round_rect(&mut self, rect: LuaRect, rounding: LuaPoint, dir: Option<LuaPathDirection>) {
        let rect: Rect = rect.into();
        self.add_round_rect(
            rect,
            (rounding.x(), rounding.y()),
            dir.unwrap_or_default_t(),
        );
        Ok(())
    }
    fn add_r_rect(&mut self, rrect: LuaRRect, dir: Option<LuaPathDirection>, start: Option<usize>) {
        let start = start.unwrap_or(1);
        self.add_rrect(rrect.unwrap(), Some((dir.unwrap_or_default_t(), start)));
        Ok(())
    }
    fn arc_to(&mut self, oval: LuaRect, start_angle: f32, sweep_angle: f32, force_move_to: bool) {
        let oval: Rect = oval.into();
        self.arc_to(oval, start_angle, sweep_angle, force_move_to);
        Ok(())
    }
    fn close(&mut self) {
        self.close();
        Ok(())
    }
    fn compute_tight_bounds(&self) -> LuaRect {
        Ok(LuaRect::from(self.compute_tight_bounds()))
    }
    fn conic_to(&mut self, p1: LuaPoint, p2: LuaPoint, w: f32) {
        self.conic_to(p1, p2, w);
        Ok(())
    }
    fn conservatively_contains_rect(&self, rect: LuaRect) -> bool {
        let rect: Rect = rect.into();
        Ok(self.conservatively_contains_rect(rect))
    }
    fn contains(&self, p: LuaPoint) -> bool {
        Ok(self.contains(p))
    }
    fn count_points(&self) -> usize {
        Ok(self.count_points())
    }
    fn count_verbs(&self) -> usize {
        Ok(self.count_verbs())
    }
    fn cubic_to(&mut self, p1: LuaPoint, p2: LuaPoint, p3: LuaPoint) {
        self.cubic_to(p1, p2, p3);
        Ok(())
    }
    fn get_bounds(&self) -> LuaRect {
        Ok(LuaRect::from(*self.bounds()))
    }
    fn get_fill_type(&self) -> LuaPathFillType {
        Ok(LuaPathFillType(self.fill_type()))
    }
    fn get_generation_id(&self) -> u32 {
        Ok(self.generation_id())
    }
    fn get_last_pt(&self) -> LuaPoint {
        Ok(self.last_pt().map(LuaPoint::from))
    }
    fn get_point(&self, index: usize) -> LuaPoint {
        Ok(self.get_point(index).map(LuaPoint::from))
    }
    fn get_points<'lua>(&self, lua: &'lua LuaContext, count: Option<usize>) -> LuaTable<'lua> {
        unsafe {
            let count = count.unwrap_or_else(|| self.count_points());
            let layout = Layout::from_size_align(size_of::<Point>() * count, align_of::<Point>())
                .expect("invalid Point array layout");
            let data = std::alloc::alloc(layout) as *mut Point;
            let slice = std::slice::from_raw_parts_mut(data, count);

            self.0.get_points(slice);

            let result = lua.create_table()?;
            for (i, point) in slice.iter_mut().enumerate() {
                result.set(i, LuaPoint::from(*point).into_lua(lua)?)?;
            }
            std::alloc::dealloc(data as *mut u8, layout);
            Ok(result)
        }
    }
    fn get_segment_masks<'lua>(&self, lua: &'lua LuaContext) {
        LuaSegmentMask(self.segment_masks()).to_table(lua)
    }
    fn get_verbs<'lua>(&self, lua: &'lua LuaContext, count: Option<usize>) -> LuaTable<'lua> {
        unsafe {
            let count = count.unwrap_or_else(|| self.count_verbs());
            let layout = Layout::from_size_align(size_of::<Verb>() * count, align_of::<Verb>())
                .expect("invalid Verb array layout");
            let data = std::alloc::alloc(layout);
            let slice = std::slice::from_raw_parts_mut(data, count * size_of::<Verb>());

            self.0.get_verbs(slice);
            let slice = std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut Verb, count);

            let result = lua.create_table()?;
            for (i, verb) in slice.iter().enumerate() {
                result.set(i, LuaVerb(*verb))?;
            }
            std::alloc::dealloc(data as *mut u8, layout);
            Ok(result)
        }
    }
    fn inc_reserve(&mut self, extra_pt_count: usize) {
        self.inc_reserve(extra_pt_count);
        Ok(())
    }
    fn interpolate(&self, ending: LuaPath, weight: f32) {
        self.interpolate(&ending, weight);
        Ok(())
    }
    fn is_convex(&self) -> bool {
        Ok(self.is_convex())
    }
    fn is_empty(&self) -> bool {
        Ok(self.is_empty())
    }
    fn is_finite(&self) -> bool {
        Ok(self.is_finite())
    }
    fn is_interpolatable(&self, other: LuaPath) -> bool {
        Ok(self.is_interpolatable(&other))
    }
    fn is_inverse_fill_type(&self) -> bool {
        Ok(self.is_inverse_fill_type())
    }
    fn is_last_contour_closed(&self) -> bool {
        Ok(self.is_last_contour_closed())
    }
    fn is_line(&self) -> LuaLine {
        Ok(self.is_line().map(LuaLine::from))
    }
    fn is_oval(&self) -> LuaRect {
        Ok(self.is_oval().map(LuaRect::from))
    }
    fn is_rect(&self) {
        Ok(self.is_rect().map(|(rect, _, _)| LuaRect::from(rect)))
    }
    fn is_r_rect(&self) -> LuaRRect {
        Ok(self.is_rrect().map(LuaRRect))
    }
    fn is_valid(&self) -> bool {
        Ok(self.is_valid())
    }
    fn is_volatile(&self) -> bool {
        Ok(self.is_volatile())
    }
    fn line_to(&mut self, point: LuaPoint) {
        self.line_to(point);
        Ok(())
    }
    fn make_scale(&mut self, sx: f32, sy: Option<f32>) -> LuaPath {
        let sy = sy.unwrap_or(sx);
        Ok(LuaPath(self.make_scale((sx, sy))))
    }
    fn make_transform(&mut self, matrix: LuaMatrix, pc: Option<bool>) -> LuaPath {
        let matrix = matrix.into();
        let pc = match pc.unwrap_or(true) {
            true => skia_safe::matrix::ApplyPerspectiveClip::Yes,
            false => skia_safe::matrix::ApplyPerspectiveClip::No,
        };
        Ok(LuaPath(self.make_transform(&matrix, pc)))
    }
    fn move_to(&mut self, p: LuaPoint) {
        self.move_to(p);
        Ok(())
    }
    fn offset(&mut self, d: LuaPoint) {
        self.offset(d);
        Ok(())
    }
    fn quad_to(&mut self, p1: LuaPoint, p2: LuaPoint) {
        self.quad_to(p1, p2);
        Ok(())
    }
    fn r_arc_to(
        &mut self,
        r: LuaPoint,
        x_axis_rotate: f32,
        arc_size: LuaArcSize,
        sweep: LuaPathDirection,
        d: LuaPoint,
    ) {
        self.r_arc_to_rotated(r, x_axis_rotate, *arc_size, *sweep, d);
        Ok(())
    }
    fn r_conic_to(&mut self, d1: LuaPoint, d2: LuaPoint, w: f32) {
        self.r_conic_to(d1, d2, w);
        Ok(())
    }
    fn r_cubic_to(&mut self, d1: LuaPoint, d2: LuaPoint, d3: LuaPoint) {
        self.r_cubic_to(d1, d2, d3);
        Ok(())
    }
    fn reset(&mut self) {
        self.reset();
        Ok(())
    }
    fn reverse_add_path(&mut self, path: LuaPath) {
        self.reverse_add_path(&path);
        Ok(())
    }
    fn rewind(&mut self) {
        self.rewind();
        Ok(())
    }
    fn r_line_to(&mut self, point: LuaPoint) {
        self.r_line_to(point);
        Ok(())
    }
    fn r_move_to(&mut self, point: LuaPoint) {
        self.r_move_to(point);
        Ok(())
    }
    fn r_quad_to(&mut self, dx1: LuaPoint, dx2: LuaPoint) {
        self.r_quad_to(dx1, dx2);
        Ok(())
    }
    fn set_fill_type(&mut self, fill_type: LuaPathFillType) {
        self.set_fill_type(*fill_type);
        Ok(())
    }
    fn set_is_volatile(&mut self, is_volatile: bool) {
        self.set_is_volatile(is_volatile);
        Ok(())
    }
    fn set_last_pt(&mut self, point: LuaPoint) {
        self.set_last_pt(point);
        Ok(())
    }
    fn toggle_inverse_fill_type(&mut self) {
        self.toggle_inverse_fill_type();
        Ok(())
    }
    fn transform(&mut self, matrix: LuaMatrix) {
        let matrix = matrix.into();
        self.transform(&matrix);
        Ok(())
    }
}

wrap_skia_handle!(RRect);

#[lua_methods(lua_name: RRect)]
impl LuaRRect {
    // TODO: Constructor
    fn make() -> LuaRRect {
        Ok(LuaRRect(RRect::new()))
    }

    fn contains(&self, rect: LuaRect) -> bool {
        let rect: Rect = rect.into();
        Ok(self.contains(rect))
    }
    fn get_bounds(&self) -> LuaRect {
        Ok(LuaRect::from(self.bounds().clone()))
    }
    fn get_simple_radii(&self) -> LuaPoint {
        Ok(LuaPoint::from(self.simple_radii()))
    }
    fn get_type(&self) -> LuaRRectType {
        Ok(LuaRRectType(self.get_type()))
    }
    fn height(&self) -> f32 {
        Ok(self.height())
    }
    fn inset(&mut self, delta: LuaPoint) {
        self.inset(delta);
        Ok(())
    }
    fn is_complex(&self) -> bool {
        Ok(self.is_complex())
    }
    fn is_empty(&self) -> bool {
        Ok(self.is_empty())
    }
    fn is_nine_patch(&self) -> bool {
        Ok(self.is_nine_patch())
    }
    fn is_oval(&self) -> bool {
        Ok(self.is_oval())
    }
    fn is_rect(&self) -> bool {
        Ok(self.is_rect())
    }
    fn is_simple(&self) -> bool {
        Ok(self.is_simple())
    }
    fn is_valid(&self) -> bool {
        Ok(self.is_valid())
    }
    fn make_offset(&self, delta: LuaPoint) -> LuaRRect {
        Ok(LuaRRect(self.with_offset(delta)))
    }
    fn offset(&mut self, delta: LuaPoint) {
        self.offset(delta);
        Ok(())
    }
    fn outset(&mut self, delta: LuaPoint) {
        self.outset(delta);
        Ok(())
    }
    fn radii(&self, corner: Option<LuaRRectCorner>) -> LuaPoint {
        let radii = match corner {
            Some(it) => self.radii(*it),
            None => self.simple_radii(),
        };
        Ok(LuaPoint::from(radii))
    }
    fn rect(&self) -> LuaRect {
        Ok(LuaRect::from(self.rect().clone()))
    }
    fn set_empty(&mut self) {
        self.set_empty();
        Ok(())
    }
    fn set_nine_patch(&mut self, rect: LuaRect, sides: SidePack) {
        let rect: Rect = rect.into();
        self.0
            .set_nine_patch(rect, sides.left, sides.top, sides.right, sides.bottom);
        Ok(())
    }
    fn set_oval(&mut self, oval: LuaRect) {
        let oval: Rect = oval.into();
        self.set_oval(oval);
        Ok(())
    }
    fn set_rect(&mut self, rect: LuaRect) {
        let rect: Rect = rect.into();
        self.set_rect(rect);
        Ok(())
    }
    fn set_rect_radii(&mut self, rect: LuaRect, radii: Vec<LuaPoint>) {
        let rect: Rect = rect.into();
        if radii.len() < 4 {
            // TODO: Take exactly 4 LuaPoints, maybe unpacked
            return Err(LuaError::RuntimeError(format!(
                "RRect:setRectRadii expects 4 radii points; got {}",
                radii.len()
            )));
        }
        let radii: Vec<Point> = radii.into_iter().take(4).map(LuaPoint::into).collect();
        let radii: [Point; 4] = radii.try_into().expect("radii should have 4 Points");
        self.set_rect_radii(rect, &radii);
        Ok(())
    }
    fn set_rect_xy(&mut self, rect: LuaRect, x_rad: f32, y_rad: f32) {
        let rect: Rect = rect.into();
        self.set_rect_xy(rect, x_rad, y_rad);
        Ok(())
    }
    fn transform(&self, matrix: LuaMatrix) -> LuaRRect {
        let matrix: Matrix = matrix.into();
        Ok(self.transform(&matrix).map(LuaRRect))
    }
    #[lua(rename: "type")]
    fn get_type(&self) -> LuaRRectType {
        Ok(LuaRRectType(self.get_type()))
    }
    fn width(&self) {
        Ok(self.width())
    }
}

wrap_skia_handle!(ColorInfo);

#[lua_methods(lua_name: ColorInfo)]
impl LuaColorInfo {
    fn alpha_type(&self) -> LuaAlphaType {
        Ok(LuaAlphaType(self.alpha_type()))
    }
    fn bytes_per_pixel(&self) -> usize {
        Ok(self.bytes_per_pixel())
    }
    fn color_space(&self) -> LuaColorSpace {
        Ok(self.color_space().map(LuaColorSpace))
    }
    fn color_type(&self) -> LuaColorType {
        Ok(LuaColorType(self.color_type()))
    }
    fn gamma_close_to_srgb(&self) -> bool {
        Ok(self.is_gamma_close_to_srgb())
    }
    fn is_opaque(&self) -> bool {
        Ok(self.is_opaque())
    }
    fn make_alpha_type(&self, alpha_type: LuaAlphaType) -> LuaColorInfo {
        Ok(LuaColorInfo(self.with_alpha_type(*alpha_type)))
    }
    fn make_color_space(&self, color_space: Option<LuaColorSpace>) -> LuaColorInfo {
        Ok(LuaColorInfo(
            self.with_color_space(color_space.map(LuaColorSpace::unwrap)),
        ))
    }
    fn make_color_type(&self, color_type: LuaColorType) -> LuaColorInfo {
        Ok(LuaColorInfo(self.with_color_type(*color_type)))
    }
    fn shift_per_pixel(&self) -> usize {
        Ok(self.shift_per_pixel())
    }
}

wrap_skia_handle!(ImageInfo);

#[lua_methods(lua_name: ImageInfo)]
impl LuaImageInfo {
    fn alpha_type(&self) -> LuaAlphaType {
        Ok(LuaAlphaType(self.alpha_type()))
    }
    fn bounds(&self) -> LuaRect {
        Ok(LuaRect::from(self.bounds()))
    }
    fn bytes_per_pixel(&self) -> usize {
        Ok(self.bytes_per_pixel())
    }
    fn color_info<'lua>(&self, lua: &'lua LuaContext) -> LuaTable<'lua> {
        let result = lua.create_table()?;
        let info = self.0.color_info();
        result.set("colorSpace", info.color_space().map(LuaColorSpace))?;
        result.set("colorType", LuaColorType(info.color_type()))?;
        result.set("alphaType", LuaAlphaType(info.alpha_type()))?;
        result.set("isOpaque", info.is_opaque())?;
        result.set("gammaCloseToSrgb", info.is_gamma_close_to_srgb())?;
        result.set("bytesPerPixel", info.bytes_per_pixel())?;
        result.set("shiftPerPixel", info.shift_per_pixel())?;
        Ok(result)
    }
    fn color_space(&self) -> LuaColorSpace {
        Ok(self.color_space().map(LuaColorSpace))
    }
    fn color_type(&self) -> LuaColorType {
        Ok(LuaColorType(self.color_type()))
    }
    fn compute_byte_size(&self, row_bytes: usize) -> usize {
        Ok(self.compute_byte_size(row_bytes))
    }
    fn compute_min_byte_size(&self) -> usize {
        Ok(self.compute_min_byte_size())
    }
    fn compute_offset(&self, point: LuaPoint, row_bytes: usize) -> usize {
        Ok(self.compute_offset(point, row_bytes))
    }
    fn dimensions(&self) -> LuaSize {
        Ok(LuaSize::from(self.dimensions()))
    }
    fn gamma_close_to_srgb(&self) -> bool {
        Ok(self.is_gamma_close_to_srgb())
    }
    fn width(&self) -> f32 {
        Ok(self.width())
    }
    fn height(&self) -> f32 {
        Ok(self.height())
    }
    fn is_empty(&self) -> bool {
        Ok(self.is_empty())
    }
    fn is_opaque(&self) -> bool {
        Ok(self.is_opaque())
    }
    fn make_alpha_type(&self, alpha_type: LuaAlphaType) -> LuaImageInfo {
        Ok(LuaImageInfo(self.with_alpha_type(*alpha_type)))
    }
    fn make_color_space(&self, color_space: LuaColorSpace) -> LuaImageInfo {
        Ok(LuaImageInfo(self.with_color_space(color_space.unwrap())))
    }
    fn make_color_type(&self, color_type: LuaColorType) -> LuaImageInfo {
        Ok(LuaImageInfo(self.with_color_type(*color_type)))
    }
    fn make_dimensions(&self, dimensions: LuaSize) -> LuaImageInfo {
        Ok(LuaImageInfo(self.with_dimensions(dimensions)))
    }
    fn min_row_bytes(&self) -> usize {
        Ok(self.min_row_bytes())
    }
    fn reset(&mut self) {
        self.reset();
        Ok(())
    }
    fn shift_per_pixel(&self) -> usize {
        Ok(self.shift_per_pixel())
    }
    fn valid_row_bytes(&self, row_bytes: usize) -> bool {
        Ok(self.valid_row_bytes(row_bytes))
    }
}

type_like_table!(ImageInfo: |value: LuaTable| {
    let dimensions: LuaSize = LuaSize::try_from(value.get::<_, LuaTable>("dimensions")?)?;
    // TODO: Check values if specified
    let color_type = LuaColorType::try_from(
        value
            .get::<_, String>("color_type")
            .unwrap_or("unknown".to_string()),
    )?;
    let alpha_type = LuaAlphaType::try_from(
        value
            .get::<_, String>("alpha_type")
            .unwrap_or("unknown".to_string()),
    )?;
    let color_space = value
        .get_user_data::<_, LuaColorSpace>("color_space")
        .ok()
        .map(|it| it.0);

    let result = ImageInfo::new(dimensions, *color_type, *alpha_type, color_space);

    Ok(LuaImageInfo(result))
});

wrap_skia_handle!(SurfaceProps);

#[lua_methods(lua_name: SurfaceProps)]
impl LuaSurfaceProps {
    fn flags<'lua>(&self, lua: &'lua LuaContext) -> LuaTable<'lua> {
        LuaSurfacePropsFlags(self.0.flags()).to_table(lua)
    }
    fn pixel_geometry(&self) -> LuaPixelGeometry {
        Ok(LuaPixelGeometry(self.pixel_geometry()))
    }
    fn is_use_device_independent_fonts(&self) -> bool {
        Ok(self.is_use_device_independent_fonts())
    }
    fn is_always_dither(&self) -> bool {
        Ok(self.is_always_dither())
    }
}

type_like_table!(SurfaceProps: |value: LuaTable| {
    let flags = match value.get::<_, OneOf<LuaTable, LuaValue>>("flags") {
        Ok(OneOf::A(it)) => LuaSurfacePropsFlags::from_table(it)?.0,
        Ok(OneOf::B(it)) if matches!(it, LuaNil) => {
            SurfacePropsFlags::empty()
        }
        Ok(OneOf::B(other)) => {
            return Err(LuaError::FromLuaConversionError { from: other.type_name(), to: "SurfacePropFlags", message: None })
        }
        Ok(_) => unreachable!(),
        Err(other) => return Err(other)
    };
    let pixel_geometry = LuaPixelGeometry::try_from(value.get::<_, String>("pixel_geometry").unwrap_or("unknown".to_string()))?;

    Ok(LuaSurfaceProps(SurfaceProps::new(flags, *pixel_geometry)))
});

struct LuaSamplingOptions {
    filter_mode: FilterMode,
    mipmap_mode: MipmapMode,
}

impl Default for LuaSamplingOptions {
    fn default() -> Self {
        LuaSamplingOptions {
            filter_mode: FilterMode::Nearest,
            mipmap_mode: MipmapMode::None,
        }
    }
}

/// ## Supported formats
/// - { filter: Filter, mipmap: Mipmap }
/// - FilterMode, Mipmap
impl<'lua> FromArgPack<'lua> for LuaSamplingOptions {
    fn convert(args: &mut ArgumentContext<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        if args.is_empty() {
            return Ok(Self::default());
        }

        if let Some(table) = args.pop_typed::<LuaTable<'lua>>() {
            let filter = table
                .get::<_, String>("filter")
                .or(table.get("filter_mode"))
                .and_then(LuaFilterMode::try_from);
            let mipmap = table
                .get::<_, String>("mipmap")
                .or(table.get("mipmap_mode"))
                .and_then(LuaMipmapMode::try_from);

            if filter.is_err() && mipmap.is_err() {
                args.revert(LuaValue::Table(table));
                return Ok(Self::default());
            }

            return Ok(LuaSamplingOptions {
                filter_mode: filter.unwrap_or_t(FilterMode::Nearest),
                mipmap_mode: mipmap.unwrap_or_t(MipmapMode::None),
            });
        }

        let first = match args.pop_typed::<LuaString<'lua>>() {
            Some(it) => it,
            None => return Ok(Self::default()),
        };

        let filter_mode = match first.to_str().and_then(LuaFilterMode::from_str).ok() {
            Some(it) => it,
            None => {
                args.revert(first);
                return Ok(Self::default());
            }
        };

        const SECOND_MISSING: &str = "only filtering mode provided; unpacked SamplingOptions require both filtering and mipmapping to be specified to avoid ambiguity";

        let second: LuaString<'lua> = match args.pop_typed_or(Some(SECOND_MISSING)) {
            Ok(it) => it,
            Err(err) => {
                args.revert(first);
                return Err(err);
            }
        };

        let second = match second.to_str().and_then(LuaMipmapMode::from_str) {
            Ok(it) => it,
            Err(err) => {
                args.revert(second);
                args.revert(first);

                return Err(LuaError::CallbackError {
                    traceback: SECOND_MISSING.to_string(),
                    cause: Arc::new(err),
                });
            }
        };

        Ok(LuaSamplingOptions {
            filter_mode: *filter_mode,
            mipmap_mode: *second,
        })
    }
}

impl Into<SamplingOptions> for LuaSamplingOptions {
    #[inline]
    fn into(self) -> SamplingOptions {
        SamplingOptions::new(self.filter_mode, self.mipmap_mode)
    }
}

wrap_skia_handle!(Surface);

#[lua_methods(lua_name: Surface)]
impl LuaSurface {
    fn null(size: LuaSize) -> Option<LuaSurface> {
        let size: ISize = size.into();
        Ok(surfaces::null(size).map(LuaSurface))
    }
    fn raster(
        info: LikeImageInfo,
        row_bytes: LuaFallible<usize>,
        props: LuaFallible<LikeSurfaceProps>,
    ) -> Option<LuaSurface> {
        let info: ImageInfo = info.unwrap();
        let row_bytes = row_bytes.unwrap_or_else(|| info.min_row_bytes());
        let props: Option<SurfaceProps> = props.map_t();

        Ok(surfaces::raster(&info, row_bytes, props.as_ref()).map(LuaSurface))
    }
    // wrap_pixels - not able to detect table value updates

    // capabilities - not useful from Lua?
    // characterize - no graphite bindings
    fn draw(
        &mut self,
        canvas: &LuaCanvas,
        offset: LuaPoint,
        sampling: LuaFallible<LuaSamplingOptions>,
        paint: LuaFallible<LikePaint>,
    ) {
        let sampling: SamplingOptions = sampling.unwrap_or_default().into();
        let paint = paint.map(LikePaint::unwrap);

        self.draw(&canvas, offset, sampling, paint.as_ref());
        Ok(())
    }
    // generationID - not useful from Lua without graphite?
    fn get_canvas(&mut self) -> LuaCanvas {
        Ok(LuaCanvas::Owned(self.0.clone()))
    }
    fn width(&self) -> f32 {
        Ok(self.width())
    }
    fn height(&self) -> f32 {
        Ok(self.height())
    }
    fn image_info(&mut self) {
        Ok(LuaImageInfo(self.image_info()))
    }
    // isCompatible - no low-level renderer bindings in Lua
    fn make_image_snapshot(&mut self) {
        Ok(LuaImage(self.image_snapshot()))
    }
    fn make_surface(&mut self, image_info: LikeImageInfo) {
        Ok(self.new_surface(&image_info.unwrap()).map(LuaSurface))
    }
    // peekPixels - very complicated to handle properly
    fn props(&self) -> LuaSurfaceProps {
        Ok(LuaSurfaceProps(self.props().clone()))
    }
    fn read_pixels<'lua>(
        &mut self,
        lua: &'lua LuaContext,
        rect: Option<LuaRect>,
        info: Option<LuaImageInfo>,
    ) -> Option<Vec> {
        let area = rect
            .map(Into::into)
            .unwrap_or_else(|| IRect::new(0, 0, self.width(), self.height()));
        let image_info = info
            .map(LuaImageInfo::unwrap)
            .unwrap_or_else(|| self.image_info().with_dimensions(area.size()));
        let row_bytes = area.width() as usize * image_info.bytes_per_pixel();
        let mut result = Vec::with_capacity(row_bytes * area.height() as usize);
        let is_some = self.0.read_pixels(
            &image_info,
            result.as_mut_slice(),
            row_bytes,
            IPoint::new(area.x(), area.y()),
        );
        match is_some {
            true => {
                let result = lua.create_table_from_vec(result)?;
                result.set("info", LuaImageInfo(image_info))?;
                Ok(Some(result))
            }
            false => Ok(None),
        }
    }
    fn write_pixels(
        &mut self,
        dst: LuaPoint,
        data: LuaTable,
        info: LuaFallible<LikeImageInfo>,
        size: LuaFallible<LuaSize>,
    ) -> bool {
        let info = info
            .or_else(|| data.get("info").ok())
            .map(LikeImageInfo::unwrap)
            .unwrap_or_else(|| self.image_info());
        let size = size
            .map(LuaSize::into)
            .unwrap_or_else(|| ISize::new(info.width(), info.height()));
        let row_bytes = info.bytes_per_pixel();

        // TODO: Properly handle data.width/height != size to allow
        // easy resizing from Lua
        let mut pixels: Vec<u8> = data
            .sequence_values::<u8>()
            .filter_map(Result::ok)
            .take(row_bytes * size.height as usize)
            .collect();

        if pixels.len() < row_bytes * size.height as usize {
            return Ok(false);
        }

        let pm = Pixmap::new(&info, pixels.as_mut_slice(), row_bytes)
            .expect("can't construct Pixmap from buffer based on info parameters");
        let dst: IVector = dst.into();
        self.write_pixels_from_pixmap(&pm, dst);
        Ok(true)
    }
    // recorder - graphite bindings not supported
    // recordingContext - graphite bindings not supported
    // replaceBackendTexture - graphite bindings not supported
}

// SAFETY: Clunky handles Lua and rendering on the same thread
unsafe impl Send for LuaSurface {}

wrap_skia_handle!(FontStyleSet);

#[lua_methods(lua_name: FontStyleSet)]
impl LuaFontStyleSet {
    fn create_empty() -> LuaFontStyleSet {
        Ok(LuaFontStyleSet(FontStyleSet::new_empty()))
    }

    fn count(&mut self) -> usize {
        Ok(self.0.count())
    }
    fn get_style(&mut self, index: usize) -> (LuaFontStyle, String) {
        let (style, name) = self.style(index);
        Ok((LuaFontStyle(style), name))
    }
    fn create_typeface(&mut self, index: usize) -> LuaTypeface {
        Ok(self.new_typeface(index).map(LuaTypeface))
    }
    fn match_style(&mut self, index: usize, pattern: LuaFontStyle) -> LuaTypeface {
        Ok(self.match_style(index, pattern.unwrap()).map(LuaTypeface))
    }
}

// SAFETY: Clunky handles Lua and rendering on the same thread
unsafe impl Send for LuaFontStyleSet {}

pub struct LuaText {
    pub text: OsString,
    pub encoding: TextEncoding,
}

fn encoding_size(encoding: TextEncoding) -> usize {
    match encoding {
        TextEncoding::UTF8 => 1,
        TextEncoding::UTF16 => 2,
        TextEncoding::UTF32 => 4,
        TextEncoding::GlyphId => size_of::<GlyphId>(),
    }
}

impl<'lua> FromArgPack<'lua> for LuaText {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        // TODO: MACRO match pop
        if let Some(text) = args.pop_typed::<mlua::String<'lua>>() {
            let text = OsString::from_str(text.to_str()?).unwrap();
            return Ok(LuaText {
                text,
                encoding: TextEncoding::UTF8,
            });
        }
        let bytes = args.pop_typed_or::<LuaTable<'lua>, String>(None)?;

        if !bytes.is_homogeneous_sequence::<LuaNumber>() {
            args.revert(bytes);
            return Err(args.bad_argument(mlua::Error::FromLuaConversionError {
                from: LuaType::Table.name(),
                to: "number array",
                message: None,
            }));
        }

        let bytes = LuaArray::from(bytes);

        let encoding = match args.pop_typed::<mlua::String<'lua>>() {
            Some(encoding) => {
                if let Ok(it) = LuaTextEncoding::try_from(encoding.clone()) {
                    *it
                } else {
                    args.revert(encoding);
                    TextEncoding::UTF8
                }
            }
            None => TextEncoding::UTF8,
        };

        let text = if matches!(encoding, TextEncoding::UTF8) {
            bytes.into_iter::<u8>(lua).collect()
        } else {
            let size = encoding_size(encoding);
            let mut result = Vec::with_capacity(bytes.len() * size);

            match size {
                2 => bytes.into_iter::<u16>(lua).for_each(|it| {
                    let _ = result.write_u16::<byteorder::NativeEndian>(it);
                }),
                4 => bytes.into_iter::<u32>(lua).for_each(|it| {
                    let _ = result.write_u32::<byteorder::NativeEndian>(it);
                }),
                _ => unreachable!("unhandled encoding size"),
            }

            result
        };

        Ok(LuaText {
            text: OsString::from_vec(text),
            encoding,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LuaFontMgr {
    Default,
    Empty,
}
impl LuaFontMgr {
    pub fn unwrap(self) -> FontMgr {
        match self {
            LuaFontMgr::Default => FontMgr::default(),
            LuaFontMgr::Empty => FontMgr::empty(),
        }
    }
}

#[lua_methods(lua_name: FontMgr)]
impl LuaFontMgr {
    fn default() -> LuaFontMgr {
        Ok(LuaFontMgr::Default)
    }
    fn empty() -> LuaFontMgr {
        Ok(LuaFontMgr::Empty)
    }

    fn count_families(&self) -> usize {
        Ok(self.unwrap().count_families())
    }
    fn create_style_set(&self, index: usize) -> LuaFontStyleSet {
        Ok(LuaFontStyleSet(self.unwrap().new_style_set(index)))
    }
    fn get_family_name(&self, index: usize) -> String {
        Ok(self.unwrap().family_name(index))
    }
    // NYI: legacyMakeTypeface by skia_safe
    fn make_from_data(&self, bytes: Vec<u8>, ttc: Option<usize>) -> LuaTypeface {
        Ok(self.unwrap().new_from_data(&bytes, ttc).map(LuaTypeface))
    }
    fn make_from_file(&self, path: String, ttc: Option<usize>) -> LuaTypeface {
        let bytes = match std::fs::read(path.as_str()) {
            Ok(it) => it,
            Err(_) => {
                return Err(LuaError::RuntimeError(format!(
                    "unable to read font file: {}",
                    path
                )))
            }
        };
        Ok(self.unwrap().new_from_data(&bytes, ttc).map(LuaTypeface))
    }
    // makeFromStream - Lua has no streams
    fn match_family(&self, family_name: String) -> LuaFontStyleSet {
        Ok(LuaFontStyleSet(self.unwrap().match_family(family_name)))
    }
    fn match_family_style(&self, family_name: String, style: LuaFontStyle) -> LuaTypeface {
        Ok(self
            .unwrap()
            .match_family_style(family_name, style.unwrap())
            .map(LuaTypeface))
    }
    fn match_family_style_character(
        &self,
        family_name: String,
        style: LuaFontStyle,
        bcp47: Vec<String>,
        character: Unichar,
    ) -> LuaTypeface {
        let bcp_refs: Vec<&str> = bcp47.iter().map(|it| it.as_ref()).collect();
        Ok(self
            .unwrap()
            .match_family_style_character(family_name, style.unwrap(), &bcp_refs, character)
            .map(LuaTypeface))
    }
}

wrap_skia_handle!(Typeface);

#[lua_methods(lua_name: Typeface)]
impl LuaTypeface {
    fn make_default() -> LuaTypeface {
        Ok(LuaTypeface(Typeface::default()))
    }
    // NYI: Typeface::make_empty by skia_safe
    fn make_from_name(
        family_name: String,
        font_style: LuaFallible<LuaFontStyle>,
    ) -> Option<LuaTypeface> {
        let font_style = font_style.map(LuaFontStyle::unwrap).unwrap_or_default();
        Ok(FontMgr::default()
            .match_family_style(family_name, font_style)
            .map(LuaTypeface))
    }
    fn make_from_data(data: Vec<u8>, index: LuaFallible<usize>) -> Option<LuaTypeface> {
        Ok(FontMgr::default()
            .new_from_data(&data, index.unwrap_or_default())
            .map(LuaTypeface))
    }
    fn make_from_file(path: String, index: LuaFallible<usize>) -> Option<LuaTypeface> {
        let data = match std::fs::read(path.as_str()) {
            Ok(it) => it,
            Err(_) => {
                return Err(LuaError::RuntimeError(format!(
                    "unable to read font file: {}",
                    path
                )))
            }
        };
        Ok(FontMgr::default()
            .new_from_data(&data, index.unwrap_or_default())
            .map(LuaTypeface))
    }

    fn count_glyphs(&self) -> usize {
        Ok(self.count_glyphs())
    }
    fn count_tables(&self) -> usize {
        Ok(self.count_tables())
    }
    // createFamilyNameIterator -> familyNames; Lua doesn't have iterators
    fn family_names(&self) -> HashMap<String, String> {
        let names: HashMap<_, _> = self
            .0
            .new_family_name_iterator()
            .map(|it| (it.language, it.string))
            .collect();
        Ok(names)
    }
    // NYI: createScalerContext by skia_safe
    // NYI: filterRec by skia_safe
    fn font_style(&self) -> LuaFontStyle {
        Ok(LuaFontStyle(self.font_style()))
    }
    fn get_bounds(&self) -> LuaRect {
        Ok(LuaRect::from(self.bounds()))
    }
    fn get_family_name(&self) -> String {
        Ok(self.family_name())
    }
    // methods.add_method_ext("getFontDescriptor" Ok(()));
    fn get_kerning_pair_adjustments(&self, glyphs: Vec<GlyphId>) -> Vec<i32> {
        let mut adjustments = Vec::with_capacity(glyphs.len());
        self.0
            .get_kerning_pair_adjustments(glyphs.as_ref(), adjustments.as_mut_slice());
        Ok(adjustments)
    }
    fn get_post_script_name(&self) -> Option<String> {
        Ok(self.post_script_name())
    }
    fn get_table_data(&self, tag: FontTableTag) -> Vec<u8> {
        match self.get_table_size(tag) {
            Some(size) => {
                let mut result = Vec::with_capacity(size);
                self.0.get_table_data(tag, result.as_mut_slice());
                Ok(result)
            }
            None => Ok(vec![]),
        }
    }
    fn get_table_size(&self, tag: FontTableTag) -> Option<usize> {
        Ok(self.0.get_table_size(tag))
    }
    fn get_table_tags(&self) -> Vec<Option<FontTableTag>> {
        Ok(self.0.table_tags())
    }
    fn get_units_per_em(&self) -> Option<i32> {
        Ok(self.0.units_per_em())
    }
    // TODO: methods.add_method_ext("getVariationDesignParameters" Ok(()));
    // TODO: methods.add_method_ext("getVariationDesignPosition" Ok(()));
    fn is_bold(&self) -> bool {
        Ok(self.is_bold())
    }
    fn is_fixed_pitch(&self) -> bool {
        Ok(self.is_fixed_pitch())
    }
    fn is_italic(&self) -> bool {
        Ok(self.is_italic())
    }
    fn make_clone(&self) -> LuaTypeface {
        Ok(LuaTypeface(self.0.clone()))
    }
    // NYI: openExistingStream by skia_safe
    // NYI: openStream by skia_safe
    fn text_to_glyphs(&self, text: LuaText) -> Vec<GlyphId> {
        let mut result = Vec::with_capacity(text.text.len());
        self.0
            .text_to_glyphs(text.text.as_bytes(), text.encoding, result.as_mut_slice());
        Ok(result)
    }
    fn string_to_glyphs(&self, text: String) -> Vec<GlyphId> {
        let mut result = Vec::with_capacity(text.len());
        self.0.str_to_glyphs(&text, result.as_mut_slice());
        Ok(result)
    }
    fn unichars_to_glyphs(&self, unichars: Vec<Unichar>) -> Vec<GlyphId> {
        let mut result = Vec::new();
        self.0.unichars_to_glyphs(&unichars, result.as_mut_slice());
        Ok(result)
    }
    fn unichar_to_glyph(&self, unichar: Unichar) -> GlyphId {
        Ok(self.0.unichar_to_glyph(unichar))
    }
}

#[derive(Clone, Copy)]
pub struct FromLuaFontWeight(pub i32);

impl FromLuaFontWeight {
    pub fn to_skia_weight(&self) -> Weight {
        Weight::from(self.0)
    }
}

impl<'lua> FromArgPack<'lua> for FromLuaFontWeight {
    fn convert(args: &mut ArgumentContext<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        static EXPECTED: &str = "'invisible', 'thin', 'extra_light', 'light', 'normal', 'medium', 'semi_bold', 'bold', 'extra_bold', 'black', 'extra_black'";
        match args.pop() {
            LuaNil => Ok(FromLuaFontWeight(*Weight::NORMAL)),
            LuaValue::Integer(number) => {
                if number < 0 {
                    return Err(LuaError::RuntimeError(
                        "font weight can't be a negative value".to_string(),
                    ));
                }
                Ok(FromLuaFontWeight(number as i32))
            }
            LuaValue::Number(number) => {
                if number < 0. {
                    return Err(LuaError::RuntimeError(
                        "font weight can't be a negative value".to_string(),
                    ));
                }
                if number.is_infinite() {
                    return Err(LuaError::RuntimeError(
                        "font weight must be finite".to_string(),
                    ));
                }
                if number.is_nan() {
                    return Err(LuaError::RuntimeError(
                        "invalid (NaN) font weight".to_string(),
                    ));
                }
                Ok(FromLuaFontWeight(number.floor() as i32))
            }
            LuaValue::String(name) => match name.to_str()? {
                "invisible" => Ok(FromLuaFontWeight(*Weight::INVISIBLE)),
                "thin" => Ok(FromLuaFontWeight(*Weight::THIN)),
                "extra_light" => Ok(FromLuaFontWeight(*Weight::EXTRA_LIGHT)),
                "light" => Ok(FromLuaFontWeight(*Weight::LIGHT)),
                "normal" => Ok(FromLuaFontWeight(*Weight::NORMAL)),
                "medium" => Ok(FromLuaFontWeight(*Weight::MEDIUM)),
                "semi_bold" => Ok(FromLuaFontWeight(*Weight::SEMI_BOLD)),
                "bold" => Ok(FromLuaFontWeight(*Weight::BOLD)),
                "extra_bold" => Ok(FromLuaFontWeight(*Weight::EXTRA_BOLD)),
                "black" => Ok(FromLuaFontWeight(*Weight::BLACK)),
                "extra_black" => Ok(FromLuaFontWeight(*Weight::EXTRA_BLACK)),
                other => Err(LuaError::RuntimeError(format!(
                    "unknown weight name: '{}'; expected a number or one of: {}",
                    other, EXPECTED
                ))),
            },
            other => Err(LuaError::RuntimeError(format!(
                "invalid font weight: '{:?}'; expected a number or name ({})",
                other, EXPECTED
            ))),
        }
    }
}

#[derive(Clone, Copy)]
pub struct FromLuaFontWidth(pub i32);

impl FromLuaFontWidth {
    pub fn to_skia_width(&self) -> Width {
        Width::from(self.0)
    }
}

impl<'lua> FromArgPack<'lua> for FromLuaFontWidth {
    fn convert(args: &mut ArgumentContext<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        static EXPECTED: &str = "'invisible', 'thin', 'extra_light', 'light', 'normal', 'medium', 'semi_bold', 'bold', 'extra_bold', 'black', 'extra_black'";
        match args.pop() {
            LuaNil => Ok(FromLuaFontWidth(*Width::NORMAL)),
            LuaValue::Integer(number) => {
                if number < 0 {
                    return Err(LuaError::RuntimeError(
                        "font width can't be a negative value".to_string(),
                    ));
                }
                Ok(FromLuaFontWidth(number as i32))
            }
            LuaValue::Number(number) => {
                if number < 0. {
                    return Err(LuaError::RuntimeError(
                        "font width can't be a negative value".to_string(),
                    ));
                }
                if number.is_infinite() {
                    return Err(LuaError::RuntimeError(
                        "font width must be finite".to_string(),
                    ));
                }
                if number.is_nan() {
                    return Err(LuaError::RuntimeError(
                        "invalid (NaN) font width".to_string(),
                    ));
                }
                Ok(FromLuaFontWidth(number.floor() as i32))
            }
            LuaValue::String(name) => match name.to_str()? {
                "ultra_condensed" => Ok(FromLuaFontWidth(*Width::ULTRA_CONDENSED)),
                "extra_condensed" => Ok(FromLuaFontWidth(*Width::EXTRA_CONDENSED)),
                "condensed" => Ok(FromLuaFontWidth(*Width::CONDENSED)),
                "semi_condensed" => Ok(FromLuaFontWidth(*Width::SEMI_CONDENSED)),
                "normal" => Ok(FromLuaFontWidth(*Width::NORMAL)),
                "semi_expanded" => Ok(FromLuaFontWidth(*Width::SEMI_EXPANDED)),
                "expanded" => Ok(FromLuaFontWidth(*Width::EXPANDED)),
                "extra_expanded" => Ok(FromLuaFontWidth(*Width::EXTRA_EXPANDED)),
                "ultra_expanded" => Ok(FromLuaFontWidth(*Width::ULTRA_EXPANDED)),
                other => Err(LuaError::FromLuaConversionError {
                    from: "string",
                    to: "Width",
                    message: Some(format!(
                        "unknown width name: '{}'; expected a number or one of: {}",
                        other, EXPECTED
                    )),
                }),
            },
            other => Err(LuaError::FromLuaConversionError {
                from: other.type_name(),
                to: "Width",
                message: Some(format!(
                    "invalid font width: '{:?}'; expected a number or name ({})",
                    other, EXPECTED
                )),
            }),
        }
    }
}

wrap_skia_handle!(FontStyle);

#[lua_methods(lua_name: FontStyle)]
impl LuaFontStyle {
    fn make(
        weight: Option<FromLuaFontWeight>,
        width: Option<FromLuaFontWidth>,
        slant: Option<LuaSlant>,
    ) -> LuaFontStyle {
        let weight = weight
            .map(|it| it.to_skia_weight())
            .unwrap_or(Weight::NORMAL);
        let width = width.map(|it| it.to_skia_width()).unwrap_or(Width::NORMAL);
        let slant = slant.unwrap_or_t(Slant::Upright);
        Ok(LuaFontStyle(FontStyle::new(weight, width, slant)))
    }

    fn weight(&self) -> i32 {
        Ok(*self.0.weight())
    }
    fn width(&self) -> i32 {
        Ok(*self.0.width())
    }
    fn slant(&self) -> LuaSlant {
        Ok(LuaSlant(self.slant()))
    }
}

wrap_skia_handle!(Font);

#[lua_methods(lua_name: Font)]
impl LuaFont {
    #[lua(constructor)]
    fn make(
        typeface: LuaTypeface,
        size: Option<f32>,
        scale_x: Option<f32>,
        skew_x: Option<f32>,
    ) -> LuaFont {
        let size = size.unwrap_or(12.0);
        let scale_x = scale_x.unwrap_or(1.0);
        let skew_x = skew_x.unwrap_or(0.0);
        Ok(LuaFont(Font::from_typeface_with_params(
            typeface, size, scale_x, skew_x,
        )))
    }

    fn count_text(&self, text: LuaText) -> usize {
        Ok(self.0.count_text(text.text.as_bytes(), text.encoding))
    }
    fn get_bounds(&self, glyphs: Vec<GlyphId>, paint: Option<LuaPaint>) -> Vec<LuaRect> {
        let mut bounds = [Rect::new_empty()].repeat(glyphs.len());
        self.0
            .get_bounds(&glyphs, &mut bounds, paint.map(LuaPaint::unwrap).as_ref());
        let bounds: Vec<LuaRect> = bounds.into_iter().map(LuaRect::from).collect();
        Ok(bounds)
    }
    fn get_edging(&self) -> LuaFontEdging {
        Ok(LuaFontEdging(self.edging()))
    }
    fn get_hinting(&self) -> LuaFontHinting {
        Ok(LuaFontHinting(self.hinting()))
    }
    fn get_intercepts(
        &self,
        glyphs: Vec<GlyphId>,
        points: Vec<LuaPoint>,
        top: f32,
        bottom: f32,
        paint: Option<LuaPaint>,
    ) -> Vec<f32> {
        let points: Vec<Point> = points.into_iter().map(|it| it.into()).collect();
        let paint = paint.map(|it| it.0);
        let intercepts = self
            .0
            .get_intercepts(&glyphs, &points, (top, bottom), paint.as_ref());
        Ok(intercepts)
    }
    fn get_metrics<'lua>(&self, lua: &'lua LuaContext) -> LuaTable<'lua> {
        self.metrics().1.to_table(lua)
    }
    fn get_path(&self, glyph: GlyphId) -> LuaPath {
        Ok(self.get_path(glyph).map(LuaPath))
    }
    fn get_paths(&self, glyphs: Vec<GlyphId>) -> HashMap<GlyphId, LuaPath> {
        Ok(glyphs
            .into_iter()
            .filter_map(|it| self.get_path(it).map(LuaPath).map(|b| (it, b)))
            .collect::<HashMap<GlyphId, LuaPath>>())
    }
    fn get_pos(&self, glyphs: Vec<GlyphId>, origin: LuaFallible<LuaPoint>) -> Vec<LuaPoint> {
        let mut points = [Point::new(0., 0.)].repeat(glyphs.len());
        let origin = origin.map(LuaPoint::into);
        self.0.get_pos(&glyphs, &mut points, origin);
        let points: Vec<_> = points.into_iter().map(LuaPoint::from).collect();
        Ok(points)
    }
    fn get_scale_x(&self) -> f32 {
        Ok(self.scale_x())
    }
    fn get_size(&self) -> f32 {
        Ok(self.size())
    }
    fn get_skew_x(&self) -> f32 {
        Ok(self.skew_x())
    }
    fn get_spacing(&self) -> f32 {
        Ok(self.spacing())
    }
    fn get_typeface(&self) -> LuaTypeface {
        Ok(self.typeface().map(LuaTypeface))
    }
    fn get_widths(&self, glyphs: Vec<GlyphId>) -> Vec<f32> {
        let mut widths = Vec::with_capacity(glyphs.len());
        self.0.get_widths(&glyphs, &mut widths);
        Ok(widths)
    }
    fn get_widths_bounds<'lua>(
        &self,
        lua: &'lua LuaContext,
        glyphs: Vec<GlyphId>,
        paint: Option<LuaPaint>,
    ) -> LuaTable<'lua> {
        let mut widths = Vec::with_capacity(glyphs.len());
        let mut bounds = Vec::with_capacity(glyphs.len());
        self.0.get_widths_bounds(
            &glyphs,
            Some(&mut widths),
            Some(&mut bounds),
            paint.map(LuaPaint::unwrap).as_ref(),
        );
        let result = lua.create_table()?;
        result.set("widths", widths)?;
        result.set(
            "bounds",
            bounds.into_iter().map(LuaRect::from).collect::<Vec<_>>(),
        )?;
        Ok(result)
    }
    fn get_x_pos(&self, glyphs: Vec<GlyphId>, origin: Option<f32>) -> Vec<f32> {
        let mut result = Vec::with_capacity(glyphs.len());
        self.0.get_x_pos(&glyphs, &mut result, origin);
        Ok(result)
    }
    fn is_baseline_snap(&self) -> bool {
        Ok(self.0.is_baseline_snap())
    }
    fn is_embedded_bitmaps(&self) -> bool {
        Ok(self.0.is_embedded_bitmaps())
    }
    fn is_embolden(&self) -> bool {
        Ok(self.0.is_embolden())
    }
    fn is_force_auto_hinting(&self) -> bool {
        Ok(self.0.is_force_auto_hinting())
    }
    fn is_linear_metrics(&self) -> bool {
        Ok(self.0.is_linear_metrics())
    }
    fn is_subpixel(&self) -> bool {
        Ok(self.0.is_subpixel())
    }
    fn make_with_size(&self, size: f32) -> LuaFont {
        Ok(self.with_size(size).map(LuaFont))
    }
    fn measure_text(&self, text: LuaText, paint: Option<LuaPaint>) -> (f32, LuaRect) {
        let measurements = self.0.measure_text(
            text.text.as_bytes(),
            text.encoding,
            paint.map(LuaPaint::unwrap).as_ref(),
        );
        Ok((measurements.0, LuaRect::from(measurements.1)))
    }
    fn set_baseline_snap(&mut self, baseline_snap: bool) {
        self.set_baseline_snap(baseline_snap);
        Ok(())
    }
    fn set_edging(&mut self, edging: LuaFontEdging) {
        self.set_edging(*edging);
        Ok(())
    }
    fn set_embedded_bitmaps(&mut self, embedded_bitmaps: bool) {
        self.set_embedded_bitmaps(embedded_bitmaps);
        Ok(())
    }
    fn set_embolden(&mut self, embolden: bool) {
        self.set_embolden(embolden);
        Ok(())
    }
    fn set_force_auto_hinting(&mut self, force_auto_hinting: bool) {
        self.set_force_auto_hinting(force_auto_hinting);
        Ok(())
    }
    fn set_hinting(&mut self, hinting: LuaFontHinting) {
        self.set_hinting(*hinting);
        Ok(())
    }
    fn set_linear_metrics(&mut self, linear_metrics: bool) {
        self.set_linear_metrics(linear_metrics);
        Ok(())
    }
    fn set_scale_x(&mut self, scale: f32) {
        self.set_scale_x(scale);
        Ok(())
    }
    fn set_size(&mut self, size: f32) {
        self.set_size(size);
        Ok(())
    }
    fn set_skew_x(&mut self, skew: f32) {
        self.set_skew_x(skew);
        Ok(())
    }
    fn set_subpixel(&mut self, subpixel: bool) {
        self.set_subpixel(subpixel);
        Ok(())
    }
    fn set_typeface(&mut self, typeface: LuaTypeface) {
        self.set_typeface(typeface.unwrap());
        Ok(())
    }
    fn text_to_glyphs(&self, text: LuaText) {
        self.text_to_glyphs_vec(text.text.as_bytes(), text.encoding);
        Ok(())
    }
    fn unichars_to_glyphs(&self, unichars: Vec<Unichar>) -> Vec<GlyphId> {
        let mut result = Vec::with_capacity(unichars.len());
        self.unichar_to_glyphs(&unichars, &mut result);
        Ok(result)
    }
    fn unichar_to_glyph(&self, unichar: Unichar) {
        Ok(self.unichar_to_glyph(unichar))
    }
}

wrap_skia_handle!(TextBlob);

#[lua_methods(lua_name: TextBlob)]
impl LuaTextBlob {
    fn make_from_pos_text(text: LuaText, pos: Vec<LuaPoint>, font: LuaFont) -> Option<LuaTextBlob> {
        let pos: Vec<Point> = pos.into_iter().map(LuaPoint::into).collect();
        Ok(
            TextBlob::from_pos_text(text.text.as_bytes(), &pos, &font, text.encoding)
                .map(LuaTextBlob),
        )
    }
    fn make_from_pos_text_h(
        text: LuaText,
        x_pos: Vec<f32>,
        const_y: f32,
        font: LuaFont,
    ) -> Option<LuaTextBlob> {
        Ok(
            TextBlob::from_pos_text_h(text.text.as_bytes(), &x_pos, const_y, &font, text.encoding)
                .map(LuaTextBlob),
        )
    }
    // TODO: make_from_RSXform()
    fn make_from_string(string: String, font: LuaFont) -> Option<LuaTextBlob> {
        Ok(TextBlob::new(string, &font).map(LuaTextBlob))
    }
    fn make_from_text(text: LuaText, font: LuaFont) -> Option<LuaTextBlob> {
        Ok(TextBlob::from_text(text.text.as_bytes(), text.encoding, &font).map(LuaTextBlob))
    }

    fn bounds(&self) -> LuaRect {
        Ok(LuaRect::from(*self.0.bounds()))
    }
    fn get_intercepts(&self, bounds: LuaPoint, paint: Option<LikePaint>) -> Vec<f32> {
        Ok(self
            .0
            .get_intercepts(bounds.as_array(), paint.map(LikePaint::unwrap).as_ref()))
    }
}

#[derive(Clone)]
pub struct LuaSaveLayerRec {
    bounds: Option<Rect>,
    paint: Option<LikePaint>,
    backdrop: Option<LuaImageFilter>,
    flags: SaveLayerFlags,
}

impl LuaSaveLayerRec {
    pub fn to_skia_save_layer_rec(&self) -> SaveLayerRec {
        let mut result = SaveLayerRec::default();
        if let Some(bounds) = &self.bounds {
            result = result.bounds(bounds);
        }
        if let Some(paint) = &self.paint {
            result = result.paint(paint);
        }
        if let Some(backdrop) = &self.backdrop {
            result = result.backdrop(backdrop);
        }
        if !self.flags.is_empty() {
            result = result.flags(self.flags);
        }
        result
    }
}

impl<'lua> FromArgPack<'lua> for LuaSaveLayerRec {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        let mut result = LuaSaveLayerRec {
            bounds: None,
            paint: None,
            backdrop: None,
            flags: SaveLayerFlags::empty(),
        };
        let table = match args.pop() {
            LuaValue::Table(it) => it,
            LuaNil => return Ok(result),
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "SaveTableRec",
                    message: Some("expected a SaveTableRec table or nil".to_string()),
                });
            }
        };

        if table.contains_key("bounds")? {
            result.bounds = Some(
                table
                    .get::<&'static str, LuaRect>("bounds")
                    .map_err(|inner| LuaError::CallbackError {
                        traceback: "while reading SaveLayerRec bounds entry".to_string(),
                        cause: Arc::new(inner),
                    })?
                    .into(),
            );
        }

        if table.contains_key("paint")? {
            result.paint = Some(table.get("paint")?)
        }

        if table.contains_key("backdrop")? {
            result.backdrop = Some(table.get_user_data("backdrop")?)
        }

        if table.contains_key("flags")? {
            let flags_value: LuaValue = table.get("flags")?;
            match flags_value {
                LuaValue::String(flag) => {
                    result.flags = LuaSaveLayerFlags::try_from(flag)?.0;
                }
                LuaValue::Table(list) => {
                    result.flags = LuaSaveLayerFlags::from_table(list)?.0;
                }
                LuaNil => {}
                _ => {
                    return Err(LuaError::FromLuaConversionError {
                        from: "table",
                        to: "SaveLayerFlags",
                        message: Some("expected SaveLayerRec.flags entry to be a SaveLayerFlags string of array of strings".to_string()),
                    })
                }
            }
        }

        Ok(result)
    }
}

#[derive(Clone)]
pub enum LuaCanvas<'a> {
    Owned(Surface),
    Borrowed(&'a Canvas),
}

unsafe impl<'a> Send for LuaCanvas<'a> {}

impl<'a> std::ops::Deref for LuaCanvas<'a> {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target {
        match self {
            LuaCanvas::Owned(surface) => {
                let surface = unsafe {
                    // SAFETY: This isn't safe. BUT, owning a RCHandle<SkSurface>
                    // doesn't guarantee unique mutable access to surface
                    // data either due to how the C++ API is built.
                    // This mut cast is however necessary because &Canvas allows
                    // mutating underlying data even though it's "immutable access".

                    // FIXME: Investigate Surface-Canvas ownership
                    addr_of!(*surface).cast_mut().as_mut().unwrap_unchecked()
                };
                surface.canvas()
            }
            LuaCanvas::Borrowed(it) => it,
        }
    }
}

#[lua_methods(lua_name: Canvas)]
impl<'a> LuaCanvas<'a> {
    fn clear(&self, color: LuaFallible<LuaColor>) {
        let color = color
            .map(LuaColor::into)
            .unwrap_or(skia_safe::colors::TRANSPARENT);
        self.clear(color);
        Ok(())
    }
    fn draw_color(&self, color: LuaColor, blend_mode: LuaFallible<LuaBlendMode>) {
        self.draw_color(color, blend_mode.map_t());
        Ok(())
    }
    fn draw_paint(&self, paint: LikePaint) {
        self.draw_paint(&paint);
        Ok(())
    }
    fn draw_rect(&self, rect: LuaRect, paint: LikePaint) {
        let rect: Rect = rect.into();
        self.draw_rect(rect, &paint);
        Ok(())
    }
    fn draw_oval(&self, oval: LuaRect, paint: LikePaint) {
        let oval: Rect = oval.into();
        self.draw_oval(oval, &paint);
        Ok(())
    }
    fn draw_circle(&self, point: LuaPoint, r: f32, paint: LikePaint) {
        self.draw_circle(point, r, &paint);
        Ok(())
    }
    fn draw_image(&self, image: LuaImage, point: LuaPoint, paint: LuaFallible<LikePaint>) {
        self.draw_image(image.unwrap(), point, paint.map(LikePaint::unwrap).as_ref());
        Ok(())
    }
    fn draw_image_rect(
        &self,
        image: LuaImage,
        src_rect: Option<LuaRect>,
        dst_rect: LuaRect,
        paint: Option<LikePaint>,
    ) {
        let paint: Paint = match paint {
            Some(it) => it.unwrap(),
            None => Paint::default(),
        };
        let src_rect = src_rect.map(|it| it.into());
        let dst_rect: Rect = dst_rect.into();
        self.draw_image_rect(
            image.unwrap(),
            src_rect
                .as_ref()
                .map(|rect| (rect, canvas::SrcRectConstraint::Fast)),
            dst_rect,
            &paint,
        );
        Ok(())
    }
    fn draw_patch(
        &self,
        cubics_table: Vec<LuaPoint>,
        colors: LuaFallible<Vec<LuaColor>>,
        tex_coords: LuaFallible<Vec<LuaPoint>>,
        blend_mode: LuaBlendMode,
        paint: LikePaint,
    ) {
        if cubics_table.len() != 12 {
            return Err(LuaError::RuntimeError(
                "expected 12 cubic points".to_string(),
            ));
        }
        let mut cubics = [Point::new(0.0, 0.0); 12];
        for i in 0..12 {
            cubics[i] = cubics_table[i].into();
        }

        let colors = match colors.into_inner() {
            Some(colors) => {
                let mut result = [Color::TRANSPARENT; 4];
                for i in 0..4 {
                    result[i] = colors[i].into();
                }
                Some(result)
            }
            None => None,
        };

        let tex_coords = match tex_coords.into_inner() {
            Some(coords) => {
                if coords.len() != 4 {
                    return Err(LuaError::RuntimeError(
                        "expected 4 texture coordinates".to_string(),
                    ));
                }
                let mut result = [Point::new(0.0, 0.0); 4];
                for i in 0..4 {
                    result[i] = coords[i].into();
                }
                Some(result)
            }
            None => None,
        };

        self.draw_patch(
            &cubics,
            colors.as_ref(),
            tex_coords.as_ref(),
            *blend_mode,
            &paint,
        );
        Ok(())
    }
    fn draw_path(&self, path: LuaPath, paint: LikePaint) {
        self.draw_path(&path, &paint);
        Ok(())
    }
    fn draw_picture(
        &self,
        picture: LuaPicture,
        matrix: LuaFallible<LuaMatrix>,
        paint: LuaFallible<LikePaint>,
    ) {
        let matrix: Option<Matrix> = matrix.map(LuaMatrix::into);
        let paint: Option<Paint> = paint.map(LikePaint::unwrap);
        self.draw_picture(picture, matrix.as_ref(), paint.as_ref());
        Ok(())
    }
    fn draw_text_blob(&self, blob: LuaTextBlob, point: LuaPoint, paint: LikePaint) {
        self.draw_text_blob(blob.unwrap(), point, &paint);
        Ok(())
    }
    fn get_save_count(&self) -> usize {
        Ok(self.save_count())
    }
    fn get_local_to_device(&self) -> LuaMatrix {
        Ok(LuaMatrix::Four(self.local_to_device()))
    }
    fn get_local_to_device3x3(&self) -> LuaMatrix {
        Ok(LuaMatrix::Three(self.local_to_device_as_3x3()))
    }
    fn save(&self) -> usize {
        Ok(self.save())
    }
    fn save_layer(&self, save_layer_rec: LuaSaveLayerRec) -> usize {
        Ok(self.save_layer(&save_layer_rec.to_skia_save_layer_rec()))
    }
    fn restore(&self) {
        self.restore();
        Ok(())
    }
    fn restore_to_count(&self, count: usize) {
        self.restore_to_count(count);
        Ok(())
    }
    fn scale(&self, sx: f32, sy: LuaFallible<f32>) {
        let sy = sy.unwrap_or(sx);
        self.deref().scale((sx, sy));
        Ok(())
    }
    fn translate(&self, point: LuaPoint) {
        self.translate(point);
        Ok(())
    }
    fn rotate(&self, degrees: f32, point: LuaFallible<LuaPoint>) {
        let point = point.map(LuaPoint::into);
        self.rotate(degrees, point);
        Ok(())
    }
    fn concat(&self, matrix: LuaMatrix) {
        match matrix {
            LuaMatrix::Three(matrix) => self.concat(&matrix),
            LuaMatrix::Four(matrix) => self.concat_44(&matrix),
        };
        Ok(())
    }
    fn new_surface(&self, info: LikeImageInfo, props: LuaFallible<LikeSurfaceProps>) {
        self.new_surface(&info, props.map(|it| *it).as_ref());
        Ok(())
    }
    fn width(&self) -> i32 {
        Ok(self.base_layer_size().width)
    }
    fn height(&self) -> i32 {
        Ok(self.base_layer_size().height)
    }
}

macro_rules! global_constructors {
    ($ctx: ident: $($t: ty),* $(,)?) => {paste::paste!{
        $(
            [<Lua $t>]::register_globals($ctx)?;
        )*
    }};
}

// TODO: filter conversion isn't automatic
#[allow(non_snake_case)]
pub fn setup(lua: &LuaContext) -> Result<(), mlua::Error> {
    global_constructors!(lua:
        ColorFilter,
        ColorSpace,
        Font,
        FontMgr,
        FontStyle,
        FontStyleSet,
        Image,
        ImageFilter,
        Matrix,
        Paint,
        Path,
        PathEffect,
        RRect,
        StrokeRec,
        Surface,
        TextBlob,
        Typeface,
    );
    Ok(())
}
