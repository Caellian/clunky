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

mod fb {
    use crate::render::buffer::FrameBuffer;
    use std::sync::OnceLock;

    static FRAMEBUFFER: OnceLock<FrameBuffer> = OnceLock::new();

    pub fn framebuffer() -> &'static FrameBuffer {
        FRAMEBUFFER.get_or_init(|| FrameBuffer::new())
    }
}
pub use fb::framebuffer;

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

    let buffer = FrameBuffer::new();

    let max_w = 1920;
    let max_h = 1050;

    let (mut target, mut queue) = RenderTargetImpl::create(
        TargetConfig {
            position: IVec2::new(0, 0),
            size: UVec2::new(max_w, max_h),
            ..Default::default()
        },
        buffer,
    )
    .expect("unable to create a render target");

    // https://gafferongames.com/post/fix_your_timestep/
    let initial = Instant::now();
    let mut prev = initial;
    while target.active() {
        let current = Instant::now();
        log::info!("frame time: {}ms", (current - prev).as_millis());
        prev = current;
        let offset = ((current - initial).as_secs() as f32).sin() / 2.0 + 0.5;

        queue.dispatch_pending(&mut target).unwrap();
        draw(target.buffer());
        let _ = queue.flush();

        target
            .resize(UVec2::new(max_w / 2, max_h / 2), &queue.handle())
            .unwrap();
    }
}
