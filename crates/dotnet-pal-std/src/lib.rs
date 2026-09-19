//! Compatibility facade: re-exports only the current OS's platform crate.
//! New consumers may depend on dotnet-pal-linux-std, dotnet-pal-macos or
//! dotnet-pal-windows directly. No platform implementation lives here.
#[cfg(target_os = "linux")]
pub use dotnet_pal_linux_std::*;
#[cfg(target_os = "macos")]
pub use dotnet_pal_macos::*;
#[cfg(target_os = "windows")]
pub use dotnet_pal_windows::*;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("choose or implement a PAL crate for this platform");
