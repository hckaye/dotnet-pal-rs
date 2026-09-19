//! All of the port's memory comes from one static region: the RAM between the
//! end of the image's boot stack (`__heap_start`) and the end of the machine's
//! RAM (`__heap_end`, 0x8000_0000 with `-m 1024`).
//!
//! **This is eager RAM, not virtual memory.** Every byte the region hands out is
//! already mapped, readable and writable by the identity map the boot code
//! installed. There are no inaccessible reservations, no page protection and no
//! way to fault on an uncommitted access. What the port does keep are the
//! contracts the boundary's front ends check: `reserve` returns a range aligned
//! as asked, ranges are page multiples, distinct live allocations never overlap,
//! and freshly reserved or decommitted storage reads back as zero.
use crate::sync::Single;
use crate::Baremetal;
use core::{ffi::c_void, ptr};
use dotnet_pal_rs::port::{self, Error, Result};

pub const PAGE: usize = 4096;
/// Slice of the region the native helper heap takes on its first allocation.
const NATIVE_HEAP_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXTENTS: usize = 256;

extern "C" {
    static __heap_start: u8;
    static __heap_end: u8;
}

pub const fn round_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[derive(Clone, Copy)]
struct Extent {
    start: usize,
    end: usize,
}
const NONE: Extent = Extent { start: 0, end: 0 };

/// Address-ordered free list with coalescing. First fit, no per-allocation
/// metadata: the caller returns the same size it was given, exactly as the C
/// boundary requires for `release`.
pub struct Region {
    extents: [Extent; MAX_EXTENTS],
    count: usize,
}
impl Region {
    const fn new() -> Self {
        Self { extents: [NONE; MAX_EXTENTS], count: 0 }
    }
    fn remove(&mut self, index: usize) {
        for i in index..self.count - 1 {
            self.extents[i] = self.extents[i + 1];
        }
        self.count -= 1;
    }
    fn insert(&mut self, index: usize, extent: Extent) {
        let mut i = self.count;
        while i > index {
            self.extents[i] = self.extents[i - 1];
            i -= 1;
        }
        self.extents[index] = extent;
        self.count += 1;
    }
    /// Carves `size` bytes aligned to `alignment`, or `None` when no extent fits.
    pub fn take(&mut self, size: usize, alignment: usize) -> Option<usize> {
        if size == 0 || !alignment.is_power_of_two() {
            return None;
        }
        for index in 0..self.count {
            let extent = self.extents[index];
            let start = round_up(extent.start, alignment);
            let Some(end) = start.checked_add(size) else { continue };
            if start < extent.start || end > extent.end {
                continue;
            }
            match (start == extent.start, end == extent.end) {
                (true, true) => self.remove(index),
                (true, false) => self.extents[index].start = end,
                (false, true) => self.extents[index].end = start,
                (false, false) => {
                    if self.count == MAX_EXTENTS {
                        continue; // no room to record the tail; try the next extent
                    }
                    self.extents[index].end = start;
                    self.insert(index + 1, Extent { start: end, end: extent.end });
                }
            }
            return Some(start);
        }
        None
    }
    /// Returns a range to the free list. `false` means the free list is full and
    /// the range is leaked rather than merged into a neighbour.
    pub fn give(&mut self, start: usize, size: usize) -> bool {
        let Some(end) = start.checked_add(size) else { return false };
        if size == 0 {
            return false;
        }
        let mut index = 0;
        while index < self.count && self.extents[index].start < start {
            index += 1;
        }
        let after_previous = index > 0 && self.extents[index - 1].end == start;
        let before_next = index < self.count && self.extents[index].start == end;
        match (after_previous, before_next) {
            (true, true) => {
                self.extents[index - 1].end = self.extents[index].end;
                self.remove(index);
            }
            (true, false) => self.extents[index - 1].end = end,
            (false, true) => self.extents[index].start = start,
            (false, false) => {
                if self.count == MAX_EXTENTS {
                    return false;
                }
                self.insert(index, Extent { start, end });
            }
        }
        true
    }
    /// Bytes still free, for the report the test prints.
    pub fn available(&self) -> usize {
        let mut total = 0;
        for index in 0..self.count {
            total += self.extents[index].end - self.extents[index].start;
        }
        total
    }
}

static REGION: Single<Region> = Single::new(Region::new());

