use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::mem::align_of;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rlua::UserData;

use super::data::CollectorCallback;

pub struct EventBuffer {
    inner: BinaryHeap<Reverse<EventData>>,
}

impl EventBuffer {
    pub fn new() -> Self {
        EventBuffer {
            inner: BinaryHeap::with_capacity(32),
        }
    }

    pub fn take_scheduled(&mut self) -> Vec<EventData> {
        let now = Instant::now();

        let mut result = Vec::new();
        while let Some(it) = self.inner.peek().map(|it| &it.0) {
            if it.time() > now {
                break;
            }
            result.push(self.inner.pop().unwrap().0);
        }

        result
    }

    pub fn schedule(&mut self, event_list: Vec<EventData>) {
        self.inner
            .extend(event_list.into_iter().map(|it| Reverse(it)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Consumer {
    DataCollectors,
}

#[repr(C, u8)]
pub enum EventData {
    DataUpdate {
        time: Instant,
        name: String,
        callback: CollectorCallback,
    } = 0,
}

impl EventData {
    #[inline]
    fn discriminant(&self) -> u8 {
        // SAFETY: Because `Self` is marked `repr(u8)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `u8` discriminant as its first
        // field, so we can read the discriminant without offsetting the pointer.
        unsafe { *<*const _>::from(self).cast::<u8>() }
    }

    #[inline]
    pub fn consumer(&self) -> Consumer {
        match self.discriminant() {
            0 => Consumer::DataCollectors,
            _ => unreachable!(),
        }
    }

    #[inline]
    pub fn time(&self) -> Instant {
        // SAFETY: Because `Self` is marked `repr(u8)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `time` field as its second
        // field (following discriminant), so we can read the time by offsetting
        // the pointer by disciminant size.
        unsafe {
            let base = <*const _>::from(self).cast::<u8>().add(1);
            let align = base.align_offset(align_of::<Instant>());
            *base.add(align).cast::<Instant>()
        }
    }
}

impl PartialEq for EventData {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::DataUpdate {
                    time: l_time,
                    name: l_name,
                    ..
                },
                Self::DataUpdate {
                    time: r_time,
                    name: r_name,
                    ..
                },
            ) => l_time == r_time && l_name == r_name,
        }
    }
}

impl Eq for EventData {}

impl PartialOrd for EventData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.time().partial_cmp(&other.time())
    }
}

impl Ord for EventData {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time().cmp(&other.time())
    }
}

#[derive(Default, Clone)]
pub struct Status {
    inner: Arc<Mutex<StatusData>>,
}

#[derive(Default)]
struct StatusData {
    pub next_update: Option<Instant>,
}

impl Status {
    pub fn next_update(&self) -> Option<Instant> {
        self.inner.lock().next_update
    }
}

impl UserData for Status {
    fn add_methods<'lua, T: rlua::prelude::LuaUserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method_mut("requestUpdate", |_, this, millis: u64| {
            let mut inner = this.inner.lock();
            inner.next_update = Some(Instant::now() + Duration::from_millis(millis));
            Ok(())
        });
    }
}
