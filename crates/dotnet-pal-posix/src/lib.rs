#![no_std]
//! Source assets for reference C POSIX heap/support providers. Nothing is
//! compiled or linked until the build helper explicitly selects an adapter.
pub const SOURCE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/native");
