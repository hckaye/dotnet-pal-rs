//! The PL011 UART of the QEMU virt machine, mapped Device-nGnRnE at 0x0900_0000.
//!
//! Transmission polls the FIFO-full flag; there is no interrupt, no receive path
//! and no flow control. QEMU's PL011 accepts data without programming the baud
//! rate divisors, so this driver leaves the line configuration alone.
use core::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x0900_0000;
const DATA: *mut u32 = BASE as *mut u32;
const FLAG: *const u32 = (BASE + 0x18) as *const u32;
const TRANSMIT_FULL: u32 = 1 << 5;

pub fn write(bytes: &[u8]) {
    for &byte in bytes {
        // SAFETY: the boot code maps this page as device memory before Rust runs.
        unsafe {
            while read_volatile(FLAG) & TRANSMIT_FULL != 0 {}
            write_volatile(DATA, byte as u32);
        }
    }
}
pub fn write_str(text: &str) {
    write(text.as_bytes());
}
/// No serial input is wired up: the port advertises no console read service.
pub fn read() -> Option<u8> {
    None
}
pub fn write_dec(mut value: u64) {
    let mut digits = [0u8; 20];
    let mut used = 0;
    loop {
        digits[used] = b'0' + (value % 10) as u8;
        used += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut out = [0u8; 20];
    for i in 0..used {
        out[i] = digits[used - 1 - i];
    }
    write(&out[..used]);
}
pub fn write_hex(value: u64) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 18];
    out[0] = b'0';
    out[1] = b'x';
    for i in 0..16 {
        out[2 + i] = DIGITS[((value >> (60 - 4 * i)) & 0xf) as usize];
    }
    write(&out);
}
