use std::{
    fs::File,
    hash::BuildHasher,
    os::fd::{AsFd, AsRawFd},
    thread::sleep,
};

use glam::{IVec2, UVec2};
use image::{buffer, Frame};
use parking_lot::Condvar;
use skia_safe::luma_color_filter::new;
use wayland_client::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback, wl_compositor,
        wl_keyboard::{self, KeyState, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat,
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

use crate::{
    error::{ClunkyError, RenderError},
    require_some,
};

use super::{
    buffer::{ColorFormat, FrameParameters},
    FrameBuffer, RenderTarget, TargetConfig,
};

pub enum CallbackKind {
    Frame,
}

pub struct WaylandState {
    running: bool,

    position: IVec2,
    size: UVec2,

    anchor: Anchor,

    color_format: ColorFormat,
    frame_buffer: Option<FrameBuffer>,

    wl_surface: Option<WlSurface>,

    layer_shell: Option<ZwlrLayerShellV1>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,

    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,

    configured: bool,

    // TODO: Insert check through all constructor code
    error: Option<ClunkyError>,
    do_render: bool,
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

    fn attach_buffer(&mut self) {
        if self.error.is_some() || !self.configured {
            return;
        }
        let surface = require_some!(&self.wl_surface);
        let framebuffer = require_some!(&self.frame_buffer);
        surface.attach(Some(framebuffer.buffer()), 0, 0);
        surface.commit();
    }
}

impl RenderTarget<EventQueue<Self>> for WaylandState {
    type QH = QueueHandle<Self>;

    fn create(config: TargetConfig) -> Result<(Self, Connection, EventQueue<Self>), ClunkyError> {
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

                color_format: ColorFormat::RGBA8888,
                frame_buffer: None,

                wl_surface: None,
                layer_shell: None,
                layer_surface: None,
                keyboard: None,
                pointer: None,

                error: None,
                do_render: false,
            },
            event_queue,
        );

        while !state.configured && state.error.is_none() {
            queue
                .blocking_dispatch(&mut state)
                .map_err(RenderError::WaylandDispatch)?;
        }

        match state.error {
            Some(err) => Err(err),
            None => Ok((state, connection, queue)),
        }
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

    fn resize(&mut self, new_size: UVec2, qh: Self::QH) -> crate::error::Result<()> {
        log::info!("Resizing surface to: {}x{}", new_size.x, new_size.y);
        self.size = new_size;

        let frame_buffer = self.frame_buffer.as_mut().expect("buffer not initialized");
        frame_buffer.switch_params(
            FrameParameters {
                dimensions: self.size,
                format: self.color_format,
            },
            qh,
        )?;

        self.attach_buffer();

        Ok(())
    }

    fn push_frame(&mut self, qh: Self::QH) {
        let surface = require_some!(&self.wl_surface);
        surface.frame(&qh, CallbackKind::Frame);
        self.do_render = false;
        surface.commit();
    }

    fn destroy(&mut self) -> crate::error::Result<()> {
        self.running = false;
        Ok(())
    }

    fn frame_parameters(&self) -> FrameParameters {
        FrameParameters {
            dimensions: self.size,
            format: self.color_format,
        }
    }

    fn buffer(&mut self) -> &mut FrameBuffer {
        self.frame_buffer.as_mut().expect("buffer not initialized")
    }

    fn running(&self) -> bool {
        self.running
    }

    fn can_render(&self) -> bool {
        self.do_render
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

impl Dispatch<wl_callback::WlCallback, CallbackKind> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        kind: &CallbackKind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done {
            callback_data: time,
        } = event
        {
            log::info!("Frame complete");
            match kind {
                CallbackKind::Frame => {
                    state.do_render = true;
                }
            }
        }
    }
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
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

                    let fb = FrameBuffer::new(
                        &shm,
                        FrameParameters {
                            dimensions: state.size,
                            format: state.color_format,
                        },
                        qh,
                    );

                    state.frame_buffer = match fb {
                        Ok(it) => Some(it),
                        Err(err) => {
                            state.error = Some(err.into());
                            return;
                        }
                    };

                    state.attach_buffer();
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
        state: &mut Self,
        _: &wl_shm::WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_shm::Event::Format {
                format: WEnum::Value(format),
            } => {
                if let Some(color_format) = ColorFormat::from_wl_format(format) {
                    if color_format < state.color_format {
                        state.color_format = color_format;
                    }
                }
            }
            _ => {}
        }
    }
}

stub_listener!(wl_shm_pool::WlShmPool);

impl Dispatch<wl_buffer::WlBuffer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_buffer::Event::Release => {
                if let Some(fb) = &state.frame_buffer {
                    if fb.buffer().id() == buffer.id() {
                        log::info!("Buffer released");
                    }
                }
            }
            _ => {}
        }
    }
}

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
                let wl_surface = state.wl_surface.as_ref().unwrap();
                wl_surface.commit();
                state.configured = true;

                state.attach_buffer();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}
