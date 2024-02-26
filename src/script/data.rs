use std::{collections::HashMap, sync::Arc};

use mlua::prelude::{Lua, LuaError, LuaRegistryKey as RegistryKey, LuaResult, LuaTable, LuaValue};

use super::{
    events::{EventBuffer, EventChannel, EventData, Status},
    ScriptContext,
};

#[derive(Debug, Clone)]
pub struct CollectorCallback(Arc<RegistryKey>);

impl CollectorCallback {
    pub fn value<'lua>(&self, ctx: &'lua Lua) -> LuaValue<'lua> {
        ctx.registry_value(self.0.as_ref())
            .expect("callback destroyed while running")
    }
}

type CollectedEntries = HashMap<String, RegistryKey>;

#[derive(Debug)]
pub struct DataCollectors {
    pub collectors: HashMap<String, CollectorCallback>,
    pub state: CollectedEntries,
}

impl Default for DataCollectors {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl DataCollectors {
    pub fn new() -> Self {
        DataCollectors {
            collectors: HashMap::with_capacity(16),
            state: HashMap::with_capacity(16),
        }
    }

    pub fn new_lua_collectors<'lua>(lua: &'lua Lua, list: LuaTable<'lua>) -> LuaResult<Self> {
        let mut result = Self::new();
        let collectors = &mut result.collectors;

        for entry in list.pairs::<String, LuaValue>() {
            let (name, fun) = match entry {
                Ok(it) => it,
                Err(inner) => {
                    return Err(LuaError::CallbackError {
                        traceback: "data collector table must contain only str->value entries"
                            .to_string(),
                        cause: Arc::new(inner),
                    })
                }
            };

            let fn_key = lua.create_registry_value(fun)?;
            collectors.insert(name, CollectorCallback(Arc::new(fn_key)));
        }
        Ok(result)
    }

    pub fn init_state(
        &mut self,
        ctx: Option<&mut ScriptContext>,
        evb: &mut EventBuffer,
    ) -> LuaResult<()> {
        let ctx = match ctx {
            Some(it) => it,
            None => return Ok(()),
        };

        let table = ctx.lua().create_table()?;

        // retain previous values, if any
        for (name, key) in &self.state {
            if let Ok(value) = ctx.lua().registry_value::<LuaValue>(key) {
                table.set(name.as_str(), value)?;
            }
        }

        evb.schedule(self.collectors.iter().filter_map(|(name, callback)| {
            handle_callback(ctx, &table, &mut self.state, name, callback)
        }));

        let mut data = ctx.lua().create_registry_value(table)?;
        std::mem::swap(&mut ctx.collected_data, &mut data);
        ctx.lua().remove_registry_value(data)?;

        Ok(())
    }

    pub fn update_state(
        &mut self,
        ctx: Option<&mut ScriptContext>,
        evb: &mut EventBuffer,
    ) -> LuaResult<()> {
        let ctx = match ctx {
            Some(it) => it,
            None => return Ok(()),
        };

        let table = ctx.lua().create_table()?;

        // retain previous values, if any
        for (name, key) in &self.state {
            if let Ok(value) = ctx.lua().registry_value::<LuaValue>(key) {
                table.set(name.as_str(), value)?;
            }
        }

        let next: Vec<_> = evb
            .poll(EventChannel::DATA)
            .filter_map(|ev| match ev {
                EventData::DataUpdate { name, callback, .. } => {
                    handle_callback(ctx, &table, &mut self.state, &name, &callback)
                }
                _ => None,
            })
            .collect();
        evb.schedule(next);

        let mut data = ctx.lua().create_registry_value(table)?;
        std::mem::swap(&mut ctx.collected_data, &mut data);
        ctx.lua().remove_registry_value(data)?;

        Ok(())
    }
}

fn run_callback<'lua>(
    lua: &'lua Lua,
    name: &str,
    cb: &CollectorCallback,
) -> Option<(Status, LuaValue<'lua>)> {
    let value = cb.value(lua);
    let status = Status::default();

    let returned = match value {
        LuaValue::Function(callback) => match callback.call(status.clone()) {
            Ok(it) => it,
            Err(err) => {
                log::warn!("data collector callback for '{}' failed: {}", name, err);
                return None;
            }
        },
        other => other,
    };

    Some((status, returned))
}

fn handle_callback(
    ctx: &ScriptContext,
    results: &LuaTable,
    state: &mut CollectedEntries,
    name: &str,
    cb: &CollectorCallback,
) -> Option<EventData> {
    let lua = ctx.lua();

    let (status, value) = match run_callback(lua, name, cb) {
        Some(it) => it,
        None => return None,
    };

    match results.set(name, value.clone()) {
        Ok(()) => {}
        Err(error) => {
            log::error!("unable to update callback result table: {}", error)
        }
    }

    let next_event = status
        .next_update()
        .map(|next_update| EventData::DataUpdate {
            time: next_update,
            name: name.to_string(),
            callback: cb.clone(),
        });

    let new_key = match lua.create_registry_value(value) {
        Ok(it) => it,
        Err(error) => {
            log::warn!("unable to commit value to state: {}", error);
            return next_event;
        }
    };

    state.insert(name.to_string(), new_key);

    next_event
}
