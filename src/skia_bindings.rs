use std::{
    alloc::Layout,
    mem::{align_of, size_of},
    sync::OnceLock,
};

use phf::phf_map;
use rlua::{prelude::*, Context as LuaContext, Table as LuaTable, UserData, Variadic};
use skia_safe::{
    canvas::{self, SaveLayerFlags, SaveLayerRec},
    font_style::{Slant, Weight, Width},
    image_filters::{self, CropRect},
    matrix::{ScaleToFit, TypeMask},
    paint::{Cap as PaintCap, Join as PaintJoin},
    path::{AddPathMode, ArcSize, SegmentMask, Verb},
    *,
};

use crate::render::skia::ext::m44_as_slice_mut;

#[inline]
fn read_color_table(table: LuaTable) -> Color4f {
    let r = table.get("r").unwrap_or_default();
    let g = table.get("g").unwrap_or_default();
    let b = table.get("b").unwrap_or_default();
    let a = table.get("a").unwrap_or(1.0);
    Color4f { r, g, b, a }
}

#[inline]
fn write_color_table<'lua>(
    color: Color,
    ctx: LuaContext<'lua>,
) -> Result<LuaTable<'lua>, LuaError> {
    let result = ctx.create_table()?;
    let rgb = color.to_rgb();
    result.set("r", rgb.r / u8::MAX)?;
    result.set("g", rgb.g / u8::MAX)?;
    result.set("b", rgb.b / u8::MAX)?;
    result.set("a", color.a() / u8::MAX)?;
    Ok(result)
}

#[inline]
fn write_color4f_table<'lua>(
    color: Color4f,
    ctx: LuaContext<'lua>,
) -> Result<LuaTable<'lua>, LuaError> {
    let result = ctx.create_table()?;
    result.set("r", color.r)?;
    result.set("g", color.g)?;
    result.set("b", color.b)?;
    result.set("a", color.a)?;
    Ok(result)
}

#[inline]
fn read_rect_table(table: LuaTable) -> Result<Rect, LuaError> {
    let left = table.get("left").unwrap_or_default();
    let top = table.get("top").unwrap_or_default();
    let right = table
        .get("right")
        .map_err(|_| LuaError::FromLuaConversionError {
            from: "table",
            to: "Rect",
            message: Some("Rect table missing 'right' field".to_string()),
        })?;
    let bottom = table
        .get("bottom")
        .map_err(|_| LuaError::FromLuaConversionError {
            from: "table",
            to: "Rect",
            message: Some("Rect table missing 'bottom' field".to_string()),
        })?;
    Ok(Rect {
        left,
        top,
        right,
        bottom,
    })
}

#[inline]
fn write_rect_table<'lua>(rect: Rect, ctx: LuaContext<'lua>) -> Result<LuaTable<'lua>, LuaError> {
    let result = ctx.create_table()?;
    result.set("top", rect.top)?;
    result.set("left", rect.left)?;
    result.set("right", rect.right)?;
    result.set("bottom", rect.bottom)?;
    Ok(result)
}

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

#[derive(Clone, Copy)]
pub struct LuaPoint<const N: usize = 2> {
    value: [f32; N],
}

const COORD_NAME: &[&'static str] = &["x", "y", "z", "w"];

impl<const N: usize> LuaPoint<N> {
    #[inline(always)]
    pub fn into_skia_point(self) -> Point {
        Point::new(self.value[0], self.value[1])
    }

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
    fn from(value: Point) -> Self {
        LuaPoint {
            value: [value.x, value.y],
        }
    }
}

impl From<Point3> for LuaPoint<3> {
    fn from(value: Point3) -> Self {
        LuaPoint {
            value: [value.x, value.y, value.z],
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
                message: Some(
                    "Point value expects either an array with 2 values or 2 number values"
                        .to_string(),
                ),
            });
        }
        let mut values = values.into_iter();
        let a = values.next();

        #[inline(always)]
        fn bad_table_entries<const N: usize>(_: LuaError) -> LuaError {
            LuaError::FromLuaConversionError {
                from: "table",
                to: "Point",
                message: Some(format!(
                    "Point table requires '{}' number entries",
                    COORD_NAME[0..N].join("', '")
                )),
            }
        }

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

        match a {
            Some(LuaValue::Table(table))
                if COORD_NAME[0..N]
                    .iter()
                    .all(|it| table.contains_key(*it).ok() == Some(true)) =>
            {
                let mut value = [0.0; N];
                for (i, coord) in COORD_NAME[0..N].iter().enumerate() {
                    value[i] = table.get(*coord).map_err(bad_table_entries::<N>)?;
                }
                *consumed += 1;
                Ok(LuaPoint { value })
            }
            Some(LuaValue::Table(table)) => {
                if table.len()? != N as i64 {
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
                *consumed += 1;
                Ok(LuaPoint { value })
            }
            Some(LuaValue::Number(x)) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaPoint { value })
            }
            Some(LuaValue::Integer(x)) => {
                let mut value = [x as f32; N];
                for i in 1..N {
                    value[i] = read_coord::<N>(values.next())?;
                }
                *consumed += N;
                Ok(LuaPoint { value })
            }
            Some(other) => {
                log::debug!("{:?}", other);
                Err(LuaError::FromLuaConversionError {
                    from: other.type_name(),
                    to: "Point",
                    message: Some(
                        "Point value expects either an array with 2 values or 2 number values"
                            .to_string(),
                    ),
                })
            }
            None => Err(LuaError::FromLuaConversionError {
                from: "()",
                to: "Point",
                message: Some(
                    "Point value expects either an array with 2 values or 2 number values"
                        .to_string(),
                ),
            }),
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
pub struct LuaLine<const N: usize> {
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

        }
    };
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

