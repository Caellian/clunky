use std::{
    alloc::Layout,
    default,
    mem::{align_of, size_of},
    sync::{Arc, OnceLock},
};

use phf::phf_map;
use rlua::{prelude::*, Context as LuaContext, Table as LuaTable, UserData, Variadic};
use skia_safe::{
    canvas::{self, SaveLayerFlags, SaveLayerRec},
    font_style::{Slant, Weight, Width},
    image_filter::MapDirection,
    image_filters::{self, CropRect},
    matrix::{ScaleToFit, TypeMask},
    paint::{Cap as PaintCap, Join as PaintJoin},
    path::{AddPathMode, ArcSize, SegmentMask, Verb},
    path_effect::DashInfo,
    stroke_rec::{InitStyle as StrokeRecInitStyle, Style as StrokeRecStyle},
    *,
};

use crate::{
    render::skia::ext::{m44_as_slice, m44_as_slice_mut, matrix_as_slice, matrix_as_slice_mut},
    util::hsl_to_rgb,
};

macro_rules! enum_naming {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        #[allow(unused)]
        static [<NAME_TO_ $kind:snake:upper>]: phf::Map<&'static str, $kind> = phf_map! {
            $($name => $value),
            +
        };

        #[inline]
        #[allow(unused)]
        fn [<read_ $kind:snake>](name: impl AsRef<str>) -> Result<$kind, LuaError> {
            const EXPECTED: OnceLock<String> = OnceLock::new();

            Ok(*[<NAME_TO_ $kind:snake:upper>]
                .get(name.as_ref().to_ascii_lowercase().as_str())
                .ok_or_else(|| LuaError::FromLuaConversionError {
                    from: "string",
                    to: stringify!($kind),
                    message: Some(format!(
                        concat!["unknown ", stringify!($kind), " name: '{}'; expected one of: {}"],
                        name.as_ref(),
                        EXPECTED.get_or_init(|| [
                            $(concat!("'", $name, "'")),+
                        ].join(", "))
                    ))
                })?)
        }

        #[allow(unreachable_patterns, unused)]
        const fn [<$kind:snake _name>](value: $kind) -> Option<&'static str> {
          Some(match value {
            $($value => $name,)
            +
            _ => return None,
          })
        }
    }};
}

