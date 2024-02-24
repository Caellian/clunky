//! Extensions for mlua.
//!
//! This module provides a lot of utility wrappers and traits that make it
//! easier to handle conversion from Lua types.

use std::{fmt::Display, mem::MaybeUninit, ops::Deref, sync::Arc};

use mlua::{
    AnyUserData, Error, FromLua, Integer, IntoLua, LightUserData, Lua, MultiValue,
    Result as LuaResult, Table, UserData,
    Value::{self, Nil},
};

use crate::util::OptionStrOwned;

/// Argument that's allowed to fail conversion and will be skipped, yielding
/// `None` in case of failure.
pub struct LuaFallible<T>(Option<T>);

impl<T> LuaFallible<T> {
    pub fn into_inner(self) -> Option<T> {
        self.0
    }

    pub fn map<R, F: Fn(T) -> R>(self, f: F) -> Option<R> {
        self.0.map(f)
    }

    pub fn or_else<F: Fn() -> Option<T>>(self, f: F) -> Option<T> {
        self.0.or_else(f)
    }

    pub fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        self.0.unwrap_or_default()
    }
}

impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for LuaFallible<T> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        match T::convert(args, lua) {
            Ok(it) => Ok(LuaFallible(Some(it))),
            Err(_) => Ok(LuaFallible(None)),
        }
    }
}

impl<T> Deref for LuaFallible<T> {
    type Target = Option<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuaType {
    Nil,
    Boolean,
    LightUserData,
    Integer,
    Number,
    String,
    Table,
    Function,
    Thread,
    UserData,
    Error,
    Any,
}

impl LuaType {
    pub fn of(value: &Value<'_>) -> Self {
        match value {
            Nil => LuaType::Nil,
            Value::Boolean(_) => LuaType::Boolean,
            Value::LightUserData(_) => LuaType::LightUserData,
            Value::Integer(_) => LuaType::Integer,
            Value::Number(_) => LuaType::Number,
            Value::String(_) => LuaType::String,
            Value::Table(_) => LuaType::Table,
            Value::Function(_) => LuaType::Function,
            Value::Thread(_) => LuaType::Thread,
            Value::UserData(_) => LuaType::UserData,
            Value::Error(_) => LuaType::Error,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LuaType::Nil => "nil",
            LuaType::Boolean => "boolean",
            LuaType::LightUserData => "light_user_data",
            LuaType::Integer => "integer",
            LuaType::Number => "number",
            LuaType::String => "string",
            LuaType::Table => "table",
            LuaType::Function => "function",
            LuaType::Thread => "thread",
            LuaType::UserData => "user_data",
            LuaType::Error => "error",
            LuaType::Any => "any",
        }
    }
}

impl Display for LuaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

pub type ArgumentNames = Option<&'static [&'static str]>;

#[derive(Debug, Clone)]
pub(crate) struct ArgumentContext<'lua> {
    value: Vec<Value<'lua>>,
    argument_names: ArgumentNames,
    initial_count: usize,
    logical_argument: usize,
    call_name: Option<&'static str>,
}

#[allow(unused)]
impl<'lua> ArgumentContext<'lua> {
    fn new(
        inner: MultiValue<'lua>,
        argument_names: ArgumentNames,
        call_name: Option<&'static str>,
    ) -> Self {
        let mut value = inner.into_vec();
        value.reverse();
        ArgumentContext {
            initial_count: value.len(),
            argument_names,
            value,
            logical_argument: 0,
            call_name,
        }
    }

