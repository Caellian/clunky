use std::{
    alloc::Layout,
    collections::{HashSet, VecDeque},
    fmt::Display,
    fs::File,
    io::ErrorKind,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    os::fd::{AsFd, BorrowedFd},
    sync::atomic::{AtomicU16, AtomicU32, AtomicU8, AtomicUsize, Ordering},
};

use drain_filter_polyfill::VecExt;
use glam::UVec2;
use image::Frame;
use memmap2::{MmapMut, RemapOptions};
use parking_lot::Mutex;

use crate::error::FrameBufferError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameOwner {
    None = 0,
    Renderer = 1,
    Compositor = 2,
}

#[derive(Debug)]
#[repr(transparent)]
pub struct FrameState {
    value: AtomicU32,
}

impl FrameState {
    #[inline]
    pub fn new() -> Self {
        FrameState {
            value: AtomicU32::new(0),
        }
    }

    #[inline]
    fn load_owner(&self) -> FrameOwner {
        unsafe { std::mem::transmute((self.value.load(Ordering::SeqCst) & u8::MAX as u32) as u8) }
    }

    #[inline]
    pub fn store_owner(&self, owner: FrameOwner) {
        self.value
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                Some((state & u8::MIN as u32) | owner as u32)
            });
    }
}

#[derive(Debug)]
pub struct FrameInfo {
    offset: usize,
    length: usize,
    row_width: u32,
    row_count: u32,
    bpp: u8,
    state: FrameState,
}

impl FrameInfo {
    pub fn new(offset: usize, size: UVec2, bpp: u8) -> Self {
        let row_width = size.x * bpp as u32;
        FrameInfo {
            offset,
            length: row_width as usize * size.y as usize,
            row_width,
            row_count: size.y,
            bpp,
            state: FrameState::new(),
        }
    }

    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.length
    }

    pub fn dimensions(&self) -> UVec2 {
        let width = self.row_width / self.bpp as u32;
        UVec2::new(width, self.row_count)
    }
}

pub struct FrameParameters {
    pub dimensions: UVec2,
    pub bpp: u8,
}

impl FrameParameters {
    pub fn new(dimensions: UVec2, bpp: u8) -> Self {
        FrameParameters { dimensions, bpp }
    }

    #[inline]
    pub fn len(&self) -> usize {
        (self.dimensions.x * self.dimensions.y) as usize * self.bpp as usize
    }
}

pub struct FrameRef {
    source: *const FrameBuffer,
    entry: FrameInfo,
}

impl FrameRef {
    // # Safety
    // FrameBuffer can't move while there's frames in flight.
    #[inline]
    unsafe fn new(owner: &FrameBuffer, info: FrameInfo) -> Self {
        FrameRef {
            source: owner,
            entry: info,
        }
    }

    #[inline]
    pub fn info(&self) -> &FrameInfo {
        &self.entry
    }

    #[inline]
    fn byte_range(&self) -> ByteRange {
        ByteRange(self.entry.offset, self.entry.offset + self.entry.length)
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            // SAFETY: EntryRef is only issued on frame creation and owned by
            // a single place in code
            // mmap can't move while ref to slice exists
            &mut (*self.source).mmap[self.entry.offset..self.entry.offset + self.entry.length]
        }
    }
}

unsafe impl Send for FrameRef {}
unsafe impl Sync for FrameRef {}

pub struct FrameBuffer {
    source: File,
    mmap: MmapMut,
    slice_access: AtomicU32,
    free_segments: SegmentTracker,
}

impl FrameBuffer {
    pub fn new() -> Self {
        let source = tempfile::tempfile().unwrap();
        let buffer = unsafe { MmapMut::map_mut(&source).expect("unable to memory map file") };

        FrameBuffer {
            source,
            mmap: buffer,
            slice_access: AtomicU32::new(0),
            free_segments: SegmentTracker::new(0),
        }
    }

    pub fn as_fd(&self) -> BorrowedFd {
        self.source.as_fd()
    }

