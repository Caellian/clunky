use std::path::{Path, PathBuf};

use rlua::{Lua, Function, RegistryKey};
use crate::{error::{Result, ClunkyError, Detail}, layout::Layout};

static LAYOUT_FN: &str = "layout";

pub struct Context {
    source: PathBuf,
    lua: Lua,

    layout_fn: RegistryKey,
}

impl Context {
    pub fn new(path: impl AsRef<Path>) -> Result<Context> {
        let canonical_path = path.as_ref().canonicalize().expect("unable to canonicalize source file");
        let init_script = std::fs::read_to_string(path.as_ref()).expect("unable to read init script");

        let lua = Lua::new();

        let layout_fn = lua.context::<_, Result<RegistryKey>>(|lua_ctx| {
            let g = lua_ctx.globals();

            let add_component_fn = lua_ctx.create_function(bindings::add_component)
                .expect("invalid add_component binding");
            g.set("add_component", add_component_fn)?;

            if let Some(file_name) = path.as_ref().to_str() {
                g.set("_name", file_name)?;
                g.set("_logger_name", file_name)?;
            }
            if let Some(parent) = canonical_path.parent() {
                if let Some(parent) = parent.to_str() {
                    g.set("_dir", parent)?;
                } else {
                    log::warn!("unable to determine script parent directory, '_dir' will not be defined")
                }
            }

            lua_ctx.load(&init_script)
                .set_name(path.as_ref().to_str().unwrap_or("user script"))
                .expect("invalid user script name")
                .exec()?;

            let layout_fn: Function = g.get(LAYOUT_FN).expect(&format!("user script has no {} function", LAYOUT_FN));
            Ok(lua_ctx.create_registry_value(layout_fn)?)
        })?;
    
        Ok(Context { source: path.as_ref().to_path_buf(), lua, layout_fn })
    }

    pub fn update_layout(&self, layout: &mut Layout) -> Result<()> {
        self.lua.context::<_, Result<()>>(|lua_ctx| {
            let layout_fn: Function = lua_ctx.registry_value(&self.layout_fn)?;
            layout_fn.call::<(), ()>(())?;
            Ok(())
        })?;

        Ok(())
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.lua.context(|ctx| {
            ctx.expire_registry_values();
        })
    }
}

mod bindings {
    use rlua::{Context as LContext, Result as LResult, Table};

    use crate::{component::{try_component_from_lua_table, Component}};

    pub fn add_component<'l>(c: LContext<'l>, table: Table) -> LResult<()> {
        let component: Box<dyn Component> = try_component_from_lua_table(&table).map_err(|err| err.into())?;

        log::info!("'{}' component added", component.component_type_name());
    
        Ok(())
    }    
}