    pub fn call_name(&self) -> Option<&'static str> {
        self.call_name
    }

    #[inline]
    pub fn try_pop(&mut self) -> Option<Value<'lua>> {
        self.value.pop()
    }

    pub fn pop(&mut self) -> Value<'lua> {
        self.value.pop().unwrap_or(Value::Nil)
    }

    pub fn peek(&self) -> &Value<'lua> {
        self.value.last().unwrap_or(&Value::Nil)
    }

    #[inline]
    pub fn peek_type(&self) -> LuaType {
        LuaType::of(self.peek())
    }

    pub fn assert_next_type(&self, one_of: &[LuaType]) -> Result<(), mlua::Error> {
        let next_type = self.peek_type();
        if one_of.contains(&next_type) {
            return Ok(());
        }

        if one_of.len() == 1 {
            return Err(self.bad_argument(mlua::Error::RuntimeError(format!(
                "expected {}",
                unsafe {
                    // Safety: length checked
                    one_of.get_unchecked(0).name()
                }
            ))));
        }

        let mut expected = String::with_capacity(one_of.len() * 6);
        for (i, ty) in one_of.iter().map(LuaType::name).enumerate() {
            if i > 0 {
                expected.push_str(", ");
            }
            expected.push_str(ty);
        }
        Err(self.bad_argument(mlua::Error::RuntimeError(format!(
            "expected one of: {}",
            expected
        ))))
    }

    #[inline]
    pub fn revert(&mut self, value: impl IsValue<'lua>) {
        self.value.push(value.into_value())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.value.len()
    }

    #[inline]
    pub fn initial_len(&self) -> usize {
        self.initial_count
    }

    #[inline]
    pub fn at(&self) -> usize {
        self.initial_count - self.len()
    }

    pub fn at_name(&self) -> Option<&'static str> {
        self.argument_names
            .and_then(|it| it.get(self.logical_argument).copied())
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn advance_name(&mut self) {
        self.logical_argument += 1;
    }

    pub fn bad_argument(&self, inner: mlua::Error) -> mlua::Error {
        mlua::Error::BadArgument {
            to: self.call_name.cloned(),
            pos: self.at(),
            name: self.at_name().cloned(),
            cause: Arc::new(inner),
        }
    }

    /// Attempts poping a [`Value`] of type `T` from argument list and
    /// returns it if the type matches. If the type doesn't match, the value
    /// is [`revert`](Self::revert)ed back into the argument list and
    /// returns `None`.
    pub fn pop_typed<T: IsValue<'lua>>(&mut self) -> Option<T> {
        match T::from_value(self.pop()) {
            Ok(it) => Some(it),
            Err((_, v)) => {
                self.revert(v);
                None
            }
        }
    }

    /// Attempts poping a [`Value`] of type `T` from argument list and
    /// returns it if the type matches. If the type doesn't match, the value
    /// is [`revert`](Self::revert)ed back into the argument list and a
    /// [`BadArgument`](mlua::Error::BadArgument) error is returned, with
    /// the provided error `message`.
    pub fn pop_typed_or<T: IsValue<'lua>, S: ToString>(
        &mut self,
        message: Option<S>,
    ) -> LuaResult<T> {
        match T::from_value(self.pop()) {
            Ok(it) => Ok(it),
            Err((conversion, v)) => {
                self.revert(v);
                Err(self.bad_argument(mlua::Error::FromLuaConversionError {
                    from: conversion.from,
                    to: conversion.to,
                    message: message.map(|it| it.to_string()),
                }))
            }
        }
    }

    pub fn pop_all(&mut self) -> Vec<Value<'lua>> {
        let mut result = Vec::new();
        std::mem::swap(&mut self.value, &mut result);
        result.reverse();
        result
    }
}

impl<'lua> From<ArgumentContext<'lua>> for MultiValue<'lua> {
    fn from(mut val: ArgumentContext<'lua>) -> Self {
        val.value.reverse();
        MultiValue::from_vec(val.value)
    }
}

pub(crate) struct ConversionError {
    from: &'static str,
    to: &'static str,
}

// TODO: Some macro magic to automate string interning.

