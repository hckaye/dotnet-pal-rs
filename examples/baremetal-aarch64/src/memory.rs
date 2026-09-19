//! All of the port's memory comes from one static region: the RAM between the
//! end of the image's boot stack (`__heap_start`) and the end of the machine's
//! RAM (`__heap_end`, 0x8000_0000 with `-m 1024`).
//!
//! The RAM is identity-backed and bounded by the machine's physical capacity;
//! it cannot overcommit or remap a freed physical frame at a different address.
//! Unlike eager storage, reservations and decommitted pages are inaccessible.
//! The 4 KiB page descriptors enforce mapping permissions at EL1. Commit zeroes
//! newly accessible pages and preserves pages that were already committed.
use crate::sync::Single;
use crate::Baremetal;
use core::{ffi::c_void, ptr};
use dotnet_pal_rs::port::{self, Error, Result};

mod pages;
pub use pages::readable;

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
            let Some(start) = extent.start.checked_add(alignment - 1).map(|n| n & !(alignment - 1)) else { continue };
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
    /// Whether a return can be recorded without overlap or metadata exhaustion.
    fn can_give(&self, start: usize, size: usize) -> bool {
        let Some(end) = start.checked_add(size) else { return false };
        if size == 0 { return false; }
        let mut adjacent = false;
        for extent in &self.extents[..self.count] {
            if start < extent.end && extent.start < end { return false; }
            adjacent |= extent.end == start || extent.start == end;
        }
        adjacent || self.count < MAX_EXTENTS
    }
    /// Returns a range to the free list. Failure leaves the list unchanged.
    pub fn give(&mut self, start: usize, size: usize) -> bool {
        if !self.can_give(start, size) { return false; }
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
    assert!(region.give(start, end - start));
    // No free page is accessible. The image, page tables and boot stack are
    // below start and stay mapped; take() maps owned storage before using it.
    unsafe { pages::protect(start, end - start, 0) }.expect("RAM page table bounds");
}
pub fn bounds() -> (usize, usize) {
    (
        round_up(ptr::addr_of!(__heap_start) as usize, PAGE),
        ptr::addr_of!(__heap_end) as usize & !(PAGE - 1),
    )
}
/// Takes `size` bytes (a page multiple) aligned to `alignment` from the region.
pub fn take(size: usize, alignment: usize) -> Option<usize> {
    if size == 0 || size % PAGE != 0 || alignment < PAGE || !alignment.is_power_of_two() { return None; }
    // SAFETY: no borrow is held across a call that could switch threads.
    let region = unsafe { REGION.get() };
    let address = region.take(size, alignment)?;
    if unsafe { pages::protect(address, size, dotnet_pal_rs::runtime::READ | dotnet_pal_rs::runtime::WRITE) }.is_err() {
        assert!(region.give(address, size)); // exactly reverses the take
        return None;
    }
    Some(address)
}
pub fn give(start: usize, size: usize) -> bool {
    // Check that the return can be recorded before revoking the caller's memory.
    // Single core, no yield: can_give remains true until give records the return.
    let region = unsafe { REGION.get() };
    if !region.can_give(start, size) { return false; }
    if unsafe { pages::protect(start, size, 0) }.is_err() { return false; }
    region.give(start, size)
}
pub fn available() -> usize {
    unsafe { REGION.get() }.available()
}
pub fn zero(address: usize, size: usize) {
    // SAFETY: the caller owns the range and has made all of its pages writable.
    unsafe { ptr::write_bytes(address as *mut u8, 0, size) };
}

impl port::VirtualMemory for Baremetal {
    fn page_size() -> usize { PAGE }
    unsafe fn reserve(size: usize, alignment: usize) -> Result<*mut c_void> {
        crate::trace(b"vm reserve size", size as u64);
        let address = take(size, alignment).ok_or(Error::OutOfMemory)?;
        // No callback or yield can expose the temporary writable mapping.
        if let Err(error) = unsafe { pages::protect(address, size, 0) } {
            assert!(give(address, size));
            return Err(error);
        }
        crate::trace(b"vm reserve at", address as u64);
        Ok(address as *mut c_void)
    }
    unsafe fn commit(address: *mut c_void, size: usize) -> Result<()> {
        crate::trace(b"vm commit at", address as u64);
        unsafe { pages::commit(address as usize, size) }
    }
    unsafe fn decommit(address: *mut c_void, size: usize) -> Result<()> {
        unsafe { pages::protect(address as usize, size, 0) }
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        if give(address as usize, size) { Ok(()) } else { Err(Error::Os) }
    }
    /// Reset may retain contents. Unlike decommit, it does not revoke access.
    unsafe fn reset(_address: *mut c_void, _size: usize) -> Result<()> { Ok(()) }
}