impl UserData for LuaColorSpace {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(ImageFilter);

impl UserData for LuaImageFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(ColorFilter);

impl UserData for LuaColorFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("toAColorMode", |ctx, this, ()| {
            if let Some((color, mode)) = this.to_a_color_mode() {
                let result = ctx.create_table()?;
                result.set(0, write_color_table(color, ctx)?)?;
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
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(PathEffect);

impl UserData for LuaPathEffect {}

#[derive(Clone)]
pub enum LuaMatrix {
    Three(Matrix),
    Four(M44),
}

impl UserData for LuaMatrix {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getDimensions", |_, this, ()| match this {
            LuaMatrix::Three(_) => Ok(3),
            LuaMatrix::Four(_) => Ok(4),
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
        methods.add_method_mut("setScaleZ", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(_) => {
                    return Err(LuaError::RuntimeError(
                        "3x3 matrix doesn't have a Z scale component".to_string(),
                    ))
                }
                LuaMatrix::Four(it) => {
                    m44_as_slice_mut(it)[10] = value;
                }
            }
            Ok(())
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
        methods.add_method_mut("setTranslateZ", |_, this, value: f32| {
            match this {
                LuaMatrix::Three(_) => {
                    return Err(LuaError::RuntimeError(
                        "3x3 matrix doesn't have a Z translation component".to_string(),
                    ))
                }
                LuaMatrix::Four(it) => {
                    m44_as_slice_mut(it)[11] = value;
                }
            }
            Ok(())
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
            |_, this, (from, to, stf): (LuaTable, LuaTable, String)| {
                let from = read_rect_table(from)?;
                let to = read_rect_table(to)?;
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
        methods.add_method("invert", |ctx, this, ()| {
            let inverse = match this {
                LuaMatrix::Three(mx) => mx.invert().map(LuaMatrix::Three),
                LuaMatrix::Four(mx) => mx.invert().map(LuaMatrix::Four),
            };
            match inverse {
                Some(it) => it.to_lua(ctx),
                None => Ok(LuaNil),
            }
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
        methods.add_method("mapRect", |ctx, this, rect: LuaTable| {
            let rect = read_rect_table(rect)?;
            let rect = match this {
                LuaMatrix::Three(it) => it.map_rect(rect).0,
                LuaMatrix::Four(it) => {
                    let a = it.map(rect.left, rect.top, 0.0, 1.0);
                    let b = it.map(rect.right, rect.bottom, 0.0, 1.0);
                    Rect::new(a.x, a.y, b.x, b.y)
                }
            };
            write_rect_table(rect, ctx)
        });
    }
}

wrap_skia_handle!(Paint);

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
        methods.add_method_mut("getImageFilter", |ctx, this, ()| {
            match this.image_filter() {
                Some(it) => Ok(LuaImageFilter(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            }
        });
        methods.add_method_mut(
            "setImageFilter",
            |_, this, image_filter: Option<LuaImageFilter>| {
                this.set_image_filter(image_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getMaskFilter", |ctx, this, ()| match this.mask_filter() {
            Some(it) => Ok(LuaMaskFilter(it).to_lua(ctx)?),
            None => Ok(LuaNil),
        });
        methods.add_method_mut(
            "setMaskFilter",
            |_, this, mask_filter: Option<LuaMaskFilter>| {
                this.set_mask_filter(mask_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getColorFilter", |ctx, this, ()| {
            match this.color_filter() {
                Some(it) => Ok(LuaColorFilter(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            }
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
        methods.add_method("getColor", |ctx, this, ()| {
            write_color4f_table(this.color4f(), ctx)
        });
        methods.add_method_mut(
            "setColor",
            |_, this, (color, color_space): (LuaTable, Option<LuaColorSpace>)| {
                let color = read_color_table(color);
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
            |_, this, (oval, start_angle, sweep_angle): (LuaTable, f32, f32)| {
                let oval = read_rect_table(oval)?;
                this.add_arc(oval, start_angle, sweep_angle);
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
                this.add_circle(point.into_skia_point(), radius, dir);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addOval",
            |_, this, (oval, dir, start): (LuaTable, Option<String>, Option<usize>)| {
                let oval = read_rect_table(oval)?;
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
                this.add_path(&other, point.into_skia_point(), mode);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addPoly",
            |_, this, (points, close): (Variadic<LuaPoint>, bool)| {
                let points: Vec<_> = points.into_iter().map(LuaPoint::into_skia_point).collect();
                this.add_poly(&points, close);
                Ok(())
            },
        );
        methods.add_method_mut(
            "addRect",
            |_, this, (rect, dir, start): (LuaTable, Option<String>, Option<usize>)| {
                let rect = read_rect_table(rect)?;
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
            |_, this, (rect, rounding, dir): (LuaTable, LuaPoint, Option<String>)| {
                let rect = read_rect_table(rect)?;
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
        methods.add_method_mut("arcTo", |_, this, (oval, start_angle, sweep_angle, force_move_to): (LuaTable, f32, f32, bool)| {
            let oval = read_rect_table(oval)?;
            this.arc_to(oval, start_angle, sweep_angle, force_move_to);
            Ok(())
        });
        methods.add_method_mut("close", |_, this, ()| {
            this.close();
            Ok(())
        });
        methods.add_method("computeTightBounds", |ctx, this, ()| {
            let bounds = write_rect_table(this.compute_tight_bounds(), ctx)?;
            Ok(bounds)
        });
        methods.add_method_mut(
            "conicTo",
            |_, this, (p1, p2, w): (LuaPoint, LuaPoint, f32)| {
                this.conic_to(p1.into_skia_point(), p2.into_skia_point(), w);
                Ok(())
            },
        );
        methods.add_method("conservativelyContainsRect", |_, this, rect: LuaTable| {
            let rect = read_rect_table(rect)?;
            Ok(this.conservatively_contains_rect(rect))
        });
        methods.add_method("contains", |_, this, p: LuaPoint| {
            Ok(this.contains(p.into_skia_point()))
        });
        methods.add_method("countPoints", |_, this, ()| Ok(this.count_points()));
        methods.add_method("countVerbs", |_, this, ()| Ok(this.count_verbs()));
        methods.add_method_mut(
            "cubicTo",
            |_, this, (p1, p2, p3): (LuaPoint, LuaPoint, LuaPoint)| {
                this.cubic_to(
                    p1.into_skia_point(),
                    p2.into_skia_point(),
                    p3.into_skia_point(),
                );
                Ok(())
            },
        );
        methods.add_method("getBounds", |ctx, this, ()| {
            let bounds = write_rect_table(*this.bounds(), ctx)?;
            Ok(bounds)
        });
        methods.add_method("getFillType", |_, this, ()| {
            Ok(path_fill_type_name(this.fill_type()))
        });
        methods.add_method("getGenerationID", |_, this, ()| Ok(this.generation_id()));
        methods.add_method("getLastPt", |ctx, this, ()| match this.last_pt() {
            Some(it) => Ok(LuaPoint::from(it).to_lua(ctx)?),
            None => Ok(LuaNil),
        });
        methods.add_method("getPoint", |ctx, this, index: usize| {
            match this.get_point(index) {
                Some(it) => LuaPoint::from(it).to_lua(ctx),
                None => Ok(LuaNil),
            }
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
        methods.add_method("isLine", |ctx, this, ()| {
            Ok(match this.is_line() {
                Some((from, to)) => LuaLine {
                    from: LuaPoint::from(from),
                    to: LuaPoint::from(to),
                }
                .to_lua(ctx),
                None => Ok(LuaNil),
            })
        });
        methods.add_method("isOval", |ctx, this, ()| {
            Ok(match this.is_oval() {
                Some(oval) => write_rect_table(oval, ctx)?.to_lua(ctx)?,
                None => LuaNil,
            })
        });
        methods.add_method("isRect", |ctx, this, ()| {
            Ok(match this.is_rect() {
                Some((rect, _, _)) => write_rect_table(rect, ctx)?.to_lua(ctx)?,
                None => LuaNil,
            })
        });
        methods.add_method("isRRect", |ctx, this, ()| {
            Ok(match this.is_rrect() {
                Some(rrect) => LuaRRect(rrect).to_lua(ctx),
                None => Ok(LuaNil),
            })
        });
        methods.add_method("isValid", |_, this, ()| Ok(this.is_valid()));
        methods.add_method("isVolatile", |_, this, ()| Ok(this.is_volatile()));
        methods.add_method_mut("lineTo", |_, this, point: LuaPoint| {
            this.line_to(point.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("makeScale", |_, this, (sx, sy): (f32, Option<f32>)| {
            let sy = sy.unwrap_or(sx);
            Ok(LuaPath(this.make_scale((sx, sy))))
        });
        methods.add_method_mut(
            "makeTransform",
            |_, this, (matrix, pc): (LuaMatrix, Option<bool>)| {
                let matrix = match matrix {
                    LuaMatrix::Three(it) => it,
                    LuaMatrix::Four(_) => {
                        return Err(LuaError::RuntimeError(
                            "can't apply 4x4 transform matrix to path".to_string(),
                        ));
                    }
                };
                let pc = match pc.unwrap_or(true) {
                    true => skia_safe::matrix::ApplyPerspectiveClip::Yes,
                    false => skia_safe::matrix::ApplyPerspectiveClip::No,
                };
                Ok(LuaPath(this.make_transform(&matrix, pc)))
            },
        );
        methods.add_method_mut("moveTo", |_, this, p: LuaPoint| {
            this.move_to(p.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("offset", |_, this, d: LuaPoint| {
            this.offset(d.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("quadTo", |_, this, (p1, p2): (LuaPoint, LuaPoint)| {
            this.quad_to(p1.into_skia_point(), p2.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("rArcTo", |_, this, (r, x_axis_rotate, arc_size, sweep, d): (LuaPoint, f32, String, String, LuaPoint)| {
            let arc_size = read_arc_size(arc_size)?;
            let sweep = read_path_direction(sweep)?;
            this.r_arc_to_rotated(r.into_skia_point(), x_axis_rotate, arc_size, sweep, d.into_skia_point());
            Ok(())
        });
        methods.add_method_mut(
            "rConicTo",
            |_, this, (d1, d2, w): (LuaPoint, LuaPoint, f32)| {
                this.r_conic_to(d1.into_skia_point(), d2.into_skia_point(), w);
                Ok(())
            },
        );
        methods.add_method_mut(
            "rCubicTo",
            |_, this, (d1, d2, d3): (LuaPoint, LuaPoint, LuaPoint)| {
                this.r_cubic_to(
                    d1.into_skia_point(),
                    d2.into_skia_point(),
                    d3.into_skia_point(),
                );
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
            this.r_line_to(point.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("rMoveTo", |_, this, point: LuaPoint| {
            this.r_move_to(point.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("rQuadTo", |_, this, (dx1, dx2): (LuaPoint, LuaPoint)| {
            this.r_quad_to(dx1.into_skia_point(), dx2.into_skia_point());
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
            this.set_last_pt(point.into_skia_point());
            Ok(())
        });
        methods.add_method_mut("toggleInverseFillType", |_, this, ()| {
            this.toggle_inverse_fill_type();
            Ok(())
        });
        methods.add_method_mut("transform", |_, this, matrix: LuaMatrix| match matrix {
            LuaMatrix::Three(matrix) => {
                this.transform(&matrix);
                Ok(())
            }
            LuaMatrix::Four(_) => Err(LuaError::RuntimeError(
                "can't 3D transform path with a 4x4 matrix".to_string(),
            )),
        });
    }
}

wrap_skia_handle!(RRect);

impl UserData for LuaRRect {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(Typeface);

impl UserData for LuaTypeface {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
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
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(Font);

impl UserData for LuaFont {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

wrap_skia_handle!(TextBlob);

impl UserData for LuaTextBlob {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct FromLuaSaveLayerRec {
    bounds: Option<Rect>,
    paint: Option<LuaPaint>,
    backdrop: Option<LuaImageFilter>,
    flags: SaveLayerFlags,
}

impl FromLuaSaveLayerRec {
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

impl<'lua> FromLua<'lua> for FromLuaSaveLayerRec {
    fn from_lua(lua_value: LuaValue<'lua>, _: LuaContext<'lua>) -> LuaResult<Self> {
        let mut result = FromLuaSaveLayerRec {
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
            let bounds_table: LuaValue = table.get("bounds")?;
            match bounds_table {
                LuaValue::Table(bounds_table) => {
                    result.bounds = Some(read_rect_table(bounds_table)?)
                }
                _ => {
                    return Err(LuaError::FromLuaConversionError {
                        from: "table",
                        to: "Rect",
                        message: Some(format!(
                            "expected SaveLayerRec.bounds entry to be a Rect table"
                        )),
                    })
                }
            };
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
        methods.add_method("clear", |_, this, (color,): (Option<LuaTable>,)| {
            let color = color
                .map(read_color_table)
                .unwrap_or(skia_safe::colors::TRANSPARENT);
            this.clear(color);
            Ok(())
        });
        methods.add_method(
            "drawColor",
            |_, this, (color, blend_mode): (LuaTable, Option<String>)| {
                let color = read_color_table(color);
                let mode = match blend_mode {
                    Some(it) => Some(read_blend_mode(it)?),
                    None => None,
                };
                this.draw_color(color, mode);
                Ok(())
            },
        );
        methods.add_method("drawPaint", |_, this, (paint,): (LuaPaint,)| {
            this.draw_paint(&paint);
            Ok(())
        });
        methods.add_method(
            "drawRect",
            |_, this, (rect, paint): (LuaTable, LuaPaint)| {
                let rect = read_rect_table(rect)?;
                this.draw_rect(rect, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawOval",
            |_, this, (oval, paint): (LuaTable, LuaPaint)| {
                let oval = read_rect_table(oval)?;
                this.draw_oval(oval, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawCircle",
            |_, this, (point, r, paint): (LuaPoint, f32, LuaPaint)| {
                this.draw_circle(point.into_skia_point(), r, &paint);
                Ok(())
            },
        );
        methods.add_method(
            "drawImage",
            |_, this, (image, point, paint): (LuaImage, LuaPoint, Option<LuaPaint>)| {
                this.draw_image(
                    image.0,
                    point.into_skia_point(),
                    paint.map(|it| it.0).as_ref(),
                );
                Ok(())
            },
        );
        methods.add_method(
            "drawImageRect",
            |_,
             this,
             (image, src_rect, dst_rect, paint): (
                LuaImage,
                Option<LuaTable>,
                LuaTable,
                Option<LuaPaint>,
            )| {
                let paint: Paint = match paint {
                    Some(it) => it.0,
                    None => Paint::default(),
                };
                let src_rect = match src_rect {
                    Some(it) => Some(read_rect_table(it)?),
                    None => None,
                };
                let dst_rect = read_rect_table(dst_rect)?;
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
                LuaPaint,
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
                            result[i] = read_color_table(colors.get(i)?).to_color();
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
        methods.add_method("drawPath", |_, this, (path, paint): (LuaPath, LuaPaint)| {
            this.draw_path(&path, &paint);
            Ok(())
        });
        //TODO: methods.add_method("drawPicture", |_, this, ()| Ok(()));
        methods.add_method(
            "drawTextBlob",
            |_, this, (blob, point, paint): (LuaTextBlob, LuaPoint, LuaPaint)| {
                this.draw_text_blob(blob.0, point.into_skia_point(), &paint);
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
        methods.add_method(
            "saveLayer",
            |_, this, save_layer_rec: FromLuaSaveLayerRec| {
                Ok(this.save_layer(&save_layer_rec.to_skia_save_layer_rec()))
            },
        );
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
            this.translate(point.into_skia_point());
            Ok(())
        });
        methods.add_method(
            "rotate",
            |_, this, (degrees, point): (f32, Option<LuaPoint>)| {
                let point = point.map(LuaPoint::into_skia_point);
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
        //TODO: methods.add_method("newSurface", |_, this, ()| Ok(()));
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
        //TODO: methods.add_method("newRasterSurface", |ctx, this, ()| Ok(()));
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
        )
    }
}

#[allow(non_snake_case)]
pub fn setup<'lua>(ctx: LuaContext<'lua>) -> Result<(), rlua::Error> {
    let gfx = ctx.create_userdata(LuaGfx)?;
    ctx.globals().set("Gfx", gfx)?;
    Ok(())
}
