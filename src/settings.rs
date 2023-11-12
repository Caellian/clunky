use rlua::Table;

#[derive(Debug)]
pub struct Settings {
    pub framerate: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { framerate: 60 }
    }
}

impl Settings {
    pub fn load(table: Table) -> Self {
        let mut result = Settings::default();

        if let Ok(framerate) = table.get("framerate") {
            result.framerate = framerate;
        }

        result
    }
}
