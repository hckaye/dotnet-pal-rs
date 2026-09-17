//! Assembles the native adapter objects a browser build of the .NET runtime
//! needs next to this port, when the WASI SDK is available.
//!
//! The boundary tests (`run.sh`) need no adapters: they link `tests/browser.c`
//! against the Rust archive only. The managed application (`app/`) is compiled
//! by the experimental NativeAOT LLVM toolchain for `wasm32-wasip1`; for that
//! link the port is built with `--target wasm32-wasip1` and the adapters below
//! are compiled with the WASI SDK compiler named by `WASI_SDK_PATH`.
use dotnet_pal_build::{write_nativeaot_props, Adapter, Build, Target};
use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=WASI_SDK_PATH");
    println!("cargo:rerun-if-env-changed=DOTNET_PAL_OBSERVER_ONLY");
    let target = env::var("TARGET").unwrap_or_default();
    let Some(sdk) = env::var_os("WASI_SDK_PATH") else {
        println!("cargo:warning=WASI_SDK_PATH not set: building the boundary only, no NativeAOT adapters");
        return;
    };
    if target != "wasm32-wasip1" {
        println!("cargo:warning=NativeAOT adapters are built for --target wasm32-wasip1 only (current: {target})");
        return;
    }
    let sdk = PathBuf::from(sdk);
    // Point the cc crate at the SDK compilers for this target triple.
    env::set_var("CC_wasm32-wasip1", sdk.join("bin/clang"));
    env::set_var("CC_wasm32-wasip2", sdk.join("bin/clang"));
    env::set_var("CXX_wasm32-wasip1", sdk.join("bin/clang++"));
    env::set_var("AR_wasm32-wasip1", sdk.join("bin/llvm-ar"));
    let observer_only = env::var("DOTNET_PAL_OBSERVER_ONLY").is_ok_and(|v| v == "1");
    let mut build = Build::new()
        .target(Target::Wasm32Wasip1)
        .adapter(Adapter::LlvmGcLinear { observer_only })
        .adapter(Adapter::MinipalEntropy)
        .adapter(Adapter::P1ErrorText)
        .name("browser_port_adapters");
    // The heap variant takes its storage from wasi-libc through the C hooks.
    if env::var_os("CARGO_FEATURE_HEAP").is_some() { build = build.adapter(Adapter::LinearHeapPosix); }
    let artifacts = build.compile().expect("native adapters compile with the WASI SDK");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    // OUT_DIR is <target-dir>/<triple>/<profile>/build/<pkg>-<hash>/out; the archive sits three levels up.
    let port_library = out.ancestors().nth(3).map(|p| p.join("libbrowser_port.a"))
        .unwrap_or_else(|| PathBuf::from(format!("target/wasm32-wasip1/{profile}/libbrowser_port.a")));
    write_nativeaot_props(out.join("browser-port.props"), &port_library, &artifacts, &["-Wl,--max-memory=134217728"])
        .expect("MSBuild props");
    println!("cargo:warning=NativeAOT link inputs: {}", out.join("browser-port.props").display());
}
