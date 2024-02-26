use std::{
    fmt::{Display, Write},
    path::PathBuf,
    sync::Arc,
};

use thiserror::Error;

pub type LuaError = mlua::Error;

#[derive(Clone, Debug)]
pub struct Detail(pub Option<String>);
impl std::fmt::Display for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(detail) = &self.0 {
            f.write_str(" (")?;
            f.write_str(detail)?;
            f.write_char(')')
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum ValueType {
    Number,
    String,
}

impl Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueType::Number => f.write_str("number"),
            ValueType::String => f.write_str("string"),
        }
    }
}

#[macro_export]
macro_rules! unknown_component {
    ($found: expr) => {
        ClunkyError::UnknownComponent {
            found: $found.clone(),
            detail: Detail(None),
        }
        .into()
    };
    ($found: expr, $detail: expr) => {
        ClunkyError::UnknownComponent {
            found: $found.clone(),
            detail: Detail(Some($detail.clone())),
        }
        .into()
    };
}

#[derive(Debug, Error)]
pub enum FrameBufferError {
    #[error("can't move framebuffer while it's being writen to by {0} threads")]
    MmapInUse(u32),
    #[error(transparent)]
    IO(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error(transparent)]
    #[cfg(feature = "wayland")]
    WaylandConnect(#[from] wayland_client::ConnectError),
    #[error(transparent)]
    #[cfg(feature = "wayland")]
    WaylandDispatch(#[from] wayland_client::DispatchError),
}

#[derive(Debug, Error)]
pub enum ClunkyError {
    #[error("invalid script path: {0}")]
    InvalidScript(PathBuf),
    #[error("empty component type string")]
    EmptyComponentType,
    #[error("unknown component type '{found}'{detail}")]
    UnknownComponent { found: String, detail: Detail },
    #[error("missing '{name}' (type: {value}) field in component table")]
    MissingComponentProperty {
        name: &'static str,
        value: ValueType,
    },

    #[error(transparent)]
    FrameBuffer(#[from] FrameBufferError),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Lua(#[from] mlua::Error),
    #[error(transparent)]
    IO(#[from] std::io::Error),
}

impl From<ClunkyError> for mlua::Error {
    fn from(val: ClunkyError) -> Self {
        match val {
            ClunkyError::Lua(err) => err,
            other => mlua::Error::ExternalError(Arc::new(other)),
        }
    }
}

pub type Result<T> = std::result::Result<T, ClunkyError>;