fn page_rounded(size: usize) -> Result<usize> {
    if size == 0 { return Err(Error::InvalidArgument); }
    size.checked_add(PAGE - 1).map(|n| n & !(PAGE - 1)).ok_or(Error::InvalidArgument)
}
impl port::NativeMapping for Baremetal {
    fn page_size() -> usize { PAGE }
    unsafe fn allocate(size: usize, protection: u32) -> Result<*mut c_void> {
        pages::flags(protection)?; // reject unrepresentable permissions before allocating
        let size = page_rounded(size)?;
        let address = take(size, PAGE).ok_or(Error::OutOfMemory)?;
        zero(address, size);
        if let Err(error) = unsafe { pages::protect(address, size, protection) } {
            assert!(give(address, size));
            return Err(error);
        }
        Ok(address as *mut c_void)
    }
    unsafe fn release(address: *mut c_void, size: usize) -> Result<()> {
        if give(address as usize, page_rounded(size)?) { Ok(()) } else { Err(Error::Os) }
    }
    unsafe fn protect(address: *mut c_void, size: usize, protection: u32) -> Result<()> {
        unsafe { pages::protect(address as usize, page_rounded(size)?, protection) }
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
/// the image's only user of `alloc`; over-aligned requests keep a back-pointer
/// so deallocation can return the underlying native block.
struct PortAllocator;
// SAFETY: blocks come from the native heap above, which hands out exclusive,
// 16-byte aligned storage and keeps a failed resize's old block intact. A stricter
// alignment (the in-memory file system maps files in whole pages) is a larger heap
// block with the aligned address inside it and the block's own address in the word
// before that, which is where `dealloc` finds it again.
unsafe impl core::alloc::GlobalAlloc for PortAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if layout.align() <= GRAIN {
            return unsafe { <Baremetal as port::NativeHeap>::allocate(layout.size().max(1), false) }.map_or(ptr::null_mut(), |p| p.cast());
        }
        const WORD: usize = core::mem::size_of::<usize>();
        let Some(total) = layout.size().checked_add(layout.align()).and_then(|n| n.checked_add(WORD)) else { return ptr::null_mut(); };
        let Ok(block) = (unsafe { <Baremetal as port::NativeHeap>::allocate(total, false) }) else { return ptr::null_mut(); };
        let aligned = (block as usize + WORD).next_multiple_of(layout.align());
        // SAFETY: `aligned - WORD` lies inside the block, at or after its start, and is word-aligned because `aligned` is.
        unsafe { ((aligned - WORD) as *mut usize).write(block as usize) };
        aligned as *mut u8
    }
    unsafe fn dealloc(&self, address: *mut u8, layout: core::alloc::Layout) {
        let block = if layout.align() <= GRAIN { address.cast() } else {
            // SAFETY: `alloc` stored the block's address in the word before an over-aligned address.
            unsafe { (address as *const usize).sub(1).read() as *mut core::ffi::c_void }
        };
        let _ = unsafe { <Baremetal as port::NativeHeap>::release(block) };
    }
    unsafe fn realloc(&self, address: *mut u8, layout: core::alloc::Layout, size: usize) -> *mut u8 {
        if layout.align() <= GRAIN {
            return unsafe { <Baremetal as port::NativeHeap>::resize(address.cast(), size.max(1)) }.map_or(ptr::null_mut(), |p| p.cast());
        }
        // An over-aligned block cannot be resized in place: the heap would keep the block's alignment, not the address's.
        let Ok(wanted) = core::alloc::Layout::from_size_align(size, layout.align()) else { return ptr::null_mut(); };
        let fresh = unsafe { self.alloc(wanted) };
        if !fresh.is_null() {
            unsafe { ptr::copy_nonoverlapping(address, fresh, layout.size().min(size)); self.dealloc(address, layout); }
        }
        fresh
    }
}
#[global_allocator]
static ALLOCATOR: PortAllocator = PortAllocator;
