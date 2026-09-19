//! Updates the 4 KiB RAM descriptors built by boot.S. Only the allocatable
//! region may be changed: the image, stacks used during boot and page tables
//! themselves must remain mapped. The port is single-core and these routines
//! never yield or retain a borrow across a context switch.
use super::{bounds, zero, PAGE};
use core::{arch::asm, ptr};
use dotnet_pal_rs::port::{Error, Result};
use dotnet_pal_rs::runtime::{EXECUTE, READ, WRITE};

const RAM_START: usize = 0x4000_0000;
const RAM_PAGES: usize = 0x4000_0000 / PAGE;
const VALID: u64 = 1;
const PAGE_FLAGS: u64 = 3 | (1 << 2) | (3 << 8) | (1 << 10);
const READ_ONLY: u64 = 1 << 7;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

extern "C" {
    static mut pal_ram_pages: [u64; RAM_PAGES];
}

/// AArch64 at EL1 cannot express write-only or execute-only mappings. Refuse
/// them instead of silently granting read access. No access is representable
/// by an invalid descriptor, and the remaining combinations use AP and PXN.
pub fn flags(protection: u32) -> Result<u64> {
    if protection & !(READ | WRITE | EXECUTE) != 0 {
        return Err(Error::InvalidArgument);
    }
    if protection == 0 { return Ok(0); }
    if protection & READ == 0 { return Err(Error::Unsupported); }
    Ok(PAGE_FLAGS | UXN
        | if protection & WRITE == 0 { READ_ONLY } else { 0 }
        | if protection & EXECUTE == 0 { PXN } else { 0 })
}

fn range(address: usize, size: usize) -> Result<(usize, usize)> {
    let (low, high) = bounds();
    let end = address.checked_add(size).ok_or(Error::InvalidArgument)?;
    if size == 0 || address % PAGE != 0 || size % PAGE != 0
        || address < low || end > high || low < RAM_START
        || high > RAM_START + RAM_PAGES * PAGE
    {
        return Err(Error::InvalidArgument);
    }
    Ok(((address - RAM_START) / PAGE, (end - RAM_START) / PAGE))
}

unsafe fn entry(index: usize) -> *mut u64 {
    // SAFETY: every caller checked its indices against RAM_PAGES. No reference to
    // the static is created, and the descriptors are accessed only volatilely.
    unsafe { ptr::addr_of_mut!(pal_ram_pages).cast::<u64>().add(index) }
}

unsafe fn publish() {
    // Make page table writes visible before any subsequent translated access.
    unsafe { asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
}

/// Change a whole range using break-before-make, including when moving from a
/// valid descriptor to another valid descriptor with different permissions.
pub unsafe fn protect(address: usize, size: usize, protection: u32) -> Result<()> {
    let attributes = flags(protection)?;
    let (first, end) = range(address, size)?;
    for index in first..end {
        unsafe { ptr::write_volatile(entry(index), 0) };
    }
    unsafe {
        asm!("dsb ishst", "tlbi vmalle1", "dsb ish", "isb",
            options(nostack, preserves_flags));
    }
    if attributes != 0 {
        for index in first..end {
            let physical = (RAM_START + index * PAGE) as u64;
            unsafe { ptr::write_volatile(entry(index), physical | attributes) };
        }
        unsafe { publish() };
        if protection & EXECUTE != 0 {
            unsafe { sync_instructions(address, size) };
        }
    }
    Ok(())
}

/// Commit only previously inaccessible pages. Recommitting an already valid
/// page must preserve its contents; a decommitted page must read back as zero.
pub unsafe fn commit(address: usize, size: usize) -> Result<()> {
    let (mut index, end) = range(address, size)?;
    let attributes = flags(READ | WRITE)?;
    while index < end {
        if unsafe { ptr::read_volatile(entry(index)) } & VALID != 0 {
            index += 1;
            continue;
        }
        let first = index;
        while index < end && unsafe { ptr::read_volatile(entry(index)) } & VALID == 0 {
            let physical = (RAM_START + index * PAGE) as u64;
            unsafe { ptr::write_volatile(entry(index), physical | attributes) };
            index += 1;
        }
        // reserve/decommit already invalidated the old translations. No valid
        // mapping is replaced here, so only publication is needed.
        unsafe { publish() };
        zero(RAM_START + first * PAGE, (index - first) * PAGE);
    }
    Ok(())
}

/// Instruction fetch must see code written before an executable mapping is
/// published, including a second RW -> RX transition at the same address.
unsafe fn sync_instructions(address: usize, size: usize) {
    let ctr: u64;
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let data_line = 4usize << ((ctr >> 16) & 15);
    let instruction_line = 4usize << (ctr & 15);
    let end = address + size; // range() has checked this addition.
    let mut line = address & !(data_line - 1);
    while line < end {
        unsafe { asm!("dc cvau, {}", in(reg) line, options(nostack, preserves_flags)) };
        line += data_line;
    }
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    line = address & !(instruction_line - 1);
    while line < end {
        unsafe { asm!("ic ivau, {}", in(reg) line, options(nostack, preserves_flags)) };
        line += instruction_line;
    }
    unsafe { asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// Queries actual RAM descriptors, including image pages below the allocator.
/// Being inside the RAM address window is not proof that a page is readable.
pub fn readable(address: usize, size: usize) -> Result<bool> {
    let end = address.checked_add(size).ok_or(Error::InvalidArgument)?;
    let ram_end = RAM_START + RAM_PAGES * PAGE;
    if address < RAM_START || address >= ram_end || end > ram_end { return Ok(false); }
    if size == 0 { return Ok(true); }
    let first = (address - RAM_START) / PAGE;
    let last = (end - 1 - RAM_START) / PAGE;
    for index in first..=last {
        // SAFETY: indices are inside the statically allocated RAM page table.
        if unsafe { ptr::read_volatile(entry(index)) } & VALID == 0 { return Ok(false); }
    }
    Ok(true)
}