/// Allows auto-wrapping of [`Value`]s in function arguments.
///
/// This trait assumes [`Lua`] context isn't available, because
/// [`ArgumentContext`] doesn't require passing the context in.
pub(crate) trait IsValue<'lua>: Sized {
    const TYPE: LuaType;

    fn into_value(self) -> Value<'lua>;
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)>;
}
impl<'lua> IsValue<'lua> for Value<'lua> {
    const TYPE: LuaType = LuaType::Any;

    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        self
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        Ok(wrapped)
    }
}
impl<'lua> IsValue<'lua> for () {
    const TYPE: LuaType = LuaType::Nil;

    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::Nil
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if wrapped == Value::Nil {
            Ok(())
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: Value::Nil.type_name(),
                },
                wrapped,
            ))
        }
    }
}
impl<'lua> IsValue<'lua> for bool {
    const TYPE: LuaType = LuaType::Boolean;

    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::Boolean(self)
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if let Value::Boolean(it) = wrapped {
            Ok(it)
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: LuaType::Boolean.name(),
                },
                wrapped,
            ))
        }
    }
}
impl<'lua> IsValue<'lua> for LightUserData {
    const TYPE: LuaType = LuaType::LightUserData;

    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::LightUserData(self)
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if let Value::LightUserData(it) = wrapped {
            Ok(it)
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: LuaType::LightUserData.name(),
                },
                wrapped,
            ))
        }
    }
}
macro_rules! int_is_val {
    ($($int: ty),+) => {
        $(impl<'lua> IsValue<'lua> for $int {
            const TYPE: LuaType = LuaType::Integer;

            #[inline(always)]
            fn into_value(self) -> Value<'lua> {
                Value::Integer(self as i64)
            }
            fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
                if let Value::Integer(it) = wrapped {
                    Ok(it as $int)
                } else if let Value::Number(it) = wrapped {
                    Ok(it as $int)
                } else {
                    Err((ConversionError {
                        from: wrapped.type_name(),
                        to: stringify!($int),
                    }, wrapped))
                }
            }
        })+
    };
}
int_is_val![u8, u16, u32, u64, i8, i16, i32, i64];

macro_rules! float_is_val {
    ($($float: ty),+) => {
        $(impl<'lua> IsValue<'lua> for $float {
            const TYPE: LuaType = LuaType::Number;

            #[inline(always)]
            fn into_value(self) -> Value<'lua> {
                Value::Number(self as f64)
            }
            fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
                if let Value::Number(it) = wrapped {
                    Ok(it as $float)
                } else if let Value::Integer(it) = wrapped {
                    Ok(it as $float)
                }  else {
                    Err((ConversionError {
                        from: wrapped.type_name(),
                        to: stringify!($float),
                    }, wrapped))
                }
            }
        })+
    };
}
float_is_val![f32, f64];

macro_rules! lifetimed_is_val {
    ($($name: ident),+) => {
        $(impl<'lua> IsValue<'lua> for mlua::$name<'lua> {
            const TYPE: LuaType = LuaType::$name;

            #[inline(always)]
            fn into_value(self) -> Value<'lua> {
                Value::$name(self)
            }
            fn from_value(
                wrapped: Value<'lua>,
            ) -> Result<Self, (ConversionError, Value<'lua>)> {
                if let Value::$name(it) = wrapped {
                    Ok(it)
                } else {
                    Err((
                        ConversionError {
                            from: wrapped.type_name(),
                            to: LuaType::$name.name(),
                        },
                        wrapped,
                    ))
                }
            }
        })+
    };
}
lifetimed_is_val!(Table, Function, Thread);

impl<'lua> IsValue<'lua> for mlua::String<'lua> {
    const TYPE: LuaType = LuaType::String;

    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::String(self)
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if let Value::String(it) = wrapped {
            Ok(it)
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: Value::Nil.type_name(),
                },
                wrapped,
            ))
        }
    }
}
// moving Rust string types requires context

impl<'lua> IsValue<'lua> for mlua::AnyUserData<'lua> {
    const TYPE: LuaType = LuaType::UserData;
    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::UserData(self)
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if let Value::UserData(it) = wrapped {
            Ok(it)
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: LuaType::UserData.name(),
                },
                wrapped,
            ))
        }
    }
}
impl<'lua> IsValue<'lua> for mlua::Error {
    const TYPE: LuaType = LuaType::Error;
    #[inline(always)]
    fn into_value(self) -> Value<'lua> {
        Value::Error(self)
    }
    fn from_value(wrapped: Value<'lua>) -> Result<Self, (ConversionError, Value<'lua>)> {
        if let Value::Error(it) = wrapped {
            Ok(it)
        } else {
            Err((
                ConversionError {
                    from: wrapped.type_name(),
                    to: LuaType::Error.name(),
                },
                wrapped,
            ))
        }
    }
}

/// Mediates conversion of _one or many_ Lua arguments into structs.
pub trait FromArgPack<'lua>: Sized {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self>;
    #[inline]
    fn convert_value(value: Value<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let mut args = ArgumentContext::new(MultiValue::from_iter([value]), None, None);
        Self::convert(&mut args, lua)
    }
}

