use rlua::prelude::*;

use crate::ext::rlua::TableExt;

/// Allows declaring a wrapper type and automatically implementing all of the
/// remaining utility traits on that type and wrappers around it.
pub(crate) trait WrapperT<'lua> {
    type Wrapped;
    fn unwrap(self) -> Self::Wrapped;
}

/// Mapping and unwrapping utilities for [`Option`]al values.
pub(crate) trait FromLuaOption<T>: Sized {
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

/// Applies TableExt to reading table values with wrapper types, automatically
/// handling unwrapping.
pub(crate) trait TableWrapperExt<'lua>: TableExt<'lua> {
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

#[macro_export]
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
            impl<'lua> crate::wrap::WrapperT<'lua> for [<Lua $handle>] {
                type Wrapped = $handle;

                #[inline]
                fn unwrap(self) -> $handle {
                    self.0
                }
            }
        }
    };
}

#[macro_export]
macro_rules! type_like {
    ($handle: ty) => {
        paste::paste! {
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
            impl<'lua> crate::wrap::WrapperT<'lua> for [<Like $handle>] {
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
