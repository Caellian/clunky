use std::path::PathBuf;
use clap::Parser;

#[derive(Parser)]
#[command(author, version, about)]
pub struct Arguments {
    /// Script location
    #[clap(short, long, value_name = "FILE")]
    #[cfg_attr(debug_assertions, clap(default_value="examples/init.lua"))]
    #[cfg_attr(all(not(debug_assertions), target_family = "unix"), clap(default_value="~/.config/clunky/init.lua"))]
    pub script: PathBuf,
}