#[macro_export]
macro_rules! from_lua_argpack {
    ($($T: ty),+) => {
        $(
        impl<'lua> FromArgPack<'lua> for $T {
            fn convert(args: &mut $crate::lua::ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<$T> {
                let (arg, was_none) = match args.try_pop() {
                    Some(it) => (it, false),
                    None => (mlua::Nil, true),
                };
                match <$T>::from_lua(arg.clone(), lua) {
                    Ok(it) => Ok(it),
                    Err(err) => {
                        if !was_none {
                            args.revert(arg);
                        }
                        Err(args.bad_argument(err))
                    }
                }
            }
        }
        )+
    };
}

#[rustfmt::skip]
from_lua_argpack![
    bool,
    u8, u16, u32, u64, usize,
    i8, i16, i32, i64, isize,
    f32, f64,
    String
];

impl<'lua> FromArgPack<'lua> for MultiValue<'lua> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        Ok(MultiValue::from_vec(args.pop_all()))
    }
}
impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for Option<T> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        if let Some(()) = args.pop_typed::<()>() {
            return Ok(None);
        }
        Ok(Some(T::convert(args, lua)?))
    }
}
impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for Result<T, mlua::Error> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        if let Some(err) = args.pop_typed::<mlua::Error>() {
            return Ok(Err(err));
        }
        Ok(T::convert(args, lua))
    }
}
impl<'lua> FromArgPack<'lua> for Value<'lua> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        Ok(args.pop())
    }
}
impl<'lua> FromArgPack<'lua> for AnyUserData<'lua> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        args.pop_typed_or::<_, String>(None)
    }
}
impl<'lua> FromArgPack<'lua> for Table<'lua> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        args.pop_typed_or::<_, String>(None)
    }
}
impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for Vec<T> {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        let table = args.pop_typed_or::<Table<'lua>, String>(None)?;

        let mut result = Vec::with_capacity(table.len()? as usize);
        for it in table.sequence_values::<FromLuaCompat<T>>() {
            result.push(it.map(|it| it.0)?);
        }

        Ok(result)
    }
}

impl<'lua, T: FromArgPack<'lua>, const N: usize> FromArgPack<'lua> for [T; N] {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let table = args.pop();
        let result = Vec::<T>::convert_value(table.clone(), lua)?;
        match result.try_into() {
            Ok(it) => Ok(it),
            Err(it) => {
                let err = Error::FromLuaConversionError {
                    from: LuaType::Table.name(),
                    to: std::any::type_name::<[T; N]>(),
                    message: Some(format!("expected {N} values; got: {}", it.len())),
                };
                args.revert(table);
                Err(err)
            }
        }
    }
}

pub struct Unpacked<T>(T);
impl<T> Unpacked<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> std::ops::Deref for Unpacked<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> std::ops::DerefMut for Unpacked<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
pub struct MaybeUnpacked<T>(T);
impl<T> MaybeUnpacked<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> std::ops::Deref for MaybeUnpacked<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> std::ops::DerefMut for MaybeUnpacked<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for Unpacked<Vec<T>> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let mut result: Vec<T> = Vec::new();
        while let Ok(value) = T::convert(args, lua) {
            result.push(value);
        }
        Ok(Unpacked(result))
    }
}
pub(crate) type NoneOrMany<T> = Unpacked<Vec<T>>;

impl<'lua, T: FromArgPack<'lua>> FromArgPack<'lua> for MaybeUnpacked<Vec<T>> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let first_arg = args.pop();
        match Vec::<T>::convert_value(first_arg.clone(), lua) {
            Ok(result) => return Ok(MaybeUnpacked(result)),
            Err(_) => args.revert(first_arg),
        };
        Ok(MaybeUnpacked(Unpacked::<Vec<T>>::convert(args, lua)?.0))
    }
}

impl<'lua, T: FromArgPack<'lua>, const N: usize> FromArgPack<'lua> for Unpacked<[T; N]> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let mut result: MaybeUninit<[T; N]> = MaybeUninit::uninit();
        unsafe {
            let initial = args.clone();
            for i in 0..N {
                match T::convert(args, lua) {
                    Ok(value) => {
                        (result.as_mut_ptr() as *mut T).add(i).write(value);
                    }
                    Err(err) => {
                        *args = initial;
                        return Err(err);
                    }
                }
            }

            Ok(Unpacked(result.assume_init()))
        }
    }
}
impl<'lua, T: FromArgPack<'lua>, const N: usize> FromArgPack<'lua> for MaybeUnpacked<[T; N]> {
    fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let first_arg = args.pop();
        match <[T; N]>::convert_value(first_arg.clone(), lua) {
            Ok(result) => return Ok(MaybeUnpacked(result)),
            Err(_) => args.revert(first_arg),
        };
        Ok(MaybeUnpacked(Unpacked::<[T; N]>::convert(args, lua)?.0))
    }
}