/// Publishes the RAM behind the image to the region. Called once from the boot
/// path before any provider runs.
pub fn init() {
    let start = round_up(ptr::addr_of!(__heap_start) as usize, PAGE);
    let end = ptr::addr_of!(__heap_end) as usize & !(PAGE - 1);
    // SAFETY: single core, no other borrow, called once before threads exist.
    let region = unsafe { REGION.get() };
    *region = Region::new();
    region.give(start, end - start);
}
pub fn bounds() -> (usize, usize) {
    (
        round_up(ptr::addr_of!(__heap_start) as usize, PAGE),
        ptr::addr_of!(__heap_end) as usize & !(PAGE - 1),
    )
}
/// Takes `size` bytes (a page multiple) aligned to `alignment` from the region.
pub fn take(size: usize, alignment: usize) -> Option<usize> {
    // SAFETY: no borrow is held across a call that could switch threads.
    unsafe { REGION.get() }.take(size, alignment)
}
pub fn give(start: usize, size: usize) -> bool {
    unsafe { REGION.get() }.give(start, size)
}
pub fn available() -> usize {
    unsafe { REGION.get() }.available()
}
pub fn zero(address: usize, size: usize) {
    // SAFETY: the caller owns the range and every region address is mapped RAM.
    unsafe { ptr::write_bytes(address as *mut u8, 0, size) };
}

impl port::VirtualMemory for Baremetal {
    fn page_size() -> usize {
        PAGE
    }
    /// The range is handed out already accessible and zeroed, so the first
    /// `commit` observes the zero-filled memory the GC expects.
    unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
        crate::trace(b"vm reserve size", size as u64);
        let address = take(size, alignment).ok_or(Error::OutOfMemory);
        crate::trace(b"vm reserve at", address.map_or(0, |a| a as u64));
        let address = address?;
        zero(address, size);
        Ok(address as *mut c_void)
    }
    /// Nothing to do: this port has no inaccessible reservations. Commit cannot
    /// fail here and cannot be a memory-pressure signal for the caller.
    unsafe fn commit(_address: *mut c_void, _size: usize) -> Result<()> {
        crate::trace(b"vm commit at", _address as u64);
        Ok(())
    }
    /// Zeroes the range. The memory stays readable afterwards; a decommitted
    /// access faults on a real OS and does not fault here.
    unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
        zero(address as usize, size);
        Ok(())
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        if give(address as usize, size) {
            Ok(())
        } else {
            Err(Error::Os)
        }
    }
    /// A reset only says the contents are no longer needed. Keeping them is a
    /// valid implementation and costs nothing here.
    unsafe fn reset(_address: *mut c_void, _size: usize) -> Result<()> {
        Ok(())
    }
}

impl port::NativeMapping for Baremetal {
    fn page_size() -> usize {
        PAGE
    }
    /// Same storage as `VirtualMemory`, with the protection bits ignored: every
    /// mapping is readable, writable and executable because the identity map has
    /// one set of permissions for all of RAM.
    unsafe fn allocate(size: usize, _protection: u32) -> Result<*mut c_void> {
        crate::trace(b"mapping allocate", size as u64);
        let size = round_up(size, PAGE);
        let address = take(size, PAGE).ok_or(Error::OutOfMemory)?;
        zero(address, size);
        Ok(address as *mut c_void)
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        if give(address as usize, round_up(size, PAGE)) {
            Ok(())
        } else {
            Err(Error::Os)
        }
    }
    unsafe fn protect(_address: *mut c_void, _size: usize, _protection: u32) -> Result<()> {
        Ok(())
    }
}

/// Native helper heap: one 64 MiB slice of the region, managed as an implicit
/// list of 16-byte-aligned blocks with a 16-byte header. First fit, with free
/// neighbours merged during the search.
#[repr(C)]
#[derive(Clone, Copy)]
struct Block {
    size: usize, // payload bytes, a multiple of 16
    free: usize, // 1 when the block is available
}
const HEADER: usize = 16;
const GRAIN: usize = 16;

struct Heap {
    start: usize,
    end: usize,
}
static HEAP: Single<Heap> = Single::new(Heap { start: 0, end: 0 });

