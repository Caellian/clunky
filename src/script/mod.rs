use std::path::{Path, PathBuf};

use crate::error::ClunkyError;
use mlua::prelude::*;
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

        let lua = Lua::new_with(LuaStdLib::ALL_SAFE, LuaOptions::new())
            .expect("unable to construct Lua context");

        let g = lua.globals();

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
        drop(g);

        crate::render::frontend::bindings::setup(&lua)?;

        lua.load(&init_script)
            .set_name(path.as_ref().to_str().unwrap_or("user script"))
            .exec()?;

        Ok(ScriptContext {
            source: path.as_ref().to_path_buf(),
            lua,
        })
    }

    pub fn load_settings(&self) -> Settings {
        let load_result = self
            .lua
            .globals()
            .get("settings")
            .and_then(|it| Settings::load(&self.lua, it));

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
        self.lua.expire_registry_values();
    }
}

pub fn lua_is_eq<'lua, A: IntoLua<'lua>, B: IntoLua<'lua>>(ctx: &'lua Lua, a: A, b: B) -> bool {
    // TODO: Remove when https://github.com/amethyst/rlua/issues/112 is resolved
    let check: LuaFunction<'lua> = ctx
        .load("function(a, b) return a == b end")
        .eval()
        .expect("invalid check expression");
    check.call((a, b)).unwrap_or_default()
}
