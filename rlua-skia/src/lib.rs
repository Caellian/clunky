use std::{
    alloc::Layout,
    any::type_name,
    collections::{HashMap, VecDeque},
    ffi::OsString,
    mem::{align_of, size_of},
    os::unix::ffi::{OsStrExt, OsStringExt},
    ptr::addr_of,
    str::FromStr,
    sync::{Arc, OnceLock},
};

use byteorder::WriteBytesExt;
use phf::phf_map;
use rlua::{prelude::*, Context as LuaContext, Table as LuaTable, UserData};
use skia_safe::{
    canvas::{self, SaveLayerFlags, SaveLayerRec},
    color_filter::color_filters,
    font::Edging as FontEdging,
    font_style::{Slant, Weight, Width},
    gradient_shader::interpolation::{ColorSpace as InColorSpace, HueMethod, InPremul},
    gradient_shader::Interpolation,
    image_filter::MapDirection,
    image_filters::{self, CropRect},
    matrix::{ScaleToFit, TypeMask},
    paint::{Cap as PaintCap, Join as PaintJoin, Style as PaintStyle},
    path::{AddPathMode, ArcSize, SegmentMask, Verb},
    path_effect::DashInfo,
    rrect::{Corner as RRectCorner, Type as RRectType},
    stroke_rec::{InitStyle as StrokeRecInitStyle, Style as StrokeRecStyle},
    trim_path_effect::Mode as TrimMode,
    typeface::FontTableTag,
    *,
};

pub mod ext;
pub mod util;

use crate::{ext::rlua::*, ext::skia::*, util::hsl_to_rgb};

// SECTION: Utility traits

/// Allows declaring a wrapper type and automatically implementing all of the
/// remaining utility traits on that type and wrappers around it.
trait WrapperT<'lua> {
    type Wrapped;
    fn unwrap(self) -> Self::Wrapped;
}

/// Mapping and unwrapping utilities for [`Option`]al values.
trait FromLuaOption<T>: Sized {
    fn map_t(self) -> Option<T>;

    fn unwrap_or_t(self, value: T) -> T;

    #[inline]
    fn unwrap_or_default_t(self) -> T
    where
        T: Default,
    {
        Self::unwrap_or_t(self, T::default())
    }

    fn unwrap_or_else_t(self, value_fn: impl Fn() -> T) -> T;
}

impl<'lua, T, W: WrapperT<'lua, Wrapped = T>> FromLuaOption<T> for Option<W> {
    #[inline(always)]
    fn map_t(self) -> Option<T> {
        self.map(WrapperT::unwrap)
    }

    #[inline(always)]
    fn unwrap_or_t(self, value: T) -> T {
        self.map(WrapperT::unwrap).unwrap_or(value)
    }

    #[inline(always)]
    fn unwrap_or_else_t(self, value_fn: impl Fn() -> T) -> T {
        self.map(WrapperT::unwrap).unwrap_or_else(value_fn)
    }
}

/// Mapping and unwrapping utilities for [`Result`].
trait FromLuaResult<T>: Sized {
    type Error;

    fn map_t(self) -> Result<T, Self::Error>;

    fn unwrap_or_t(self, value: T) -> T;

    #[inline]
    fn unwrap_or_default_t(self) -> T
    where
        T: Default,
    {
        Self::unwrap_or_t(self, T::default())
    }

    fn unwrap_or_else_t(self, value_fn: impl Fn() -> T) -> T;
}

/// Any [`Result`] for which [`FromLuaOption`] is implemented can be handled
/// though that implementation as `FromLuaResult` only touches the `Ok` case.
impl<T, F, E> FromLuaResult<T> for Result<F, E>
where
    Option<F>: FromLuaOption<T>,
{
    type Error = E;

    #[inline(always)]
    fn map_t(self) -> Result<T, Self::Error> {
        Ok(Option::<F>::Some(self?).map_t().unwrap())
    }

    #[inline(always)]
    fn unwrap_or_t(self, value: T) -> T {
        self.ok().unwrap_or_t(value)
    }

    #[inline(always)]
    fn unwrap_or_else_t(self, value_fn: impl Fn() -> T) -> T {
        self.ok().unwrap_or_else_t(value_fn)
    }
}

/// Applies TableExt to reading table values with wrapper types, automatically
/// handling unwrapping.
trait TableWrapperExt<'lua>: TableExt<'lua> {
    #[inline(always)]
    fn try_get_t<K: ToLua<'lua>, W: WrapperT<'lua> + FromLua<'lua>>(
        &self,
        key: K,
        lua: LuaContext<'lua>,
    ) -> Result<Option<W::Wrapped>, LuaError> {
        TableExt::try_get(self, key, lua).map(|result| result.map(W::unwrap))
    }

    #[inline(always)]
    fn try_get_or_t<K: ToLua<'lua>, W: WrapperT<'lua> + FromLua<'lua>>(
        &self,
        key: K,
        lua: LuaContext<'lua>,
        default: W::Wrapped,
    ) -> Result<W::Wrapped, LuaError> {
        self.try_get_t::<K, W>(key, lua)
            .map(|it| it.unwrap_or(default))
    }

    #[inline(always)]
    fn try_get_or_default_t<K: ToLua<'lua>, W: WrapperT<'lua> + FromLua<'lua>>(
        &self,
        key: K,
        lua: LuaContext<'lua>,
    ) -> Result<W::Wrapped, LuaError>
    where
        W::Wrapped: Default,
    {
        self.try_get_or_t::<K, W>(key, lua, W::Wrapped::default())
    }
}

impl<'lua> TableWrapperExt<'lua> for LuaTable<'lua> {}

// !SECTION

macro_rules! named_enum {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct [<Lua $kind>]($kind);

        #[allow(unused)]
        static [<NAME_TO_ $kind:snake:upper>]: phf::Map<&'static str, $kind> = phf_map! {
            $($name => $value),
            +
        };

        impl [<Lua $kind>] {
            fn expected_values() -> &'static str {
                static EXPECTED: OnceLock<String> = OnceLock::new();

                EXPECTED.get_or_init(|| [
                    $(concat!("'", $name, "'")),+
                ].join(", "))
            }

            pub fn unwrap(&self) -> $kind {
                self.0
            }
        }

        impl<'lua> WrapperT<'lua> for [<Lua $kind>] {
            type Wrapped = $kind;

            #[inline]
            fn unwrap(self) -> $kind {
                self.0
            }
        }

        impl std::ops::Deref for [<Lua $kind>] {
            type Target = $kind;

            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl std::ops::DerefMut for [<Lua $kind>] {
            #[inline]
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl AsRef<$kind> for [<Lua $kind>] {
            #[inline]
            fn as_ref(&self) -> &$kind {
                &self.0
            }
        }

        impl<'lua> FromStr for [<Lua $kind>] {
            type Err = LuaError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let value = match [<NAME_TO_ $kind:snake:upper>].get(value.to_ascii_lowercase().as_str()) {
                    Some(it) => *it,
                    None => return Err(LuaError::FromLuaConversionError {
                        from: "string",
                        to: stringify!($kind),
                        message: Some(format!(
                            concat!["unknown ", stringify!($kind), " name: '{}'; expected one of: {}"],
                            value,
                            Self::expected_values()
                        )),
                    }),
                };

                Ok([<Lua $kind>](value))
            }
        }

        impl<'lua> TryFrom<String> for [<Lua $kind>] {
            type Error = LuaError;

            #[inline(always)]
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::from_str(value.as_str())
            }
        }

        impl<'lua> TryFrom<LuaString<'lua>> for [<Lua $kind>] {
            type Error = LuaError;

            #[inline(always)]
            fn try_from(value: LuaString<'lua>) -> Result<Self, Self::Error> {
                Self::from_str(value.to_str()?)
            }
        }

        impl<'lua> FromLua<'lua> for [<Lua $kind>] {
            fn from_lua(text: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
                let text = match text {
                    LuaValue::String(it) => it,
                    other => {
                        return Err(LuaError::FromLuaConversionError {
                            from: other.type_name(),
                            to: "PaintStyle",
                            message: Some(format!(
                                "expected a PaintStyle string value; one of: {}",
                                Self::expected_values()
                            )),
                        })
                    }
                };

                Self::try_from(text)
            }
        }

        impl<'lua> ToLua<'lua> for [<Lua $kind>] {
            #[allow(unreachable_patterns)]
            fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
                lua.create_string(match self.0 {
                    $($value => $name,)
                    +
                    _ => return Ok(LuaNil),
                }).map(LuaValue::String)
            }
        }
    }};
}

named_enum! { BlendMode: [
    BlendMode::Clear => "clear",
    BlendMode::Src => "src",
    BlendMode::Dst => "dst",
    BlendMode::SrcOver => "src_over",
    BlendMode::DstOver => "dst_over",
    BlendMode::SrcIn => "src_in",
    BlendMode::DstIn => "dst_in",
    BlendMode::SrcOut => "src_out",
    BlendMode::DstOut => "dst_out",
    BlendMode::SrcATop => "src_a_top",
    BlendMode::DstATop => "dst_a_top",
    BlendMode::Xor => "xor",
    BlendMode::Plus => "plus",
    BlendMode::Modulate => "modulate",
    BlendMode::Screen => "screen",
    BlendMode::Overlay => "overlay",
    BlendMode::Darken => "darken",
    BlendMode::Lighten => "lighten",
    BlendMode::ColorDodge => "color_dodge",
    BlendMode::ColorBurn => "color_burn",
    BlendMode::HardLight => "hard_light",
    BlendMode::SoftLight => "soft_light",
    BlendMode::Difference => "difference",
    BlendMode::Exclusion => "exclusion",
    BlendMode::Multiply => "multiply",
    BlendMode::Hue => "hue",
    BlendMode::Saturation => "saturation",
    BlendMode::Color => "color",
    BlendMode::Luminosity => "luminosity",
]}

named_enum! { PaintCap : [
    PaintCap::Butt => "butt",
    PaintCap::Round => "round",
    PaintCap::Square => "square",
]}

named_enum! { PaintJoin : [
    PaintJoin::Miter => "miter",
    PaintJoin::Round => "round",
    PaintJoin::Bevel => "bevel",
]}

named_enum! { Slant : [
    Slant::Upright => "upright",
    Slant::Italic => "italic",
    Slant::Oblique => "oblique",
]}

named_enum! { ScaleToFit : [
    ScaleToFit::Fill => "fill",
    ScaleToFit::Start => "start",
    ScaleToFit::Center => "center",
    ScaleToFit::End => "end",
]}

named_enum! { PathDirection : [
    PathDirection::CW => "cw",
    PathDirection::CCW => "ccw",
]}

named_enum! { AddPathMode : [
    AddPathMode::Append => "append",
    AddPathMode::Extend => "extend",
]}

named_enum! { ArcSize : [
    ArcSize::Small => "small",
    ArcSize::Large => "large",
]}

named_enum! { Verb : [
    Verb::Move => "move",
    Verb::Line => "line",
    Verb::Quad => "quad",
    Verb::Conic => "conic",
    Verb::Cubic => "cubic",
    Verb::Close => "close",
    Verb::Done => "done",
]}

named_enum! { PathFillType : [
    PathFillType::Winding => "winding",
    PathFillType::EvenOdd => "evenodd",
    PathFillType::InverseWinding => "inverse_winding",
    PathFillType::InverseEvenOdd => "inverse_evenodd",
]}

named_enum! { MapDirection : [
    MapDirection::Forward => "forward",
    MapDirection::Reverse => "reverse",
]}

named_enum! { StrokeRecInitStyle : [
    StrokeRecInitStyle::Hairline => "hairline",
    StrokeRecInitStyle::Fill => "fill",
]}

named_enum! { StrokeRecStyle : [
    StrokeRecStyle::Hairline => "hairline",
    StrokeRecStyle::Fill => "fill",
    StrokeRecStyle::Stroke => "stroke",
    StrokeRecStyle::StrokeAndFill => "stroke_and_fill",
]}

named_enum! { ColorType : [
    ColorType::Unknown => "unknown",
    ColorType::Alpha8 => "alpha8",
    ColorType::RGB565 => "rgb565",
    ColorType::ARGB4444 => "argb4444",
    ColorType::RGBA8888 => "rgba8888",
    ColorType::RGB888x => "rgb888x",
    ColorType::BGRA8888 => "bgra8888",
    ColorType::RGBA1010102 => "rgba1010102",
    ColorType::BGRA1010102 => "bgra1010102",
    ColorType::RGB101010x => "rgb101010x",
    ColorType::BGR101010x => "bgr101010x",
    ColorType::BGR101010xXR => "bgr101010xxr",

    ColorType::RGBA10x6 => "rgba10x6",
    ColorType::Gray8 => "gray8",
    ColorType::RGBAF16Norm => "rgbaf16_norm",
    ColorType::RGBAF16 => "rgbaf16",
    ColorType::RGBAF32 => "rgbaf32",

    ColorType::R8G8UNorm => "r8g8u_norm",

    ColorType::A16Float => "a16_float",
    ColorType::R16G16Float => "r16g16_float",

    ColorType::A16UNorm => "a16u_norm",
    ColorType::R16G16UNorm => "r16g16u_norm",
    ColorType::R16G16B16A16UNorm => "r16g16b16a16u_norm",

    ColorType::SRGBA8888 => "srgba8888",
    ColorType::R8UNorm => "r8u_norm",
]}

named_enum! { AlphaType : [
    AlphaType::Unknown => "unknown",
    AlphaType::Opaque => "opaque",
    AlphaType::Premul => "premul",
    AlphaType::Unpremul => "unpremul",
]}

named_enum! { PixelGeometry: [
    PixelGeometry::Unknown => "unknown",
    PixelGeometry::RGBH => "rgbh",
    PixelGeometry::BGRH => "bgrh",
    PixelGeometry::RGBV => "rgbv",
    PixelGeometry::BGRV => "bgrv",
]}

named_enum! { FontEdging: [
    FontEdging::Alias => "alias",
    FontEdging::AntiAlias => "anti_alias",
    FontEdging::SubpixelAntiAlias => "subpixel_anti_alias",
]}

named_enum! { FontHinting: [
    FontHinting::None => "none",
    FontHinting::Slight => "slight",
    FontHinting::Normal => "normal",
    FontHinting::Full => "full",
]}

named_enum! { TextEncoding: [
    TextEncoding::UTF8 => "utf8",
    TextEncoding::UTF16 => "utf16",
    TextEncoding::UTF32 => "utf32",
]}

named_enum! { RRectType: [
    RRectType::Empty => "empty",
    RRectType::Rect => "rect",
    RRectType::Oval => "oval",
    RRectType::Simple => "simple",
    RRectType::NinePatch => "nine_patch",
    RRectType::Complex => "complex",
]}

named_enum! { RRectCorner: [
    RRectCorner::UpperLeft => "upper_left",
    RRectCorner::UpperRight => "upper_right",
    RRectCorner::LowerRight => "lower_right",
    RRectCorner::LowerLeft => "lower_left",
]}

named_enum! { TrimMode: [
    TrimMode::Normal => "normal",
    TrimMode::Inverted => "inverted",
]}

named_enum! { FilterMode: [
    FilterMode::Nearest => "nearest",
    FilterMode::Linear => "linear",
]}

named_enum! { MipmapMode: [
    MipmapMode::None => "none",
    MipmapMode::Nearest => "nearest",
    MipmapMode::Linear => "linear",
]}

named_enum! { TileMode: [
    TileMode::Clamp => "clamp",
    TileMode::Repeat => "repeat",
    TileMode::Mirror => "mirror",
    TileMode::Decal => "decal",
]}

named_enum! { ColorChannel: [
    ColorChannel::R => "r",
    ColorChannel::G => "g",
    ColorChannel::B => "b",
    ColorChannel::A => "a",
]}

named_enum! { HueMethod: [
    HueMethod::Shorter => "shorter",
    HueMethod::Longer => "longer",
    HueMethod::Increasing => "increasing",
    HueMethod::Decreasing => "decreasing",
]}

named_enum! { InColorSpace: [
    InColorSpace::Destination => "destination",
    InColorSpace::SRGBLinear => "srgb_linear",
    InColorSpace::Lab => "lab",
    InColorSpace::OKLab => "oklab",
    InColorSpace::LCH => "lch",
    InColorSpace::OKLCH => "oklch",
    InColorSpace::SRGB => "srgb",
    InColorSpace::HSL => "hsl",
    InColorSpace::HWB => "hwb",
]}

named_enum! { BlurStyle: [
    BlurStyle::Normal => "normal",
    BlurStyle::Solid => "solid",
    BlurStyle::Outer => "outer",
    BlurStyle::Inner => "inner",
]}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaInPremul(InPremul);

#[allow(unused)]
static NAME_TO_IN_PREMUL: phf::Map<&'static str, InPremul> = phf_map! {
  "yes" => (InPremul::Yes),
  "true" => (InPremul::Yes),
  "no" => (InPremul::No),
  "false" => (InPremul::No)
};
impl LuaInPremul {
    fn expected_values() -> &'static str {
        static EXPECTED: OnceLock<String> = OnceLock::new();
        EXPECTED.get_or_init(|| [concat!("'", "yes", "'"), concat!("'", "no", "'")].join(", "))
    }
    pub fn unwrap(&self) -> InPremul {
        self.0
    }
}

impl<'lua> WrapperT<'lua> for LuaInPremul {
    type Wrapped = InPremul;

    #[inline]
    fn unwrap(self) -> InPremul {
        self.0
    }
}

impl std::ops::Deref for LuaInPremul {
    type Target = InPremul;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for LuaInPremul {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl AsRef<InPremul> for LuaInPremul {
    #[inline]
    fn as_ref(&self) -> &InPremul {
        &self.0
    }
}
impl<'lua> FromStr for LuaInPremul {
    type Err = LuaError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = match NAME_TO_IN_PREMUL.get(value.to_ascii_lowercase().as_str()) {
            Some(it) => *it,
            None => {
                return Err(LuaError::FromLuaConversionError {
                    from: "string",
                    to: stringify!(InPremul),
                    message: Some(format!(
                        concat![
                            "unknown ",
                            stringify!(InPremul),
                            " name: '{}'; expected one of: {}"
                        ],
                        value,
                        Self::expected_values()
                    )),
                })
            }
        };
        Ok(LuaInPremul(value))
    }
}
impl<'lua> TryFrom<String> for LuaInPremul {
    type Error = LuaError;
    #[inline(always)]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_str(value.as_str())
    }
}
impl<'lua> TryFrom<LuaString<'lua>> for LuaInPremul {
    type Error = LuaError;
    #[inline(always)]
    fn try_from(value: LuaString<'lua>) -> Result<Self, Self::Error> {
        Self::from_str(value.to_str()?)
    }
}
impl<'lua> FromLua<'lua> for LuaInPremul {
    fn from_lua(text: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let text = match text {
            LuaValue::String(it) => it,
            LuaValue::Boolean(value) => {
                return Ok(match value {
                    true => LuaInPremul(InPremul::Yes),
                    false => LuaInPremul(InPremul::No),
                })
            }
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "PaintStyle",
                    message: Some(format!(
                        "expected a PaintStyle string value; one of: {}",
                        Self::expected_values()
                    )),
                })
            }
        };
        Self::try_from(text)
    }
}
impl<'lua> ToLua<'lua> for LuaInPremul {
    #[allow(unreachable_patterns)]
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        lua.create_string(match self.0 {
            InPremul::Yes => "yes",
            InPremul::No => "no",
            _ => return Ok(LuaNil),
        })
        .map(LuaValue::String)
    }
}

