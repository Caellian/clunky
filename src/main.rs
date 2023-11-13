use std::{
    mem::MaybeUninit,
    process::exit,
    thread::sleep,
    time::{Duration, Instant},
};

use args::Arguments;
use clap::Parser;
use env_logger::Env;
use error::ClunkyError;
use glam::{IVec2, UVec2};
use layout::Layout;
use math::rect::Rect;
use render::{skia::draw, RenderTarget, RenderTargetImpl, TargetConfig};

use crate::{render::buffer::FrameBuffer, script::Context};

mod args;
pub mod component;
pub mod error;
pub mod layout;
pub mod math;
pub mod render;
pub mod script;
pub mod settings;
pub mod util;

fn main() {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    let args = Arguments::parse();

    let mut layout = Layout::new();
    let c = Context::new(args.script).expect("unable to load lua context");

    let settings = c.load_settings();
    log::info!("Target framerate: {}fps", settings.framerate);

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

    let max_w = 1920;
    let max_h = 1050;

    let (mut target, mut conn, mut queue) = RenderTargetImpl::create(TargetConfig {
        position: IVec2::new(0, 0),
        size: UVec2::new(max_w, max_h),
        ..Default::default()
    })
    .expect("unable to create a render target");

    let params = target.frame_parameters();
    draw(target.buffer(), params).unwrap();
    target.push_frame(queue.handle());

    // https://gafferongames.com/post/fix_your_timestep/
    let initial = Instant::now();
    let mut prev = initial;
    while target.running() {
        let current = Instant::now();
        log::debug!("frame time: {}ms", (current - prev).as_millis());
        prev = current;
        let offset = (((current - initial).as_millis() as f32 / 1000.0).sin() + 1.2) / 2.5;

        queue.blocking_dispatch(&mut target).unwrap();
        //let _ = queue.flush();
        //conn.prepare_read()

        if target.can_render() {
            let params = target.frame_parameters();
            draw(target.buffer(), params).unwrap();
            target.push_frame(queue.handle());
        } else {
            sleep(Duration::from_millis(1));
        }

        target
            .resize(
                UVec2::new(
                    (max_w as f32 * offset) as u32,
                    (max_h as f32 * offset) as u32,
                ),
                queue.handle(),
            )
            .unwrap();
    }
}