    fn ensure_capacity(&self, new_capacity: usize) -> Result<(), FrameBufferError> {
        if self.mmap.len() >= new_capacity {
            return Ok(());
        }

        self.source.set_len(new_capacity as u64)?;
        let use_count = self.slice_access.load(Ordering::SeqCst);
        unsafe {
            self.mmap
                .remap(new_capacity, RemapOptions::new().may_move(use_count == 0))
                .map_err(|_| FrameBufferError::MmapInUse(use_count))?
        };

        self.free_segments.grow(new_capacity);
        Ok(())
    }

    pub fn allocate_frame(&self, params: FrameParameters) -> Result<FrameRef, FrameBufferError> {
        let param_length = params.len();

        let mut region = None;
        loop {
            region = self.free_segments.take_next(param_length);
            if region.is_none() {
                self.ensure_capacity(
                    self.mmap.len() + param_length - self.free_segments.tailing_free_bytes(),
                )?;
            } else {
                break;
            }
        }
        let region = region.unwrap();

        let frame_info = FrameInfo::new(region.0, params.dimensions, params.bpp);

        return Ok(unsafe {
            // SAFETY: nothing else has access to last_frame
            FrameRef::new(self, frame_info)
        });
    }

    pub fn drop_frame(&self, entry: FrameRef) {
        self.free_segments.release(entry.byte_range());
    }
}

unsafe impl Sync for FrameBuffer {}

pub struct SegmentTracker {
    size: usize,
    unoccupied: Vec<ByteRange>,
}

impl SegmentTracker {
    /// Constructs a new `SegmentTracker` of the provided `size`.
    fn new(size: usize) -> Self {
        SegmentTracker {
            size,
            unoccupied: if size > 0 {
                vec![ByteRange(0, size)]
            } else {
                vec![]
            },
        }
    }

    /// Returns the total memory size being tracked.
    fn size(&self) -> usize {
        self.size
    }

    /// Returns a [`ByteRange`] encompassing the entire tracked memory region.
    fn whole_range(&self) -> ByteRange {
        ByteRange(0, self.size)
    }

    /// Grows the available memory range represented by this structure to
    /// provided `new_size` and returns the new size.
    fn grow(&mut self, new_size: usize) -> usize {
        if new_size < self.size {
            return self.size;
        }

        match self.unoccupied.last_mut() {
            Some(it) if it.1 == self.size => {
                // if the last free region ends at the end of tracked region
                // grow it
                it.1 = new_size;
            }
            _ => {
                self.unoccupied.push(ByteRange(self.size, new_size));
            }
        }
        self.size = new_size;
        self.size
    }

    /// Returns `true` if the provided type `layout` can ne stored within any
    /// unused segments of the represented memory region.
    fn can_store(&self, size: usize) -> bool {
        if size == 0 {
            return true;
        } else if size > self.size {
            return false;
        }

        self.unoccupied
            .iter()
            .enumerate()
            .any(|(_, it)| it.len() >= size)
    }

    /// Returns the appropriate [`Location`] that can accommodate the given type
    /// `layout`.
    ///
    /// If the `layout` cannot be stored within any unused segments of the
    /// represented memory region, `None` is returned instead.
    ///
    /// This function mutably borrows because the returned `Location` is only
    /// valid until this tracker gets mutated from somewhere else.
    /// The returned value can also apply mutation on `self` via a call to
    /// [`Location::mark_occupied`].
    fn peek_next(&mut self, size: usize) -> Option<Location> {
        if size == 0 {
            return Some(Location::zero_sized(self));
        } else if size > self.size {
            return None;
        }

        // try to find the smallest free ByteRange that can hold the given
        // layout while keeping it properly aligned.
        let (found_position, found_range) = self
            .unoccupied
            .iter()
            .enumerate()
            .filter(|(_, it)| it.len() >= size)
            .min_by_key(|(_, it)| it.len())?;

        let available = found_range.cap_size(size);

        Some(Location {
            parent: self,
            index: found_position,
            whole: *found_range,
            usable: available,
        })
    }