macro_rules! named_bitflags {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        named_enum! { $kind : [
            $($value => $name,)+
        ]}

        impl [<Lua $kind>] {
            pub fn from_table<'lua>(table: LuaTable<'lua>) -> Result<Self, LuaError> {
                let mut result = $kind::empty();
                for pair in table.pairs::<usize, String>() {
                    if let Ok((_, name)) = pair {
                        result |= [<Lua $kind>]::try_from(name)?.0;
                    } else {
                        return Err(LuaError::FromLuaConversionError {
                            from: "table",
                            to: stringify!($kind),
                            message: Some(concat!("expected ", stringify!($kind), " array to be an array of strings").to_string()),
                        });
                    }
                }
                Ok(Self(result))
            }

            pub fn to_table<'lua>(&self, ctx: LuaContext<'lua>) -> Result<LuaTable<'lua>, LuaError> {
                let result = ctx.create_table()?;
                let mut i: usize = 0;
                for entry in [$($value),+] {
                    if self.contains(entry) {
                        result.set(i, [<Lua $kind>](entry))?;
                        i += 1;
                    }
                }
                Ok(result)
            }
        }
    }};
}

named_bitflags! { SaveLayerFlags : [
    SaveLayerFlags::PRESERVE_LCD_TEXT => "preserve_lcd_text",
    SaveLayerFlags::INIT_WITH_PREVIOUS => "init_with_previous",
    SaveLayerFlags::F16_COLOR_TYPE => "f16_color_type",
]}

named_bitflags! { TypeMask : [
    TypeMask::IDENTITY => "identity",
    TypeMask::TRANSLATE => "translate",
    TypeMask::SCALE => "scale",
    TypeMask::AFFINE => "affine",
    TypeMask::PERSPECTIVE => "perspective",
]}

named_bitflags! { SegmentMask : [
    SegmentMask::LINE => "line",
    SegmentMask::QUAD => "quad",
    SegmentMask::CONIC => "conic",
    SegmentMask::CUBIC => "cubic",
]}

named_bitflags! { SurfacePropsFlags: [
    SurfacePropsFlags::USE_DEVICE_INDEPENDENT_FONTS => "use_device_independent_fonts",
    SurfacePropsFlags::DYNAMIC_MSAA => "dynamic_msaa",
    SurfacePropsFlags::ALWAYS_DITHER => "always_dither",
]}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LuaPaintStyle(PaintStyle);

#[allow(unused)]
static NAME_TO_PAINT_STYLE: phf::Map<&'static str, PaintStyle> = phf_map! {
  "fill" => (PaintStyle::Fill),
  "stroke" => (PaintStyle::Stroke),
  "fill_and_stroke" => (PaintStyle::StrokeAndFill),
  "fill,stroke" => (PaintStyle::StrokeAndFill),
  "stroke_and_fill" => (PaintStyle::StrokeAndFill),
  "stroke,fill" => (PaintStyle::StrokeAndFill)
};

impl LuaPaintStyle {
    fn expected_values() -> &'static str {
        "'fill', 'stroke', 'stroke_and_fill'"
    }
    pub fn unwrap(&self) -> PaintStyle {
        self.0
    }
}
impl<'lua> WrapperT<'lua> for LuaPaintStyle {
    type Wrapped = PaintStyle;

    #[inline]
    fn unwrap(self) -> PaintStyle {
        self.0
    }
}
impl std::ops::Deref for LuaPaintStyle {
    type Target = PaintStyle;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for LuaPaintStyle {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl AsRef<PaintStyle> for LuaPaintStyle {
    #[inline]
    fn as_ref(&self) -> &PaintStyle {
        &self.0
    }
}
impl<'lua> FromStr for LuaPaintStyle {
    type Err = LuaError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = match NAME_TO_PAINT_STYLE.get(value.to_ascii_lowercase().as_str()) {
            Some(it) => *it,
            None => {
                return Err(LuaError::FromLuaConversionError {
                    from: "string",
                    to: stringify!(PaintStyle),
                    message: Some(format!(
                        concat![
                            "unknown ",
                            stringify!(PaintStyle),
                            " name: '{}'; expected one of: {}"
                        ],
                        value,
                        Self::expected_values()
                    )),
                })
            }
        };
        Ok(LuaPaintStyle(value))
    }
}
impl<'lua> TryFrom<String> for LuaPaintStyle {
    type Error = LuaError;
    #[inline(always)]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_str(value.as_str())
    }
}
impl<'lua> TryFrom<LuaString<'lua>> for LuaPaintStyle {
    type Error = LuaError;
    #[inline(always)]
    fn try_from(value: LuaString<'lua>) -> Result<Self, Self::Error> {
        Self::from_str(value.to_str()?)
    }
}
impl<'lua> FromLua<'lua> for LuaPaintStyle {
    fn from_lua(text: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let text = match text {
            LuaValue::String(it) => it,
            LuaValue::Table(list) => {
                let count = list.clone().sequence_values::<String>().count();
                let values: Vec<_> = list
                    .sequence_values::<String>()
                    .filter_map(Result::ok)
                    .collect();

                if values.len() != count {
                    return Err(LuaError::FromLuaConversionError {
                        from: "table",
                        to: "PaintStyle",
                        message: Some(
                            "PaintStyle table array must contain only string values".to_string(),
                        ),
                    });
                }

                let fill = values.iter().any(|it| it == "fill");
                let stroke = values.iter().any(|it| it == "stroke");

                return match (fill, stroke) {
                    (true, false) => Ok(LuaPaintStyle(PaintStyle::Fill)),
                    (false, true) => Ok(LuaPaintStyle(PaintStyle::Fill)),
                    (true, true) => Ok(LuaPaintStyle(PaintStyle::Fill)),
                    (false, false) => return Err(LuaError::FromLuaConversionError {
                        from: "table",
                        to: "PaintStyle",
                        message: Some("expected PaintStyle table array to contain one of (or both): 'fill', 'stroke'".to_string()),
                    }),
                };
            }
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "PaintStyle",
                    message: Some(format!(
                        "expected a PaintStyle string value or table array; one of: {}",
                        Self::expected_values()
                    )),
                })
            }
        };
        Self::try_from(text)
    }
}

impl<'lua> ToLua<'lua> for LuaPaintStyle {
    #[allow(unreachable_patterns)]
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        lua.create_string(match self.0 {
            PaintStyle::Fill => "fill",
            PaintStyle::Stroke => "stroke",
            PaintStyle::StrokeAndFill => "stroke_and_fill",
            _ => return Ok(LuaNil),
        })
        .map(LuaValue::String)
    }
}

enum NeverT {}
impl<'lua> FromLua<'lua> for NeverT {
    fn from_lua(lua_value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        Err(LuaError::FromLuaConversionError {
            from: lua_value.type_name(),
            to: "!",
            message: Some("no type can be converted into ! type".to_string()),
        })
    }
}

macro_rules! impl_one_of {
    ($($ts: ident)*) => {
        #[non_exhaustive]
        #[allow(private_interfaces)]
        pub enum OneOf<$($ts = NeverT),*> {
            $($ts($ts)),*
        }

        impl<$($ts),*> OneOf<$($ts),*> {
            fn expected_types() -> &'static str {
                static STORE: OnceLock<&'static str> = OnceLock::new();

                STORE.get_or_init(|| {
                    let mut result = String::new();
                    for entry in [$(type_name::<$ts>()),*] {
                        if entry == type_name::<NeverT>() {
                            continue
                        }

                        if !result.is_empty() {
                            result.push(',');
                        }

                        result.extend(entry.chars());
                    }
                    result.leak()
                })
            }

            fn target_t() -> &'static str {
                static STORE: OnceLock<&'static str> = OnceLock::new();

                STORE.get_or_init(|| {
                    format!("OneOf<{}>", Self::expected_types()).leak()
                })
            }

            fn type_name(&self) -> &'static str {
                match self {
                    $(
                        Self::$ts(_) => type_name::<$ts>(),
                    )*
                }
            }

            paste::paste!{$(
                pub fn [<is_ $ts:lower>](&self) -> bool {
                    match self {
                        Self::$ts(_) => true,
                        _ => false,
                    }
                }
                pub fn [<as_ $ts:lower>](&self) -> Option<&$ts> {
                    match self {
                        Self::$ts(it) => Some(it),
                        _ => None,
                    }
                }
                pub fn [<unwrap_ $ts:lower>](self) -> $ts {
                    match self {
                        Self::$ts(it) => it,
                        other => panic!(concat!["expected ", stringify!($ts), "; found {:?} variant instead"], other.type_name()),
                    }
                }
            )*}
        }

        impl<'lua, $($ts),*> FromLua<'lua> for OneOf<$($ts),*> where $($ts: FromLua<'lua>),* {
            fn from_lua(value: LuaValue<'lua>, lua: LuaContext<'lua>) -> LuaResult<Self> {
                $(
                    if type_name::<$ts>() != type_name::<NeverT>() { // optimize NeverT checks away
                        if let Ok(it) = $ts::from_lua(value.clone(), lua) {
                            return Ok(Self::$ts(it));
                        }
                    }
                )*

                Err(LuaError::FromLuaConversionError {
                    from: value.type_name(),
                    to: Self::target_t(),
                    message: Some(format!("unable to convert into any of expected types: {}", Self::expected_types()))
                })
            }
        }
    };
}

impl_one_of!(A B C D E F G H I J K L M N O P);

#[derive(Clone, Copy, PartialEq)]
pub struct LuaColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for LuaColor {
    fn default() -> Self {
        LuaColor {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }
}

impl<'lua> FromLua<'lua> for LuaColor {
    fn from_lua(value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let color = match value {
            LuaValue::Table(it) => it,
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Color",
                    message: Some("expected a Color table".to_string()),
                })
            }
        };

        let is_rgb =
            color.contains_key("r")? || color.contains_key("g")? || color.contains_key("b")?;

        if is_rgb {
            let r = color.get("r").unwrap_or_default();
            let g = color.get("g").unwrap_or_default();
            let b = color.get("b").unwrap_or_default();
            let a = color.get("a").unwrap_or(1.0);

            return Ok(LuaColor { r, g, b, a });
        }

        let is_hsl =
            color.contains_key("h")? || color.contains_key("s")? || color.contains_key("l")?;

        if is_hsl {
            let h = color.get("h").unwrap_or_default();
            let s = color.get("s").unwrap_or_default();
            let l = color.get("l").unwrap_or_default();
            let a = color.get("a").unwrap_or(1.0);

            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Ok(LuaColor { r, g, b, a });
        }

        fn unknown_format() -> LuaError {
            LuaError::FromLuaConversionError {
                from: "table",
                to: "Color",
                message: Some("unknown Color format".to_string()),
            }
        }

        let len = color.clone().pairs::<LuaValue, LuaValue>().count();
        {
            let indexed_floats = color
                .clone()
                .pairs::<usize, f32>()
                .filter_map(|it| it.ok())
                .count();
            if indexed_floats != len {
                return Err(unknown_format());
            }
        };

        match len {
            0 => Ok(LuaColor::default()),
            3 | 4 => {
                let r = color.get(1 as LuaInteger).map_err(|_| unknown_format())?;
                let g = color.get(2 as LuaInteger).map_err(|_| unknown_format())?;
                let b = color.get(3 as LuaInteger).map_err(|_| unknown_format())?;
                let a = color.get(4 as LuaInteger).unwrap_or(1.);
                Ok(LuaColor { r, g, b, a })
            }
            _ => Err(unknown_format()),
        }
    }
}

impl<'lua> ToLua<'lua> for LuaColor {
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;
        result.set("r", self.r)?;
        result.set("g", self.g)?;
        result.set("b", self.b)?;
        result.set("a", self.a)?;
        result.to_lua(lua)
    }
}

impl From<Color4f> for LuaColor {
    #[inline]
    fn from(value: Color4f) -> Self {
        LuaColor {
            r: value.r,
            g: value.g,
            b: value.b,
            a: value.a,
        }
    }
}

impl Into<Color4f> for LuaColor {
    #[inline]
    fn into(self) -> Color4f {
        Color4f::new(self.r, self.g, self.b, self.a)
    }
}

impl From<Color> for LuaColor {
    #[inline]
    fn from(value: Color) -> Self {
        let rgb = value.to_rgb();
        LuaColor {
            r: rgb.r as f32 / u8::MAX as f32,
            g: rgb.g as f32 / u8::MAX as f32,
            b: rgb.b as f32 / u8::MAX as f32,
            a: value.a() as f32 / u8::MAX as f32,
        }
    }
}

impl Into<Color> for LuaColor {
    #[inline]
    fn into(self) -> Color {
        Into::<Color4f>::into(self).to_color()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct LuaRect {
    pub from: LuaPoint,
    pub to: LuaPoint,
}

impl<'lua> FromLua<'lua> for LuaRect {
    fn from_lua(value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let rect = match value {
            LuaValue::Table(it) => it,
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Rect",
                    message: Some("expected a Rect table".to_string()),
                })
            }
        };

        #[inline(always)]
        fn required_field<'lua, T: FromLua<'lua>>(
            rect: &LuaTable<'lua>,
            field: &'static str,
        ) -> LuaResult<T> {
            rect.get(field)
                .map_err(|_| LuaError::FromLuaConversionError {
                    from: "table",
                    to: "Rect",
                    message: Some(format!("Rect table missing '{}' field", field)),
                })
        }

        let skia_format = rect.contains_key("right")? || rect.contains_key("bottom")?;

        if skia_format {
            let left = rect.get("left").unwrap_or_default();
            let top = rect.get("top").unwrap_or_default();
            let right = required_field(&rect, "right")?;
            let bottom = required_field(&rect, "bottom")?;

            return Ok(LuaRect {
                from: LuaPoint { value: [left, top] },
                to: LuaPoint {
                    value: [right, bottom],
                },
            });
        }

        let xywh_format = (rect.contains_key("width")? || rect.contains_key("w")?)
            || (rect.contains_key("height")? || rect.contains_key("h")?);

        if xywh_format {
            let x = rect.get("x").unwrap_or_default();
            let y = rect.get("y").unwrap_or_default();
            let width: f32 = required_field(&rect, "w").or(required_field(&rect, "width"))?;
            let height: f32 = required_field(&rect, "h").or(required_field(&rect, "height"))?;

            return Ok(LuaRect {
                from: LuaPoint { value: [x, y] },
                to: LuaPoint {
                    value: [x + width, y + height],
                },
            });
        }

        let from_to_format = rect.contains_key("from")? && rect.contains_key("to")?;

        if from_to_format {
            let from: LuaTable = required_field(&rect, "from")?;
            let from = LuaPoint::try_from(from).map_err(|inner| LuaError::CallbackError {
                traceback: "while converting 'from' Point table of Rect".to_string(),
                cause: Arc::new(inner),
            })?;
            let to: LuaTable = required_field(&rect, "to")?;
            let to = LuaPoint::try_from(to).map_err(|inner| LuaError::CallbackError {
                traceback: "while converting 'to' Point table of Rect".to_string(),
                cause: Arc::new(inner),
            })?;

            return Ok(LuaRect { from, to });
        }

        Err(LuaError::FromLuaConversionError {
            from: "table",
            to: "Rect",
            message: Some("unknown Rect format; expected one of:\n- { left, top, right, bottom }\n- { x, y, width, height }\n- { from, to }".to_string()),
        })
    }
}

impl<'lua> ToLua<'lua> for LuaRect {
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;
        result.set("top", self.from.x())?;
        result.set("left", self.from.y())?;
        result.set("right", self.to.x())?;
        result.set("bottom", self.to.y())?;
        result.to_lua(lua)
    }
}

impl From<Rect> for LuaRect {
    fn from(value: Rect) -> Self {
        LuaRect {
            from: LuaPoint {
                value: [value.left, value.top],
            },
            to: LuaPoint {
                value: [value.right, value.bottom],
            },
        }
    }
}
impl Into<Rect> for LuaRect {
    fn into(self) -> Rect {
        Rect::new(self.from.x(), self.from.y(), self.to.x(), self.to.y())
    }
}
impl From<IRect> for LuaRect {
    fn from(value: IRect) -> Self {
        LuaRect {
            from: LuaPoint {
                value: [value.left as f32, value.top as f32],
            },
            to: LuaPoint {
                value: [value.right as f32, value.bottom as f32],
            },
        }
    }
}
impl Into<IRect> for LuaRect {
    fn into(self) -> IRect {
        IRect::new(
            self.from.x() as i32,
            self.from.y() as i32,
            self.to.x() as i32,
            self.to.y() as i32,
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct LuaSize<const N: usize = 2> {
    value: [f32; N],
}

const DIM_NAME: &[&'static str] = &["width", "height", "depth"];
const DIM_NAME_SHORT: &[&'static str] = &["w", "h", "d"];

impl<const N: usize> LuaSize<N> {
    #[inline(always)]
    pub fn width(&self) -> f32 {
        self.value[0]
    }
    #[inline(always)]
    pub fn height(&self) -> f32 {
        self.value[1]
    }
    #[inline(always)]
    pub fn depth(&self) -> f32 {
        self.value[2]
    }
}

impl From<ISize> for LuaSize {
    fn from(value: ISize) -> Self {
        LuaSize {
            value: [value.width as f32, value.height as f32],
        }
    }
}
impl Into<ISize> for LuaSize {
    fn into(self) -> ISize {
        ISize {
            width: self.width() as i32,
            height: self.height() as i32,
        }
    }
}
impl<'lua, const N: usize> FromLuaMulti<'lua> for LuaSize<N> {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        if values.is_empty() {
            return Err(LuaError::FromLuaConversionError {
                from: "...",
                to: "Size",
                message: Some(format!(
                    "Size value expects either an array with {0} values or {0} number values",
                    N
                )),
            });
        }
        let mut values = values.into_iter();

        let first = match values.next() {
            Some(it) => it,
            None => {
                return Err(LuaError::FromLuaConversionError {
                    from: "nil",
                    to: "Size",
                    message: Some(format!(
                        "Size value expects either an array with {0} values or {0} number values",
                        N
                    )),
                })
            }
        };

        #[inline(always)]
        fn missing_argument<const N: usize>() -> LuaError {
            LuaError::FromLuaConversionError {
                from: "...",
                to: "Size",
                message: Some(format!(
                    "Size requires {} ({}) arguments",
                    N,
                    COORD_NAME[0..N].join(", ")
                )),
            }
        }

        #[inline(always)]
        fn invalid_argument_type(from: &'static str) -> LuaError {
            LuaError::FromLuaConversionError {
                from,
                to: "f32",
                message: Some("Size arguments must be numbers".to_string()),
            }
        }

        #[inline]
        fn read_coord<const N: usize>(it: Option<LuaValue>) -> Result<f32, LuaError> {
            let it = it.ok_or_else(missing_argument::<N>)?;
            match it {
                LuaValue::Integer(it) => Ok(it as f32),
                LuaValue::Number(it) => Ok(it as f32),
                other => return Err(invalid_argument_type(other.type_name())),
            }
        }

        match first {
            LuaValue::Table(table) => {
                let result = Self::try_from(table)?;
                *consumed += 1;
                Ok(result)
            }
            LuaValue::Number(x) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaSize { value })
            }
            LuaValue::Integer(x) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaSize { value })
            }
            other => {
                log::debug!("{:?}", other);
                Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Size",
                    message: Some(format!(
                        "Size value expects either an array with {0} values or {0} number values",
                        N
                    )),
                })
            }
        }
    }
}

impl<'lua, const N: usize> ToLua<'lua> for LuaSize<N> {
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;

        for (i, coord) in COORD_NAME[0..N].iter().enumerate() {
            result.set(*coord, self.value[i])?;
        }

        Ok(LuaValue::Table(result))
    }
}

