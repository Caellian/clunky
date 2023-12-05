pub mod skia {
    use std::ptr::{addr_of, addr_of_mut};

    use skia_safe::{Matrix, M44};
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("invalid number of matrix values, expected {expected} values; found: {found}")]
    pub struct BadSize {
        expected: usize,
        found: usize,
    }

    pub trait MatrixExt: Sized {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize>;
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize>;
        fn as_slice(&self) -> &[f32];
        fn as_slice_mut(&mut self) -> &mut [f32];
        fn to_vec(&self) -> Vec<f32> {
            self.as_slice().to_vec()
        }
    }

    impl MatrixExt for Matrix {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize> {
            if values.len() != 9 {
                return Err(BadSize {
                    expected: 9,
                    found: values.len(),
                });
            }
            let mut result = Matrix::new_identity();

            result.as_slice_mut().copy_from_slice(&values);
            Ok(result)
        }

        #[inline]
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize> {
            let values: Vec<f32> = iter.into_iter().take(9).collect();
            Self::from_vec(values)
        }

        #[inline]
        fn as_slice(&self) -> &[f32] {
            unsafe {
                (addr_of!(*self) as *mut [f32; 9])
                    .as_ref()
                    .unwrap_unchecked()
            }
        }

        #[inline]
        fn as_slice_mut(&mut self) -> &mut [f32] {
            unsafe {
                (addr_of_mut!(*self) as *mut [f32; 9])
                    .as_mut()
                    .unwrap_unchecked()
            }
        }
    }

    impl MatrixExt for M44 {
        fn from_vec(values: Vec<f32>) -> Result<Self, BadSize> {
            if values.len() != 16 {
                return Err(BadSize {
                    expected: 16,
                    found: values.len(),
                });
            }
            let mut result = M44::new_identity();
            result.as_slice_mut().copy_from_slice(&values);
            Ok(result)
        }

        #[inline]
        fn from_iter<I: IntoIterator<Item = f32>>(iter: I) -> Result<Self, BadSize> {
            let values: Vec<f32> = iter.into_iter().take(16).collect();
            Self::from_vec(values)
        }

        #[inline]
        fn as_slice(&self) -> &[f32] {
            unsafe {
                (addr_of!(*self) as *mut [f32; 16])
                    .as_ref()
                    .unwrap_unchecked()
            }
        }

        #[inline]
        fn as_slice_mut(&mut self) -> &mut [f32] {
            unsafe {
                (addr_of_mut!(*self) as *mut [f32; 16])
                    .as_mut()
                    .unwrap_unchecked()
            }
        }
    }
}

pub mod rlua {
    use rlua::{Context, Error, FromLua, Integer, Table, ToLua, Value};

    pub trait ContextExt<'lua> {
        fn create_table_from_vec<T: ToLua<'lua> + Send>(
            &self,
            vec: Vec<T>,
        ) -> Result<Table<'lua>, Error>;
    }

    impl<'lua> ContextExt<'lua> for Context<'lua> {
        #[inline]
        fn create_table_from_vec<T: ToLua<'lua> + Send>(
            &self,
            vec: Vec<T>,
        ) -> Result<Table<'lua>, Error> {
            self.create_table_from(
                vec.into_iter()
                    .enumerate()
                    .map(|(i, it)| (i as Integer, it)),
            )
        }
    }

    pub trait TableExt<'lua> {
        fn try_get<K: ToLua<'lua>, V: FromLua<'lua>>(
            &self,
            key: K,
            lua: Context<'lua>,
        ) -> Result<Option<V>, Error>;

        #[inline]
        fn try_get_or<K: ToLua<'lua>, V: FromLua<'lua>>(
            &self,
            key: K,
            lua: Context<'lua>,
            default: V,
        ) -> Result<V, Error> {
            self.try_get(key, lua).map(|it| it.unwrap_or(default))
        }

        #[inline]
        fn try_get_or_default<K: ToLua<'lua>, V: Default + FromLua<'lua>>(
            &self,
            key: K,
            lua: Context<'lua>,
        ) -> Result<V, Error> {
            self.try_get_or(key, lua, V::default())
        }
    }

    impl<'lua> TableExt<'lua> for Table<'lua> {
        fn try_get<K: ToLua<'lua>, V: FromLua<'lua>>(
            &self,
            key: K,
            lua: Context<'lua>,
        ) -> Result<Option<V>, Error> {
            match self.get::<K, Value>(key) {
                Ok(Value::Nil) => Ok(None),
                Ok(other) => V::from_lua(other, lua).map(Some),
                Err(err) => Err(err),
            }
        }
    }

    pub mod combinators {
        use super::*;
        use std::any::type_name;
        use std::sync::OnceLock;

        pub enum NeverT {}
        impl<'lua> FromLua<'lua> for NeverT {
            fn from_lua(lua_value: Value<'lua>, _: Context<'lua>) -> Result<Self, Error> {
                Err(Error::FromLuaConversionError {
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
                    fn from_lua(value: Value<'lua>, lua: Context<'lua>) -> Result<Self, Error> {
                        $(
                            if type_name::<$ts>() != type_name::<NeverT>() { // optimize NeverT checks away
                                if let Ok(it) = $ts::from_lua(value.clone(), lua) {
                                    return Ok(Self::$ts(it));
                                }
                            }
                        )*

                        Err(Error::FromLuaConversionError {
                            from: value.type_name(),
                            to: Self::target_t(),
                            message: Some(format!("unable to convert into any of expected types: {}", Self::expected_types()))
                        })
                    }
                }
            };
        }

        impl_one_of!(A B C D E F G H I J K L M N O P);
    }
}
