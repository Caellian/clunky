use rlua::Table;

use crate::{
    error::{self, ClunkyError, ValueType},
};

pub trait Component: 'static {
    fn component_type_name(&self) -> String;

    fn try_from_lua_table<'l>(table: &Table<'l>) -> error::Result<Box<dyn Component>> where Self: Sized;
}

macro_rules! get_component_property {
    ($table: ident, $name: literal, $kind: path) => {
        $table.get($name).map_err(|_| ClunkyError::MissingComponentProperty { name: $name, value: $kind })
    };
}

pub struct Position {
    pub x: u32,
    pub y: u32,
}

//TODO: Should be Lua UserData with syntax "Position:new(x, y)"
impl<'l> TryFrom<&Table<'l>> for Position {
    type Error = ClunkyError;

    fn try_from(table: &Table<'l>) -> error::Result<Self> {
        let x = get_component_property!(table, "x", ValueType::Number)?;
        let y = get_component_property!(table, "y", ValueType::Number)?;

        Ok(Position { x, y })
    }
}

pub struct Label {
    pub position: Position,
    pub text: String,
}

impl Component for Label {
    fn component_type_name(&self) -> String {
        "Label".to_string()
    }

    fn try_from_lua_table<'l>(table: &Table<'l>) -> error::Result<Box<dyn Component>> {
        let position = Position::try_from(table)?;
        let text = get_component_property!(table, "text", ValueType::String)?;

        Ok(Box::new(Label { position, text }))
    }
}

pub struct Button {
    pub position: Position,
    pub text: String,
}

impl Component for Button {
    fn component_type_name(&self) -> String {
        "Button".to_string()
    }

    fn try_from_lua_table<'l>(table: &Table<'l>) -> error::Result<Box<dyn Component>> {
        let position = Position::try_from(table)?;
        let text = get_component_property!(table, "text", ValueType::String)?;

        Ok(Box::new(Button { position, text }))
    }
}

macro_rules! declare_parsers {
    [$($name: ident),+] => {
        static COMPONENT_PARSERS: &[(&str, fn(&Table) -> error::Result<Box<dyn Component>>)] = &[
            $((stringify!($name), $name::try_from_lua_table)),
            +
        ];
    };
}

declare_parsers![
    Label,
    Button
];

pub fn try_component_from_lua_table<'l>(table: &Table<'l>) -> error::Result<Box<dyn Component>> {
    let ty: String = table.raw_get("type")?;

    if let Some(first) = ty.chars().next() {
        if !first.is_uppercase() {
            return Err(
                ClunkyError::UnknownComponent {
                    detail: error::Detail(Some(format!("component types use UpperCamelCase naming convention; try using '{}{}' instead", first.to_uppercase(), &ty[1..]))),
                    found: ty,
                });
        }
    } else if ty.len() == 0 {
        return Err(ClunkyError::EmptyComponentType.into());
    };

    for (name, table_parser) in COMPONENT_PARSERS.iter() {
        if *name == ty {
            return Ok(table_parser(table)?)
        }
    }

    return Err(ClunkyError::UnknownComponent {
        found: ty,
        detail: error::Detail(None)
    });
}