impl<'lua, const N: usize> TryFrom<LuaTable<'lua>> for LuaSize<N> {
    type Error = LuaError;

    fn try_from(table: LuaTable<'lua>) -> Result<Self, Self::Error> {
        #[inline(always)]
        fn bad_table_entries<const N: usize>(_: LuaError) -> LuaError {
            LuaError::FromLuaConversionError {
                from: "table",
                to: "Size",
                message: Some(format!(
                    "Size table requires {{'{}'}} number entries, optionally named",
                    DIM_NAME[0..N].join("', '")
                )),
            }
        }

        if DIM_NAME[0..N]
            .iter()
            .all(|it| table.contains_key(*it).ok() == Some(true))
        {
            let mut value = [0.0; N];
            for (i, coord) in DIM_NAME[0..N].iter().enumerate() {
                value[i] = table.get(*coord).map_err(bad_table_entries::<N>)?;
            }
            Ok(LuaSize { value })
        } else if DIM_NAME_SHORT[0..N]
            .iter()
            .all(|it| table.contains_key(*it).ok() == Some(true))
        {
            let mut value = [0.0; N];
            for (i, coord) in DIM_NAME_SHORT[0..N].iter().enumerate() {
                value[i] = table.get(*coord).map_err(bad_table_entries::<N>)?;
            }
            Ok(LuaSize { value })
        } else {
            let len = table
                .clone()
                .sequence_values::<f32>()
                .filter(|it| it.is_ok())
                .count();
            if len != N {
                return Err(LuaError::FromLuaConversionError {
                    from: "table",
                    to: "Size",
                    message: Some(format!("Size value array expects {} values", N)),
                });
            }

            let mut value = [0.0; N];
            for (value, entry) in value.iter_mut().zip(table.sequence_values::<f32>()) {
                *value = entry.map_err(bad_table_entries::<N>)?;
            }
            Ok(LuaSize { value })
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct LuaPoint<const N: usize = 2> {
    value: [f32; N],
}

const COORD_NAME: &[&'static str] = &["x", "y", "z", "w"];

impl<const N: usize> LuaPoint<N> {
    #[inline(always)]
    pub fn x(&self) -> f32 {
        self.value[0]
    }
    #[inline(always)]
    pub fn y(&self) -> f32 {
        self.value[1]
    }
    #[inline(always)]
    pub fn z(&self) -> f32 {
        self.value[2]
    }
    #[inline(always)]
    pub fn w(&self) -> f32 {
        self.value[3]
    }
}

impl From<Point> for LuaPoint {
    #[inline]
    fn from(value: Point) -> Self {
        LuaPoint {
            value: [value.x, value.y],
        }
    }
}
impl Into<Point> for LuaPoint {
    fn into(self) -> Point {
        Point {
            x: self.x(),
            y: self.y(),
        }
    }
}
impl Into<IVector> for LuaPoint {
    fn into(self) -> IVector {
        IVector {
            x: self.x() as i32,
            y: self.y() as i32,
        }
    }
}
impl From<Point3> for LuaPoint<3> {
    #[inline]
    fn from(value: Point3) -> Self {
        LuaPoint {
            value: [value.x, value.y, value.z],
        }
    }
}
impl Into<Point3> for LuaPoint<3> {
    fn into(self) -> Point3 {
        Point3 {
            x: self.x(),
            y: self.y(),
            z: self.z(),
        }
    }
}

impl<'lua, const N: usize> FromLuaMulti<'lua> for LuaPoint<N> {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        if values.is_empty() {
            return Err(LuaError::FromLuaConversionError {
                from: "...",
                to: "Point",
                message: Some(format!(
                    "Point value expects either an array with {0} values or {0} number values",
                    N
                )),
            });
        }
        let mut values = values.into_iter();

        let first = match values.next() {
            Some(it) => it,
            None => {
                return Err(LuaError::FromLuaConversionError {
                    from: "nil",
                    to: "Point",
                    message: Some(format!(
                        "Point value expects either an array with {0} values or {0} number values",
                        N
                    )),
                })
            }
        };

        #[inline(always)]
        fn missing_argument<const N: usize>() -> LuaError {
            LuaError::FromLuaConversionError {
                from: "...",
                to: "Point",
                message: Some(format!(
                    "Point requires {} ({}) arguments",
                    N,
                    COORD_NAME[0..N].join(", ")
                )),
            }
        }

        #[inline(always)]
        fn invalid_argument_type(from: &'static str) -> LuaError {
            LuaError::FromLuaConversionError {
                from,
                to: "f32",
                message: Some("Point arguments must be numbers".to_string()),
            }
        }

        #[inline]
        fn read_coord<const N: usize>(it: Option<LuaValue>) -> Result<f32, LuaError> {
            let it = it.ok_or_else(missing_argument::<N>)?;
            match it {
                LuaValue::Integer(it) => Ok(it as f32),
                LuaValue::Number(it) => Ok(it as f32),
                other => return Err(invalid_argument_type(other.type_name())),
            }
        }

        match first {
            LuaValue::Table(table) => {
                let result = Self::try_from(table)?;
                *consumed += 1;
                Ok(result)
            }
            LuaValue::Number(x) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaPoint { value })
            }
            LuaValue::Integer(x) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaPoint { value })
            }
            other => {
                log::debug!("{:?}", other);
                Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Point",
                    message: Some(format!(
                        "Point value expects either an array with {0} values or {0} number values",
                        N
                    )),
                })
            }
        }
    }
}

impl<'lua, const N: usize> TryFrom<LuaTable<'lua>> for LuaPoint<N> {
    type Error = LuaError;

    fn try_from(table: LuaTable<'lua>) -> Result<Self, Self::Error> {
        #[inline(always)]
        fn bad_table_entries<const N: usize>(_: LuaError) -> LuaError {
            LuaError::FromLuaConversionError {
                from: "table",
                to: "Point",
                message: Some(format!(
                    "Point table requires {{'{}'}} number entries, optionally named",
                    COORD_NAME[0..N].join("', '")
                )),
            }
        }

        if COORD_NAME[0..N]
            .iter()
            .all(|it| table.contains_key(*it).ok() == Some(true))
        {
            let mut value = [0.0; N];
            for (i, coord) in COORD_NAME[0..N].iter().enumerate() {
                value[i] = table.get(*coord).map_err(bad_table_entries::<N>)?;
            }
            Ok(LuaPoint { value })
        } else {
            let len = table
                .clone()
                .sequence_values::<f32>()
                .filter(|it| it.is_ok())
                .count();
            if len != N {
                return Err(LuaError::FromLuaConversionError {
                    from: "table",
                    to: "Point",
                    message: Some(format!("Point value array expects {} values", N)),
                });
            }

            let mut value = [0.0; N];
            for (value, entry) in value.iter_mut().zip(table.sequence_values::<f32>()) {
                *value = entry.map_err(bad_table_entries::<N>)?;
            }
            Ok(LuaPoint { value })
        }
    }
}

impl<'lua, const N: usize> ToLua<'lua> for LuaPoint<N> {
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;

        for (i, coord) in COORD_NAME[0..N].iter().enumerate() {
            result.set(*coord, self.value[i])?;
        }

        result.to_lua(lua)
    }
}

#[derive(Clone)]
pub struct LuaLine<const N: usize = 2> {
    pub from: LuaPoint<N>,
    pub to: LuaPoint<N>,
}

impl<'lua, const N: usize> ToLua<'lua> for LuaLine<N> {
    fn to_lua(self, lua: LuaContext<'lua>) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;

        result.set("from", self.from.to_lua(lua)?)?;
        result.set("to", self.to.to_lua(lua)?)?;

        result.to_lua(lua)
    }
}

impl From<(Point, Point)> for LuaLine {
    fn from((from, to): (Point, Point)) -> Self {
        LuaLine {
            from: LuaPoint::from(from),
            to: LuaPoint::from(to),
        }
    }
}

pub struct SidePack {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl<'lua> FromLuaMulti<'lua> for SidePack {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        let mut values = values.into_iter();

        #[inline(always)]
        fn bad_argument_count() -> LuaError {
            LuaError::FromLuaConversionError {
                from: "...",
                to: "Side",
                message: Some("Side requires 2 (vertical, horizontal), 4 (left, top, right, bottom) arguments, or a table with those values".to_string()),
            }
        }

        let first = values.next().ok_or_else(|| LuaError::CallbackError {
            traceback: "expected a Side argument pack or table".to_string(),
            cause: Arc::new(LuaError::FromLuaConversionError {
                from: "nil",
                to: "Side",
                message: Some("Side parameters missing".to_string()),
            }),
        })?;

        match first {
            LuaValue::Table(table) => {
                *consumed += 1;
                Self::try_from(table)
            }
            LuaValue::Integer(_) | LuaValue::Number(_) => {
                let mut numbers = Vec::with_capacity(4);
                numbers.push(match first {
                    LuaValue::Integer(it) => it as f32,
                    LuaValue::Number(it) => it as f32,
                    _ => unreachable!(),
                });
                numbers.extend(
                    values
                        .take(3)
                        .map(|it| match it {
                            LuaValue::Integer(it) => Some(it as f32),
                            LuaValue::Number(it) => Some(it as f32),
                            _ => None,
                        })
                        .take_while(Option::is_some)
                        .filter_map(|it| it),
                );

                match numbers.len() {
                    1 => unsafe {
                        // SAFETY: numbers length checked by outer match
                        let all = *numbers.get(0).unwrap_unchecked();
                        *consumed += 1;
                        Ok(SidePack {
                            left: all,
                            top: all,
                            right: all,
                            bottom: all,
                        })
                    },
                    2 | 3 => unsafe {
                        // SAFETY: numbers length checked by outer match
                        let vertical = *numbers.get(0).unwrap_unchecked();
                        let horizontal = *numbers.get(1).unwrap_unchecked();
                        *consumed += 2;
                        Ok(SidePack {
                            left: horizontal,
                            top: vertical,
                            right: horizontal,
                            bottom: vertical,
                        })
                    },
                    _ => unsafe {
                        // SAFETY: numbers length checked by outer match
                        let left = *numbers.get(0).unwrap_unchecked();
                        let top = *numbers.get(1).unwrap_unchecked();
                        let right = *numbers.get(2).unwrap_unchecked();
                        let bottom = *numbers.get(3).unwrap_unchecked();
                        *consumed += 4;
                        Ok(SidePack {
                            left,
                            top,
                            right,
                            bottom,
                        })
                    },
                }
            }
            _ => Err(bad_argument_count()),
        }
    }
}

impl<'lua> TryFrom<LuaTable<'lua>> for SidePack {
    type Error = LuaError;

    fn try_from(table: LuaTable<'lua>) -> Result<Self, Self::Error> {
        {
            let left: Option<f32> = table.get("left").or_else(|_| table.get("l")).ok();
            let top: Option<f32> = table.get("top").or_else(|_| table.get("t")).ok();
            let right: Option<f32> = table.get("right").or_else(|_| table.get("r")).ok();
            let bottom: Option<f32> = table.get("bottom").or_else(|_| table.get("b")).ok();

            let is_explicit =
                left.is_some() || top.is_some() || right.is_some() || bottom.is_some();
            if is_explicit {
                return Ok(SidePack {
                    left: left.unwrap_or_default(),
                    top: top.unwrap_or_default(),
                    right: right.unwrap_or_default(),
                    bottom: bottom.unwrap_or_default(),
                });
            }
        }

        {
            let vertical: Option<f32> = table.get("vertical").or_else(|_| table.get("v")).ok();
            let horizontal: Option<f32> = table.get("horizontal").or_else(|_| table.get("h")).ok();
            let is_symmetrical = vertical.is_some() || horizontal.is_some();
            if is_symmetrical {
                return Ok(SidePack {
                    left: horizontal.unwrap_or_default(),
                    top: vertical.unwrap_or_default(),
                    right: horizontal.unwrap_or_default(),
                    bottom: vertical.unwrap_or_default(),
                });
            }
        }

        {
            let all: Option<f32> = table.get("all").or_else(|_| table.get("a")).ok();
            if let Some(all) = all {
                return Ok(SidePack {
                    left: all,
                    top: all,
                    right: all,
                    bottom: all,
                });
            }
        }

        let mut values: VecDeque<Result<_, _>> = table.sequence_values::<f32>().collect();

        match values.len() {
            1 => unsafe {
                // SAFETY: Length of values is checked by outer match
                let all = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'all' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;

                Ok(SidePack {
                    left: all,
                    top: all,
                    right: all,
                    bottom: all,
                })
            },
            2 => unsafe {
                // SAFETY: Length of values is checked by outer match
                let v = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'vertical' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;
                let h = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'horizontal' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;

                Ok(SidePack {
                    left: h,
                    top: v,
                    right: h,
                    bottom: v,
                })
            },
            4 => unsafe {
                // SAFETY: Length of values is checked by outer match
                let left = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'left' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;
                let top = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'top' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;
                let right = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'right' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;
                let bottom = values.pop_front().unwrap_unchecked().map_err(|inner| {
                    LuaError::CallbackError {
                        traceback: "reading Side 'bottom' length".to_string(),
                        cause: Arc::new(inner),
                    }
                })?;

                Ok(SidePack {
                    left,
                    top,
                    right,
                    bottom,
                })
            },
            other_len => Err(LuaError::FromLuaConversionError {
                from: "table",
                to: "Side",
                message: Some(format!(
                    "invalid Side table array value count, expected exactly 1, 2 or 4; got: {}",
                    other_len
                )),
            }),
        }
    }
}

macro_rules! wrap_skia_handle {
    ($handle: ty) => {
        paste::paste! {
            #[derive(Clone)]
            pub struct [<Lua $handle>](pub $handle);

            impl std::ops::Deref for [<Lua $handle>] {
                type Target = $handle;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }
            impl std::ops::DerefMut for [<Lua $handle>] {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }
            impl Into<$handle> for [<Lua $handle>] {
                fn into(self) -> $handle {
                    self.0
                }
            }
            impl AsRef<$handle> for [<Lua $handle>] {
                fn as_ref(&self) -> &$handle {
                    &self.0
                }
            }
            impl<'lua> WrapperT<'lua> for [<Lua $handle>] {
                type Wrapped = $handle;

                #[inline]
                fn unwrap(self) -> $handle {
                    self.0
                }
            }
        }
    };
}

macro_rules! type_like {
    ($handle: ty) => {
        paste::paste! {
            #[derive(Clone)]
            pub struct [<Like $handle>]([<Lua $handle>]);

            impl [<Like $handle>] {
                pub fn unwrap(self) -> $handle {
                    self.0.unwrap()
                }
            }

            impl Into<[<Lua $handle>]> for [<Like $handle>] {
                fn into(self) -> [<Lua $handle>] {
                    self.0
                }
            }
            impl std::ops::Deref for [<Like $handle>] {
                type Target = $handle;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }
            impl std::ops::DerefMut for [<Like $handle>] {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }
            impl AsRef<$handle> for [<Like $handle>] {
                fn as_ref(&self) -> &$handle {
                    &self.0
                }
            }
            impl<'lua> WrapperT<'lua> for [<Like $handle>] {
                type Wrapped = $handle;

                #[inline]
                fn unwrap(self) -> $handle {
                    self.0.0
                }
            }
        }
    };
}

macro_rules! type_like_table {
    ($handle: ty: |$ident: ident: LuaTable, $ctx: ident: LuaContext| $body: block) => {
        type_like!($handle);
        paste::paste! {
            impl<'lua> TryFrom<(LuaTable<'lua>, LuaContext<'lua>)> for [<Lua $handle>] {
                type Error = LuaError;

                fn try_from(($ident, $ctx): (LuaTable<'lua>, LuaContext<'lua>)) -> Result<Self, Self::Error> $body
            }
            impl<'lua> FromLua<'lua> for [<Like $handle>] {
                fn from_lua(lua_value: LuaValue<'lua>, ctx: LuaContext<'lua>) -> LuaResult<Self> {
                    let table = match lua_value {
                        LuaValue::UserData(ud) if ud.is::<[<Lua $handle>]>() => {
                            return Ok([<Like $handle>](ud.borrow::<[<Lua $handle>]>()?.to_owned()));
                        }
                        LuaValue::Table(it) => it,
                        other => {
                            return Err(LuaError::FromLuaConversionError {
                                from: other.type_name(),
                                to: stringify!($handle),
                                message: Some(concat!["expected ", stringify!($handle), " or constructor Table"].to_string()),
                            });
                        }
                    };
                    [<Lua $handle>]::try_from((table, ctx)).map([<Like $handle>])
                }
            }
        }
    };
    ($handle: ty: |$ident: ident: LuaTable| $body: block) => {
        type_like_table!($handle: |$ident: LuaTable, _unused_lua_ctx: LuaContext| $body);
    }
}

macro_rules! decl_constructors {
    ($handle: ident: {$(
        fn $name: ident ($($argn: tt: $argt: ty),*) -> _ $imp: block
    )*}) => {
        paste::paste! {
            pub struct [<$handle Constructors>];

            impl UserData for [<$handle Constructors>] {
                #[allow(unused_parens)]
                fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
                    $(
                        methods.add_method(
                            stringify!([<$name:camel>]), |_, _, ($($argn),*): ($($argt),*)| $imp,
                        );
                    )*
                }
            }
        }
    };
}

macro_rules! match_value_iter {
    ($matched: ident as $expected: literal: $(
        $arm: pat => $value: expr $(,)?
    )+) => {
        match $matched.next() {
            $(
                Some($arm) => $value,
            )+
            Some(other) => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: $expected,
                    message: None,
                });
            }
            None => {
                return Err(LuaError::FromLuaConversionError {
                    from: "nil",
                    to: $expected,
                    message: None,
                });
            }
        }
    };
}

pub trait StructToTable<'lua> {
    fn to_table(&self, ctx: LuaContext<'lua>) -> LuaResult<LuaTable<'lua>>;
}

