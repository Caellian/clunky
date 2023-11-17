use std::sync::OnceLock;

use phf::phf_map;
use rlua::{prelude::*, Context as LuaContext, Table as LuaTable, UserData};
use skia_safe::{
    canvas::{self, SaveLayerFlags, SaveLayerRec},
    font_style::{Slant, Weight, Width},
    image_filters::{self, CropRect},
    paint::Cap as PaintCap,
    paint::Join as PaintJoin,
    BlendMode, Canvas, Color, Color4f, ColorFilter, ColorSpace, Data, Font, FontStyle, Image,
    ImageFilter, MaskFilter, Matrix, Paint, Path, PathEffect, Point, Rect, SamplingOptions, Shader,
    TextBlob, TileMode, Typeface, M44,
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
fn read_table_rect(table: LuaTable) -> Result<Rect, LuaError> {
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

macro_rules! enum_naming {
    ($kind: ty: [$($value: expr => $name: literal,)+]) => {paste::paste!{
        static [<NAME_TO_ $kind:snake:upper>]: phf::Map<&'static str, $kind> = phf_map! {
            $($name => $value),
            +
        };

        #[inline]
        fn [<read_ $kind:snake:lower>](name: impl AsRef<str>) -> Result<$kind, LuaError> {
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

        #[allow(unreachable_patterns)]
        const fn [<$kind:snake:lower _name>](value: $kind) -> Option<&'static str> {
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

enum_naming! { SaveLayerFlags : [
    SaveLayerFlags::PRESERVE_LCD_TEXT => "preserve_lcd_text",
    SaveLayerFlags::INIT_WITH_PREVIOUS => "init_with_previous",
    SaveLayerFlags::F16_COLOR_TYPE => "f16_color_type",
]}

#[derive(Clone)]
pub struct LuaShader(pub Shader);

impl UserData for LuaShader {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isOpaque", |_, this, ()| Ok(this.0.is_opaque()));
        methods.add_method("isAImage", |_, this, ()| Ok(this.0.is_a_image()));
    }
}

#[derive(Clone)]
pub struct LuaImage(pub Image);

impl UserData for LuaImage {
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
                .map(LuaShader)
                .ok_or(LuaError::RuntimeError(
                    "can't create shader from image".to_string(),
                ))
        });
    }
}

#[derive(Clone)]
pub struct LuaColorSpace(pub ColorSpace);

impl UserData for LuaColorSpace {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaImageFilter(pub ImageFilter);

impl UserData for LuaImageFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaColorFilter(pub ColorFilter);

impl UserData for LuaColorFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("toAColorMode", |ctx, this, ()| {
            if let Some((color, mode)) = this.0.to_a_color_mode() {
                let result = ctx.create_table()?;
                result.set(0, write_color_table(color, ctx)?)?;
                result.set(1, blend_mode_name(mode))?;
                Ok(LuaValue::Table(result))
            } else {
                Ok(LuaNil)
            }
        });
        methods.add_method("toAColorMatrix", |ctx, this, ()| {
            if let Some(mx) = this.0.to_a_color_matrix() {
                Ok(LuaValue::Table(
                    ctx.create_table_from(mx.into_iter().enumerate())?,
                ))
            } else {
                Ok(LuaNil)
            }
        });
        methods.add_method("makeComposed", |_, this, inner: LuaColorFilter| {
            Ok(LuaColorFilter(this.0.composed(inner.0).ok_or(
                LuaError::RuntimeError("unable to compose filters".to_string()),
            )?))
        });
        methods.add_method(
            "makeWithWorkingColorSpace",
            |_, this, color_space: LuaColorSpace| {
                Ok(LuaColorFilter(
                    this.0.with_working_color_space(color_space.0).ok_or(
                        LuaError::RuntimeError("unable to apply color space to filter".to_string()),
                    )?,
                ))
            },
        );
    }
}

#[derive(Clone)]
pub struct LuaMaskFilter(pub MaskFilter);

