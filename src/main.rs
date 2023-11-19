use std::{
    error::Error,
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
use math::rect::Rect;
use render::{skia::draw, RenderTarget, RenderTargetImpl, TargetConfig};
use rlua::Function;
use settings::Settings;

use crate::{render::buffer::FrameBuffer, script::ScriptContext};

mod args;
pub mod error;
pub mod math;
pub mod render;
pub mod script;
pub mod settings;
pub mod skia_bindings;
pub mod util;

fn draw_frame<Q, T: RenderTarget<Q>>(
    target: &mut T,
    qh: T::QH,
    script: &ScriptContext,
    settings: &Settings,
) {
    if let Some(bg) = &settings.background {
        let params = target.frame_parameters();
        let result = script.lua().context(|l| {
            let render_fn: Function = l.registry_value(bg)?;
            draw(target.buffer(), params, render_fn)?;
            Ok::<_, ClunkyError>(())
        });

        if let Err(err) = result {
            log::error!("{}", err);
            if let Some(source) = err.source() {
                log::error!("{}", source);
            }
            exit(1);
        }

        target.push_frame(qh);
    }
}

fn main() {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    let args = Arguments::parse();

    let script = ScriptContext::new(args.script).expect("unable to load lua context");

    let settings = script.load_settings();

    /*
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
    } */

    let max_w = 1920;
    let max_h = 1050;

    let (mut target, _, mut queue) = RenderTargetImpl::create(TargetConfig {
        position: IVec2::new(0, 0),
        size: UVec2::new(max_w, max_h),
        ..Default::default()
    })
    .expect("unable to create a render target");

    draw_frame(&mut target, queue.handle(), &script, &settings);

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
            draw_frame(&mut target, queue.handle(), &script, &settings);
        } else {
            sleep(Duration::from_millis(1));
        }
    }
}