macro_rules! struct_to_table {
    ($ty: ident : {$($name: literal: |$this: ident, $ctx: tt| $access: expr),+ $(,)?}) => {
        impl<'lua> StructToTable<'lua> for $ty {paste::paste!{
            fn to_table(&self, ctx: LuaContext<'lua>) -> LuaResult<LuaTable<'lua>> {
                let result = ctx.create_table()?;
                $(
                    result.set($name, (|$this: &$ty, $ctx: &LuaContext| $access)(self, &ctx))?;
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

impl UserData for LuaShader {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isOpaque", |_, this, ()| Ok(this.is_opaque()));
        methods.add_method("isAImage", |_, this, ()| Ok(this.is_a_image()));
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
    fn from_lua(value: LuaValue<'lua>, lua: LuaContext<'lua>) -> LuaResult<Self> {
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

pub struct ColorStops {
    positions: Vec<f32>,
    colors: Vec<Color4f>,
}

impl<'lua> FromLuaMulti<'lua> for ColorStops {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        let mut values = values.into_iter();
        let first = match_value_iter!(values as "ColorStops":
            LuaValue::Table(it) => it,
        );

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
                return Err(LuaError::FromLuaConversionError {
                    from: "table",
                    to: "ColorStops",
                    message: Some("ColorStops expects a table with only Color values".to_string()),
                });
            }

            let (positions, colors) = stops.into_iter().unzip();
            *consumed += 1;
            return Ok(ColorStops { positions, colors });
        }

        let colors: Vec<Color4f> = first
            .sequence_values::<LuaColor>()
            .filter_map(|it| match it {
                Ok(it) => Some(it.into()),
                Err(_) => None,
            })
            .collect();
        *consumed += 1;

        let second = match values.next() {
            Some(LuaValue::Table(it)) => Some(it),
            Some(LuaValue::Nil) => {
                *consumed += 1;
                None
            }
            _ => None, // nil value means evenly spaced stops
        };

        let positions = if let Some(second) = second {
            let count = second.clone().sequence_values::<f32>().count();
            let positions: Vec<f32> = second
                .sequence_values::<f32>()
                .filter_map(Result::ok)
                .collect();

            if positions.len() < count {
                None
            } else {
                *consumed += 1;
                Some(positions)
            }
        } else {
            None
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

decl_constructors!(GradientShader: {
    fn make_linear(
        from: LuaPoint, to: LuaPoint, stops: ColorStops,
        color_space: Option<LuaColorSpace>, tile_mode: Option<LuaTileMode>,
        interpolation: Option<LuaInterpolation>, local: Option<LuaMatrix>
    ) -> _ {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::linear_gradient_with_interpolation(
            (from, to),
            (stops.colors.as_slice(), color_space.map(LuaColorSpace::unwrap)),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        ).map(LuaShader))
    }
    fn make_radial(
        center: LuaPoint, radius: f32, stops: ColorStops,
        color_space: Option<LuaColorSpace>, tile_mode: Option<LuaTileMode>,
        interpolation: Option<LuaInterpolation>, local: Option<LuaMatrix>
    ) -> _ {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::radial_gradient_with_interpolation(
            (center, radius),
            (stops.colors.as_slice(), color_space.map(LuaColorSpace::unwrap)),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        ).map(LuaShader))
    }
    fn make_sweep(
        center: LuaPoint, stops: ColorStops,
        color_space: Option<LuaColorSpace>, tile_mode: Option<LuaTileMode>,
        angles: Option<(f32, f32)>,
        interpolation: Option<LuaInterpolation>, local: Option<LuaMatrix>
    ) -> _ {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::sweep_gradient_with_interpolation(
            center,
            (stops.colors.as_slice(), color_space.map(LuaColorSpace::unwrap)),
            Some(stops.positions.as_slice()),
            tile_mode,
            angles,
            interpolation,
            local.as_ref(),
        ).map(LuaShader))
    }
    fn make_two_point_conical(
        start: LuaPoint, start_radius: f32,
        end: LuaPoint, end_radius: f32,
        stops: ColorStops,
        color_space: Option<LuaColorSpace>, tile_mode: Option<LuaTileMode>,
        interpolation: Option<LuaInterpolation>, local: Option<LuaMatrix>
    ) -> _ {
        let tile_mode = tile_mode.unwrap_or_t(TileMode::Clamp);
        let interpolation = interpolation.unwrap_or_default().0;
        let local: Option<Matrix> = local.map(LuaMatrix::into);

        Ok(Shader::two_point_conical_gradient_with_interpolation(
            (start, start_radius),
            (end, end_radius),
            (stops.colors.as_slice(), color_space.map(LuaColorSpace::unwrap)),
            Some(stops.positions.as_slice()),
            tile_mode,
            interpolation,
            local.as_ref(),
        ).map(LuaShader))
    }
});

wrap_skia_handle!(Image);

impl UserData for LuaImage {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("width", |_, this, ()| Ok(this.width()));
        methods.add_method("height", |_, this, ()| Ok(this.height()));
        methods.add_method(
            "newShader",
            |_,
             this,
             (tile_x, tile_y, sampling, local_matrix): (
                Option<LuaTileMode>,
                Option<LuaTileMode>,
                Option<LuaSamplingOptions>,
                Option<LuaMatrix>,
            )| {
                let tile_modes = if tile_x.is_none() && tile_y.is_none() {
                    None
                } else {
                    let n_tile_x = tile_x.unwrap_or_t(TileMode::Clamp);
                    let n_tile_y = tile_x.unwrap_or_t(n_tile_x);
                    Some((n_tile_x, n_tile_y))
                };
                let local_matrix = local_matrix.map(LuaMatrix::into);

                Ok(this
                    .to_shader(
                        tile_modes,
                        sampling.unwrap_or_default(),
                        local_matrix.as_ref(),
                    )
                    .map(LuaShader))
            },
        );
    }
}

decl_constructors!(Image: {
    fn load(path: String) -> _ {
        let handle: Data = Data::new_copy(
            &std::fs::read(path)
                .map_err(|io_err| rlua::Error::RuntimeError(io_err.to_string()))?,
        );
        Image::from_encoded(handle)
            .map(LuaImage)
            .ok_or(LuaError::RuntimeError(
                "Unsupported encoded image format".to_string(),
            ))
    }
});

wrap_skia_handle!(ColorSpace);

impl Default for LuaColorSpace {
    fn default() -> Self {
        LuaColorSpace(ColorSpace::new_srgb())
    }
}

impl UserData for LuaColorSpace {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isSRGB", |_, this, ()| Ok(this.is_srgb()));
        methods.add_method("toXYZD50Hash", |_, this, ()| Ok(this.to_xyzd50_hash().0));
        methods.add_method("makeLinearGamma", |_, this, ()| {
            Ok(LuaColorSpace(this.with_linear_gamma()))
        });
        methods.add_method("makeSRGBGamma", |_, this, ()| {
            Ok(LuaColorSpace(this.with_srgb_gamma()))
        });
        methods.add_method("makeColorSpin", |_, this, ()| {
            Ok(LuaColorSpace(this.with_color_spin()))
        });
    }
}

decl_constructors!(ColorSpace: {
    fn make_SRGB() -> _ {
        Ok(LuaColorSpace(ColorSpace::new_srgb()))
    }
    fn make_SRGB_linear() -> _ {
        Ok(LuaColorSpace(ColorSpace::new_srgb_linear()))
    }
});

wrap_skia_handle!(Picture);
impl UserData for LuaPicture {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("playback", |_, this, canvas: LuaCanvas| {
            this.playback(&canvas);
            Ok(())
        });
        methods.add_method("cullRect", |_, this, ()| {
            Ok(LuaRect::from(this.cull_rect()))
        });
        methods.add_method("approximateOpCount", |_, this, nested: Option<bool>| {
            Ok(this.approximate_op_count_nested(nested.unwrap_or_default()))
        });
        methods.add_method("approximateBytesUsed", |_, this, ()| {
            Ok(this.approximate_bytes_used())
        });
        methods.add_method(
            "makeShader",
            |_,
             this,
             (tile_x, tile_y, mode, local_matrix, tile_rect): (
                Option<LuaTileMode>,
                Option<LuaTileMode>,
                Option<LuaFilterMode>,
                Option<LuaMatrix>,
                Option<LuaRect>,
            )| {
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

                Ok(LuaShader(this.to_shader(
                    tm,
                    mode,
                    local_matrix.as_ref(),
                    tile_rect.as_ref(),
                )))
            },
        );
    }
}

wrap_skia_handle!(ImageFilter);

impl UserData for LuaImageFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method(
            "filterBounds",
            |_,
             this,
             (src, ctm, map_direction, input_rect): (
                LuaRect,
                LuaMatrix,
                LuaMapDirection,
                Option<LuaRect>,
            )| {
                let src: IRect = src.into();
                let ctm: Matrix = ctm.into();
                let input_rect = input_rect.map(Into::<IRect>::into);
                let filtered = this.filter_bounds(src, &ctm, *map_direction, input_rect.as_ref());
                Ok(LuaRect::from(filtered))
            },
        );
        methods.add_method("isColorFilterNode", |_, this, ()| {
            Ok(this.color_filter_node().map(LuaColorFilter))
        });
        methods.add_method("asAColorFilter", |_, this, ()| {
            Ok(this.to_a_color_filter().map(LuaColorFilter))
        });
        methods.add_method("countInputs", |_, this, ()| Ok(this.count_inputs()));
        methods.add_method("getInput", |_, this, index: usize| {
            Ok(this.get_input(index).map(LuaImageFilter))
        });
        methods.add_method("computeFastBounds", |_, this, rect: LuaRect| {
            let rect: Rect = rect.into();
            let bounds = this.compute_fast_bounds(rect);
            Ok(LuaRect::from(bounds))
        });
        methods.add_method("canComputeFastBounds", |_, this, ()| {
            Ok(this.can_compute_fast_bounds())
        });
        methods.add_method("makeWithLocalMatrix", |_, this, matrix: LuaMatrix| {
            let matrix: Matrix = matrix.into();
            Ok(this.with_local_matrix(&matrix).map(LuaImageFilter))
        });
    }
}

decl_constructors!(ImageFilters: {
    fn arithmetic(
        k1: f32, k2: f32, k3: f32, k4: f32,
        enforce_pm_color: bool,
        background: Option<LuaImageFilter>,
        foreground: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let background = background.map(LuaImageFilter::unwrap);
        let foreground = foreground.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::arithmetic(
            k1, k2, k3, k4, enforce_pm_color, background, foreground, crop_rect
        ).map(LuaImageFilter))
    }

    fn blend(
        mode: LuaBlendMode,
        background: Option<LuaImageFilter>,
        foreground: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let background = background.map(LuaImageFilter::unwrap);
        let foreground = foreground.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::blend(
            *mode, background, foreground, crop_rect
        ).map(LuaImageFilter))
    }

    fn blur(sigma_x: f32, sigma_y: Option<f32>, tile_mode: Option<LuaTileMode>, input: Option<LuaImageFilter>, crop_rect: Option<LuaRect>) -> _ {
        if !sigma_x.is_finite() || sigma_x < 0f32 {
            return Err(LuaError::RuntimeError(
                "x sigma must be a positive, finite scalar".to_string(),
            ));
        }
        let sigma_y = match sigma_y {
            Some(sigma_y) if !sigma_y.is_finite() || sigma_y < 0f32 => {
                return Err(LuaError::RuntimeError(
                    "y sigma must be a positive, finite scalar".to_string(),
                ));
            }
            Some(it) => it,
            None => sigma_x
        };

        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::blur((sigma_x, sigma_y), tile_mode.map_t(), input, crop_rect)
            .map(LuaImageFilter))
    }

    fn color_filter(
        cf: LuaColorFilter,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::color_filter(cf.unwrap(), input, crop_rect)
            .map(LuaImageFilter))
    }

    fn compose(outer: LuaImageFilter, inner: LuaImageFilter) -> _ {
        Ok(image_filters::compose(outer.unwrap(), inner.unwrap())
            .map(LuaImageFilter))
    }

    fn crop(rect: LuaRect, tile_mode: Option<LuaTileMode>, input: Option<LuaImageFilter>) -> _ {
        let rect: Rect = rect.into();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::crop(&rect, tile_mode.map_t(), input)
            .map(LuaImageFilter))
    }

    fn dilate(
        radius_x: f32, radius_y: Option<f32>,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        if !radius_x.is_finite() || radius_x < 0f32 {
            return Err(LuaError::RuntimeError(
                "x radius must be a positive, finite scalar".to_string(),
            ));
        }
        let radius_y = match radius_y {
            Some(radius_y) if !radius_y.is_finite() || radius_y < 0f32 => {
                return Err(LuaError::RuntimeError(
                    "y radius must be a positive, finite scalar".to_string(),
                ));
            }
            Some(it) => it,
            None => radius_x
        };
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::dilate((radius_x, radius_y), input, crop_rect)
            .map(LuaImageFilter))
    }

    fn displacement_map(
        x_channel_selector: LuaColorChannel,
        y_channel_selector: LuaColorChannel,
        scale: f32,
        displacement: Option<LuaImageFilter>,
        color: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let displacement = displacement.map(LuaImageFilter::unwrap);
        let color = color.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::displacement_map(
            (x_channel_selector.unwrap(), y_channel_selector.unwrap()),
            scale, displacement, color, crop_rect
        ).map(LuaImageFilter))
    }
    fn distant_lit_diffuse(
        direction: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::distant_lit_diffuse(
            direction, light_color, surface_scale,
            kd, input, crop_rect
        ).map(LuaImageFilter))
    }
    fn distant_lit_specular(
        direction: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        ks: f32,
        shininess: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::distant_lit_specular(
            direction, light_color, surface_scale, ks, shininess,
            input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn drop_shadow(
        offset: LuaPoint,
        sigma_x: f32,
        sigma_y: f32,
        color: LuaColor,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::drop_shadow(
            offset, (sigma_x, sigma_y),
            color, input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn drop_shadow_only(
        offset: LuaPoint,
        sigma_x: f32,
        sigma_y: f32,
        color: LuaColor,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::drop_shadow_only(
            offset, (sigma_x, sigma_y),
            color, input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn empty() -> _ {
        Ok(LuaImageFilter(image_filters::empty()))
    }
    fn erode(
        radius_x: f32, radius_y: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::erode(
            (radius_x, radius_y), input, crop_rect
        ).map(LuaImageFilter))
    }
    fn image(
        image: LuaImage,
        src_rect: Option<LuaRect>,
        dst_rect: Option<LuaRect>,
        sampling: Option<LuaSamplingOptions>
    ) -> _ {
        let src_rect: Option<Rect> = src_rect.map(LuaRect::into);
        let dst_rect: Option<Rect> = dst_rect.map(LuaRect::into);
        let sampling: SamplingOptions = sampling.unwrap_or_default().into();
        Ok(image_filters::image(
            image.unwrap(), src_rect.as_ref(), dst_rect.as_ref(), sampling
        ).map(LuaImageFilter))
    }
    fn magnifier(
        lens_bounds: LuaRect,
        zoom_amount: f32,
        inset: f32,
        sampling: Option<LuaSamplingOptions>,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let lens_bounds: Rect = lens_bounds.into();
        let sampling: SamplingOptions = sampling.unwrap_or_default().into();
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::magnifier(
            lens_bounds, zoom_amount, inset, sampling, input, crop_rect
        ).map(LuaImageFilter))
    }
    fn matrix_convolution(
        kernel_size: LuaSize,
        kernel: Vec<f32>,
        gain: f32, bias: f32,
        kernel_offset: LuaPoint,
        tile_mode: LuaTileMode,
        convolve_alpha: bool,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::matrix_convolution(
            kernel_size, &kernel, gain, bias, kernel_offset,
            *tile_mode, convolve_alpha,
            input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn matrix_transform(
        matrix: LuaMatrix,
        sampling: Option<LuaSamplingOptions>,
        input: Option<LuaImageFilter>
    ) -> _ {
        let matrix: Matrix = matrix.into();
        let sampling = sampling.unwrap_or_default();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::matrix_transform(
            &matrix, sampling, input
        ).map(LuaImageFilter))
    }
    fn merge(filters: Vec<LuaImageFilter>, crop_rect: Option<LuaRect>) -> _ {
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::merge(
            filters.into_iter().map(|it| Some(it.unwrap())), crop_rect
        ).map(LuaImageFilter))
    }
    fn offset(
        offset: LuaPoint,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::offset(
            offset, input, crop_rect
        ).map(LuaImageFilter))
    }
    fn picture(
        pic: LuaPicture,
        target_rect: Option<LuaRect>
    ) -> _ {
        let target_rect: Option<Rect> = target_rect.map(LuaRect::into);
        Ok(image_filters::picture(
            pic.unwrap(), target_rect.as_ref()
        ).map(LuaImageFilter))
    }
    fn point_lit_diffuse(
        location: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::point_lit_diffuse(
            location, light_color, surface_scale, kd, input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn point_lit_specular(
        location: LuaPoint<3>,
        light_color: LuaColor,
        surface_scale: f32,
        ks: f32,
        shininess: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::point_lit_specular(
            location, light_color, surface_scale, ks, shininess,
            input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn shader(shader: LuaShader, crop_rect: Option<LuaRect>) -> _ {
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::shader(
            shader.unwrap(), crop_rect
        ).map(LuaImageFilter))
    }
    fn spot_lit_diffuse(
        location: LuaPoint<3>,
        target: LuaPoint<3>,
        falloff_exponent: f32,
        cutoff_angle: f32,
        light_color: LuaColor,
        surface_scale: f32,
        kd: f32,
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();

        Ok(image_filters::spot_lit_diffuse(
            location, target, falloff_exponent, cutoff_angle, light_color,
            surface_scale, kd, input, crop_rect,
        ).map(LuaImageFilter))
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
        input: Option<LuaImageFilter>,
        crop_rect: Option<LuaRect>
    ) -> _ {
        let input = input.map(LuaImageFilter::unwrap);
        let crop_rect: CropRect = crop_rect.map(|it| {
            let it: Rect = it.into();
            CropRect::from(it)
        }).unwrap_or_default();
        Ok(image_filters::spot_lit_specular(
            location, target, falloff_exponent, cutoff_angle,
            light_color, surface_scale, ks, shininess,
            input, crop_rect,
        ).map(LuaImageFilter))
    }
    fn tile(src: LuaRect, dst: LuaRect, input: Option<LuaImageFilter>) -> _ {
        let src: Rect = src.into();
        let dst: Rect = dst.into();
        let input = input.map(LuaImageFilter::unwrap);
        Ok(image_filters::tile(&src, &dst, input).map(LuaImageFilter))
    }
});

wrap_skia_handle!(ColorFilter);

impl UserData for LuaColorFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("toAColorMode", |ctx, this, ()| {
            if let Some((color, mode)) = this.to_a_color_mode() {
                let result = ctx.create_table()?;
                result.set(0, LuaColor::from(color))?;
                result.set(1, LuaBlendMode(mode))?;
                Ok(LuaValue::Table(result))
            } else {
                Ok(LuaNil)
            }
        });
        methods.add_method("toAColorMatrix", |ctx, this, ()| {
            if let Some(mx) = this.to_a_color_matrix() {
                Ok(LuaValue::Table(
                    ctx.create_table_from(mx.into_iter().enumerate())?,
                ))
            } else {
                Ok(LuaNil)
            }
        });
        methods.add_method("isAlphaUnchanged", |_, this, ()| {
            Ok(this.is_alpha_unchanged())
        });
        methods.add_method(
            "filterColor",
            |_, this, (color, src_cs, dst_cs): (LuaColor, Option<LuaColorSpace>, Option<LuaColorSpace>)| {
                match src_cs {
                    None => Ok(LuaColor::from(this.filter_color(color))),
                    Some(src_cs) => {
                        let color: Color4f = color.into();
                        Ok(LuaColor::from(this.filter_color4f(
                            &color,
                            &src_cs,
                            dst_cs.map(LuaColorSpace::unwrap).as_ref(),
                        )))
                    }
                }
            },
        );
        methods.add_method("makeComposed", |_, this, inner: LuaColorFilter| {
            Ok(LuaColorFilter(this.composed(inner.unwrap()).ok_or(
                LuaError::RuntimeError("unable to compose filters".to_string()),
            )?))
        });
        methods.add_method(
            "makeWithWorkingColorSpace",
            |_, this, color_space: LuaColorSpace| {
                Ok(LuaColorFilter(
                    this.with_working_color_space(color_space.unwrap()).ok_or(
                        LuaError::RuntimeError("unable to apply color space to filter".to_string()),
                    )?,
                ))
            },
        );
    }
}

decl_constructors!(ColorFilters: {
    fn blend(color: LuaColor, _: Option<LuaColorSpace>, mode: LuaBlendMode) -> _ {
        // NYI: blend color filter color_space handling
        let mode = mode.unwrap();
        Ok(color_filters::blend(color, mode).map(LuaColorFilter))
    }
    fn compose(outer: LuaColorFilter, inner: LuaColorFilter) -> _ {
        Ok(color_filters::compose(outer, inner).map(LuaColorFilter))
    }
    // TODO: ColorFilters::HSLA_matrix(matrix: LuaColorMatrix)
    fn lerp(t: f32, source: LuaColorFilter, destination: LuaColorFilter) -> _ {
        Ok(color_filters::lerp(t, source, destination).map(LuaColorFilter))
    }
    fn lighting(multiply: LuaColor, add: LuaColor) -> _ {
        Ok(color_filters::lighting(multiply, add).map(LuaColorFilter))
    }
    fn linear_to_SRGB_gamma() -> _ {
        Ok(LuaColorFilter(color_filters::linear_to_srgb_gamma()))
    }
    // TODO: ColorFilters::matrix(matrix: LuaColorMatrix)
    fn SRGB_to_linear_gamma() -> _ {
        Ok(LuaColorFilter(color_filters::srgb_to_linear_gamma()))
    }
    // TODO: ColorFilters::table(table: LuaColorTable)
    // TODO: ColorFilters::table_ARGB(table: LuaColorTable)
});

wrap_skia_handle!(MaskFilter);

impl UserData for LuaMaskFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("approximateFilteredBounds", |_, this, src: LuaRect| {
            let src: Rect = src.into();
            Ok(LuaRect::from(this.approximate_filtered_bounds(src)))
        });
    }
}