impl UserData for LuaMaskFilter {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaPathEffect(pub PathEffect);

impl UserData for LuaPathEffect {}

#[derive(Clone)]
pub enum LuaMatrix {
    Three(Matrix),
    Four(M44),
}

impl UserData for LuaMatrix {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("getSize", |_, this, ()| match this {
            LuaMatrix::Three(_) => Ok(3),
            LuaMatrix::Four(_) => Ok(4),
        });
        //methods.add_method("getType", |_, this, ()| Ok(()));
        //methods.add_method("getScaleX", |_, this, ()| Ok(()));
        //methods.add_method("getScaleY", |_, this, ()| Ok(()));
        //methods.add_method("getTranslateX", |_, this, ()| Ok(()));
        //methods.add_method("getTranslateY", |_, this, ()| Ok(()));
        //methods.add_method("setRectToRect", |_, this, ()| Ok(()));
        methods.add_method("invert", |ctx, this, ()| match this {
            LuaMatrix::Three(mx) => match mx.invert() {
                Some(it) => Ok(LuaMatrix::Three(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            },
            LuaMatrix::Four(mx) => match mx.invert() {
                Some(it) => Ok(LuaMatrix::Four(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            },
        })
        //methods.add_method("mapXY", |_, this, ()| Ok(()));
    }
}

#[derive(Clone)]
pub struct LuaPaint(pub Paint);

impl UserData for LuaPaint {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("isAntiAlias", |_, this, ()| Ok(this.0.is_anti_alias()));
        methods.add_method_mut("setAntiAlias", |_, this, anti_alias| {
            this.0.set_anti_alias(anti_alias);
            Ok(())
        });
        methods.add_method("isDither", |_, this, ()| Ok(this.0.is_dither()));
        methods.add_method_mut("setDither", |_, this, dither| {
            this.0.set_dither(dither);
            Ok(())
        });
        methods.add_method_mut("getImageFilter", |ctx, this, ()| {
            match this.0.image_filter() {
                Some(it) => Ok(LuaImageFilter(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            }
        });
        methods.add_method_mut(
            "setImageFilter",
            |_, this, image_filter: Option<LuaImageFilter>| {
                this.0.set_image_filter(image_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getMaskFilter", |ctx, this, ()| {
            match this.0.mask_filter() {
                Some(it) => Ok(LuaMaskFilter(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            }
        });
        methods.add_method_mut(
            "setMaskFilter",
            |_, this, mask_filter: Option<LuaMaskFilter>| {
                this.0.set_mask_filter(mask_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method_mut("getColorFilter", |ctx, this, ()| {
            match this.0.color_filter() {
                Some(it) => Ok(LuaColorFilter(it).to_lua(ctx)?),
                None => Ok(LuaNil),
            }
        });
        methods.add_method_mut(
            "setColorFilter",
            |_, this, color_filter: Option<LuaColorFilter>| {
                this.0.set_color_filter(color_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method("getAlpha", |_, this, ()| Ok(this.0.alpha_f()));
        methods.add_method_mut("setAlpha", |_, this, alpha| {
            this.0.set_alpha_f(alpha);
            Ok(())
        });
        methods.add_method("getColor", |ctx, this, ()| {
            write_color4f_table(this.0.color4f(), ctx)
        });
        methods.add_method_mut(
            "setColor",
            |_, this, (color, color_space): (LuaTable, Option<LuaColorSpace>)| {
                let color = read_table_color(color);
                this.0
                    .set_color4f(color, color_space.map(|it| it.0).as_ref());
                Ok(())
            },
        );
        methods.add_method("getStyle", |ctx, this, ()| {
            let result = ctx.create_table()?;
            match this.0.style() {
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
            this.0.set_style(match (fill, stroke) {
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
            Ok(paint_cap_name(this.0.stroke_cap()))
        });
        methods.add_method_mut("setStrokeCap", |_, this, cap: String| {
            this.0.set_stroke_cap(read_paint_cap(cap)?);
            Ok(())
        });
        methods.add_method("getStrokeJoin", |_, this, ()| {
            Ok(paint_join_name(this.0.stroke_join()))
        });
        methods.add_method_mut("setStrokeJoin", |_, this, join: String| {
            this.0.set_stroke_join(read_paint_join(join)?);
            Ok(())
        });
        methods.add_method("getStrokeWidth", |_, this, ()| Ok(this.0.stroke_width()));
        methods.add_method_mut("setStrokeWidth", |_, this, width| {
            this.0.set_stroke_width(width);
            Ok(())
        });
        methods.add_method("getStrokeMiter", |_, this, ()| Ok(this.0.stroke_miter()));
        methods.add_method_mut("setStrokeMiter", |_, this, miter| {
            this.0.set_stroke_miter(miter);
            Ok(())
        });
        methods.add_method("getPathEffect", |_, this, ()| {
            Ok(this.0.path_effect().map(LuaPathEffect))
        });
        methods.add_method_mut("getPathEffect", |_, this, effect: Option<LuaPathEffect>| {
            this.0.set_path_effect(effect.map(|it| it.0));
            Ok(())
        });
        methods.add_method("getColorFilter", |_, this, ()| {
            Ok(this.0.color_filter().map(LuaColorFilter))
        });
        methods.add_method_mut(
            "setColorFilter",
            |_, this, color_filter: Option<LuaColorFilter>| {
                this.0.set_color_filter(color_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method("getImageFilter", |_, this, ()| {
            Ok(this.0.image_filter().map(LuaImageFilter))
        });
        methods.add_method_mut(
            "setImageFilter",
            |_, this, image_filter: Option<LuaImageFilter>| {
                this.0.set_image_filter(image_filter.map(|it| it.0));
                Ok(())
            },
        );
        methods.add_method("getShader", |_, this, ()| {
            Ok(this.0.shader().map(LuaShader))
        });
        methods.add_method_mut("setShader", |_, this, shader: Option<LuaShader>| {
            this.0.set_shader(shader.map(|it| it.0));
            Ok(())
        });
        methods.add_method("getPathEffect", |_, this, ()| {
            Ok(this.0.path_effect().map(LuaPathEffect))
        });
        methods.add_method_mut(
            "setPathEffect",
            |_, this, path_effect: Option<LuaPathEffect>| {
                this.0.set_path_effect(path_effect.map(|it| it.0));
                Ok(())
            },
        );
    }
}

#[derive(Clone)]
pub struct LuaPath(pub Path);

impl UserData for LuaPath {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaTypeface(pub Typeface);

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

#[derive(Clone)]
pub struct LuaFontStyle(pub FontStyle);

impl UserData for LuaFontStyle {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaFont(pub Font);

impl UserData for LuaFont {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(_methods: &mut T) {}
}

#[derive(Clone)]
pub struct LuaTextBlob(pub TextBlob);

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
            result = result.paint(&paint.0);
        }
        if let Some(backdrop) = &self.backdrop {
            result = result.backdrop(&backdrop.0);
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
                    result.bounds = Some(read_table_rect(bounds_table)?)
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
                    for pair in list.pairs::<usize, String>() {
                        if let Ok((_, name)) = pair {
                            result.flags |= read_save_layer_flags(name.as_str())?;
                        } else {
                            return Err(LuaError::FromLuaConversionError {
                                from: "table",
                                to: "SaveLayerFlags",
                                message: Some("expected SaveLayerRec.flags array to be an array of strings".to_string()),
                            });
                        }
                    }
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

impl<'a> UserData for LuaCanvas<'a> {
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
        methods.add_method("drawPaint", |_, this, (paint,): (LuaPaint,)| {
            this.0.draw_paint(&paint.0);
            Ok(())
        });
        methods.add_method(
            "drawRect",
            |_, this, (rect, paint): (LuaTable, LuaPaint)| {
                let rect = read_table_rect(rect)?;
                this.0.draw_rect(rect, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawOval",
            |_, this, (oval, paint): (LuaTable, LuaPaint)| {
                let oval = read_table_rect(oval)?;
                this.0.draw_oval(oval, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawCircle",
            |_, this, (x, y, r, paint): (f32, f32, f32, LuaPaint)| {
                this.0.draw_circle((x, y), r, &paint.0);
                Ok(())
            },
        );
        methods.add_method(
            "drawImage",
            |_, this, (image, x, y, paint): (LuaImage, f32, f32, Option<LuaPaint>)| {
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
        methods.add_method("drawPath", |_, this, (path, paint): (LuaPath, LuaPaint)| {
            this.0.draw_path(&path.0, &paint.0);
            Ok(())
        });
        //TODO: methods.add_method("drawPicture", |_, this, ()| Ok(()));
        methods.add_method(
            "drawTextBlob",
            |_, this, (blob, x, y, paint): (LuaTextBlob, f32, f32, LuaPaint)| {
                this.0.draw_text_blob(blob.0, (x, y), &paint.0);
                Ok(())
            },
        );
        methods.add_method("getSaveCount", |_, this, ()| Ok(this.0.save_count()));
        methods.add_method("getLocalToDevice", |_, this, ()| {
            Ok(LuaMatrix::Four(this.0.local_to_device()))
        });
        methods.add_method("getLocalToDevice3x3", |_, this, ()| {
            Ok(LuaMatrix::Three(this.0.local_to_device_as_3x3()))
        });
        methods.add_method("save", |_, this, ()| Ok(this.0.save()));
        methods.add_method(
            "saveLayer",
            |_, this, save_layer_rec: FromLuaSaveLayerRec| {
                Ok(this.0.save_layer(&save_layer_rec.to_skia_save_layer_rec()))
            },
        );
        methods.add_method("restore", |_, this, ()| {
            this.0.restore();
            Ok(())
        });
        methods.add_method("restoreToCount", |_, this, count: usize| {
            this.0.restore_to_count(count);
            Ok(())
        });
        methods.add_method("scale", |_, this, (sx, sy): (f32, Option<f32>)| {
            let sy = sy.unwrap_or(sx);
            this.0.scale((sx, sy));
            Ok(())
        });
        methods.add_method("translate", |_, this, (x, y): (f32, f32)| {
            this.0.translate((x, y));
            Ok(())
        });
        methods.add_method(
            "rotate",
            |_, this, (degrees, x, y): (f32, Option<f32>, Option<f32>)| {
                let point =
                    match (x, y) {
                        (Some(x), Some(y)) => Some(Point::new(x, y)),
                        (None, None) => None,
                        _ => return Err(LuaError::FromLuaConversionError {
                            from: "(x, y)",
                            to: "Point",
                            message: Some(
                                "both x and y Point values must be specified; one is missing/nil"
                                    .to_string(),
                            ),
                        }),
                    };
                this.0.rotate(degrees, point);
                Ok(())
            },
        );
        methods.add_method("concat", |_, this, matrix: LuaMatrix| {
            match matrix {
                LuaMatrix::Three(matrix) => this.0.concat(&matrix),
                LuaMatrix::Four(matrix) => this.0.concat_44(&matrix),
            };
            Ok(())
        });
        //TODO: methods.add_method("newSurface", |_, this, ()| Ok(()));
    }
}

struct LuaGfx;
impl UserData for LuaGfx {
    fn add_methods<'lua, T: LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method("loadImage", |_, _, (name,): (String,)| {
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
        //TODO: methods.add_method("newRRect", |ctx, this, ()| Ok(()));
        //TODO: methods.add_method("newRasterSurface", |ctx, this, ()| Ok(()));
        methods.add_method("newTextBlob", |_, _, (text, font): (String, LuaFont)| {
            Ok(TextBlob::new(text, &font.0).map(LuaTextBlob))
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
