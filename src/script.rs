use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    error::{ClunkyError, Detail, Result},
    settings::Settings,
};
use rlua::*;

static LAYOUT_FN: &str = "layout";

pub struct ScriptContext {
    source: PathBuf,
    lua: Lua,
}

impl ScriptContext {
    pub fn new(path: impl AsRef<Path>) -> Result<ScriptContext> {
        let canonical_path = path
            .as_ref()
            .canonicalize()
            .expect("unable to canonicalize source file");
        let init_script =
            std::fs::read_to_string(path.as_ref()).expect("unable to read init script");

        let lua = Lua::new();

        lua.context::<_, Result<()>>(|lua_ctx| {
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

            lua_ctx
                .load(&init_script)
                .set_name(path.as_ref().to_str().unwrap_or("user script"))
                .expect("invalid user script")
                .exec()?;

            /*
                let add_component_fn = lua_ctx
                    .create_function(bindings::add_component)
                    .expect("invalid add_component binding");
                g.set("add_component", add_component_fn)?;



                let layout_fn: Function = g
                    .get(LAYOUT_FN)
                    .expect(&format!("user script has no {} function", LAYOUT_FN));
                Ok(lua_ctx.create_registry_value(layout_fn)?)
            */
            crate::render::frontend::bindings::setup(lua_ctx)?;
            Ok(())
        })?;

        Ok(ScriptContext {
            source: path.as_ref().to_path_buf(),
            lua,
        })
    }

    pub fn load_settings(&self) -> Settings {
        self.lua.context(|lua_ctx| {
            lua_ctx
                .globals()
                .get("settings")
                .map(|it| Settings::load(lua_ctx, it))
                .unwrap_or_default()
        })
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

pub fn lua_is_eq<'lua, A: ToLua<'lua>, B: ToLua<'lua>>(ctx: &Context<'lua>, a: A, b: B) -> bool {
    // TODO: Remove when https://github.com/amethyst/rlua/issues/112 is resolved
    let check: Function<'lua> = ctx
        .load("function(a, b) return a == b end")
        .eval()
        .expect("invalid check expression");
    check.call((a, b)).unwrap_or_default()
}

pub fn lua_get_table_key<'lua, K: FromLua<'lua>, V: ToLua<'lua> + FromLua<'lua> + Clone>(
    ctx: &Context<'lua>,
    table: Table<'lua>,
    value: &V,
) -> Option<K> {
    for entry in table.pairs::<K, V>() {
        if let Ok((k, v)) = entry {
            if lua_is_eq(ctx, value.clone(), v) {
                return Some(k);
            }
        }
    }

    None
}