    /// Returns either a start position of a free byte range at the end of the
    /// tracker, or total size if end is occupied.
    #[inline]
    fn last_offset(&self) -> usize {
        match self.unoccupied.last() {
            Some(it) if it.1 == self.size => it.0,
            _ => self.size,
        }
    }

    /// Returns a copy largest free [`ByteRange`] tracked by this tracker.
    fn largest_free_range(&self) -> Option<ByteRange> {
        self.unoccupied.iter().max_by_key(|it| it.len()).copied()
    }

    /// Returns a number of tailing free bytes in the tracker.
    #[inline]
    fn tailing_free_bytes(&self) -> usize {
        match self.unoccupied.last() {
            Some(it) if it.1 == self.size => it.len(),
            _ => 0,
        }
    }

    /// Takes the next available memory region that can hold the provided
    /// `layout`.
    ///
    /// It returns a [`ByteRange`] of the memory region that was marked as used
    /// if successful, otherwise `None`
    #[inline]
    fn take_next(&mut self, size: usize) -> Option<ByteRange> {
        let mut location = self.peek_next(size)?;
        location.mark_occupied();
        Some(location.usable)
    }

    /// Tries marking the provided memory `region` as free.
    ///
    /// # Panics
    ///
    /// This function panics in debug mode if:
    /// * the provided region falls outside of the memory tracked by the
    ///   `SegmentTracker`, or
    /// * the provided region is in part or whole already marked as free.
    fn release(&mut self, region: ByteRange) {
        if region.is_empty() {
            return;
        }
        #[cfg(debug_assertions)]
        if !self.whole_range().contains(region) {
            panic!("{} not contained in segment tracker", region);
        }

        if let Some(found) = self
            .unoccupied
            .iter_mut()
            .find(|it| region.1 == it.0 || it.1 == region.0 || it.contains(region))
        {
            #[cfg(debug_assertions)]
            if found.overlaps(region) {
                panic!("double free in segment tracker");
            }
            found.apply_union_unchecked(region);
        } else if let Some((i, _)) = self
            .unoccupied
            .iter()
            .enumerate()
            .find(|it| it.0 > region.0)
        {
            self.unoccupied.insert(i, region);
        } else {
            self.unoccupied.push(region);
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.unoccupied.clear();
        self.unoccupied.push(self.whole_range())
    }
}

/// A result of [`SegmentTracker::peek_next`] which contains information
/// about available allocation slot and wherein a certain [`Layout`] could be
/// placed.
///
/// `'a` is the lifetime of the [`SegmentTracker`] that produced this struct.
/// The reference is stored because it prevents any mutations from ocurring on
/// the tracker while a `Location` object is alive, which ensures it points to a
/// valid [`ByteRange`] stored in the tracker which can be acted upon without
/// incurring any additional lookup costs.
pub struct Location<'a> {
    parent: &'a mut SegmentTracker,
    index: usize,
    whole: ByteRange,
    usable: ByteRange,
}

impl<'a> Location<'a> {
    /// Creates a `Location` for a zero-sized struct in the `parent`.
    pub fn zero_sized(parent: &'a mut SegmentTracker) -> Self {
        Location {
            parent,
            index: 0,
            whole: ByteRange(0, 0),
            usable: ByteRange(0, 0),
        }
    }

    /// Creates a `Location` for a given `SegmentTracker` with required fields.
    pub fn new(
        parent: &'a mut SegmentTracker,
        index: usize,
        whole: ByteRange,
        usable: ByteRange,
    ) -> Self {
        Location {
            parent,
            index,
            whole,
            usable,
        }
    }

    /// Returns the index of the containing byte range for the insertion
    /// location.
    pub fn position(&self) -> usize {
        self.index
    }

    /// Returns the containing byte range of the insertion location.
    pub fn range(&self) -> ByteRange {
        self.whole
    }

    /// Returns a usable byte range of the insertion location.
    pub fn usable_range(&self) -> ByteRange {
        self.usable
    }