// FIXME: Reverse tuples on error
macro_rules! from_arg_pack_tuple {
    ($($A:ident),*) => {
        impl<'lua$(,$A)*> FromArgPack<'lua> for ($($A,)*)
        where
            $($A: FromArgPack<'lua>,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
                $(
                    let $A = $A::convert(args, lua)?;
                )*
                return Ok(($($A,)*));
            }
        }
    };
}

macro_rules! smaller_tuples_too {
    ($m: ident, $ty: ident) => {
        $m!{}
        $m!{$ty}
    };
    ($m: ident, $ty: ident, $($tt: ident),*) => {
        smaller_tuples_too!{$m, $($tt),*}
        $m!{$ty, $($tt),*}
    };
}

#[rustfmt::skip]
smaller_tuples_too!(
    from_arg_pack_tuple, O, N, M, L, K, J, I, H, G, F, E, D, C, B, A
);

/// Struct that allows [`FromArgPack`] to be used wherever [`FromLua`] is required.
struct FromLuaCompat<T>(T);
impl<'lua, T: FromArgPack<'lua>> FromLua<'lua> for FromLuaCompat<T> {
    fn from_lua(value: Value<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        let mut args = ArgumentContext::new(MultiValue::from_iter([value]), None, None);
        match T::convert(&mut args, lua) {
            Ok(it) => Ok(FromLuaCompat(it)),
            Err(err) => match err {
                Error::BadArgument { cause, .. } => Err(cause.as_ref().clone()),
                other => Err(other),
            },
        }
    }
}

pub trait FromClonedUD<'lua>: UserData + Clone + 'static {
    fn from_cloned_data(ud: AnyUserData<'lua>) -> LuaResult<Self> {
        ud.borrow()
            .map(|it: std::cell::Ref<'_, Self>| (*it).clone())
    }
}
impl<'lua, D: FromClonedUD<'lua> + 'static> FromArgPack<'lua> for D {
    fn convert(args: &mut ArgumentContext<'lua>, _: &'lua Lua) -> LuaResult<Self> {
        let ud = args.pop_typed_or::<AnyUserData, _>(Some(format!(
            "expected {}",
            std::any::type_name::<D>()
        )))?;

        if !ud.is::<D>() {
            args.revert(ud);
            return Err(args.bad_argument(mlua::Error::FromLuaConversionError {
                from: LuaType::UserData.name(),
                to: std::any::type_name::<D>(),
                message: Some("incorrect user data type".to_string()),
            }));
        }

        FromClonedUD::from_cloned_data(ud)
    }
}

/// Represents composite types that can be converted from a [`MultiValue`]
/// through [`FromArgPack`] trait.
pub trait FromArgs<'lua>: Sized {
    fn from_arguments(
        args: MultiValue<'lua>,
        lua: &'lua Lua,
        call_name: Option<&'static str>,
        argument_names: ArgumentNames,
    ) -> LuaResult<Self>;
}

macro_rules! from_args_impl {
    ($($A:ident),*) => {
        impl<'lua$(,$A)*> FromArgs<'lua> for ($($A,)*)
        where
            $($A: FromArgPack<'lua>,)*
        {
            #[allow(non_snake_case, unused_variables, unused_mut)]
            fn from_arguments(
                args: MultiValue<'lua>,
                lua: &'lua Lua,
                call_name: Option<&'static str>,
                argument_names: ArgumentNames,
            ) -> LuaResult<Self> {
                let mut args = ArgumentContext::new(args, argument_names, call_name);
                $(
                    let $A = $A::convert(&mut args, lua)?;
                    args.advance_name();
                )*
                return Ok(($($A,)*));
            }
        }
    };
}

smaller_tuples_too!(from_args_impl, O, N, M, L, K, J, I, H, G, F, E, D, C, B, A);

pub trait ContextExt<'lua> {
    fn create_table_from_vec<T: IntoLua<'lua> + Send>(
        &'lua self,
        vec: Vec<T>,
    ) -> LuaResult<Table<'lua>>;
}

