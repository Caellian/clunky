use std::{collections::HashMap, sync::Arc};

use drain_filter_polyfill::VecExt;
use rlua::prelude::{
    LuaContext, LuaError, LuaRegistryKey as RegistryKey, LuaResult, LuaTable, LuaValue,
};

use crate::script::events::Consumer;

use super::events::{EventData, Status};

#[derive(Debug, Clone)]
pub struct CollectorCallback(Arc<RegistryKey>);

impl CollectorCallback {
    pub fn value<'lua>(&self, ctx: &LuaContext<'lua>) -> LuaValue<'lua> {
        ctx.registry_value(self.0.as_ref())
            .expect("callback destroyed while running")
    }
}

type CollectorState = HashMap<String, RegistryKey>;

#[derive(Debug)]
pub struct DataCollectors {
    pub collectors: HashMap<String, CollectorCallback>,
    pub state: CollectorState,
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

    pub fn new_lua_collectors<'lua>(
        lua: LuaContext<'lua>,
        list: LuaTable<'lua>,
    ) -> LuaResult<Self> {
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

    pub fn update_state<'lua>(
        &mut self,
        ctx: LuaContext<'lua>,
        scheduled: Option<&mut Vec<EventData>>,
    ) -> LuaResult<(RegistryKey, Vec<EventData>)> {
        let table = ctx.create_table()?;

        // retain previous values, if any
        for (name, key) in &self.state {
            if let Ok(value) = ctx.registry_value::<LuaValue>(&key) {
                table.set(name.as_str(), value)?;
            }
        }

        fn run_callback<'lua>(
            ctx: LuaContext<'lua>,
            name: &str,
            cb: &CollectorCallback,
        ) -> Option<(Status, LuaValue<'lua>)> {
            let value = cb.value(&ctx);
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

        fn handle_callback<'lua>(
            ctx: LuaContext<'lua>,
            results: &LuaTable<'lua>,
            state: &mut CollectorState,
            name: &str,
            cb: &CollectorCallback,
        ) -> Option<EventData> {
            let (status, value) = match run_callback(ctx, &name, cb) {
                Some(it) => it,
                None => return None,
            };

            match results.set(name, value.clone()) {
                Ok(()) => {}
                Err(error) => {
                    log::error!("unable to update callback result table: {}", error)
                }
            }

            let next_event = if let Some(next_update) = status.next_update() {
                Some(EventData::DataUpdate {
                    time: next_update,
                    name: name.to_string(),
                    callback: cb.clone(),
                })
            } else {
                None
            };

            let new_key = match ctx.create_registry_value(value) {
                Ok(it) => it,
                Err(error) => {
                    log::warn!("unable to commit value to state: {}", error);
                    return next_event;
                }
            };

            state.insert(name.to_string(), new_key);

            next_event
        }

        let scheduled_next: Vec<_> = if let Some(scheduled) = scheduled {
            let drain = scheduled.drain_filter(|it| it.consumer() == Consumer::DataCollectors);
            let scheduled = drain
                .filter_map(|ev| match ev {
                    EventData::DataUpdate { name, callback, .. } => {
                        handle_callback(ctx, &table, &mut self.state, &name, &callback)
                    }
                })
                .collect();
            scheduled
        } else {
            // initial run, allow initial scheduling
            self.collectors
                .iter()
                .filter_map(|(name, callback)| {
                    handle_callback(ctx, &table, &mut self.state, name, callback)
                })
                .collect()
        };

        let table = ctx.create_registry_value(table)?;
        Ok((table, scheduled_next))
    }
}
