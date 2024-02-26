use std::cmp::Reverse;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{cmp::Ordering, mem::align_of};

use mlua::UserData;
use parking_lot::{Mutex, MutexGuard};

use super::data::CollectorCallback;

#[derive(Clone)]
pub struct EventBuffer {
    inner: Arc<Mutex<Vec<EventData>>>,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBuffer {
    pub fn new() -> Self {
        EventBuffer {
            inner: Default::default(),
        }
    }

    pub fn poll_all(&mut self) -> EventIterator<fn(&EventData) -> bool> {
        EventIterator::new(self.inner.lock(), EventChannel::ANY)
    }
    pub fn poll(&mut self, channel: EventChannel) -> EventIterator<fn(&EventData) -> bool> {
        EventIterator::new(self.inner.lock(), channel)
    }
    pub fn poll_filter<F: Fn(&EventData) -> bool>(
        &mut self,
        channel: EventChannel,
        filter: F,
    ) -> EventIterator<F> {
        EventIterator::new_filtered(self.inner.lock(), channel, filter)
    }

    pub fn schedule_event(&self, event: EventData) {
        let mut inner = self.inner.lock();
        let insert_at = inner
            .iter()
            .take_while(|it| it.time() < event.time())
            .count();
        inner.insert(insert_at, event);
    }

    pub fn schedule<E: IntoIterator<Item = EventData>>(&self, event_list: E) {
        let mut inner = self.inner.lock();

        let mut inserted: Vec<Reverse<_>> = event_list.into_iter().map(Reverse).collect();
        match inserted.len() {
            0 => return,
            1 => {
                self.schedule_event(inserted.pop().unwrap().0);
                return;
            }
            _ => {
                inserted.sort_unstable();
            }
        }

        let mut at = 0;
        let mut next = inserted.pop().map(|it| it.0);
        while let Some(f) = next {
            let current = match inner.get(at) {
                Some(it) => it,
                None => {
                    next = Some(f);
                    break;
                }
            };

            next = if matches!(current.time().cmp(&f.time()), Ordering::Greater) {
                inner.insert(at, f);
                inserted.pop().map(|it| it.0)
            } else {
                at += 1;
                Some(f)
            }
        }
        if let Some(front) = next {
            inner.push(front);
            inner.extend(inserted.into_iter().map(|it| it.0));
        }
    }
}

pub struct EventIterator<'a, F: Fn(&EventData) -> bool> {
    /// Bitmask of querried event channels.
    channel: EventChannel,
    /// Inclusive upper time bound for last event to return.
    end: Instant,
    /// Offset in current event list.
    at: usize,
    /// Event sequence that's being iterated over.
    inner: MutexGuard<'a, Vec<EventData>>,
    /// Filter for querried events.
    ///
    /// This enables `filter_drain` like functionality.
    filter: F,
}

impl<'a> EventIterator<'a, fn(&EventData) -> bool> {
    fn new(inner: MutexGuard<'a, Vec<EventData>>, channel: EventChannel) -> Self {
        let end = Instant::now();

        EventIterator {
            channel,
            end,
            at: 0,
            inner,
            filter: |_| true,
        }
    }
}

impl<'a, F: Fn(&EventData) -> bool> EventIterator<'a, F> {
    fn new_filtered(
        inner: MutexGuard<'a, Vec<EventData>>,
        channel: EventChannel,
        filter: F,
    ) -> Self {
        let end = Instant::now();
        EventIterator {
            channel,
            end,
            at: 0,
            inner,
            filter,
        }
    }
}

impl<'a, F: Fn(&EventData) -> bool> Iterator for EventIterator<'a, F> {
    type Item = EventData;

    fn next(&mut self) -> Option<Self::Item> {
        for i in self.at..self.inner.len() {
            let it = match self.inner.get(i) {
                Some(it) => it,
                None => unreachable!("invalid EventIterator state"),
            };

            if it.time() > self.end {
                return None;
            }

            if self.channel.contains(it.consumer_channel()) && (self.filter)(it) {
                self.at = i;
                return Some(self.inner.remove(i));
            }
        }
        self.at = self.inner.len();
        None
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EventChannel: u32 {
        const ANY = u32::MAX;
        const DATA = 1;
        const FS_NOTIFY = 1 << 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TargetFile {
    UserScript,
}

#[repr(C, u32)]
pub enum EventData {
    DataUpdate {
        time: Instant,
        name: String,
        callback: CollectorCallback,
    } = 1,
    FileReload {
        time: Instant,
        file: TargetFile,
    } = 1 << 1,
}

impl EventData {
    #[inline]
    fn discriminant(&self) -> u32 {
        // SAFETY: Because `Self` is marked `repr(u32)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `u32` discriminant as its first
        // field, so we can read the discriminant without offsetting the pointer.
        unsafe { *<*const _>::from(self).cast::<u32>() }
    }

    #[inline]
    pub fn consumer_channel(&self) -> EventChannel {
        EventChannel::from_bits_retain(self.discriminant())
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
            (
                Self::FileReload {
                    time: l_time,
                    file: l_file,
                },
                Self::FileReload {
                    time: r_time,
                    file: r_file,
                },
            ) => l_time == r_time && l_file == r_file,
            _ => false,
        }
    }
}

impl Eq for EventData {}

impl PartialOrd for EventData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventData {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time().cmp(&other.time())
    }
}

/// Wrapper for state information managed by different event calls.
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
    fn add_methods<'lua, T: mlua::UserDataMethods<'lua, Self>>(methods: &mut T) {
        methods.add_method_mut("requestUpdate", |_, this, millis: u64| {
            let mut inner = this.inner.lock();
            inner.next_update = Some(Instant::now() + Duration::from_millis(millis));
            Ok(())
        });
    }
}
