use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd},
};

use glam::{IVec2, UVec2};
use wayland_client::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_compositor,
        wl_keyboard::{self, KeyState, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat, wl_shm, wl_shm_pool,
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

use crate::{error::RenderError, require_some};

use super::{RenderTarget, TargetConfig};

pub struct WaylandState {
    running: bool,

    position: IVec2,
    size: UVec2,

    anchor: Anchor,

    wl_surface: Option<WlSurface>,
    buffer: Option<WlBuffer>,

    layer_shell: Option<ZwlrLayerShellV1>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,

    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,

    configured: bool,
}

impl WaylandState {
    fn init_surface(&mut self, qh: &QueueHandle<Self>) {
        if self.layer_surface.is_some() {
            return;
        }

        let wl_surface = require_some!(&self.wl_surface);
        let layer_shell = require_some!(&self.layer_shell);

        self.layer_surface = Some({
            let surface = layer_shell.get_layer_surface(
                wl_surface,
                None,
                zwlr_layer_shell_v1::Layer::Bottom,
                "widget".to_string(),
                qh,
                (),
            );
            surface.set_anchor(self.anchor);
            surface.set_size(self.size.x, self.size.y);
            let (top, right, bottom, left) = position_to_margins(self.anchor, self.position);
            surface.set_margin(top, right, bottom, left);
            /*
            surface
                .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
             */
            surface
        });

        wl_surface.commit();
    }
}

impl RenderTarget<EventQueue<Self>> for WaylandState {
    fn create(config: TargetConfig) -> crate::error::Result<(Self, EventQueue<Self>)> {
        let connection =
            Connection::connect_to_env().map_err(|err| RenderError::WaylandConnect(err))?;

        let event_queue: EventQueue<Self> = connection.new_event_queue();
        let qhandle = event_queue.handle();

        let display = connection.display();
        display.get_registry(&qhandle, ());

        let (mut state, mut queue) = (
            WaylandState {
                running: true,
                configured: false,

                position: config.position,
                size: config.size,
                anchor: config.anchor,

                wl_surface: None,
                buffer: None,
                layer_shell: None,
                layer_surface: None,
                keyboard: None,
                pointer: None,
            },
            event_queue,
        );

        while !state.configured {
            queue
                .blocking_dispatch(&mut state)
                .map_err(RenderError::WaylandDispatch)?;
        }

        Ok((state, queue))
    }

    fn reposition(&mut self, new_position: IVec2) -> crate::error::Result<()> {
        let wl_surface = require_some!((&self.wl_surface) or return Ok(()));
        let layer_surface = require_some!((&self.layer_surface) or return Ok(()));

        let (top, right, bottom, left) = position_to_margins(self.anchor, self.position);
        layer_surface.set_margin(top, right, bottom, left);
        self.position = new_position;

        wl_surface.commit();

        Ok(())
    }

    fn resize(&mut self, new_size: UVec2) -> crate::error::Result<()> {
        Ok(())
    }

    fn destroy(&mut self) -> crate::error::Result<()> {
        Ok(())
    }

    fn active(&self) -> bool {
        self.running
    }
}

#[inline]
fn position_to_margins(anchor: Anchor, position: IVec2) -> (i32, i32, i32, i32) {
    let (top, bottom) = match anchor.difference(Anchor::Left | Anchor::Right) {
        Anchor::Top => (position.y as i32, 0),
        Anchor::Bottom => (0, -position.y as i32),
        _ => (0, 0),
    };
    let (right, left) = match anchor.difference(Anchor::Top | Anchor::Bottom) {
        Anchor::Left => (0, position.x as i32),
        Anchor::Right => (-position.x as i32, 0),
        _ => (0, 0),
    };

    (top, right, bottom, left)
}

fn draw(tmp: &mut File, (buf_x, buf_y): (u32, u32)) {
    use std::{cmp::min, io::Write};
    let mut buf = std::io::BufWriter::new(tmp);
    for y in 0..buf_y {
        for x in 0..buf_x {
            let a = 0xFF;
            let r = min(((buf_x - x) * 0xFF) / buf_x, ((buf_y - y) * 0xFF) / buf_y);
            let g = min((x * 0xFF) / buf_x, ((buf_y - y) * 0xFF) / buf_y);
            let b = min(((buf_x - x) * 0xFF) / buf_x, (y * 0xFF) / buf_y);

            let color = (a << 24) + (r << 16) + (g << 8) + b;
            buf.write_all(&color.to_ne_bytes()).unwrap();
        }
    }
    buf.flush().unwrap();
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        data: &(),
        conn: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    let compositor: wl_compositor::WlCompositor = registry.bind(name, 6, qh, ());
                    let surface = compositor.create_surface(qh, ());
                    state.wl_surface = Some(surface);

                    state.init_surface(qh);
                }
                "wl_shm" => {
                    let shm: wl_shm::WlShm = registry.bind(name, 1, qh, ());

                    let mut file = tempfile::tempfile().unwrap();
                    draw(&mut file, (state.size.x, state.size.y));
                    let pool = shm.create_pool(
                        file.as_fd(),
                        (state.size.x * state.size.y * 4) as i32,
                        qh,
                        (),
                    );
                    let buffer = pool.create_buffer(
                        0,
                        state.size.x as i32,
                        state.size.y as i32,
                        (state.size.x * 4) as i32,
                        wl_shm::Format::Argb8888,
                        qh,
                        (),
                    );
                    state.buffer = Some(buffer.clone());

                    if state.configured {
                        let surface = state.wl_surface.as_ref().unwrap();
                        surface.attach(Some(&buffer), 0, 0);
                        surface.commit();
                    }
                }
                "wl_seat" => {
                    registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ());
                }
                "zwlr_layer_shell_v1" => {
                    let layer_shell = registry.bind::<ZwlrLayerShellV1, _, _>(name, 1, qh, ());
                    state.layer_shell = Some(layer_shell);

                    state.init_surface(qh);
                }
                other => {
                    log::trace!("unhandled interface: {}", other);
                }
            }
        }
    }
}

macro_rules! stub_listener {
    ($interface: path) => {
        impl Dispatch<$interface, ()> for WaylandState {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

stub_listener!(wl_compositor::WlCompositor);

impl Dispatch<WlSurface, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::PreferredBufferScale { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_shm::Event::Format { .. } => {
                // TODO: Move buffer creation to wl_shm::WlShm handler when
                // available formats are known
            }
            _ => {}
        }
    }
}

stub_listener!(wl_shm_pool::WlShmPool);
stub_listener!(wl_buffer::WlBuffer);

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { .. } => {}
            wl_keyboard::Event::Enter { .. } => {}
            wl_keyboard::Event::Leave { .. } => {}
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                if key == 1 && key_state == WEnum::Value(KeyState::Pressed) {
                    // ESC key
                    state.running = false;
                }
            }
            wl_keyboard::Event::Modifiers { .. } => {}
            wl_keyboard::Event::RepeatInfo { .. } => {}
            _ => todo!(),
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { .. } => {}
            wl_pointer::Event::Leave { .. } => {}
            wl_pointer::Event::Motion { .. } => {
                log::info!("movement event");
            }
            wl_pointer::Event::Button { .. } => {}
            wl_pointer::Event::Axis { .. } => {}
            _ => {}
        }
    }
}

stub_listener!(ZwlrLayerShellV1);

impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                proxy.ack_configure(serial);
                state.configured = true;

                let wl_surface = state.wl_surface.as_ref().unwrap();
                wl_surface.commit();

                if let Some(buffer) = &state.buffer {
                    wl_surface.attach(Some(buffer), 0, 0);
                    wl_surface.commit();
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}
