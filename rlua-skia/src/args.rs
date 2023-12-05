use std::{collections::VecDeque, sync::Arc};

use rlua::prelude::*;
use skia_safe::{Color, Color4f, IPoint, IRect, ISize, Point, Point3, Rect};

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
