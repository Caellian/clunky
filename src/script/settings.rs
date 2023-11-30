use rlua::{Context, Function, RegistryKey, Result as LuaResult, Table};

use super::data::DataCollectors;

#[derive(Debug)]
pub struct Settings {
    /// Targetted framerate
    pub framerate: u16,
    /// Data update frequency in ms
    ///
    /// Can't be lower than 200ms
    pub update_frequency: u32,

    pub data_collectors: Option<DataCollectors>,

    pub draw: Option<RegistryKey>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            framerate: 60,
            update_frequency: 1000,

            data_collectors: None,

            draw: None,
        }
    }
}

impl Settings {
    pub fn load<'lua>(ctx: Context<'lua>, table: Table<'lua>) -> LuaResult<Self> {
        let mut result = Settings::default();

        if let Ok(framerate) = table.get("framerate") {
            result.framerate = framerate;
        }

        if let Ok(update_frequency) = table.get::<_, u32>("update") {
            result.update_frequency = update_frequency.max(200);
        }

        if let Ok(collectors) = table.get::<_, Table>("collectors") {
            result.data_collectors = Some(DataCollectors::new_lua_collectors(ctx, collectors)?);
        }

        if let Ok(draw) = table.get::<_, Function>("draw") {
            result.draw = ctx.create_registry_value(draw).ok();
        }

        Ok(result)
    }
}