    /// Returns `true` if the pointed to location is zero-sized.
    #[inline]
    pub fn is_zero_sized(&self) -> bool {
        self.usable.len() == 0
    }

    /// Marks the pointed to location as occupied.
    pub fn mark_occupied(&mut self) {
        if self.is_zero_sized() {
            return;
        }

        let left = ByteRange(self.whole.0, self.usable.0);
        let right = ByteRange(self.usable.1, self.whole.1);

        // these are intentionally ordered by likelyhood to reduce cache misses
        match (left.is_empty(), right.is_empty()) {
            (true, false) => {
                // left aligned
                self.parent.unoccupied[self.index] = right;
            }
            (false, false) => {
                // remaining space before and after
                self.parent.unoccupied[self.index] = left;
                self.parent.unoccupied.insert(self.index + 1, right);
            }
            (true, true) => {
                // available occupies entirety of found
                self.parent.unoccupied.remove(self.index);
            }
            (false, true) => {
                // right aligned
                self.parent.unoccupied[self.index] = left;
            }
        }
    }
}

/// Represents a range of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange(
    /// **Inclusive** lower bound of this byte range.
    pub usize,
    /// **Exclusive** upper bound of this byte range.
    pub usize,
);

#[allow(unused)]
impl ByteRange {
    pub const EMPTY: ByteRange = ByteRange(0, 0);

    /// Constructs a new byte range, ensuring that `from` and `to` are ordered
    /// correctly.
    pub fn new(from: usize, to: usize) -> Self {
        ByteRange(from.min(to), to.max(from))
    }

    /// Constructs a new byte range without checking `from` and `to` ordering.
    pub fn new_unchecked(from: usize, to: usize) -> Self {
        ByteRange(from, to)
    }

    /// Aligns the start of this byte range to the provided `alignment`.
    pub fn aligned(&self, alignment: usize) -> Self {
        let modulo = self.0 % alignment;
        if modulo == 0 {
            return *self;
        }
        ByteRange(self.0 + alignment - modulo, self.1)
    }

    /// Aligns the start of this byte range to the provided `alignment`.
    pub fn offset_aligned(&self, alignment: usize) -> Self {
        let modulo = self.0 % alignment;
        if modulo == 0 {
            return *self;
        }
        self.offset(alignment - modulo)
    }

    /// Caps the size of this byte range to the provided `size`.
    pub fn cap_size(&self, size: usize) -> Self {
        if self.len() < size {
            return *self;
        }
        ByteRange(self.0, self.0 + size)
    }

    /// Offsets this byte range by a provided unsigned `offset`.
    #[inline]
    pub fn offset(&self, offset: usize) -> Self {
        ByteRange(self.0 + offset, self.1 + offset)
    }

    /// Offsets this byte range by a provided signed offset.
    pub fn offset_signed(&self, offset: isize) -> Self {
        ByteRange(
            ((self.0 as isize).wrapping_add(offset)) as usize,
            ((self.1 as isize).wrapping_add(offset)) as usize,
        )
    }

    /// Returns length of this byte range.
    #[inline]
    pub fn len(&self) -> usize {
        self.1 - self.0
    }

    /// Returns true if this byte range is zero-sized.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == self.1
    }

    /// Returns `true` if this byte range contains `other` byte range.
    #[inline]
    pub fn contains(&self, other: Self) -> bool {
        self.0 <= other.0 && other.1 <= self.1
    }

    /// Returns `true` if `other` byte range overlaps this byte range.
    #[inline]
    pub fn overlaps(&self, other: Self) -> bool {
        self.contains(other)
            || (other.0 <= self.0 && other.1 > self.0)
            || (other.0 < self.1 && other.1 > self.1)
    }

    /// Merges another `other` byte range into this one, resulting in a byte
    /// range that contains both.
    pub fn apply_union_unchecked(&mut self, other: Self) {
        self.0 = self.0.min(other.0);
        self.1 = self.1.max(other.1);
    }
}

impl Display for ByteRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[{:x}, {:x})", self.0, self.1)
    }
}
