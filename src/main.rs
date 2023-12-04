use std::{
    error::Error,
    mem::MaybeUninit,
    process::exit,
    ptr::{addr_of, addr_of_mut},
    thread::sleep,
    time::{Duration, Instant},
};

use args::Arguments;
use clap::Parser;
use env_logger::Env;
use error::ClunkyError;
use glam::{IVec2, UVec2};
use render::{
    frontend::{bindings::LuaCanvas, FrameBufferSurface},
    RenderTarget, RenderTargetImpl, TargetConfig,
};
use rlua::prelude::*;
use script::{events::EventBuffer, settings::Settings};
use skia_safe::{Color, Color4f};

use crate::{render::buffer::FrameBuffer, script::ScriptContext};

mod args;
pub mod error;
pub mod render;
pub mod script;
pub mod util;

fn main() {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    let args = Arguments::parse();

    let script = match ScriptContext::new(args.script) {
        Ok(it) => it,
        Err(err) => {
            print_stack_trace(&err);
            exit(1);
        }
    };

    let (settings, mut collectors) = {
        let mut s = script.load_settings();
        let collectors = s.data_collectors.take().unwrap_or_default();
        (s, collectors)
    };
    let (mut evb, state) = {
        let mut evb = EventBuffer::new();

        let (state, scheduled) = script
            .lua()
            .context(|ctx| collectors.update_state(ctx, None))
            .expect("unable to initialize state table");

        evb.schedule(scheduled);

        (evb, state)
    };

    let max_w = 1920;
    let max_h = 1050;

    let (mut target, _, mut queue) = RenderTargetImpl::create(TargetConfig {
        position: IVec2::new(0, 0),
        size: UVec2::new(max_w, max_h),
        ..Default::default()
    })
    .expect("unable to create a render target");

    draw_frame(&mut target, queue.handle(), &script, &settings, state);

    // https://gafferongames.com/post/fix_your_timestep/
    let initial = Instant::now();
    let mut prev = initial;
    while target.running() {
        let current = Instant::now();
        log::debug!("frame time: {}ms", (current - prev).as_millis());
        prev = current;

        queue.blocking_dispatch(&mut target).unwrap();
        let mut scheduled = evb.take_scheduled();

        let (state, scheduled) = script
            .lua()
            .context(|ctx| collectors.update_state(ctx, Some(&mut scheduled)))
            .expect("can't update state");
        evb.schedule(scheduled);

        if target.can_render() {
            draw_frame(&mut target, queue.handle(), &script, &settings, state);
        } else {
            sleep(Duration::from_millis(1));
        }
    }
}

fn draw_frame<Q, T: RenderTarget<Q>>(
    target: &mut T,
    qh: T::QH,
    script: &ScriptContext,
    settings: &Settings,
    state: LuaRegistryKey,
) {
    if let Some(draw_cb) = &settings.draw {
        let result = script.lua().context(|lua| {
            let render_fn: LuaFunction = lua.registry_value(draw_cb)?;

            let mut surface = target.buffer().to_surface();
            let canvas = surface.canvas();
            canvas.clear(Color4f::from(Color::TRANSPARENT));
            let canvas = unsafe {
                // SAFETY: calling render_fn will block the current thread
                // until Lua function is done executing. During that time,
                // `target` reference won't be dropped so canvas will stay
                // valid.
                // render_fn.call takes ownership of `surface` and through
                // that also the refence to `target`. Passing actual
                // references isn't supported so canvas lifetime has
                // to be erased for temporary LuaCanvas wrapper.
                LuaCanvas::Borrowed(addr_of!(*surface.canvas()).as_ref().unwrap_unchecked())
            };

            let state_value: LuaTable = lua
                .registry_value(&state)
                .expect("expired state in registry");

            render_fn
                .call((canvas, state_value))
                .map_err(crate::error::ClunkyError::Lua)?;

            let _ = lua.remove_registry_value(state);
            Ok::<_, ClunkyError>(())
        });

        if let Err(err) = result {
            print_stack_trace(&err);
            exit(1);
        }

        target.push_frame(qh);
    }
}

fn print_stack_trace(error: &dyn Error) {
    log::error!("{}", error);
    let mut current = error.source();
    while let Some(err) = current {
        log::error!("{}", err);
        current = err.source();
    }
}
