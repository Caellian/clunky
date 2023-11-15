use rlua::{Context, Function, RegistryKey, Table};

#[derive(Debug)]
pub struct Settings {
    pub framerate: u16,

    pub background: Option<RegistryKey>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            framerate: 60,

            background: None,
        }
    }
}

impl Settings {
    pub fn load<'lua>(ctx: Context<'lua>, table: Table<'lua>) -> Self {
        let mut result = Settings::default();

        if let Ok(framerate) = table.get("framerate") {
            result.framerate = framerate;
        }

        if let Ok(background) = table.get::<_, Function>("background") {
            result.background = ctx.create_registry_value(background).ok();
        }

        result
    }
}
