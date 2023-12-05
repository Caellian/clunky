use std::path::{Path, PathBuf};

use crate::error::ClunkyError;
use rlua::prelude::*;
use settings::Settings;

pub mod data;
pub mod events;
pub mod settings;

pub struct ScriptContext {
    source: PathBuf,
    lua: Lua,
}

impl ScriptContext {
    pub fn new(path: impl AsRef<Path>) -> Result<ScriptContext, ClunkyError> {
        let canonical_path = path
            .as_ref()
            .canonicalize()
            .expect("unable to canonicalize source file");
        let init_script =
            std::fs::read_to_string(path.as_ref()).expect("unable to read init script");

        let lua = Lua::new();

        lua.context::<_, Result<(), ClunkyError>>(|lua_ctx| {
            let g = lua_ctx.globals();

            if let Some(file_name) = path.as_ref().to_str() {
                g.set("_name", file_name)?;
                g.set("_logger_name", file_name)?;
            }
            if let Some(parent) = canonical_path.parent() {
                if let Some(parent) = parent.to_str() {
                    g.set("_dir", parent)?;
                } else {
                    log::warn!(
                        "unable to determine script parent directory, '_dir' will not be defined"
                    )
                }
            }

            crate::render::frontend::bindings::setup(lua_ctx)?;

            lua_ctx
                .load(&init_script)
                .set_name(path.as_ref().to_str().unwrap_or("user script"))
                .expect("invalid user script")
                .exec()?;
            Ok(())
        })?;

        Ok(ScriptContext {
            source: path.as_ref().to_path_buf(),
            lua,
        })
    }

    pub fn load_settings(&self) -> Settings {
        let load_result = self.lua.context(|lua_ctx| {
            lua_ctx
                .globals()
                .get("settings")
                .and_then(|it| Settings::load(lua_ctx, it))
        });

        match load_result {
            Ok(it) => it,
            Err(err) => {
                panic!("unable to load settings: {}", err)
            }
        }
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

impl Drop for ScriptContext {
    fn drop(&mut self) {
        self.lua.context(|ctx| {
            ctx.expire_registry_values();
        })
    }
}

pub fn lua_is_eq<'lua, A: ToLua<'lua>, B: ToLua<'lua>>(ctx: &LuaContext<'lua>, a: A, b: B) -> bool {
    // TODO: Remove when https://github.com/amethyst/rlua/issues/112 is resolved
    let check: LuaFunction<'lua> = ctx
        .load("function(a, b) return a == b end")
        .eval()
        .expect("invalid check expression");
    check.call((a, b)).unwrap_or_default()
}

pub use rlua_skia::ext::rlua as ext;
