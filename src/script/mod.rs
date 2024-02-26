use std::path::{Path, PathBuf};

use crate::{error::ClunkyError, util::ErrHandleExt};
use mlua::prelude::*;
use settings::Settings;

pub mod data;
pub mod events;
pub mod settings;

pub struct ScriptContext {
    source: PathBuf,
    lua: Lua,
    pub settings: Settings,
    pub collected_data: LuaRegistryKey,
}

impl ScriptContext {
    pub fn new(path: impl AsRef<Path>) -> Result<ScriptContext, ClunkyError> {
        let canonical_path = path
            .as_ref()
            .canonicalize()
            .map_err(|_| ClunkyError::InvalidScript(path.as_ref().to_path_buf()))?;
        let init_script = std::fs::read_to_string(path.as_ref())
            .map_err(|_| ClunkyError::InvalidScript(path.as_ref().to_path_buf()))?;

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
            .exec()
            .some_or_log(None);

        let collected_data = lua.create_registry_value(lua.create_table()?)?;

        let settings = lua
            .globals()
            .get("settings")
            .and_then(|it| Settings::load(&lua, it))
            .some_or_log(Some("script missing 'settings' global".to_string()))
            .unwrap_or_default();

        Ok(ScriptContext {
            source: canonical_path,
            lua,
            settings,
            collected_data,
        })
    }

    pub fn reload(&mut self, path: impl AsRef<Path>) -> Result<(), ClunkyError> {
        self.lua.expire_registry_values();
        let init_script = std::fs::read_to_string(&self.source)
            .map_err(|_| ClunkyError::InvalidScript(path.as_ref().to_path_buf()))?;

        self.lua
            .load(&init_script)
            .set_name(self.source.to_str().unwrap_or("user script"))
            .exec()
            .some_or_log(None);

        self.settings = self
            .lua
            .globals()
            .get("settings")
            .and_then(|it| Settings::load(&self.lua, it))
            .some_or_log(Some("script missing 'settings' global".to_string()))
            .unwrap_or_default();

        Ok(())
    }

    #[inline(always)]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn draw_fn(&self) -> Option<LuaFunction> {
        self.settings
            .draw
            .as_ref()
            .and_then(|it| self.lua.registry_value(it).ok())
    }

    pub fn collected_data(&self) -> LuaResult<LuaTable> {
        self.lua.registry_value(&self.collected_data)
    }

    #[inline(always)]
    pub fn path(&self) -> &Path {
        self.source.as_path()
    }
}

impl Drop for ScriptContext {
    fn drop(&mut self) {
        self.lua.expire_registry_values();
    }
}