fn header(address: usize) -> *mut Block {
    address as *mut Block
}
fn heap_ready() -> Result<(usize, usize)> {
    // SAFETY: single core, no borrow held across a thread switch.
    let heap = unsafe { HEAP.get() };
    if heap.start == 0 {
        let start = take(NATIVE_HEAP_BYTES, PAGE).ok_or(Error::OutOfMemory)?;
        // SAFETY: the slice is owned by this heap from now on.
        unsafe { header(start).write(Block { size: NATIVE_HEAP_BYTES - HEADER, free: 1 }) };
        heap.start = start;
        heap.end = start + NATIVE_HEAP_BYTES;
    }
    Ok((heap.start, heap.end))
}
impl port::NativeHeap for Baremetal {
    unsafe fn allocate(size: usize, zero_it: bool) -> Result<*mut c_void> {
        crate::trace(b"heap allocate", size as u64);
        let (start, end) = heap_ready()?;
        let wanted = round_up(size, GRAIN);
        let mut address = start;
        while address < end {
            // SAFETY: every header address comes from walking the heap's own blocks.
            let block = header(address);
            let mut current = unsafe { block.read() };
            if current.free == 1 {
                // Merge the free neighbours that follow before judging the size.
                loop {
                    let next = address + HEADER + current.size;
                    if next >= end {
                        break;
                    }
                    let following = unsafe { header(next).read() };
                    if following.free != 1 {
                        break;
                    }
                    current.size += HEADER + following.size;
                    unsafe { block.write(current) };
                }
                if current.size >= wanted {
                    if current.size >= wanted + HEADER + GRAIN {
                        let rest = address + HEADER + wanted;
                        unsafe {
                            header(rest).write(Block { size: current.size - wanted - HEADER, free: 1 })
                        };
                        current.size = wanted;
                    }
                    current.free = 0;
                    unsafe { block.write(current) };
                    let payload = address + HEADER;
                    if zero_it {
                        zero(payload, current.size);
                    }
                    return Ok(payload as *mut c_void);
                }
            }
            address += HEADER + current.size;
        }
        Err(Error::OutOfMemory)
    }
    /// Allocates before releasing, so a failure leaves the old block untouched.
    unsafe fn resize(address: *mut c_void, size: usize) -> Result<*mut c_void> {
        if address.is_null() {
            return unsafe { <Baremetal as port::NativeHeap>::allocate(size, false) };
        }
        let (start, end) = heap_ready()?;
        let block = address as usize;
        if block < start + HEADER || block >= end || block % GRAIN != 0 {
            return Err(Error::InvalidArgument);
        }
        let old = unsafe { header(block - HEADER).read() };
        if old.free != 0 {
            return Err(Error::InvalidArgument);
        }
        if round_up(size, GRAIN) <= old.size {
            return Ok(address);
        }
        let new = unsafe { <Baremetal as port::NativeHeap>::allocate(size, false) }?;
        // SAFETY: both blocks are live, owned by the caller and do not overlap.
        unsafe { ptr::copy_nonoverlapping(address as *const u8, new as *mut u8, old.size.min(size)) };
        unsafe { <Baremetal as port::NativeHeap>::release(address) }?;
        Ok(new)
    }
    unsafe fn release(address: *mut c_void) -> Result<()> {
        if address.is_null() {
            return Ok(());
        }
        let (start, end) = heap_ready()?;
        let block = address as usize;
        if block < start + HEADER || block >= end || block % GRAIN != 0 {
            return Err(Error::InvalidArgument);
        }
        // SAFETY: the address is inside the heap and the caller owns the block.
        let head = header(block - HEADER);
        let mut current = unsafe { head.read() };
        if current.free != 0 {
            return Err(Error::InvalidArgument); // double release
        }
        current.free = 1;
        unsafe { head.write(current) };
        Ok(())
    }
}

/// The Rust global allocator, over the native heap. The in-memory file system is
/// the image's only user of `alloc`; nothing in it asks for more than the heap's
/// 16-byte alignment, and a request that did would fail instead of being misaligned.
struct PortAllocator;
// SAFETY: blocks come from the native heap above, which hands out exclusive,
// 16-byte aligned storage and keeps a failed resize's old block intact.
unsafe impl core::alloc::GlobalAlloc for PortAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if layout.align() > GRAIN { return ptr::null_mut(); }
        unsafe { <Baremetal as port::NativeHeap>::allocate(layout.size().max(1), false) }.map_or(ptr::null_mut(), |p| p.cast())
    }
    unsafe fn dealloc(&self, address: *mut u8, _: core::alloc::Layout) {
        let _ = unsafe { <Baremetal as port::NativeHeap>::release(address.cast()) };
    }
    unsafe fn realloc(&self, address: *mut u8, layout: core::alloc::Layout, size: usize) -> *mut u8 {
        if layout.align() > GRAIN { return ptr::null_mut(); }
        unsafe { <Baremetal as port::NativeHeap>::resize(address.cast(), size.max(1)) }.map_or(ptr::null_mut(), |p| p.cast())
    }
}
#[global_allocator]
static ALLOCATOR: PortAllocator = PortAllocator;
