use std::{
    path::Path,
    ptr::addr_of,
    thread::sleep,
    time::{Duration, Instant},
};

use args::Arguments;
use clap::Parser;
use env_logger::Env;
use glam::{IVec2, UVec2};
use mlua::prelude::*;
use notify::Watcher;
use render::{
    frontend::{bindings::LuaCanvas, FrameBufferSurface},
    RenderTarget, RenderTargetImpl, TargetConfig,
};
use script::{data::DataCollectors, events::EventBuffer};
use skia_safe::{Color, Color4f};

use crate::{
    script::{
        events::{EventChannel, EventData, TargetFile},
        ScriptContext,
    },
    util::ErrHandleExt,
};

mod args;
pub mod error;
pub mod render;
pub mod script;
pub mod util;

pub struct MainState {
    script: Option<ScriptContext>,
    collectors: DataCollectors,
    evb: EventBuffer,
}

impl MainState {
    pub fn init(script_path: impl AsRef<Path>) -> Self {
        let mut script =
            ScriptContext::new(script_path).some_or_log(Some("script load error".to_string()));

        let mut collectors = match &mut script {
            Some(it) => it.settings.take_collectors(),
            None => DataCollectors::default(),
        };

        let mut evb = EventBuffer::new();
        collectors
            .init_state(script.as_mut(), &mut evb)
            .expect("unable to initialize state table");

        MainState {
            script,
            collectors,
            evb,
        }
    }

    pub fn reload(&mut self, script_path: impl AsRef<Path>) {
        let script = match &mut self.script {
            Some(script) => {
                script
                    .reload(script_path)
                    .some_or_log(Some("script load error".to_string()));
                script
            }
            None => {
                match ScriptContext::new(script_path)
                    .some_or_log(Some("script load error".to_string()))
                {
                    Some(it) => {
                        self.script = Some(it);
                        self.script.as_mut().unwrap()
                    }
                    None => {
                        self.collectors = DataCollectors::default();
                        return;
                    }
                }
            }
        };
        self.collectors = script.settings.take_collectors();
        self.collectors
            .init_state(Some(script), &mut self.evb)
            .expect("unable to initialize state table");
    }

    pub fn script_tick(&mut self) {
        self.collectors
            .update_state(self.script.as_mut(), &mut self.evb)
            .expect("can't update state");
    }

    pub fn draw_frame<Q, T: RenderTarget<Q>>(&mut self, target: &mut T, qh: T::QH) {
        let script = match &self.script {
            Some(it) => it,
            None => return,
        };

        let draw_fn: LuaFunction = match script.draw_fn() {
            Some(it) => it,
            None => return,
        };

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

        let state_value = script.collected_data().expect("expired state in registry");

        draw_fn
            .call::<(LuaCanvas, LuaTable), ()>((canvas, state_value))
            .some_or_log(Some("render function error".to_string()));

        target.push_frame(qh);
    }
}

fn main() {
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    let args = Arguments::parse();

    let mut state = MainState::init(&args.script);

    let watcher_evb = state.evb.clone();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| match res {
            Ok(event) => match event.kind {
                notify::EventKind::Any
                | notify::EventKind::Create(_)
                | notify::EventKind::Modify(_) => {
                    log::info!("user script updated");
                    watcher_evb.schedule_event(EventData::FileReload {
                        time: Instant::now(),
                        file: TargetFile::UserScript,
                    })
                }
                _ => {}
            },
            Err(err) => {
                log::warn!("script watch error: {}", err);
            }
        })
        .ok();

    if let Some(watcher) = &mut watcher {
        if let Err(err) = watcher.watch(&args.script, notify::RecursiveMode::NonRecursive) {
            log::warn!("error to watch user script for changes: {}", err);
        }
    } else {
        log::warn!("unable to watch user script for changes");
    }

    let max_w = 1920;
    let max_h = 1050;

    let (mut target, _, mut queue) = RenderTargetImpl::create(TargetConfig {
        position: IVec2::new(0, 0),
        size: UVec2::new(max_w, max_h),
        ..Default::default()
    })
    .expect("unable to create a render target");

    state.draw_frame(&mut target, queue.handle());

    // https://gafferongames.com/post/fix_your_timestep/
    let initial = Instant::now();
    let mut prev = initial;
    while target.running() {
        let current = Instant::now();
        log::debug!("frame time: {}ms", (current - prev).as_millis());
        prev = current;

        queue.blocking_dispatch(&mut target).unwrap();

        if state
            .evb
            .poll_filter(EventChannel::FS_NOTIFY, |it| {
                matches!(
                    it,
                    EventData::FileReload {
                        file: TargetFile::UserScript,
                        ..
                    }
                )
            })
            .count()
            > 0
        {
            state.reload(&args.script);
        }

        state.script_tick();

        if target.can_render() {
            state.draw_frame(&mut target, queue.handle());
        } else {
            sleep(Duration::from_millis(1));
        }
    }
}