impl<'lua> ContextExt<'lua> for Lua {
    #[inline]
    fn create_table_from_vec<T: IntoLua<'lua> + Send>(
        &'lua self,
        vec: Vec<T>,
    ) -> LuaResult<Table<'lua>> {
        self.create_table_from(
            vec.into_iter()
                .enumerate()
                .map(|(i, it)| (i as Integer, it)),
        )
    }
}

pub struct LuaArray<'lua>(Vec<Value<'lua>>);

#[allow(dead_code)]
impl<'lua> LuaArray<'lua> {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_iter<T: FromLua<'lua>>(self, lua: &'lua Lua) -> impl Iterator<Item = T> + '_ {
        self.0
            .into_iter()
            .filter_map(|it| T::from_lua(it, lua).ok())
    }
}

impl<'lua> From<Table<'lua>> for LuaArray<'lua> {
    fn from(value: Table<'lua>) -> Self {
        LuaArray(
            value
                .sequence_values::<Value<'lua>>()
                .map(Result::unwrap)
                .collect(),
        )
    }
}

impl<'lua> FromLua<'lua> for LuaArray<'lua> {
    fn from_lua(value: Value<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        Ok(Self::from(Table::from_lua(value, lua)?))
    }
}

pub trait TableExt<'lua> {
    fn entry_count(&self) -> usize;
    fn seq_length(&self) -> usize;
    fn is_pure_sequence(&self) -> bool;
    fn is_homogeneous_sequence<T: FromArgPack<'lua>>(&self) -> bool;

    fn get_user_data<K: IntoLua<'lua>, D: UserData + Clone + 'static>(
        &self,
        key: K,
    ) -> LuaResult<D>;

    fn try_get<K: IntoLua<'lua>, V: FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
    ) -> LuaResult<Option<V>>;

    #[inline]
    fn try_get_or<K: IntoLua<'lua>, V: FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
        default: V,
    ) -> LuaResult<V> {
        self.try_get(key, lua).map(|it| it.unwrap_or(default))
    }

    #[inline]
    fn try_get_or_default<K: IntoLua<'lua>, V: Default + FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
    ) -> LuaResult<V> {
        self.try_get_or(key, lua, V::default())
    }
}

impl<'lua> TableExt<'lua> for Table<'lua> {
    fn entry_count(&self) -> usize {
        self.clone().pairs::<Value<'lua>, Value<'lua>>().count()
    }
    fn seq_length(&self) -> usize {
        self.clone().sequence_values::<Value<'lua>>().count()
    }
    fn is_pure_sequence(&self) -> bool {
        self.entry_count() == self.seq_length()
    }
    fn is_homogeneous_sequence<T: FromArgPack<'lua>>(&self) -> bool {
        self.entry_count()
            == self
                .clone()
                .sequence_values::<FromLuaCompat<T>>()
                .filter(Result::is_ok)
                .count()
    }

    fn get_user_data<K: IntoLua<'lua>, D: UserData + Clone + 'static>(
        &self,
        key: K,
    ) -> LuaResult<D> {
        match self.get(key)? {
            Value::UserData(data) => data.borrow().map(|it: std::cell::Ref<'_, D>| (*it).clone()),
            other => Err(mlua::Error::FromLuaConversionError {
                from: other.type_name(),
                to: std::any::type_name::<D>(),
                message: None,
            }),
        }
    }

    fn try_get<K: IntoLua<'lua>, V: FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
    ) -> LuaResult<Option<V>> {
        match self.get::<K, Value>(key) {
            Ok(Value::Nil) => Ok(None),
            Ok(other) => FromLuaCompat::<V>::from_lua(other, lua).map(|it| Some(it.0)),
            Err(err) => Err(err),
        }
    }
}