decl_constructors!(MaskFilter: {
    fn make_blur(style: LuaBlurStyle, sigma: f32, ctm: Option<bool>) -> _ {
        Ok(MaskFilter::blur(style.unwrap(), sigma, ctm).map(LuaMaskFilter))
    }
});

wrap_skia_handle!(DashInfo);
type_like!(DashInfo);

impl<'lua> TryFrom<LuaTable<'lua>> for LuaDashInfo {
    type Error = LuaError;
    fn try_from(t: LuaTable<'lua>) -> Result<Self, Self::Error> {
        let phase: f32 = t.get("intervals").unwrap_or_default();
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

impl<'lua> FromLuaMulti<'lua> for LikeDashInfo {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        ctx: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        if let Ok((intervals, phase)) = FromLuaMulti::from_lua_multi(values.clone(), ctx, consumed)
        {
            *consumed += 2;
            return Ok(LikeDashInfo(LuaDashInfo(DashInfo { intervals, phase })));
        }

        let value = values.into_iter().next().unwrap_or(LuaNil);
        let table = match value {
            LuaValue::UserData(ud) if ud.is::<LuaDashInfo>() => {
                *consumed += 1;
                return Ok(LikeDashInfo(ud.borrow::<LuaDashInfo>()?.to_owned()));
            }
            LuaValue::Table(it) => it,
            other => {
                return Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "DashInfo",
                    message: Some("expected DashInfo or constructor Table".to_string()),
                });
            }
        };
        let result = LuaDashInfo::try_from(table).map(LikeDashInfo)?;
        *consumed += 1;
        Ok(result)
    }
}

impl UserData for LuaDashInfo {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getIntervals", |_, this, ()| Ok(this.intervals.clone()));
        methods.add_method("getPhase", |_, this, ()| Ok(this.phase));
    }
}

wrap_skia_handle!(StrokeRec);

impl Default for LuaStrokeRec {
    fn default() -> Self {
        LuaStrokeRec(StrokeRec::new(StrokeRecInitStyle::Fill))
    }
}

impl UserData for LuaStrokeRec {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getStyle", |_, this, ()| {
            Ok(LuaStrokeRecStyle(this.style()))
        });
        methods.add_method("getWidth", |_, this, ()| Ok(this.width()));
        methods.add_method("getMiter", |_, this, ()| Ok(this.miter()));
        methods.add_method("getCap", |_, this, ()| Ok(LuaPaintCap(this.cap())));
        methods.add_method("getJoin", |_, this, ()| Ok(LuaPaintJoin(this.join())));
        methods.add_method(
            "isHairlineStyle",
            |_, this, ()| Ok(this.is_hairline_style()),
        );
        methods.add_method("isFillStyle", |_, this, ()| Ok(this.is_fill_style()));
        methods.add_method_mut("setFillStyle", |_, this, ()| {
            this.set_fill_style();
            Ok(())
        });
        methods.add_method_mut("setHairlineStyle", |_, this, ()| {
            this.set_hairline_style();
            Ok(())
        });
        methods.add_method_mut(
            "setStrokeStyle",
            |_, this, (width, stroke_and_fill): (f32, Option<bool>)| {
                this.set_stroke_style(width, stroke_and_fill);
                Ok(())
            },
        );
        methods.add_method_mut(
            "setStrokeParams",
            |_, this, (cap, join, miter_limit): (LuaPaintCap, LuaPaintJoin, f32)| {
                this.set_stroke_params(*cap, *join, miter_limit);
                Ok(())
            },
        );
        methods.add_method("getResScale", |_, this, ()| Ok(this.res_scale()));
        methods.add_method_mut("setResScale", |_, this, scale: f32| {
            this.set_res_scale(scale);
            Ok(())
        });
        methods.add_method("needToApply", |_, this, ()| Ok(this.need_to_apply()));
        methods.add_method("applyToPath", |_, this, path: LuaPath| {
            let mut result = Path::new();
            this.apply_to_path(&mut result, &path);
            Ok(LuaPath(result))
        });
        methods.add_method("applyToPaint", |_, this, mut paint: LuaPaint| {
            this.apply_to_paint(&mut paint);
            Ok(paint)
        });
        methods.add_method("getInflationRadius", |_, this, ()| {
            Ok(this.inflation_radius())
        });
        methods.add_method("hasEqualEffect", |_, this, other: Self| {
            Ok(this.has_equal_effect(&other))
        });
    }
}

macro_rules! decl_func_constructor {
    ($handle: ident: |$ctx: tt| $imp: block) => {
        paste::paste! {
            fn [<register_ $handle:snake _constructor>]<'lua>(lua: LuaContext<'lua>) -> Result<(), LuaError> {
                let globals = lua.globals();
                let constructor = lua.create_function(|$ctx: LuaContext, ()| {
                    $imp
                })?;
                globals.set(stringify!($handle), constructor)?;
                Ok(())
            }
        }
    };
    ($handle: ident: |$ctx: ident, $($name: ident: $value: ident $( < $($gen: tt),* > )?),*| $imp: block) => {
        paste::paste! {
            fn [<register_ $handle:snake _constructor>]<'lua>(lua: LuaContext<'lua>) -> Result<(), LuaError> {
                let globals = lua.globals();
                let constructor = lua.create_function(|$ctx: LuaContext, args: LuaMultiValue| {
                    let mut args = args.into_iter();
                    $(
                        let $name: LuaValue = args.next().unwrap_or(LuaNil);
                        let $name: $value$(<$($gen),*>)? = FromLuaMulti::from_lua_multi(LuaMultiValue::from_vec(vec![$name]), $ctx, &mut 0).map_err(|inner| LuaError::CallbackError {
                            traceback: format!("while converting '{}' argument value", stringify!($name)),
                            cause: std::sync::Arc::new(inner),
                        })?;
                    )*
                    $imp
                })?;
                globals.set(stringify!($handle), constructor)?;
                Ok(())
            }
        }
    };
    ($handle: ident: |$ctx: tt, $multi: tt| $imp: block) => {
        paste::paste! {
            fn [<register_ $handle:snake _constructor>]<'lua>(lua: LuaContext<'lua>) -> Result<(), LuaError> {
                let globals = lua.globals();
                let constructor = lua.create_function(|$ctx: LuaContext, $multi: LuaMultiValue| {
                    $imp
                })?;
                globals.set(stringify!($handle), constructor)?;
                Ok(())
            }
        }
    };
}

decl_func_constructor!(StrokeRec: |ctx, args| {
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
        },
        LuaValue::Table(paint_like) => {
            LuaPaint::try_from((paint_like, ctx))?
        },
        LuaValue::UserData(ud) if ud.is::<LuaPaint>() => {
            ud.borrow::<LuaPaint>()?.to_owned()
        },
        other => return Err(LuaError::RuntimeError(
            format!("StrokeRec constructor requires string or Paint; got: {}", other.type_name())
        )),
    };

    stroke_rec.set_stroke_params(paint.stroke_cap(), paint.stroke_join(), paint.stroke_miter());

    match args.next() {
        None => {
            return Ok(stroke_rec)
        }
        Some(LuaValue::String(style)) => {
            let stroke_and_fill = *LuaPaintStyle::try_from(style)? != PaintStyle::Stroke;
            let width = stroke_rec.width();
            stroke_rec.set_stroke_style(width, stroke_and_fill)
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
        None => {
            return Ok(stroke_rec)
        }
        Some(LuaValue::Number(number)) => {
            stroke_rec.set_res_scale(number as f32);
        }
        Some(LuaValue::Integer(number)) => {
            stroke_rec.set_res_scale(number as f32);
        }
        Some(other) => return Err(LuaError::RuntimeError(
            format!("StrokeRec constructor requires resScale (number) as third argument; got: {}", other.type_name())
        )),
    };

    Ok(stroke_rec)
});

wrap_skia_handle!(PathEffect);