enum_naming! { BlendMode: [
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

enum_naming! { PaintCap : [
    PaintCap::Butt => "butt",
    PaintCap::Round => "round",
    PaintCap::Square => "square",
]}

enum_naming! { PaintJoin : [
    PaintJoin::Miter => "miter",
    PaintJoin::Round => "round",
    PaintJoin::Bevel => "bevel",
]}

enum_naming! { Slant : [
    Slant::Upright => "upright",
    Slant::Italic => "italic",
    Slant::Oblique => "oblique",
]}

enum_naming! { ScaleToFit : [
    ScaleToFit::Fill => "fill",
    ScaleToFit::Start => "start",
    ScaleToFit::Center => "center",
    ScaleToFit::End => "end",
]}

enum_naming! { PathDirection : [
    PathDirection::CW => "cw",
    PathDirection::CCW => "ccw",
]}

enum_naming! { AddPathMode : [
    AddPathMode::Append => "append",
    AddPathMode::Extend => "extend",
]}

enum_naming! { ArcSize : [
    ArcSize::Small => "small",
    ArcSize::Large => "large",
]}

enum_naming! { Verb : [
    Verb::Move => "move",
    Verb::Line => "line",
    Verb::Quad => "quad",
    Verb::Conic => "conic",
    Verb::Cubic => "cubic",
    Verb::Close => "close",
    Verb::Done => "done",
]}

enum_naming! { PathFillType : [
    PathFillType::Winding => "winding",
    PathFillType::EvenOdd => "evenodd",
    PathFillType::InverseWinding => "inverse_winding",
    PathFillType::InverseEvenOdd => "inverse_evenodd",
]}

enum_naming! { MapDirection : [
    MapDirection::Forward => "forward",
    MapDirection::Reverse => "reverse",
]}

enum_naming! { StrokeRecInitStyle : [
    StrokeRecInitStyle::Hairline => "hairline",
    StrokeRecInitStyle::Fill => "fill",
]}

enum_naming! { StrokeRecStyle : [
    StrokeRecStyle::Hairline => "hairline",
    StrokeRecStyle::Fill => "fill",
    StrokeRecStyle::Stroke => "stroke",
    StrokeRecStyle::StrokeAndFill => "stroke_and_fill",
]}

enum_naming! { ColorType : [
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

enum_naming! { AlphaType : [
    AlphaType::Unknown => "unknown",
    AlphaType::Opaque => "opaque",
    AlphaType::Premul => "premul",
    AlphaType::Unpremul => "unpremul",
]}

enum_naming! { PixelGeometry: [
    PixelGeometry::Unknown => "unknown",
    PixelGeometry::RGBH => "rgbh",
    PixelGeometry::BGRH => "bgrh",
    PixelGeometry::RGBV => "rgbv",
    PixelGeometry::BGRV => "bgrv",
]}

macro_rules! named_bitflags {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        enum_naming! { $kind : [
            $($value => $name,)+
        ]}

        #[inline]
        pub fn [<read_ $kind:snake _table>](table: LuaTable) -> Result<$kind, LuaError> {
            let mut result = $kind::empty();
            for pair in table.pairs::<usize, String>() {
                if let Ok((_, name)) = pair {
                    result |= [<read_ $kind:snake>](name.as_str())?;
                } else {
                    return Err(LuaError::FromLuaConversionError {
                        from: "table",
                        to: stringify!($kind),
                        message: Some(concat!("expected ", stringify!($kind), " array to be an array of strings").to_string()),
                    });
                }
            }
            Ok(result)
        }

        pub fn [<write_ $kind:snake _table>](ctx: LuaContext, value: $kind) -> Result<LuaTable, LuaError> {
            let result = ctx.create_table()?;
            let mut i: usize = 0;
            for entry in [$($value),+] {
                if value.contains(entry) {
                    result.set(i, [<$kind:snake _name>](entry))?;
                    i += 1;
                }
            }
            Ok(result)
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
                let r = color.get(0).map_err(|_| unknown_format())?;
                let g = color.get(1).map_err(|_| unknown_format())?;
                let b = color.get(2).map_err(|_| unknown_format())?;
                let a = color.get(3).unwrap_or(1.);
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
        lua: LuaContext<'lua>,
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
                .pairs::<usize, f32>()
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
            for (i, value) in value.iter_mut().enumerate() {
                *value = table.get(i).map_err(bad_table_entries::<N>)?;
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
impl From<Point3> for LuaPoint<3> {
    #[inline]
    fn from(value: Point3) -> Self {
        LuaPoint {
            value: [value.x, value.y, value.z],
        }
    }
}
impl Into<Point3> for LuaPoint {
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
                .pairs::<usize, f32>()
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
            for (i, value) in value.iter_mut().enumerate() {
                *value = table.get(i).map_err(bad_table_entries::<N>)?;
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
            impl AsRef<$handle> for [<Lua $handle>] {
                fn as_ref(&self) -> &$handle {
                    &self.0
                }
            }
        }
    };
}

macro_rules! type_like_table {
    ($handle: ty: |$ident: ident: LuaTable, $ctx: ident: LuaContext| $body: block) => {
        paste::paste! {
            impl<'lua> TryFrom<(LuaTable<'lua>, LuaContext<'lua>)> for [<Lua $handle>] {
                type Error = LuaError;

                fn try_from(($ident, $ctx): (LuaTable<'lua>, LuaContext<'lua>)) -> Result<Self, Self::Error> $body
            }
            #[derive(Clone)]
            pub struct [<Like $handle>]([<Lua $handle>]);
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

wrap_skia_handle!(Shader);

impl UserData for LuaShader {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isOpaque", |_, this, ()| Ok(this.is_opaque()));
        methods.add_method("isAImage", |_, this, ()| Ok(this.is_a_image()));
    }
}

wrap_skia_handle!(Image);

impl UserData for LuaImage {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("width", |_, this, ()| Ok(this.width()));
        methods.add_method("height", |_, this, ()| Ok(this.height()));
        methods.add_method("newShader", |_, this, ()| {
            this.to_shader(
                Some((TileMode::Clamp, TileMode::Clamp)),
                SamplingOptions::default(),
                None,
            )
            .map(LuaShader)
            .ok_or(LuaError::RuntimeError(
                "can't create shader from image".to_string(),
            ))
        });
    }
}

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
                String,
                Option<LuaRect>,
            )| {
                let src: IRect = src.into();
                let ctm: Matrix = ctm.into();
                let map_direction = read_map_direction(map_direction)?;
                let input_rect = input_rect.map(Into::<IRect>::into);
                let filtered = this.filter_bounds(src, &ctm, map_direction, input_rect.as_ref());
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
        })
    }
}

wrap_skia_handle!(ColorFilter);

impl UserData for LuaColorFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("toAColorMode", |ctx, this, ()| {
            if let Some((color, mode)) = this.to_a_color_mode() {
                let result = ctx.create_table()?;
                result.set(0, LuaColor::from(color))?;
                result.set(1, blend_mode_name(mode))?;
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
                            dst_cs.map(|it| it.0).as_ref(),
                        )))
                    }
                }
            },
        );
        methods.add_method("makeComposed", |_, this, inner: LuaColorFilter| {
            Ok(LuaColorFilter(this.composed(inner.0).ok_or(
                LuaError::RuntimeError("unable to compose filters".to_string()),
            )?))
        });
        methods.add_method(
            "makeWithWorkingColorSpace",
            |_, this, color_space: LuaColorSpace| {
                Ok(LuaColorFilter(
                    this.with_working_color_space(color_space.0)
                        .ok_or(LuaError::RuntimeError(
                            "unable to apply color space to filter".to_string(),
                        ))?,
                ))
            },
        );
    }
}