/// Mapping and unwrapping utilities for [`Option`]al values.
pub trait FromLuaOption<T>: Sized {
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
impl<'lua, T, W: WrapperT<'lua, Wrapped = T>> FromLuaOption<T> for LuaFallible<W> {
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
pub(crate) trait FromLuaResult<T>: Sized {
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

/// Allows declaring a wrapper type and automatically implementing all of the
/// remaining utility traits on that type, as well as wrappers around it.
pub trait WrapperT<'lua> {
    type Wrapped;
    fn unwrap(self) -> Self::Wrapped;
}

/// Applies TableExt to reading table values with wrapper types, automatically
/// handling unwrapping.
pub trait TableWrapperExt<'lua>: TableExt<'lua> {
    #[inline(always)]
    fn try_get_t<K: IntoLua<'lua>, W: WrapperT<'lua> + FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
    ) -> LuaResult<Option<W::Wrapped>> {
        TableExt::try_get(self, key, lua).map(|result| result.map(W::unwrap))
    }

    #[inline(always)]
    fn try_get_or_t<K: IntoLua<'lua>, W: WrapperT<'lua> + FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
        default: W::Wrapped,
    ) -> LuaResult<W::Wrapped> {
        self.try_get_t::<K, W>(key, lua)
            .map(|it| it.unwrap_or(default))
    }

    #[inline(always)]
    fn try_get_or_default_t<K: IntoLua<'lua>, W: WrapperT<'lua> + FromArgPack<'lua>>(
        &self,
        key: K,
        lua: &'lua Lua,
    ) -> LuaResult<W::Wrapped>
    where
        W::Wrapped: Default,
    {
        self.try_get_or_t::<K, W>(key, lua, W::Wrapped::default())
    }
}

impl<'lua> TableWrapperExt<'lua> for Table<'lua> {}

#[macro_export]
macro_rules! wrap_skia_handle {
    ($handle: ty) => {
        paste::paste! {
            #[derive(Clone)]
            pub struct [<Lua $handle>](pub $handle);

            impl From<$handle> for [<Lua $handle>] {
                fn from(value: $handle) -> [<Lua $handle>] {
                    [<Lua $handle>](value)
                }
            }
            impl From<[<Lua $handle>]> for $handle {
                fn from(value: [<Lua $handle>]) -> $handle {
                    value.0
                }
            }
            impl AsRef<$handle> for [<Lua $handle>] {
                fn as_ref(&self) -> &$handle {
                    &self.0
                }
            }
            impl<'lua> $crate::lua::WrapperT<'lua> for [<Lua $handle>] {
                type Wrapped = $handle;

                #[inline]
                fn unwrap(self) -> $handle {
                    self.0
                }
            }
            impl<'lua> FromClonedUD<'lua> for [<Lua $handle>] {}
        }
    };
}

#[macro_export]
macro_rules! type_like {
    ($handle: ty) => {
        paste::paste! {
            #[derive(Clone)]
            pub struct [<Like $handle>]([<Lua $handle>]);

            impl From<[<Like $handle>]> for [<Lua $handle>] {
                fn from(value: [<Like $handle>]) -> [<Lua $handle>] {
                    value.0
                }
            }
            impl<'lua> $crate::lua::WrapperT<'lua> for [<Like $handle>] {
                type Wrapped = $handle;

                #[inline]
                fn unwrap(self) -> $handle {
                    self.0.0
                }
            }
        }
    };
}

#[macro_export]
macro_rules! type_like_table {
    ($handle: ty: |$ident: ident: LuaTable, $ctx: ident: &'lua Lua| $body: block) => {
        type_like!($handle);
        paste::paste! {
            impl<'lua> TryFrom<(mlua::Table<'lua>, &'lua mlua::Lua)> for [<Lua $handle>] {
                type Error = mlua::Error;

                fn try_from(($ident, $ctx): (mlua::Table<'lua>, &'lua mlua::Lua)) -> Result<Self, Self::Error> $body
            }
            impl<'lua> FromLua<'lua> for [<Like $handle>] {
                fn from_lua(value: mlua::Value<'lua>, lua: &'lua mlua::Lua) -> mlua::Result<Self> {
                    let table = match value {
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
                    [<Lua $handle>]::try_from((table, lua)).map([<Like $handle>])
                }
            }
            impl<'lua> FromArgPack<'lua> for [<Like $handle>] {
                #[inline]
                fn convert(args: &mut ArgumentContext<'lua>, lua: &'lua Lua) -> mlua::Result<Self> {
                    [<Like $handle>]::from_lua(args.pop(), lua)
                }
            }
        }
    };
    ($handle: ty: |$ident: ident: LuaTable| $body: block) => {
        type_like_table!($handle: |$ident: LuaTable, _unused_lua_ctx: &'lua Lua| $body);
    }
}