impl UserData for LuaPathEffect {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("asADash", |_, this, ()| {
            Ok(this.as_a_dash().map(LuaDashInfo))
        });
        methods.add_method(
            "filterPath",
            |ctx,
             this,
             (src, stroke_rec, cull_rect, ctm): (
                LuaPath,
                LuaStrokeRec,
                LuaRect,
                Option<LuaMatrix>,
            )| {
                let cull_rect: Rect = cull_rect.into();
                let mut dst = Path::new();
                let mut stroke_rec = stroke_rec.unwrap();
                match ctm {
                    None => match this.filter_path(&src, &stroke_rec, cull_rect) {
                        Some((new_dst, new_stroke_rec)) => {
                            dst = new_dst;
                            stroke_rec = new_stroke_rec;
                        }
                        None => return Ok(LuaNil),
                    },
                    Some(ctm) => {
                        if !this.filter_path_inplace_with_matrix(
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
                let result = ctx.create_table()?;
                result.set(0, LuaPath(dst))?;
                result.set(1, LuaStrokeRec(stroke_rec))?;
                result.to_lua(ctx)
            },
        );
        methods.add_method("needsCTM", |_, this, ()| Ok(this.needs_ctm()));
    }
}

decl_constructors!(PathEffect: {
    fn make_sum(first: LuaPathEffect, second: LuaPathEffect) -> _ {
        Ok(LuaPathEffect(path_effect::PathEffect::sum(
            first.0, second.0,
        )))
    }
    fn make_compose(outer: LuaPathEffect, inner: LuaPathEffect) -> _ {
        Ok(LuaPathEffect(path_effect::PathEffect::compose(
            outer.0, inner.0,
        )))
    }
    fn make_dash(like_dash: LikeDashInfo)-> _ {
        Ok(
            skia_safe::dash_path_effect::new(&like_dash.intervals, like_dash.phase)
                .map(LuaPathEffect),
        )
    }
    fn make_trim(start: f32, stop: f32, mode: Option<LuaTrimMode>) -> _ {
        Ok(skia_safe::trim_path_effect::new(start, stop, mode.map_t()).map(LuaPathEffect))
    }
    fn make_radius(radius: f32) -> _ {
        Ok(skia_safe::corner_path_effect::new(radius).map(LuaPathEffect))
    }
    fn make_discrete(length: f32, dev: f32, seed: Option<u32>) -> _ {
        Ok(skia_safe::discrete_path_effect::new(length, dev, seed).map(LuaPathEffect))
    }
    fn make_2D_path(width: f32, mx: LuaMatrix) -> _ {
        let mx: Matrix = mx.into();
        Ok(skia_safe::line_2d_path_effect::new(width, &mx).map(LuaPathEffect))
    }
});

#[derive(Clone)]
pub enum LuaMatrix {
    Three(Matrix),
    Four(M44),
}

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

impl UserData for LuaMatrix {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getDimensions", |_, this, ()| match this {
            LuaMatrix::Three(_) => Ok(3),
            LuaMatrix::Four(_) => Ok(4),
        });
        methods.add_method("get", |ctx, this, pos: LuaPoint| {
            let [col, row] = pos.value.map(|it| it as usize);
            match this {
                LuaMatrix::Three(it) => {
                    let i = col + row * 3;
                    if i < 9 && col < 3 {
                        it.as_slice()[i].to_lua(ctx)
                    } else {
                        Ok(LuaNil)
                    }
                }
                LuaMatrix::Four(it) => {
                    let i = col + row * 4;
                    if i < 16 && col < 4 {
                        it.as_slice()[i].to_lua(ctx)
                    } else {
                        Ok(LuaNil)
                    }
                }
            }
        });
        methods.add_method_mut("set", |_, this, (pos, value): (LuaPoint, f32)| {
            let [col, row] = pos.value.map(|it| it as usize);
            match this {
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
        });
        methods.add_method("getType", |ctx, this, ()| match this {
            LuaMatrix::Three(it) => LuaTypeMask(it.get_type())
                .to_table(ctx)
                .map(LuaValue::Table),
            LuaMatrix::Four(_) => Ok(LuaNil),
        });
        methods.add_method("getScaleX", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.scale_x(),
                LuaMatrix::Four(it) => it.row(0)[0],
            })
        });
        methods.add_method_mut("setScaleX", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_scale_x(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[0] = value;
                }
            }
            Ok(())
        });
        methods.add_method("getScaleY", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.scale_y(),
                LuaMatrix::Four(it) => it.row(1)[1],
            })
        });
        methods.add_method_mut("setScaleY", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_scale_y(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[5] = value;
                }
            }
            Ok(())
        });
        methods.add_method("getScaleZ", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(_) => LuaNil,
                LuaMatrix::Four(it) => LuaValue::Number(it.row(2)[2] as f64),
            })
        });
        methods.add_method_mut("setScaleZ", |_, this, value: f32| match this {
            LuaMatrix::Three(_) => Ok(false),
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[10] = value;
                Ok(true)
            }
        });
        methods.add_method("getTranslateX", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.translate_x(),
                LuaMatrix::Four(it) => it.row(0)[3],
            })
        });
        methods.add_method_mut("setTranslateX", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_translate_x(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[3] = value;
                }
            }
            Ok(())
        });
        methods.add_method("getTranslateY", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.translate_y(),
                LuaMatrix::Four(it) => it.row(1)[3],
            })
        });
        methods.add_method_mut("setTranslateY", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_translate_y(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[7] = value;
                }
            }
            Ok(())
        });
        methods.add_method("getTranslateZ", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(_) => LuaNil,
                LuaMatrix::Four(it) => LuaValue::Number(it.row(2)[3] as f64),
            })
        });
        methods.add_method_mut("setTranslateZ", |_, this, value: f32| match this {
            LuaMatrix::Three(_) => Ok(false),
            LuaMatrix::Four(it) => {
                it.as_slice_mut()[11] = value;
                Ok(true)
            }
        });
        methods.add_method("getSkewX", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.skew_x(),
                LuaMatrix::Four(it) => it.row(0)[1],
            })
        });
        methods.add_method_mut("setSkewX", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_skew_x(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[1] = value;
                }
            }
            Ok(())
        });
        methods.add_method("getSkewY", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(it) => it.skew_y(),
                LuaMatrix::Four(it) => it.row(1)[0],
            })
        });
        methods.add_method_mut("setSkewY", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(it) => {
                    it.set_skew_y(value);
                }
                LuaMatrix::Four(it) => {
                    it.as_slice_mut()[4] = value;
                }
            }
            Ok(())
        });
        methods.add_method_mut(
            "setRectToRect",
            |_, this, (from, to, stf): (LuaRect, LuaRect, LuaScaleToFit)| {
                let from: Rect = from.into();
                let to: Rect = to.into();
                Ok(match this {
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
            },
        );
        methods.add_method("invert", |_, this, ()| {
            Ok(match this {
                LuaMatrix::Three(mx) => mx.invert().map(LuaMatrix::Three),
                LuaMatrix::Four(mx) => mx.invert().map(LuaMatrix::Four),
            })
        });
        methods.add_method("transpose", |_, this, ()| {
            Ok(match this {
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
        });
        methods.add_method("mapXY", |ctx, this, point: LuaPoint| {
            let result = ctx.create_table()?;
            match this {
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
        });
        methods.add_method("mapXYZ", |ctx, this, point: LuaPoint<3>| {
            let result = ctx.create_table()?;
            match this {
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
        });
        methods.add_method("mapRect", |_, this, rect: LuaRect| {
            let rect: Rect = rect.into();
            let mapped = match this {
                LuaMatrix::Three(it) => it.map_rect(rect).0,
                LuaMatrix::Four(it) => {
                    let a = it.map(rect.left, rect.top, 0.0, 1.0);
                    let b = it.map(rect.right, rect.bottom, 0.0, 1.0);
                    Rect::new(a.x, a.y, b.x, b.y)
                }
            };
            Ok(LuaRect::from(mapped))
        });
    }
}

pub struct MatrixConstructors;

impl UserData for MatrixConstructors {
    #[allow(unused_parens)]
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("new", |_, _, argument: Option<LuaValue>| {
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
                        9 => return Ok(LuaMatrix::Three(unsafe {
                            Matrix::from_vec(values).unwrap_unchecked()
                        })),
                        16 => return Ok(LuaMatrix::Four(unsafe {
                            M44::from_vec(values).unwrap_unchecked()
                        })),
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
                other => Err(LuaError::RuntimeError(
                    format!("unsupported matrix size ({}); supported sizes are: 3, 4", other),
                )),
            }
        })
    }
}

wrap_skia_handle!(Paint);

type_like_table!(Paint: |value: LuaTable, lua: LuaContext| {
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

impl UserData for LuaPaint {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isAntiAlias", |_, this, ()| Ok(this.is_anti_alias()));
        methods.add_method_mut("setAntiAlias", |_, this, anti_alias| {
            this.set_anti_alias(anti_alias);
            Ok(())
        });
        methods.add_method("isDither", |_, this, ()| Ok(this.is_dither()));
        methods.add_method_mut("setDither", |_, this, dither| {
            this.set_dither(dither);
            Ok(())
        });
        methods.add_method_mut("getImageFilter", |_, this, ()| {
            Ok(this.image_filter().map(LuaImageFilter))
        });
        methods.add_method_mut(
            "setImageFilter",
            |_, this, image_filter: Option<LuaImageFilter>| {
                this.set_image_filter(image_filter.map(LuaImageFilter::unwrap));
                Ok(())
            },
        );
        methods.add_method_mut("getMaskFilter", |_, this, ()| {
            Ok(this.mask_filter().map(LuaMaskFilter))
        });
        methods.add_method_mut(
            "setMaskFilter",
            |_, this, mask_filter: Option<LuaMaskFilter>| {
                this.set_mask_filter(mask_filter.map(LuaMaskFilter::unwrap));
                Ok(())
            },
        );
        methods.add_method_mut("getColorFilter", |_, this, ()| {
            Ok(this.color_filter().map(LuaColorFilter))
        });
        methods.add_method_mut(
            "setColorFilter",
            |_, this, color_filter: Option<LuaColorFilter>| {
                this.set_color_filter(color_filter.map(LuaColorFilter::unwrap));
                Ok(())
            },
        );
        methods.add_method("getAlpha", |_, this, ()| Ok(this.alpha_f()));
        methods.add_method_mut("setAlpha", |_, this, alpha| {
            this.set_alpha_f(alpha);
            Ok(())
        });
        methods.add_method("getColor", |_, this, ()| Ok(LuaColor::from(this.color4f())));
        methods.add_method_mut(
            "setColor",
            |_, this, (color, color_space): (LuaColor, Option<LuaColorSpace>)| {
                let color: Color4f = color.into();
                this.set_color4f(color, color_space.map(LuaColorSpace::unwrap).as_ref());
                Ok(())
            },
        );
        methods.add_method("getStyle", |ctx, this, ()| {
            let result = ctx.create_table()?;
            match this.style() {
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
        });
        methods.add_method_mut("setStyle", |_, this, style: LuaTable| {
            let fill: bool = style.get("fill").unwrap_or_default();
            let stroke: bool = style.get("stroke").unwrap_or_default();
            this.set_style(match (fill, stroke) {
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
        });
        methods.add_method("getStrokeCap", |_, this, ()| {
            Ok(LuaPaintCap(this.stroke_cap()))
        });
        methods.add_method_mut("setStrokeCap", |_, this, cap: LuaPaintCap| {
            this.set_stroke_cap(*cap);
            Ok(())
        });
        methods.add_method("getStrokeJoin", |_, this, ()| {
            Ok(LuaPaintJoin(this.stroke_join()))
        });
        methods.add_method_mut("setStrokeJoin", |_, this, join: LuaPaintJoin| {
            this.set_stroke_join(*join);
            Ok(())
        });
        methods.add_method("getStrokeWidth", |_, this, ()| Ok(this.stroke_width()));
        methods.add_method_mut("setStrokeWidth", |_, this, width| {
            this.set_stroke_width(width);
            Ok(())
        });
        methods.add_method("getStrokeMiter", |_, this, ()| Ok(this.stroke_miter()));
        methods.add_method_mut("setStrokeMiter", |_, this, miter| {
            this.set_stroke_miter(miter);
            Ok(())
        });
        methods.add_method("getPathEffect", |_, this, ()| {
            Ok(this.path_effect().map(LuaPathEffect))
        });
        methods.add_method_mut("getPathEffect", |_, this, effect: Option<LuaPathEffect>| {
            this.set_path_effect(effect.map(LuaPathEffect::unwrap));
            Ok(())
        });
        methods.add_method("getShader", |_, this, ()| Ok(this.shader().map(LuaShader)));
        methods.add_method_mut("setShader", |_, this, shader: Option<LuaShader>| {
            this.set_shader(shader.map(LuaShader::unwrap));
            Ok(())
        });
    }
}

decl_func_constructor!(Paint: |ctx, color: Option<LuaColor>, color_space: Option<LuaColorSpace>| {
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
});

wrap_skia_handle!(Path);

impl UserData for LuaPath {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method_mut(
            "addArc",
            |_, this, (oval, start_angle, sweep_angle): (LuaRect, f32, f32)| {
                let oval: Rect = oval.into();
                this.add_arc(&oval, start_angle, sweep_angle);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addCircle",
            |_, this, (point, radius, dir): (LuaPoint, f32, Option<LuaPathDirection>)| {
                this.add_circle(point, radius, dir.map_t());
                Ok(())
            },
        );
        methods.add_method_mut(
            "addOval",
            |_, this, (oval, dir, start): (LuaRect, Option<LuaPathDirection>, Option<usize>)| {
                let oval: Rect = oval.into();
                let start = start.unwrap_or(1);
                this.add_oval(oval, Some((dir.unwrap_or_default_t(), start)));
                Ok(())
            },
        );
        methods.add_method_mut(
            "addPath",
            |_, this, (other, point, mode): (LuaPath, LuaPoint, Option<LuaAddPathMode>)| {
                this.add_path(&other, point, mode.map_t());
                Ok(())
            },
        );
        methods.add_method_mut(
            "addPoly",
            |_, this, (points, close): (Vec<LuaPoint>, bool)| {
                let points: Vec<_> = points.into_iter().map(LuaPoint::into).collect();
                this.add_poly(&points, close);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRect",
            |_, this, (rect, dir, start): (LuaRect, Option<LuaPathDirection>, Option<usize>)| {
                let rect: Rect = rect.into();
                let start = start.unwrap_or(1);
                this.add_rect(rect, Some((dir.unwrap_or_default_t(), start)));
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRoundRect",
            |_, this, (rect, rounding, dir): (LuaRect, LuaPoint, Option<LuaPathDirection>)| {
                let rect: Rect = rect.into();
                this.add_round_rect(
                    rect,
                    (rounding.x(), rounding.y()),
                    dir.unwrap_or_default_t(),
                );
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRRect",
            |_, this, (rrect, dir, start): (LuaRRect, Option<LuaPathDirection>, Option<usize>)| {
                let start = start.unwrap_or(1);
                this.add_rrect(rrect.unwrap(), Some((dir.unwrap_or_default_t(), start)));
                Ok(())
            },
        );
        methods.add_method_mut("arcTo", |_, this, (oval, start_angle, sweep_angle, force_move_to): (LuaRect, f32, f32, bool)| {
            let oval: Rect = oval.into();
            this.arc_to(oval, start_angle, sweep_angle, force_move_to);
            Ok(())
        });
        methods.add_method_mut("close", |_, this, ()| {
            this.close();
            Ok(())
        });
        methods.add_method("computeTightBounds", |_, this, ()| {
            Ok(LuaRect::from(this.compute_tight_bounds()))
        });
        methods.add_method_mut(
            "conicTo",
            |_, this, (p1, p2, w): (LuaPoint, LuaPoint, f32)| {
                this.conic_to(p1, p2, w);
                Ok(())
            },
        );
        methods.add_method("conservativelyContainsRect", |_, this, rect: LuaRect| {
            let rect: Rect = rect.into();
            Ok(this.conservatively_contains_rect(rect))
        });
        methods.add_method("contains", |_, this, p: LuaPoint| Ok(this.contains(p)));
        methods.add_method("countPoints", |_, this, ()| Ok(this.count_points()));
        methods.add_method("countVerbs", |_, this, ()| Ok(this.count_verbs()));
        methods.add_method_mut(
            "cubicTo",
            |_, this, (p1, p2, p3): (LuaPoint, LuaPoint, LuaPoint)| {
                this.cubic_to(p1, p2, p3);
                Ok(())
            },
        );
        methods.add_method("getBounds", |_, this, ()| Ok(LuaRect::from(*this.bounds())));
        methods.add_method("getFillType", |_, this, ()| {
            Ok(LuaPathFillType(this.fill_type()))
        });
        methods.add_method("getGenerationID", |_, this, ()| Ok(this.generation_id()));
        methods.add_method("getLastPt", |_, this, ()| {
            Ok(this.last_pt().map(LuaPoint::from))
        });
        methods.add_method("getPoint", |_, this, index: usize| {
            Ok(this.get_point(index).map(LuaPoint::from))
        });
        methods.add_method("getPoints", |ctx, this, count: Option<usize>| unsafe {
            let count = count.unwrap_or_else(|| this.count_points());
            let layout = Layout::from_size_align(size_of::<Point>() * count, align_of::<Point>())
                .expect("invalid Point array layout");
            let data = std::alloc::alloc(layout) as *mut Point;
            let slice = std::slice::from_raw_parts_mut(data, count);

            this.get_points(slice);

            let result = ctx.create_table()?;
            for (i, point) in slice.iter_mut().enumerate() {
                result.set(i, LuaPoint::from(*point).to_lua(ctx)?)?;
            }
            std::alloc::dealloc(data as *mut u8, layout);
            Ok(result)
        });
        methods.add_method("getSegmentMasks", |ctx, this, ()| {
            LuaSegmentMask(this.segment_masks()).to_table(ctx)
        });
        methods.add_method("getVerbs", |ctx, this, count: Option<usize>| unsafe {
            let count = count.unwrap_or_else(|| this.count_verbs());
            let layout = Layout::from_size_align(size_of::<Verb>() * count, align_of::<Verb>())
                .expect("invalid Verb array layout");
            let data = std::alloc::alloc(layout);
            let slice = std::slice::from_raw_parts_mut(data, count * size_of::<Verb>());

            this.get_verbs(slice);
            let slice = std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut Verb, count);

            let result = ctx.create_table()?;
            for (i, verb) in slice.iter().enumerate() {
                result.set(i, LuaVerb(*verb))?;
            }
            std::alloc::dealloc(data as *mut u8, layout);
            Ok(result)
        });
        methods.add_method_mut("incReserve", |_, this, extra_pt_count: usize| {
            this.inc_reserve(extra_pt_count);
            Ok(())
        });
        methods.add_method(
            "interpolate",
            |_, this, (ending, weight): (LuaPath, f32)| {
                this.interpolate(&ending, weight);
                Ok(())
            },
        );
        methods.add_method("isConvex", |_, this, ()| Ok(this.is_convex()));
        methods.add_method("isEmpty", |_, this, ()| Ok(this.is_empty()));
        methods.add_method("isFinite", |_, this, ()| Ok(this.is_finite()));
        methods.add_method("isInterpolatable", |_, this, other: LuaPath| {
            Ok(this.is_interpolatable(&other))
        });
        methods.add_method("isInverseFillType", |_, this, ()| {
            Ok(this.is_inverse_fill_type())
        });
        methods.add_method("isLastContourClosed", |_, this, ()| {
            Ok(this.is_last_contour_closed())
        });
        methods.add_method("isLine", |_, this, ()| {
            Ok(this.is_line().map(LuaLine::from))
        });
        methods.add_method("isOval", |_, this, ()| {
            Ok(this.is_oval().map(LuaRect::from))
        });
        methods.add_method("isRect", |_, this, ()| {
            Ok(this.is_rect().map(|(rect, _, _)| LuaRect::from(rect)))
        });
        methods.add_method("isRRect", |_, this, ()| Ok(this.is_rrect().map(LuaRRect)));
        methods.add_method("isValid", |_, this, ()| Ok(this.is_valid()));
        methods.add_method("isVolatile", |_, this, ()| Ok(this.is_volatile()));
        methods.add_method_mut("lineTo", |_, this, point: LuaPoint| {
            this.line_to(point);
            Ok(())
        });
        methods.add_method_mut("makeScale", |_, this, (sx, sy): (f32, Option<f32>)| {
            let sy = sy.unwrap_or(sx);
            Ok(LuaPath(this.make_scale((sx, sy))))
        });
        methods.add_method_mut(
            "makeTransform",
            |_, this, (matrix, pc): (LuaMatrix, Option<bool>)| {
                let matrix = matrix.into();
                let pc = match pc.unwrap_or(true) {
                    true => skia_safe::matrix::ApplyPerspectiveClip::Yes,
                    false => skia_safe::matrix::ApplyPerspectiveClip::No,
                };
                Ok(LuaPath(this.make_transform(&matrix, pc)))
            },
        );
        methods.add_method_mut("moveTo", |_, this, p: LuaPoint| {
            this.move_to(p);
            Ok(())
        });
        methods.add_method_mut("offset", |_, this, d: LuaPoint| {
            this.offset(d);
            Ok(())
        });
        methods.add_method_mut("quadTo", |_, this, (p1, p2): (LuaPoint, LuaPoint)| {
            this.quad_to(p1, p2);
            Ok(())
        });
        methods.add_method_mut(
            "rArcTo",
            |_,
             this,
             (r, x_axis_rotate, arc_size, sweep, d): (
                LuaPoint,
                f32,
                LuaArcSize,
                LuaPathDirection,
                LuaPoint,
            )| {
                this.r_arc_to_rotated(r, x_axis_rotate, *arc_size, *sweep, d);
                Ok(())
            },
        );
        methods.add_method_mut(
            "rConicTo",
            |_, this, (d1, d2, w): (LuaPoint, LuaPoint, f32)| {
                this.r_conic_to(d1, d2, w);
                Ok(())
            },
        );
        methods.add_method_mut(
            "rCubicTo",
            |_, this, (d1, d2, d3): (LuaPoint, LuaPoint, LuaPoint)| {
                this.r_cubic_to(d1, d2, d3);
                Ok(())
            },
        );
        methods.add_method_mut("reset", |_, this, ()| {
            this.reset();
            Ok(())
        });
        methods.add_method_mut("reverseAddPath", |_, this, path: LuaPath| {
            this.reverse_add_path(&path);
            Ok(())
        });
        methods.add_method_mut("rewind", |_, this, ()| {
            this.rewind();
            Ok(())
        });
        methods.add_method_mut("rLineTo", |_, this, point: LuaPoint| {
            this.r_line_to(point);
            Ok(())
        });
        methods.add_method_mut("rMoveTo", |_, this, point: LuaPoint| {
            this.r_move_to(point);
            Ok(())
        });
        methods.add_method_mut("rQuadTo", |_, this, (dx1, dx2): (LuaPoint, LuaPoint)| {
            this.r_quad_to(dx1, dx2);
            Ok(())
        });
        methods.add_method_mut("setFillType", |_, this, fill_type: LuaPathFillType| {
            this.set_fill_type(*fill_type);
            Ok(())
        });
        methods.add_method_mut("setIsVolatile", |_, this, is_volatile| {
            this.set_is_volatile(is_volatile);
            Ok(())
        });
        methods.add_method_mut("setLastPt", |_, this, point: LuaPoint| {
            this.set_last_pt(point);
            Ok(())
        });
        methods.add_method_mut("toggleInverseFillType", |_, this, ()| {
            this.toggle_inverse_fill_type();
            Ok(())
        });
        methods.add_method_mut("transform", |_, this, matrix: LuaMatrix| {
            let matrix = matrix.into();
            this.transform(&matrix);
            Ok(())
        });
    }
}

decl_constructors!(Path: {
    fn make(points: Vec<LuaPoint>, verbs: Vec<LuaVerb>, conic_weights: Vec<f32>, fill_type: LuaPathFillType, volatile: Option<bool>) -> _ {
        let points: Vec<Point> = points.into_iter().map(LuaPoint::into).collect();
        let verbs: Vec<u8> = verbs.into_iter().map(|it| it.0 as u8).collect();
        Ok(LuaPath(Path::new_from(&points, &verbs, &conic_weights, *fill_type, volatile)))
    }
});
decl_func_constructor!(Path: |_| {
    Ok(LuaPath(Path::default()))
});

wrap_skia_handle!(RRect);

impl UserData for LuaRRect {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("contains", |_, this, rect: LuaRect| {
            let rect: Rect = rect.into();
            Ok(this.contains(rect))
        });
        methods.add_method("getBounds", |_, this, ()| {
            Ok(LuaRect::from(this.bounds().clone()))
        });
        methods.add_method("getSimpleRadii", |_, this, ()| {
            Ok(LuaPoint::from(this.simple_radii()))
        });
        methods.add_method("getType", |_, this, ()| Ok(LuaRRectType(this.get_type())));
        methods.add_method("height", |_, this, ()| Ok(this.height()));
        methods.add_method_mut("inset", |_, this, delta: LuaPoint| {
            this.inset(delta);
            Ok(())
        });
        methods.add_method("isComplex", |_, this, ()| Ok(this.is_complex()));
        methods.add_method("isEmpty", |_, this, ()| Ok(this.is_empty()));
        methods.add_method("isNinePatch", |_, this, ()| Ok(this.is_nine_patch()));
        methods.add_method("isOval", |_, this, ()| Ok(this.is_oval()));
        methods.add_method("isRect", |_, this, ()| Ok(this.is_rect()));
        methods.add_method("isSimple", |_, this, ()| Ok(this.is_simple()));
        methods.add_method("isValid", |_, this, ()| Ok(this.is_valid()));
        methods.add_method("makeOffset", |_, this, delta: LuaPoint| {
            Ok(LuaRRect(this.with_offset(delta)))
        });
        methods.add_method_mut("offset", |_, this, delta: LuaPoint| {
            this.offset(delta);
            Ok(())
        });
        methods.add_method_mut("outset", |_, this, delta: LuaPoint| {
            this.outset(delta);
            Ok(())
        });
        methods.add_method("radii", |_, this, corner: Option<LuaRRectCorner>| {
            let radii = match corner {
                Some(it) => this.radii(*it),
                None => this.simple_radii(),
            };
            Ok(LuaPoint::from(radii))
        });
        methods.add_method("rect", |_, this, ()| Ok(LuaRect::from(this.rect().clone())));
        methods.add_method_mut("setEmpty", |_, this, ()| {
            this.set_empty();
            Ok(())
        });
        methods.add_method_mut(
            "setNinePatch",
            |_, this, (rect, sides): (LuaRect, SidePack)| {
                let rect: Rect = rect.into();
                this.set_nine_patch(rect, sides.left, sides.top, sides.right, sides.bottom);
                Ok(())
            },
        );
        methods.add_method_mut("setOval", |_, this, oval: LuaRect| {
            let oval: Rect = oval.into();
            this.set_oval(oval);
            Ok(())
        });
        methods.add_method_mut("setRect", |_, this, rect: LuaRect| {
            let rect: Rect = rect.into();
            this.set_rect(rect);
            Ok(())
        });
        methods.add_method_mut(
            "setRectRadii",
            |_, this, (rect, radii): (LuaRect, Vec<LuaPoint>)| {
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
                this.set_rect_radii(rect, &radii);
                Ok(())
            },
        );
        methods.add_method_mut(
            "setRectXY",
            |_, this, (rect, x_rad, y_rad): (LuaRect, f32, f32)| {
                let rect: Rect = rect.into();
                this.set_rect_xy(rect, x_rad, y_rad);
                Ok(())
            },
        );
        methods.add_method("transform", |_, this, matrix: LuaMatrix| {
            let matrix: Matrix = matrix.into();
            Ok(this.transform(&matrix).map(LuaRRect))
        });
        methods.add_method("type", |_, this, ()| Ok(LuaRRectType(this.get_type())));
        methods.add_method("width", |_, this, ()| Ok(this.width()));
    }
}

decl_constructors!(RRect: {
    fn make() -> _ {
        Ok(LuaRRect(RRect::new()))
    }
});
decl_func_constructor!(RRect: |_| {
    Ok(LuaRRect(RRect::new()))
});

wrap_skia_handle!(ColorInfo);
impl UserData for LuaColorInfo {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("alphaType", |_, this, ()| {
            Ok(LuaAlphaType(this.alpha_type()))
        });
        methods.add_method("bytesPerPixel", |_, this, ()| Ok(this.bytes_per_pixel()));
        methods.add_method("colorSpace", |_, this, ()| {
            Ok(this.color_space().map(LuaColorSpace))
        });
        methods.add_method("colorType", |_, this, ()| {
            Ok(LuaColorType(this.color_type()))
        });
        methods.add_method("gammaCloseToSRGB", |_, this, ()| {
            Ok(this.is_gamma_close_to_srgb())
        });
        methods.add_method("isOpaque", |_, this, ()| Ok(this.is_opaque()));
        methods.add_method("makeAlphaType", |_, this, alpha_type: LuaAlphaType| {
            Ok(LuaColorInfo(this.with_alpha_type(*alpha_type)))
        });
        methods.add_method(
            "makeColorSpace",
            |_, this, color_space: Option<LuaColorSpace>| {
                Ok(LuaColorInfo(
                    this.with_color_space(color_space.map(LuaColorSpace::unwrap)),
                ))
            },
        );
        methods.add_method("makeColorType", |_, this, color_type: LuaColorType| {
            Ok(LuaColorInfo(this.with_color_type(*color_type)))
        });
        methods.add_method("shiftPerPixel", |_, this, ()| Ok(this.shift_per_pixel()));
    }
}

wrap_skia_handle!(ImageInfo);
impl UserData for LuaImageInfo {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("alphaType", |_, this, ()| {
            Ok(LuaAlphaType(this.alpha_type()))
        });
        methods.add_method("bounds", |_, this, ()| Ok(LuaRect::from(this.bounds())));
        methods.add_method("bytesPerPixel", |_, this, ()| Ok(this.bytes_per_pixel()));
        methods.add_method("colorInfo", |ctx, this, ()| {
            let result = ctx.create_table()?;
            let info = this.color_info();
            result.set("colorSpace", info.color_space().map(LuaColorSpace))?;
            result.set("colorType", LuaColorType(info.color_type()))?;
            result.set("alphaType", LuaAlphaType(info.alpha_type()))?;
            result.set("isOpaque", info.is_opaque())?;
            result.set("gammaCloseToSrgb", info.is_gamma_close_to_srgb())?;
            result.set("bytesPerPixel", info.bytes_per_pixel())?;
            result.set("shiftPerPixel", info.shift_per_pixel())?;
            Ok(result)
        });
        methods.add_method("colorSpace", |_, this, ()| {
            Ok(this.color_space().map(LuaColorSpace))
        });
        methods.add_method("colorType", |_, this, ()| {
            Ok(LuaColorType(this.color_type()))
        });
        methods.add_method("computeByteSize", |_, this, row_bytes: usize| {
            Ok(this.compute_byte_size(row_bytes))
        });
        methods.add_method("computeMinByteSize", |_, this, ()| {
            Ok(this.compute_min_byte_size())
        });
        methods.add_method(
            "computeOffset",
            |_, this, (point, row_bytes): (LuaPoint, usize)| {
                Ok(this.compute_offset(point, row_bytes))
            },
        );
        methods.add_method("dimensions", |_, this, ()| {
            Ok(LuaSize::from(this.dimensions()))
        });
        methods.add_method("gammaCloseToSRGB", |_, this, ()| {
            Ok(this.is_gamma_close_to_srgb())
        });
        methods.add_method("height", |_, this, ()| Ok(this.height()));
        methods.add_method("isEmpty", |_, this, ()| Ok(this.is_empty()));
        methods.add_method("isOpaque", |_, this, ()| Ok(this.is_opaque()));
        methods.add_method("makeAlphaType", |_, this, alpha_type: LuaAlphaType| {
            Ok(LuaImageInfo(this.with_alpha_type(*alpha_type)))
        });
        methods.add_method("makeColorSpace", |_, this, color_space: LuaColorSpace| {
            Ok(LuaImageInfo(this.with_color_space(color_space.unwrap())))
        });
        methods.add_method("makeColorType", |_, this, color_type: LuaColorType| {
            Ok(LuaImageInfo(this.with_color_type(*color_type)))
        });
        methods.add_method("makeDimensions", |_, this, dimensions: LuaSize| {
            Ok(LuaImageInfo(this.with_dimensions(dimensions)))
        });
        methods.add_method("minRowBytes", |_, this, ()| Ok(this.min_row_bytes()));
        methods.add_method_mut("reset", |_, this, ()| {
            this.reset();
            Ok(())
        });
        methods.add_method("shiftPerPixel", |_, this, ()| Ok(this.shift_per_pixel()));
        methods.add_method("validRowBytes", |_, this, row_bytes: usize| {
            Ok(this.valid_row_bytes(row_bytes))
        });
        methods.add_method("width", |_, this, ()| Ok(this.width()));
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
        .get::<_, LuaColorSpace>("color_space")
        .ok()
        .map(LuaColorSpace::unwrap);

    let result = ImageInfo::new(dimensions, *color_type, *alpha_type, color_space);

    Ok(LuaImageInfo(result))
});

wrap_skia_handle!(SurfaceProps);
impl UserData for LuaSurfaceProps {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("flags", |ctx, this, ()| {
            LuaSurfacePropsFlags(this.flags()).to_table(ctx)
        });
        methods.add_method("pixelGeometry", |_, this, ()| {
            Ok(LuaPixelGeometry(this.pixel_geometry()))
        });
        methods.add_method("isUseDeviceIndependentFonts", |_, this, ()| {
            Ok(this.is_use_device_independent_fonts())
        });
        methods.add_method("isAlwaysDither", |_, this, ()| Ok(this.is_always_dither()));
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

impl<'lua> FromLuaMulti<'lua> for LuaSamplingOptions {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        let mut values = values.into_iter();
        let first = match values.next() {
            Some(it) => it,
            None => return Ok(Self::default()),
        };

        let first = match first {
            LuaValue::Table(table) => {
                let filter = table
                    .get::<_, String>("filter")
                    .or(table.get("filter_mode"))
                    .and_then(LuaFilterMode::try_from);
                let mipmap = table
                    .get::<_, String>("mipmap")
                    .or(table.get("mipmap_mode"))
                    .and_then(LuaMipmapMode::try_from);

                if filter.is_err() && mipmap.is_err() {
                    return Ok(Self::default());
                }
                *consumed += 1;

                return Ok(LuaSamplingOptions {
                    filter_mode: filter.unwrap_or_t(FilterMode::Nearest),
                    mipmap_mode: mipmap.unwrap_or_t(MipmapMode::None),
                });
            }
            LuaValue::String(filter) => match filter.to_str().and_then(LuaFilterMode::from_str) {
                Ok(it) => it,
                Err(_) => return Ok(Self::default()),
            },
            _ => return Ok(Self::default()),
        };

        const SECOND_MISSING: &'static str = "only filtering mode provided; unpacked SamplingOptions require both filtering and mipmapping to be specified to avoid ambiguity";

        let second = match values.next() {
            Some(LuaValue::String(it)) => it,
            other => {
                let other = match other {
                    Some(LuaValue::Boolean(_)) => "string, boolean",
                    Some(LuaValue::LightUserData(_)) => "string, lightuserdata",
                    Some(LuaValue::Integer(_)) => "string, integer",
                    Some(LuaValue::Number(_)) => "string, number",
                    Some(LuaValue::String(_)) => "string, string", // unreachable
                    Some(LuaValue::Table(_)) => "string, table",
                    Some(LuaValue::Function(_)) => "string, function",
                    Some(LuaValue::Thread(_)) => "string, thread",
                    Some(LuaValue::UserData(_)) => "string, userdata",
                    Some(LuaValue::Error(_)) => "string, error",
                    Some(LuaNil) | None => "string, nil",
                };

                // this is a weird edge case with FromLuaMulti where unpacked
                // values completely overlap so if the second one is missing
                // it's unclear whether the caller wanted to specify filtering
                // or mipmapping mode and thus an error must be returned
                return Err(LuaError::FromLuaConversionError {
                    from: other,
                    to: "SamplingOptions",
                    message: Some(SECOND_MISSING.to_string()),
                });
            }
        };

        let second = match second.to_str().and_then(LuaMipmapMode::from_str) {
            Ok(it) => it,
            Err(err) => {
                return Err(LuaError::CallbackError {
                    traceback: SECOND_MISSING.to_string(),
                    cause: Arc::new(err),
                })
            }
        };

        *consumed += 2;

        Ok(LuaSamplingOptions {
            filter_mode: *first,
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

impl UserData for LuaSurface {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        // capabilities - not useful from Lua?
        // characterize - no graphite bindings
        methods.add_method_mut(
            "draw",
            |_,
             this,
             (canvas, offset, sampling, paint): (
                LuaCanvas,
                LuaPoint,
                Option<LuaSamplingOptions>,
                Option<LikePaint>,
            )| {
                let sampling: SamplingOptions = sampling.unwrap_or_default().into();
                let paint = paint.map(LikePaint::unwrap);

                this.draw(&canvas, offset, sampling, paint.as_ref());
                Ok(())
            },
        );
        // generationID - not useful from Lua without graphite?
        methods.add_method_mut("getCanvas", |_, this, ()| {
            Ok(LuaCanvas::Owned(this.0.clone()))
        });
        methods.add_method("width", |_, this, ()| Ok(this.width()));
        methods.add_method("height", |_, this, ()| Ok(this.height()));
        methods.add_method_mut("imageInfo", |_, this, ()| {
            Ok(LuaImageInfo(this.image_info()))
        });
        // isCompatible - no low-level renderer bindings in Lua
        methods.add_method_mut("makeImageSnapshot", |_, this, ()| {
            Ok(LuaImage(this.image_snapshot()))
        });
        methods.add_method_mut("makeSurface", |_, this, image_info: LikeImageInfo| {
            Ok(this.new_surface(&image_info.unwrap()).map(LuaSurface))
        });
        // peekPixels - very complicated to handle properly
        methods.add_method("props", |_, this, ()| {
            Ok(LuaSurfaceProps(this.props().clone()))
        });
        methods.add_method_mut(
            "readPixels",
            |ctx, this, (rect, info): (Option<LuaRect>, Option<LuaImageInfo>)| {
                let area = rect
                    .map(Into::into)
                    .unwrap_or_else(|| IRect::new(0, 0, this.width(), this.height()));
                let image_info = info
                    .map(LuaImageInfo::unwrap)
                    .unwrap_or_else(|| this.image_info().with_dimensions(area.size()));
                let row_bytes = area.width() as usize * image_info.bytes_per_pixel();
                let mut result = Vec::with_capacity(row_bytes * area.height() as usize);
                let is_some = this.read_pixels(
                    &image_info,
                    result.as_mut_slice(),
                    row_bytes,
                    IPoint::new(area.x(), area.y()),
                );
                match is_some {
                    true => {
                        let result = ctx.create_table_from_vec(result)?;
                        result.set("info", LuaImageInfo(image_info))?;
                        Ok(Some(result))
                    }
                    false => Ok(None),
                }
            },
        );
        methods.add_method_mut(
            "writePixels",
            |_,
             this,
             (dst, data, info, size): (
                LuaPoint,
                LuaTable,
                Option<LikeImageInfo>,
                Option<LuaSize>,
            )| {
                let info = info
                    .or_else(|| data.get("info").ok())
                    .map(LikeImageInfo::unwrap)
                    .unwrap_or_else(|| this.image_info());
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
                this.write_pixels_from_pixmap(&pm, dst);
                Ok(true)
            },
        );
        // recorder - graphite bindings not supported
        // recordingContext - graphite bindings not supported
        // replaceBackendTexture - graphite bindings not supported
    }
}

// SAFETY: Clunky handles Lua and rendering on the same thread
unsafe impl Send for LuaSurface {}

decl_constructors!(Surfaces: {
    fn null(size: LuaSize) -> _ {
        let size: ISize = size.into();
        Ok(surfaces::null(size).map(LuaSurface))
    }
    fn raster(info: LikeImageInfo, row_bytes: Option<usize>, props: Option<LikeSurfaceProps>) -> _ {
        let info: ImageInfo = info.unwrap();
        let row_bytes = row_bytes.unwrap_or_else(|| info.min_row_bytes());
        let props: Option<SurfaceProps> = props.map_t();

        Ok(surfaces::raster(
            &info,
            row_bytes,
            props.as_ref(),
        ).map(LuaSurface))
    }
    // wrap_pixels - not able to detect table value updates
});

wrap_skia_handle!(FontStyleSet);

impl UserData for LuaFontStyleSet {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method_mut("count", |_, this, ()| Ok(this.count()));
        methods.add_method_mut("getStyle", |_, this, index: usize| {
            let (style, name) = this.style(index);
            Ok((LuaFontStyle(style), name))
        });
        methods.add_method_mut("createTypeface", |_, this, index: usize| {
            Ok(this.new_typeface(index).map(LuaTypeface))
        });
        methods.add_method_mut(
            "matchStyle",
            |_, this, (index, pattern): (usize, LuaFontStyle)| {
                Ok(this.match_style(index, pattern.unwrap()).map(LuaTypeface))
            },
        );
    }
}

decl_constructors!(FontStyleSet: {
    fn create_empty() -> _ {
        Ok(LuaFontStyleSet(FontStyleSet::new_empty()))
    }
});

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

impl<'lua> FromLuaMulti<'lua> for LuaText {
    fn from_lua_multi(
        values: LuaMultiValue<'lua>,
        _: LuaContext<'lua>,
        consumed: &mut usize,
    ) -> LuaResult<Self> {
        let mut values = values.into_iter();

        let bytes = match_value_iter!(values as "Text":
            LuaValue::String(text) => {
                let text = OsString::from_str(text.to_str()?).unwrap();
                *consumed += 1;
                return Ok(LuaText {
                    text,
                    encoding: TextEncoding::UTF8,
                });
            }
            LuaValue::Table(table) => table,
        );
        *consumed += 1;

        let bytes: Vec<LuaInteger> = bytes
            .sequence_values::<LuaInteger>()
            .take_while(Result::is_ok)
            .filter_map(Result::ok)
            .collect();

        let encoding = match values.next() {
            Some(LuaValue::String(encoding)) => {
                if let Ok(it) = LuaTextEncoding::try_from(encoding) {
                    *consumed += 1;
                    *it
                } else {
                    TextEncoding::UTF8
                }
            }
            _ => TextEncoding::UTF8,
        };

        let text = if matches!(encoding, TextEncoding::UTF8) {
            bytes.into_iter().map(|it| it as u8).collect()
        } else {
            let size = encoding_size(encoding);
            let mut result = Vec::with_capacity(bytes.len() * size);

            match size {
                2 => bytes.into_iter().map(|it| (it as u16)).for_each(|it| {
                    let _ = result.write_u16::<byteorder::NativeEndian>(it);
                }),
                4 => bytes.into_iter().map(|it| (it as u32)).for_each(|it| {
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

impl UserData for LuaFontMgr {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("countFamilies", |_, this, ()| {
            Ok(this.unwrap().count_families())
        });
        methods.add_method("createStyleSet", |_, this, index: usize| {
            Ok(LuaFontStyleSet(this.unwrap().new_style_set(index)))
        });
        methods.add_method("getFamilyName", |_, this, index: usize| {
            Ok(this.unwrap().family_name(index))
        });
        // NYI: legacyMakeTypeface by skia_safe
        methods.add_method(
            "makeFromData",
            |_, this, (bytes, ttc): (Vec<u8>, Option<usize>)| {
                Ok(this.unwrap().new_from_data(&bytes, ttc).map(LuaTypeface))
            },
        );
        methods.add_method(
            "makeFromFile",
            |_, this, (path, ttc): (String, Option<usize>)| {
                let bytes = match std::fs::read(path.as_str()) {
                    Ok(it) => it,
                    Err(_) => {
                        return Err(LuaError::RuntimeError(format!(
                            "unable to read font file: {}",
                            path
                        )))
                    }
                };
                Ok(this.unwrap().new_from_data(&bytes, ttc).map(LuaTypeface))
            },
        );
        // makeFromStream - Lua has no streams
        methods.add_method("matchFamily", |_, this, family_name: String| {
            Ok(LuaFontStyleSet(this.unwrap().match_family(family_name)))
        });
        methods.add_method(
            "matchFamilyStyle",
            |_, this, (family_name, style): (String, LuaFontStyle)| {
                Ok(this
                    .unwrap()
                    .match_family_style(family_name, style.unwrap())
                    .map(LuaTypeface))
            },
        );
        methods.add_method(
            "matchFamilyStyleCharacter",
            |_,
             this,
             (family_name, style, bcp47, character): (
                String,
                LuaFontStyle,
                Vec<String>,
                Unichar,
            )| {
                let bcp_refs: Vec<&str> = bcp47.iter().map(|it| it.as_ref()).collect();
                Ok(this
                    .unwrap()
                    .match_family_style_character(family_name, style.unwrap(), &bcp_refs, character)
                    .map(LuaTypeface))
            },
        );
    }
}

decl_constructors!(FontMgr: {
    fn default() -> _ {
        Ok(LuaFontMgr::Default)
    }
    fn empty() -> _ {
        Ok(LuaFontMgr::Empty)
    }
});

wrap_skia_handle!(Typeface);

impl UserData for LuaTypeface {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("countGlyphs", |_, this, ()| Ok(this.count_glyphs()));
        methods.add_method("countTables", |_, this, ()| Ok(this.count_tables()));
        // createFamilyNameIterator -> familyNames; Lua doesn't have iterators
        methods.add_method("familyNames", |_, this, ()| {
            let names: HashMap<_, _> = this
                .new_family_name_iterator()
                .map(|it| (it.language, it.string))
                .collect();
            Ok(names)
        });
        // NYI: createScalerContext by skia_safe
        // NYI: filterRec by skia_safe
        methods.add_method("fontStyle", |_, this, ()| {
            Ok(LuaFontStyle(this.font_style()))
        });
        methods.add_method("getBounds", |_, this, ()| Ok(LuaRect::from(this.bounds())));
        methods.add_method("getFamilyName", |_, this, ()| Ok(this.family_name()));
        // methods.add_method("getFontDescriptor", |_, this, ()| Ok(()));
        methods.add_method(
            "getKerningPairAdjustments",
            |_, this, glyphs: Vec<GlyphId>| {
                let mut adjustments = Vec::with_capacity(glyphs.len());
                this.get_kerning_pair_adjustments(glyphs.as_ref(), adjustments.as_mut_slice());
                Ok(adjustments)
            },
        );
        methods.add_method("getPostScriptName", |_, this, ()| {
            Ok(this.post_script_name())
        });
        methods.add_method("getTableData", |_, this, tag: FontTableTag| {
            match this.get_table_size(tag) {
                Some(size) => {
                    let mut result = Vec::with_capacity(size);
                    this.get_table_data(tag, result.as_mut_slice());
                    Ok(result)
                }
                None => Ok(vec![]),
            }
        });
        methods.add_method("getTableSize", |_, this, tag: FontTableTag| {
            Ok(this.get_table_size(tag))
        });
        methods.add_method("getTableTags", |_, this, ()| Ok(this.table_tags()));
        methods.add_method("getUnitsPerEm", |_, this, ()| Ok(this.units_per_em()));
        // TODO: methods.add_method("getVariationDesignParameters", |_, this, ()| Ok(()));
        // TODO: methods.add_method("getVariationDesignPosition", |_, this, ()| Ok(()));
        methods.add_method("isBold", |_, this, ()| Ok(this.is_bold()));
        methods.add_method("isFixedPitch", |_, this, ()| Ok(this.is_fixed_pitch()));
        methods.add_method("isItalic", |_, this, ()| Ok(this.is_italic()));
        methods.add_method("makeClone", |_, this, ()| Ok(LuaTypeface(this.0.clone())));
        // NYI: openExistingStream by skia_safe
        // NYI: openStream by skia_safe
        methods.add_method("textToGlyphs", |_, this, text: LuaText| {
            let mut result = Vec::with_capacity(text.text.len());
            this.text_to_glyphs(text.text.as_bytes(), text.encoding, result.as_mut_slice());
            Ok(result)
        });
        methods.add_method("stringToGlyphs", |_, this, text: String| {
            let mut result = Vec::with_capacity(text.len());
            this.str_to_glyphs(&text, result.as_mut_slice());
            Ok(result)
        });
        methods.add_method("unicharsToGlyphs", |_, this, unichars: Vec<Unichar>| {
            let mut result = Vec::new();
            this.unichars_to_glyphs(&unichars, result.as_mut_slice());
            Ok(result)
        });
        methods.add_method("unicharToGlyph", |_, this, unichar: Unichar| {
            Ok(this.unichar_to_glyph(unichar))
        });
    }
}

decl_constructors!(Typeface: {
    fn make_default() -> _ {
        Ok(LuaTypeface(Typeface::default()))
    }
    // NYI: Typeface::make_empty by skia_safe
    fn make_from_name(family_name: String, font_style: Option<LuaFontStyle>) -> _ {
        let font_style = font_style.map(LuaFontStyle::unwrap).unwrap_or_default();
        Ok(FontMgr::default().match_family_style(family_name, font_style)
            .map(LuaTypeface))
    }
    fn make_from_data(data: Vec<u8>, index: Option<usize>) -> _ {
        Ok(FontMgr::default().new_from_data(&data, index.unwrap_or_default())
            .map(LuaTypeface))
    }
    fn make_from_file(path: String, index: Option<usize>) -> _ {
        let data = match std::fs::read(path.as_str()) {
            Ok(it) => it,
            Err(_) => return Err(LuaError::RuntimeError(
                format!("unable to read font file: {}", path)
            ))
        };
        Ok(FontMgr::default().new_from_data(&data, index.unwrap_or_default())
            .map(LuaTypeface))
    }
});

#[derive(Clone, Copy)]
pub struct FromLuaFontWeight(pub i32);

impl FromLuaFontWeight {
    pub fn to_skia_weight(&self) -> Weight {
        Weight::from(self.0)
    }
}

impl<'lua> FromLua<'lua> for FromLuaFontWeight {
    fn from_lua(lua_value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        static EXPECTED: &'static str = "'invisible', 'thin', 'extra_light', 'light', 'normal', 'medium', 'semi_bold', 'bold', 'extra_bold', 'black', 'extra_black'";
        match lua_value {
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
            _ => Err(LuaError::RuntimeError(format!(
                "invalid font weight: '{:?}'; expected a number or name ({})",
                lua_value, EXPECTED
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

impl<'lua> FromLua<'lua> for FromLuaFontWidth {
    fn from_lua(lua_value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        static EXPECTED: &'static str = "'invisible', 'thin', 'extra_light', 'light', 'normal', 'medium', 'semi_bold', 'bold', 'extra_bold', 'black', 'extra_black'";
        match lua_value {
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
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "Width",
                message: Some(format!(
                    "invalid font width: '{:?}'; expected a number or name ({})",
                    lua_value, EXPECTED
                )),
            }),
        }
    }
}

wrap_skia_handle!(FontStyle);

impl UserData for LuaFontStyle {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("weight", |_, this, ()| Ok(*this.weight()));
        methods.add_method("width", |_, this, ()| Ok(*this.width()));
        methods.add_method("slant", |_, this, ()| Ok(LuaSlant(this.slant())));
    }
}

decl_func_constructor!(FontStyle: |ctx, weight: Option<FromLuaFontWeight>, width: Option<FromLuaFontWidth>, slant: Option<LuaSlant>| {
    let weight = weight.map(|it| it.to_skia_weight()).unwrap_or(Weight::NORMAL);
    let width = width.map(|it| it.to_skia_width()).unwrap_or(Width::NORMAL);
    let slant = slant.unwrap_or_t(Slant::Upright);
    Ok(LuaFontStyle(FontStyle::new(weight, width, slant)))
});

wrap_skia_handle!(Font);

impl UserData for LuaFont {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("countText", |_, this, text: LuaText| {
            Ok(this.count_text(text.text.as_bytes(), text.encoding))
        });
        methods.add_method(
            "getBounds",
            |_, this, (glyphs, paint): (Vec<GlyphId>, Option<LuaPaint>)| {
                let mut bounds = [Rect::new_empty()].repeat(glyphs.len());
                this.get_bounds(&glyphs, &mut bounds, paint.map(LuaPaint::unwrap).as_ref());
                let bounds: Vec<LuaRect> = bounds.into_iter().map(LuaRect::from).collect();
                Ok(bounds)
            },
        );
        methods.add_method("getEdging", |_, this, ()| Ok(LuaFontEdging(this.edging())));
        methods.add_method("getHinting", |_, this, ()| {
            Ok(LuaFontHinting(this.hinting()))
        });
        methods.add_method(
            "getIntercepts",
            |_,
             this,
             (glyphs, points, top, bottom, paint): (
                Vec<GlyphId>,
                Vec<LuaPoint>,
                f32,
                f32,
                Option<LuaPaint>,
            )| {
                let points: Vec<Point> = points.into_iter().map(|it| it.into()).collect();
                let paint = paint.map(|it| it.0);
                let intercepts =
                    this.get_intercepts(&glyphs, &points, (top, bottom), paint.as_ref());
                Ok(intercepts)
            },
        );
        methods.add_method("getMetrics", |ctx, this, ()| this.metrics().1.to_table(ctx));
        methods.add_method("getPath", |_, this, glyph: GlyphId| {
            Ok(this.get_path(glyph).map(LuaPath))
        });
        methods.add_method("getPaths", |_, this, glyphs: Vec<GlyphId>| {
            Ok(glyphs
                .into_iter()
                .filter_map(|it| this.get_path(it).map(LuaPath).map(|b| (it, b)))
                .collect::<HashMap<GlyphId, LuaPath>>())
        });
        methods.add_method(
            "getPos",
            |_, this, (glyphs, origin): (Vec<GlyphId>, Option<LuaPoint>)| {
                let mut points = [Point::new(0., 0.)].repeat(glyphs.len());
                let origin = origin.map(LuaPoint::into);
                this.get_pos(&glyphs, &mut points, origin);
                let points: Vec<_> = points.into_iter().map(LuaPoint::from).collect();
                Ok(points)
            },
        );
        methods.add_method("getScaleX", |_, this, ()| Ok(this.scale_x()));
        methods.add_method("getSize", |_, this, ()| Ok(this.size()));
        methods.add_method("getSkewX", |_, this, ()| Ok(this.skew_x()));
        methods.add_method("getSpacing", |_, this, ()| Ok(this.spacing()));
        methods.add_method("getTypeface", |_, this, ()| {
            Ok(this.typeface().map(LuaTypeface))
        });
        methods.add_method("getWidths", |_, this, glyphs: Vec<GlyphId>| {
            let mut widths = Vec::with_capacity(glyphs.len());
            this.get_widths(&glyphs, &mut widths);
            Ok(widths)
        });
        methods.add_method(
            "getWidthsBounds",
            |ctx, this, (glyphs, paint): (Vec<GlyphId>, Option<LuaPaint>)| {
                let mut widths = Vec::with_capacity(glyphs.len());
                let mut bounds = Vec::with_capacity(glyphs.len());
                this.get_widths_bounds(
                    &glyphs,
                    Some(&mut widths),
                    Some(&mut bounds),
                    paint.map(LuaPaint::unwrap).as_ref(),
                );
                let result = ctx.create_table()?;
                result.set("widths", widths)?;
                result.set(
                    "bounds",
                    bounds.into_iter().map(LuaRect::from).collect::<Vec<_>>(),
                )?;
                Ok(result)
            },
        );
        methods.add_method(
            "getXPos",
            |_, this, (glyphs, origin): (Vec<GlyphId>, Option<f32>)| {
                let mut result = Vec::with_capacity(glyphs.len());
                this.get_x_pos(&glyphs, &mut result, origin);
                Ok(result)
            },
        );
        methods.add_method("isBaselineSnap", |_, this, ()| Ok(this.is_baseline_snap()));
        methods.add_method("isEmbeddedBitmaps", |_, this, ()| {
            Ok(this.is_embedded_bitmaps())
        });
        methods.add_method("isEmbolden", |_, this, ()| Ok(this.is_embolden()));
        methods.add_method("isForceAutoHinting", |_, this, ()| {
            Ok(this.is_force_auto_hinting())
        });
        methods.add_method(
            "isLinearMetrics",
            |_, this, ()| Ok(this.is_linear_metrics()),
        );
        methods.add_method("isSubpixel", |_, this, ()| Ok(this.is_subpixel()));
        methods.add_method("makeWithSize", |_, this, size: f32| {
            Ok(this.with_size(size).map(LuaFont))
        });
        methods.add_method(
            "measureText",
            |_, this, (text, paint): (LuaText, Option<LuaPaint>)| {
                Ok(this
                    .measure_text(
                        text.text.as_bytes(),
                        text.encoding,
                        paint.map(LuaPaint::unwrap).as_ref(),
                    )
                    .0)
            },
        );
        methods.add_method_mut("setBaselineSnap", |_, this, baseline_snap: bool| {
            this.set_baseline_snap(baseline_snap);
            Ok(())
        });
        methods.add_method_mut("setEdging", |_, this, edging: LuaFontEdging| {
            this.set_edging(*edging);
            Ok(())
        });
        methods.add_method_mut("setEmbeddedBitmaps", |_, this, embedded_bitmaps: bool| {
            this.set_embedded_bitmaps(embedded_bitmaps);
            Ok(())
        });
        methods.add_method_mut("setEmbolden", |_, this, embolden: bool| {
            this.set_embolden(embolden);
            Ok(())
        });
        methods.add_method_mut(
            "setForceAutoHinting",
            |_, this, force_auto_hinting: bool| {
                this.set_force_auto_hinting(force_auto_hinting);
                Ok(())
            },
        );
        methods.add_method_mut("setHinting", |_, this, hinting: LuaFontHinting| {
            this.set_hinting(*hinting);
            Ok(())
        });
        methods.add_method_mut("setLinearMetrics", |_, this, linear_metrics: bool| {
            this.set_linear_metrics(linear_metrics);
            Ok(())
        });
        methods.add_method_mut("setScaleX", |_, this, scale: f32| {
            this.set_scale_x(scale);
            Ok(())
        });
        methods.add_method_mut("setSize", |_, this, size: f32| {
            this.set_size(size);
            Ok(())
        });
        methods.add_method_mut("setSkewX", |_, this, skew: f32| {
            this.set_skew_x(skew);
            Ok(())
        });
        methods.add_method_mut("setSubpixel", |_, this, subpixel: bool| {
            this.set_subpixel(subpixel);
            Ok(())
        });
        methods.add_method_mut("setTypeface", |_, this, typeface: LuaTypeface| {
            this.set_typeface(typeface.unwrap());
            Ok(())
        });
        methods.add_method("textToGlyphs", |_, this, text: LuaText| {
            this.text_to_glyphs_vec(text.text.as_bytes(), text.encoding);
            Ok(())
        });
        methods.add_method("unicharsToGlyphs", |_, this, unichars: Vec<Unichar>| {
            let mut result = Vec::with_capacity(unichars.len());
            this.unichar_to_glyphs(&unichars, &mut result);
            Ok(result)
        });
        methods.add_method("unicharToGlyph", |_, this, unichar: Unichar| {
            Ok(this.unichar_to_glyph(unichar))
        });
    }
}

decl_func_constructor!(Font: |ctx, typeface: LuaTypeface, size: Option<f32>, scale_x: Option<f32>, skew_x: Option<f32>| {
    let size = size.unwrap_or(12.0);
    let scale_x = scale_x.unwrap_or(1.0);
    let skew_x = skew_x.unwrap_or(0.0);
    Ok(LuaFont(Font::from_typeface_with_params(typeface, size, scale_x, skew_x)))
});

wrap_skia_handle!(TextBlob);

impl UserData for LuaTextBlob {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("bounds", |_, this, ()| Ok(LuaRect::from(*this.bounds())));
        methods.add_method(
            "getIntercepts",
            |_, this, (bounds, paint): (LuaPoint, Option<LikePaint>)| {
                let bounds = bounds.value;
                Ok(this.get_intercepts(bounds, paint.map(LikePaint::unwrap).as_ref()))
            },
        );
    }
}

decl_constructors!(TextBlob: {
    fn make_from_pos_text(text: LuaText, pos: Vec<LuaPoint>, font: LuaFont) -> _ {
        let pos: Vec<Point> = pos.into_iter().map(LuaPoint::into).collect();
        Ok(TextBlob::from_pos_text(text.text.as_bytes(), &pos, &font, text.encoding).map(LuaTextBlob))
    }
    fn make_from_pos_text_h(text: LuaText, x_pos: Vec<f32>, const_y: f32, font: LuaFont) -> _ {
        Ok(TextBlob::from_pos_text_h(text.text.as_bytes(), &x_pos, const_y, &font, text.encoding).map(LuaTextBlob))
    }
    // TODO: make_from_RSXform()
    fn make_from_string(string: String, font: LuaFont) -> _ {
        Ok(TextBlob::new(string, &font).map(LuaTextBlob))
    }
    fn make_from_text(text: LuaText, font: LuaFont) -> _ {
        Ok(TextBlob::from_text(text.text.as_bytes(), text.encoding, &font).map(LuaTextBlob))
    }
});

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
            result = result.paint(&paint);
        }
        if let Some(backdrop) = &self.backdrop {
            result = result.backdrop(&backdrop);
        }
        if !self.flags.is_empty() {
            result = result.flags(self.flags);
        }
        result
    }
}

impl<'lua> FromLua<'lua> for LuaSaveLayerRec {
    fn from_lua(lua_value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let mut result = LuaSaveLayerRec {
            bounds: None,
            paint: None,
            backdrop: None,
            flags: SaveLayerFlags::empty(),
        };
        let table = if let LuaValue::Table(table) = lua_value {
            table
        } else if let LuaNil = lua_value {
            return Ok(result);
        } else {
            return Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "SaveTableRec",
                message: Some("expected a SaveTableRec table or nil".to_string()),
            });
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
            result.backdrop = Some(table.get("backdrop")?)
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

                    // TODO: Investigate Surface-Canvas ownership
                    addr_of!(*surface).cast_mut().as_mut().unwrap_unchecked()
                };
                surface.canvas()
            }
            LuaCanvas::Borrowed(it) => &it,
        }
    }
}

impl<'a> UserData for LuaCanvas<'a> {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("clear", |_, this, (color,): (Option<LuaColor>,)| {
            let color = color
                .map(LuaColor::into)
                .unwrap_or(skia_safe::colors::TRANSPARENT);
            this.clear(color);
            Ok(())
        });
        methods.add_method(
            "drawColor",
            |_, this, (color, blend_mode): (LuaColor, Option<LuaBlendMode>)| {
                this.draw_color(color, blend_mode.map_t());
                Ok(())
            },
        );
        methods.add_method("drawPaint", |_, this, paint: LikePaint| {
            this.draw_paint(&paint);
            Ok(())
        });
        methods.add_method(
            "drawRect",
            |_, this, (rect, paint): (LuaRect, LikePaint)| {
                let rect: Rect = rect.into();
                this.draw_rect(rect, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawOval",
            |_, this, (oval, paint): (LuaRect, LikePaint)| {
                let oval: Rect = oval.into();
                this.draw_oval(oval, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawCircle",
            |_, this, (point, r, paint): (LuaPoint, f32, LikePaint)| {
                this.draw_circle(point, r, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawImage",
            |_, this, (image, point, paint): (LuaImage, LuaPoint, Option<LikePaint>)| {
                this.draw_image(image.unwrap(), point, paint.map(LikePaint::unwrap).as_ref());
                Ok(())
            },
        );
        methods.add_method(
            "drawImageRect",
            |_,
             this,
             (image, src_rect, dst_rect, paint): (
                LuaImage,
                Option<LuaRect>,
                LuaRect,
                Option<LikePaint>,
            )| {
                let paint: Paint = match paint {
                    Some(it) => it.unwrap(),
                    None => Paint::default(),
                };
                let src_rect = match src_rect {
                    Some(it) => Some(it.into()),
                    None => None,
                };
                let dst_rect: Rect = dst_rect.into();
                this.draw_image_rect(
                    image.unwrap(),
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
                Vec<LuaPoint>,
                Option<Vec<LuaColor>>,
                Option<Vec<LuaPoint>>,
                LuaBlendMode,
                LikePaint,
            )| {
                if cubics_table.len() != 12 {
                    return Err(LuaError::RuntimeError(
                        "expected 12 cubic points".to_string(),
                    ));
                }
                let mut cubics = [Point::new(0.0, 0.0); 12];
                for i in 0..12 {
                    cubics[i] = cubics_table[i].into();
                }

                let colors = match colors {
                    Some(colors) => {
                        let mut result = [Color::TRANSPARENT; 4];
                        for i in 0..4 {
                            result[i] = colors[i].into();
                        }
                        Some(result)
                    }
                    None => None,
                };

                let tex_coords = match tex_coords {
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

                this.draw_patch(
                    &cubics,
                    colors.as_ref(),
                    tex_coords.as_ref(),
                    *blend_mode,
                    &paint,
                );
                Ok(())
            },
        );
        methods.add_method(
            "drawPath",
            |_, this, (path, paint): (LuaPath, LikePaint)| {
                this.draw_path(&path, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawPicture",
            |_, this, (picture, matrix, paint): (LuaPicture, Option<LuaMatrix>, Option<LikePaint>)| {
                let matrix: Option<Matrix> = matrix.map(LuaMatrix::into);
                let paint: Option<Paint> = paint.map(LikePaint::unwrap);
                this.draw_picture(picture, matrix.as_ref(), paint.as_ref());
                Ok(())
            },
        );
        methods.add_method(
            "drawTextBlob",
            |_, this, (blob, point, paint): (LuaTextBlob, LuaPoint, LikePaint)| {
                this.draw_text_blob(blob.unwrap(), point, &paint);
                Ok(())
            },
        );
        methods.add_method("getSaveCount", |_, this, ()| Ok(this.save_count()));
        methods.add_method("getLocalToDevice", |_, this, ()| {
            Ok(LuaMatrix::Four(this.local_to_device()))
        });
        methods.add_method("getLocalToDevice3x3", |_, this, ()| {
            Ok(LuaMatrix::Three(this.local_to_device_as_3x3()))
        });
        methods.add_method("save", |_, this, ()| Ok(this.save()));
        methods.add_method("saveLayer", |_, this, save_layer_rec: LuaSaveLayerRec| {
            Ok(this.save_layer(&save_layer_rec.to_skia_save_layer_rec()))
        });
        methods.add_method("restore", |_, this, ()| {
            this.restore();
            Ok(())
        });
        methods.add_method("restoreToCount", |_, this, count: usize| {
            this.restore_to_count(count);
            Ok(())
        });
        methods.add_method("scale", |_, this, (sx, sy): (f32, Option<f32>)| {
            let sy = sy.unwrap_or(sx);
            this.scale((sx, sy));
            Ok(())
        });
        methods.add_method("translate", |_, this, point: LuaPoint| {
            this.translate(point);
            Ok(())
        });
        methods.add_method(
            "rotate",
            |_, this, (degrees, point): (f32, Option<LuaPoint>)| {
                let point = point.map(LuaPoint::into);
                this.rotate(degrees, point);
                Ok(())
            },
        );
        methods.add_method("concat", |_, this, matrix: LuaMatrix| {
            match matrix {
                LuaMatrix::Three(matrix) => this.concat(&matrix),
                LuaMatrix::Four(matrix) => this.concat_44(&matrix),
            };
            Ok(())
        });
        methods.add_method(
            "newSurface",
            |_, this, (info, props): (LikeImageInfo, Option<LikeSurfaceProps>)| {
                this.new_surface(&info, props.map(|it| *it).as_ref());
                Ok(())
            },
        );
        methods.add_method("width", |_, this, ()| Ok(this.base_layer_size().width));
        methods.add_method("height", |_, this, ()| Ok(this.base_layer_size().height));
    }
}

macro_rules! global_constructors {
    ($ctx: ident: $($t: ty),*) => {paste::paste!{
        $({
            let constructors = $ctx.create_userdata([<$t Constructors>])?;
            $ctx.globals().set(stringify!($t), constructors)?;
        })*
    }};
}

macro_rules! global_constructor_fns {
    ($ctx: ident: $($t: ty),*) => {paste::paste!{
        $(
            [<register_ $t:snake _constructor>]($ctx)?;
        )*
    }};
}

// TODO: methods.add_method("newPictureRecorder", |ctx, this, ()| Ok(()));
// TODO: filter conversion isn't automatic
#[allow(non_snake_case)]
pub fn setup<'lua>(ctx: LuaContext<'lua>) -> Result<(), rlua::Error> {
    global_constructors!(ctx:
        ColorFilters, ColorSpace, FontMgr, FontStyleSet, Image, ImageFilters,
        Matrix, Path, PathEffect, RRect, Surfaces, TextBlob, Typeface
    );
    global_constructor_fns!(ctx:
        Font, FontStyle, Paint, Path, RRect, StrokeRec
    );

    Ok(())
}
