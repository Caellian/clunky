//! This module contains representations of skia types that are used as
//! arguments.

use std::{collections::VecDeque, sync::Arc};

use mlua::prelude::*;
use skia_safe::{Color, Color4f, IPoint, IRect, ISize, Point, Point3, Rect};

use crate::{from_lua_argpack, ArgumentContext, FromArgPack, LuaType};

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
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> LuaResult<Self> {
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

            let (r, g, b) = crate::util::hsl_to_rgb(h, s, l);
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
from_lua_argpack!(LuaColor);

impl<'lua> IntoLua<'lua> for LuaColor {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;
        result.set("r", self.r)?;
        result.set("g", self.g)?;
        result.set("b", self.b)?;
        result.set("a", self.a)?;
        result.into_lua(lua)
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
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> LuaResult<Self> {
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
from_lua_argpack!(LuaRect);

impl<'lua> IntoLua<'lua> for LuaRect {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;
        result.set("top", self.from.x())?;
        result.set("left", self.from.y())?;
        result.set("right", self.to.x())?;
        result.set("bottom", self.to.y())?;
        result.into_lua(lua)
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

const DIM_NAME: &[&str] = &["width", "height", "depth"];
const DIM_NAME_SHORT: &[&str] = &["w", "h", "d"];

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
impl<'lua, const N: usize> FromArgPack<'lua> for LuaSize<N> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        const FIRST_ERR: &str = "value must be an array of coordinates or number";
        if let Ok(table) = args.pop_typed_or(Some(FIRST_ERR)) {
            let value = TryFrom::<LuaTable<'lua>>::try_from(table)?;
            Ok(value)
        } else {
            let it = args.pop_typed_or(Some(FIRST_ERR))?;
            let mut value = [it; N];
            for i in 1..N {
                value[i] =
                    args.pop_typed_or(Some(format!("Point expected {i}-th number component")))?;
            }
            Ok(LuaSize { value })
        }
    }
}

impl<'lua, const N: usize> IntoLua<'lua> for LuaSize<N> {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
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

const COORD_NAME: &[&str] = &["x", "y", "z", "w"];

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

    pub fn as_array(&self) -> [f32; N] {
        self.value
    }
    pub fn as_slice(&self) -> &[f32; N] {
        &self.value
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
impl From<IPoint> for LuaPoint {
    #[inline]
    fn from(value: IPoint) -> Self {
        LuaPoint {
            value: [value.x as f32, value.y as f32],
        }
    }
}
impl Into<IPoint> for LuaPoint {
    fn into(self) -> IPoint {
        IPoint {
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

impl<'lua, const N: usize> FromArgPack<'lua> for LuaPoint<N> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        const FIRST_ERR: &str = "value must be an array of coordinates or number";
        if let Ok(table) = args.pop_typed_or(Some(FIRST_ERR)) {
            let value = TryFrom::<LuaTable<'lua>>::try_from(table)?;
            Ok(value)
        } else {
            let it = args.pop_typed_or(Some(FIRST_ERR))?;
            let mut value = [it; N];
            for i in 1..N {
                value[i] =
                    args.pop_typed_or(Some(format!("Point expected {i}-th number component")))?;
            }
            Ok(LuaPoint { value })
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

impl<'lua, const N: usize> IntoLua<'lua> for LuaPoint<N> {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;

        for (i, coord) in COORD_NAME[0..N].iter().enumerate() {
            result.set(*coord, self.value[i])?;
        }

        result.into_lua(lua)
    }
}

#[derive(Clone)]
pub struct LuaLine<const N: usize = 2> {
    pub from: LuaPoint<N>,
    pub to: LuaPoint<N>,
}

impl<'lua, const N: usize> IntoLua<'lua> for LuaLine<N> {
    fn into_lua(self, lua: &'lua Lua) -> LuaResult<LuaValue<'lua>> {
        let result = lua.create_table()?;

        result.set("from", self.from.into_lua(lua)?)?;
        result.set("to", self.to.into_lua(lua)?)?;

        result.into_lua(lua)
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

impl<'lua> FromArgPack<'lua> for SidePack {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        args.assert_next_type(&[LuaType::Integer, LuaType::Number, LuaType::Table])?;

        if let Some(table) = args.pop_typed() {
            return TryFrom::<LuaTable<'lua>>::try_from(table);
        }

        let single = args.pop_typed().unwrap();
        let two = args.pop_typed().map(|it| [single, it]);
        let four = match two {
            Some([a, b]) => {
                // take additional two or none
                if let Some(c) = args.pop_typed() {
                    match args.pop_typed() {
                        Some(d) => Some([a, b, c, d]),
                        None => {
                            args.revert(c);
                            None
                        }
                    }
                } else {
                    None
                }
            }
            None => None,
        };

        if let Some([left, top, right, bottom]) = four {
            Ok(SidePack {
                left,
                top,
                right,
                bottom,
            })
        } else if let Some([vertical, horizontal]) = two {
            Ok(SidePack {
                left: horizontal,
                top: vertical,
                right: horizontal,
                bottom: vertical,
            })
        } else {
            Ok(SidePack {
                left: single,
                top: single,
                right: single,
                bottom: single,
            })
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
            2 | 3 => unsafe {
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
