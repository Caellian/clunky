use std::str::FromStr;
use std::sync::OnceLock;

use mlua::prelude::*;
use phf::phf_map;

use skia_safe::{
    canvas::SaveLayerFlags,
    font::Edging as FontEdging,
    font_style::Slant,
    gradient_shader::interpolation::{ColorSpace as InColorSpace, HueMethod, InPremul},
    image_filter::MapDirection,
    matrix::{ScaleToFit, TypeMask},
    paint::{Cap as PaintCap, Join as PaintJoin, Style as PaintStyle},
    path::{AddPathMode, ArcSize, SegmentMask, Verb},
    rrect::{Corner as RRectCorner, Type as RRectType},
    stroke_rec::{InitStyle as StrokeRecInitStyle, Style as StrokeRecStyle},
    trim_path_effect::Mode as TrimMode,
    *,
};

use crate::{FromArgPack, WrapperT};

macro_rules! named_enum {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct [<Lua $kind>](pub $kind);

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

        impl<'lua> $crate::lua::WrapperT<'lua> for [<Lua $kind>] {
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
            fn from_lua(text: LuaValue<'lua>, _: &'lua Lua) -> LuaResult<Self> {
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
        $crate::from_lua_argpack!([<Lua $kind>]);

        impl<'lua> IntoLua<'lua> for [<Lua $kind>] {
            #[allow(unreachable_patterns)]
            fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
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
impl FromStr for LuaInPremul {
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
impl TryFrom<String> for LuaInPremul {
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
impl<'lua> FromArgPack<'lua> for LuaInPremul {
    fn convert(args: &mut crate::ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        let text = match args.pop() {
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
impl<'lua> IntoLua<'lua> for LuaInPremul {
    #[allow(unreachable_patterns)]
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
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
            pub fn from_table(table: LuaTable) -> Result<Self, LuaError> {
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

            pub fn to_table<'lua>(&self, ctx: &'lua Lua) -> Result<LuaTable<'lua>, LuaError> {
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
impl FromStr for LuaPaintStyle {
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
impl TryFrom<String> for LuaPaintStyle {
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
impl<'lua> FromArgPack<'lua> for LuaPaintStyle {
    fn convert(args: &mut crate::ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        let text = match args.pop() {
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

impl<'lua> IntoLua<'lua> for LuaPaintStyle {
    #[allow(unreachable_patterns)]
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        lua.create_string(match self.0 {
            PaintStyle::Fill => "fill",
            PaintStyle::Stroke => "stroke",
            PaintStyle::StrokeAndFill => "stroke_and_fill",
            _ => return Ok(LuaNil),
        })
        .map(LuaValue::String)
    }
}