wrap_skia_handle!(MaskFilter);

impl UserData for LuaMaskFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("approximateFilteredBounds", |_, this, src: LuaRect| {
            let src: Rect = src.into();
            Ok(LuaRect::from(this.approximate_filtered_bounds(src)))
        });
    }
}

wrap_skia_handle!(DashInfo);

impl UserData for LuaDashInfo {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getIntervals", |_, this, ()| Ok(this.intervals.clone()));
        methods.add_method("getPhase", |_, this, ()| Ok(this.phase));
    }
}

wrap_skia_handle!(StrokeRec);

impl UserData for LuaStrokeRec {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getStyle", |_, this, ()| {
            Ok(stroke_rec_style_name(this.style()))
        });
        methods.add_method("getWidth", |_, this, ()| Ok(this.width()));
        methods.add_method("getMiter", |_, this, ()| Ok(this.miter()));
        methods.add_method("getCap", |_, this, ()| Ok(paint_cap_name(this.cap())));
        methods.add_method("getJoin", |_, this, ()| Ok(paint_join_name(this.join())));
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
            |_, this, (cap, join, miter_limit): (String, String, f32)| {
                this.set_stroke_params(read_paint_cap(cap)?, read_paint_join(join)?, miter_limit);
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
                let mut dst = src.0.clone();
                let mut stroke_rec = stroke_rec.0;
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
                let m = matrix_as_slice(&other);
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
                        let it = matrix_as_slice(it);
                        it[i].to_lua(ctx)
                    } else {
                        Ok(LuaNil)
                    }
                }
                LuaMatrix::Four(it) => {
                    let i = col + row * 4;
                    if i < 16 && col < 4 {
                        let it = m44_as_slice(it);
                        it[i].to_lua(ctx)
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
                        let it = matrix_as_slice_mut(it);
                        it[i] = value;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
                LuaMatrix::Four(it) => {
                    let i = col + row * 4;
                    if i < 16 && col < 4 {
                        let it = m44_as_slice_mut(it);
                        it[i] = value;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
            }
        });
        methods.add_method("getType", |ctx, this, ()| match this {
            LuaMatrix::Three(it) => write_type_mask_table(ctx, it.get_type()).map(LuaValue::Table),
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
                    m44_as_slice_mut(it)[0] = value;
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
                    m44_as_slice_mut(it)[5] = value;
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
                m44_as_slice_mut(it)[10] = value;
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
                    m44_as_slice_mut(it)[3] = value;
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
                    m44_as_slice_mut(it)[7] = value;
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
                m44_as_slice_mut(it)[11] = value;
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
                    m44_as_slice_mut(it)[1] = value;
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
                    m44_as_slice_mut(it)[4] = value;
                }
            }
            Ok(())
        });
        methods.add_method_mut(
            "setRectToRect",
            |_, this, (from, to, stf): (LuaRect, LuaRect, String)| {
                let from: Rect = from.into();
                let to: Rect = to.into();
                let stf = read_scale_to_fit(stf)?;
                Ok(match this {
                    LuaMatrix::Three(it) => it.set_rect_to_rect(from, to, stf),
                    #[rustfmt::skip]
                    LuaMatrix::Four(it) => {
                        let mut mat = Matrix::new_identity();
                        let result = mat.set_rect_to_rect(from, to, stf);
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

wrap_skia_handle!(Paint);

type_like_table!(Paint: |value: LuaTable, ctx: LuaContext| {
    if let Ok(color) = LuaColor::from_lua(LuaValue::Table(value), ctx) {
        let color: Color4f = color.into();
        let result = Paint::new(color, None);
        return Ok(LuaPaint(result))
    }

    // TODO: Add nested Color->Paint constructor that takes in ColorSpace

    Err(LuaError::ToLuaConversionError {
        from: "table",
        to: "Paint",
        message: Some("expected a Paint or Paint-like Table".to_string())
    })
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
                this.set_image_filter(image_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getMaskFilter", |_, this, ()| {
            Ok(this.mask_filter().map(LuaMaskFilter))
        });
        methods.add_method_mut(
            "setMaskFilter",
            |_, this, mask_filter: Option<LuaMaskFilter>| {
                this.set_mask_filter(mask_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getColorFilter", |_, this, ()| {
            Ok(this.color_filter().map(LuaColorFilter))
        });
        methods.add_method_mut(
            "setColorFilter",
            |_, this, color_filter: Option<LuaColorFilter>| {
                this.set_color_filter(color_filter.map(|it| it.0));
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
                this.set_color4f(color, color_space.map(|it| it.0).as_ref());
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
            Ok(paint_cap_name(this.stroke_cap()))
        });
        methods.add_method_mut("setStrokeCap", |_, this, cap: String| {
            this.set_stroke_cap(read_paint_cap(cap)?);
            Ok(())
        });
        methods.add_method("getStrokeJoin", |_, this, ()| {
            Ok(paint_join_name(this.stroke_join()))
        });
        methods.add_method_mut("setStrokeJoin", |_, this, join: String| {
            this.set_stroke_join(read_paint_join(join)?);
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
            this.set_path_effect(effect.map(|it| it.0));
            Ok(())
        });
        methods.add_method("getColorFilter", |_, this, ()| {
            Ok(this.color_filter().map(LuaColorFilter))
        });
        methods.add_method_mut(
            "setColorFilter",
            |_, this, color_filter: Option<LuaColorFilter>| {
                this.set_color_filter(color_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method("getImageFilter", |_, this, ()| {
            Ok(this.image_filter().map(LuaImageFilter))
        });
        methods.add_method_mut(
            "setImageFilter",
            |_, this, image_filter: Option<LuaImageFilter>| {
                this.set_image_filter(image_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method("getShader", |_, this, ()| Ok(this.shader().map(LuaShader)));
        methods.add_method_mut("setShader", |_, this, shader: Option<LuaShader>| {
            this.set_shader(shader.map(|it| it.0));
            Ok(())
        });
        methods.add_method("getPathEffect", |_, this, ()| {
            Ok(this.path_effect().map(LuaPathEffect))
        });
        methods.add_method_mut(
            "setPathEffect",
            |_, this, path_effect: Option<LuaPathEffect>| {
                this.set_path_effect(path_effect.map(|it| it.0));
                Ok(())
            },
        );
    }
}

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
            |_, this, (point, radius, dir): (LuaPoint, f32, Option<String>)| {
                let dir = match dir {
                    Some(it) => Some(read_path_direction(it)?),
                    None => None,
                };
                this.add_circle(point, radius, dir);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addOval",
            |_, this, (oval, dir, start): (LuaRect, Option<String>, Option<usize>)| {
                let oval: Rect = oval.into();
                let dir = match dir {
                    Some(it) => read_path_direction(it)?,
                    None => PathDirection::CW,
                };
                let start = start.unwrap_or(1);
                this.add_oval(oval, Some((dir, start)));
                Ok(())
            },
        );
        methods.add_method_mut(
            "addPath",
            |_, this, (other, point, mode): (LuaPath, LuaPoint, Option<String>)| {
                let mode = match mode {
                    Some(it) => Some(read_add_path_mode(it)?),
                    None => None,
                };
                this.add_path(&other, point, mode);
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
            |_, this, (rect, dir, start): (LuaRect, Option<String>, Option<usize>)| {
                let rect: Rect = rect.into();
                let dir = match dir {
                    Some(it) => read_path_direction(it)?,
                    None => PathDirection::CW,
                };
                let start = start.unwrap_or(1);
                this.add_rect(rect, Some((dir, start)));
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRoundRect",
            |_, this, (rect, rounding, dir): (LuaRect, LuaPoint, Option<String>)| {
                let rect: Rect = rect.into();
                let dir = match dir {
                    Some(it) => read_path_direction(it)?,
                    None => PathDirection::CW,
                };
                this.add_round_rect(rect, (rounding.x(), rounding.y()), dir);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRRect",
            |_, this, (rrect, dir, start): (LuaRRect, Option<String>, Option<usize>)| {
                let dir = match dir {
                    Some(it) => read_path_direction(it)?,
                    None => PathDirection::CW,
                };
                let start = start.unwrap_or(1);
                this.add_rrect(rrect.0, Some((dir, start)));
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
            Ok(path_fill_type_name(this.fill_type()))
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
            let masks = write_segment_mask_table(ctx, this.segment_masks())?;
            Ok(masks)
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
                result.set(
                    i,
                    match verb_name(*verb) {
                        Some(it) => it.to_lua(ctx)?,
                        None => LuaNil,
                    },
                )?;
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
        methods.add_method_mut("rArcTo", |_, this, (r, x_axis_rotate, arc_size, sweep, d): (LuaPoint, f32, String, String, LuaPoint)| {
            let arc_size = read_arc_size(arc_size)?;
            let sweep = read_path_direction(sweep)?;
            this.r_arc_to_rotated(r, x_axis_rotate, arc_size, sweep, d);
            Ok(())
        });
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
        methods.add_method_mut("setFillType", |_, this, fill_type: String| {
            this.set_fill_type(read_path_fill_type(fill_type)?);
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

wrap_skia_handle!(RRect);

impl UserData for LuaRRect {
    // TODO: https://api.skia.org/classSkRRect.html
}

wrap_skia_handle!(ImageInfo);
impl UserData for LuaImageInfo {
    // TODO: https://api.skia.org/classSkImageInfo.html
}

type_like_table!(ImageInfo: |value: LuaTable| {
    let dimensions: LuaSize = LuaSize::try_from(value.get::<_, LuaTable>("dimensions")?)?;
    let color_type = read_color_type(
        value
            .get::<_, String>("color_type")
            .unwrap_or("unknown".to_string()),
    )?;
    let alpha_type = read_alpha_type(
        value
            .get::<_, String>("alpha_type")
            .unwrap_or("unknown".to_string()),
    )?;
    let color_space = value
        .get::<_, LuaColorSpace>("color_space")
        .ok()
        .map(|it| it.0);

    let result = ImageInfo::new(dimensions, color_type, alpha_type, color_space);

    Ok(LuaImageInfo(result))
});

wrap_skia_handle!(SurfaceProps);
impl UserData for LuaSurfaceProps {
    // TODO: https://api.skia.org/classSkSurfaceProps.html
}

type_like_table!(SurfaceProps: |value: LuaTable| {
    let flags = match value.get::<_, LuaTable>("flags") {
        Ok(it) => read_surface_props_flags_table(it)?,
        Err(LuaError::FromLuaConversionError { from: "nil", .. }) => {
            SurfacePropsFlags::empty()
        }
        Err(other) => return Err(other)
    };
    let pixel_geometry = read_pixel_geometry(value.get::<_, String>("pixel_geometry").unwrap_or("unknown".to_string()))?;

    Ok(LuaSurfaceProps(SurfaceProps::new(flags, pixel_geometry)))
});

wrap_skia_handle!(Surface);

impl UserData for LuaSurface {
    // TODO: https://api.skia.org/classSkSurface.html
}

unsafe impl Send for LuaSurface {}

wrap_skia_handle!(Typeface);

impl UserData for LuaTypeface {
    // TODO: https://api.skia.org/classSkTypeface.html
}

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
        methods.add_method("slant", |_, this, ()| Ok(slant_name(this.slant())));
    }
}

wrap_skia_handle!(Font);

impl UserData for LuaFont {
    // TODO: https://api.skia.org/classSkFont.html
}

wrap_skia_handle!(TextBlob);

impl UserData for LuaTextBlob {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("bounds", |_, this, ()| Ok(LuaRect::from(*this.bounds())));
        methods.add_method(
            "getIntercepts",
            |_, this, (bounds, paint): (LuaPoint, Option<LikePaint>)| {
                let bounds = bounds.value;
                Ok(this.get_intercepts(bounds, paint.map(|it| it.0 .0).as_ref()))
            },
        );
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
                    result.flags = read_save_layer_flags(flag.to_str()?)?;
                }
                LuaValue::Table(list) => {
                    result.flags = read_save_layer_flags_table(list)?;
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
pub struct LuaCanvas<'a>(pub &'a Canvas);

unsafe impl<'a> Send for LuaCanvas<'a> {}

impl<'a> std::ops::Deref for LuaCanvas<'a> {
    type Target = &'a Canvas;
    fn deref(&self) -> &Self::Target {
        &self.0
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
            |_, this, (color, blend_mode): (LuaColor, Option<String>)| {
                let mode = match blend_mode {
                    Some(it) => Some(read_blend_mode(it)?),
                    None => None,
                };
                this.draw_color(color, mode);
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
                this.draw_image(image.0, point, paint.map(|it| it.0 .0).as_ref());
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
                    Some(it) => it.0 .0,
                    None => Paint::default(),
                };
                let src_rect = match src_rect {
                    Some(it) => Some(it.into()),
                    None => None,
                };
                let dst_rect: Rect = dst_rect.into();
                this.draw_image_rect(
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
                LikePaint,
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
                            result[i] = colors.get::<usize, LuaColor>(i)?.into();
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
                this.draw_patch(&cubics, colors.as_ref(), tex_coords.as_ref(), mode, &paint);
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
        //TODO: methods.add_method("drawPicture", |_, this, ()| Ok(()));
        methods.add_method(
            "drawTextBlob",
            |_, this, (blob, point, paint): (LuaTextBlob, LuaPoint, LikePaint)| {
                this.draw_text_blob(blob.0, point, &paint);
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
    }
}

struct LuaGfx;
impl UserData for LuaGfx {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("loadImage", |_, _, name: String| {
            let handle: Data = Data::new_copy(
                &std::fs::read(name)
                    .map_err(|io_err| rlua::Error::RuntimeError(io_err.to_string()))?,
            );
            Image::from_encoded(handle)
                .map(LuaImage)
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
                    .map(LuaImageFilter)
            },
        );
        //TODO: methods.add_method("newLinearGradient", |ctx, this, ()| Ok(()));
        methods.add_method("newMatrix", |_, _, size| match size {
            None | Some(3) => Ok(LuaMatrix::Three(Matrix::new_identity())),
            Some(4) => Ok(LuaMatrix::Four(M44::new_identity())),
            Some(_) => Err(LuaError::RuntimeError(
                "unsupported matrix size; supported sizes are: 3, 4".to_string(),
            )),
        });
        methods.add_method("newPaint", |_, _, ()| Ok(LuaPaint(Paint::default())));
        methods.add_method("newPath", |_, _, ()| Ok(LuaPath(Path::new())));
        //TODO: methods.add_method("newPictureRecorder", |ctx, this, ()| Ok(()));
        methods.add_method("newRRect", |_, _, ()| Ok(LuaRRect(RRect::new())));
        methods.add_method("newRasterSurface", |_, _, (info, row_bytes, props): (LikeImageInfo, Option<usize>, Option<LikeSurfaceProps>)| {
            Ok(surfaces::raster(
                &info,
                row_bytes,
                props.map(|it| *it).as_ref(),
            ).map(LuaSurface))
        });
        methods.add_method("newRasterSurfaceN32Premul", |_, _, size: LuaSize| {
            Ok(surfaces::raster_n32_premul(size).map(LuaSurface))
        });
        methods.add_method("newTextBlob", |_, _, (text, font): (String, LuaFont)| {
            Ok(TextBlob::new(text, &font).map(LuaTextBlob))
        });
        methods.add_method(
            "newFontStyle",
            |_,
             _,
             (weight, width, slant): (
                FromLuaFontWeight,
                FromLuaFontWidth,
                Option<String>,
            )| {
                let slant = match slant {
                    Some(it) => read_slant(it)?,
                    None => Slant::Upright,
                };
                Ok(LuaFontStyle(FontStyle::new(weight.to_skia_weight(), width.to_skia_width(), slant)))
            },
        );
        methods.add_method(
            "newTypeface",
            |_, _, (family_name, font_style): (String, Option<LuaFontStyle>)| {
                Ok(
                    Typeface::new(family_name, font_style.map(|it| it.0).unwrap_or_default())
                        .map(LuaTypeface),
                )
            },
        );
        methods.add_method(
            "newFont",
            |_, _, (typeface, size): (LuaTypeface, Option<f32>)| {
                Ok(LuaFont(Font::new(typeface.0, size)))
            },
        );
        methods.add_method("newColorSpace", |_, _, name: String| match name.as_ref() {
            "srgb" => Ok(LuaColorSpace(ColorSpace::new_srgb())),
            "srgb_linear" | "srgb-linear" => Ok(LuaColorSpace(ColorSpace::new_srgb_linear())),
            other => Err(LuaError::RuntimeError(format!(
                "unknown color space: {}",
                other
            ))),
        });
        methods.add_method(
            "newDashInfo",
            |_, _, (intervals, phase): (Vec<f32>, Option<f32>)| {
                Ok(LuaDashInfo(DashInfo {
                    intervals,
                    phase: phase.unwrap_or_default(),
                }))
            },
        );
        methods.add_method("newStrokeRec", |_, _, init_style: String| {
            Ok(LuaStrokeRec(StrokeRec::new(read_stroke_rec_init_style(
                init_style,
            )?)))
        });
    }
}

#[allow(non_snake_case)]
pub fn setup<'lua>(ctx: LuaContext<'lua>) -> Result<(), rlua::Error> {
    let gfx = ctx.create_userdata(LuaGfx)?;
    ctx.globals().set("Gfx", gfx)?;
    Ok(())
}
