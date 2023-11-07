use std::{process::exit, time::Instant};

use args::Arguments;
use clap::Parser;
use env_logger::Env;
use error::ClunkyError;
use glam::{IVec2, UVec2};
use layout::Layout;
use math::rect::Rect;
use render::{RenderTarget, RenderTargetImpl, TargetConfig};

use crate::script::Context;

mod args;
pub mod component;
pub mod error;
pub mod layout;
pub mod math;
pub mod render;
pub mod script;
pub mod util;

fn main() {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    let args = Arguments::parse();

    let mut layout = Layout::new();
    let c = Context::new(args.script).expect("unable to load lua context");

    match c.update_layout(&mut layout) {
        Err(err) => {
            match err {
                ClunkyError::Lua(rlua::Error::CallbackError { cause, traceback }) => {
                    log::error!("{}\n{}", cause, traceback);
                }
                other => log::error!("{}", other),
            }
            exit(1);
        }
        Ok(()) => {}
    }

    let (mut target, mut queue) = RenderTargetImpl::create(TargetConfig {
        position: IVec2::new(50, 100),
        size: UVec2::new(800, 600),
        ..Default::default()
    })
    .expect("unable to create a render target");

    while target.active() {
        queue.dispatch_pending(&mut target).unwrap();
        let _ = queue.flush();
    }
}
