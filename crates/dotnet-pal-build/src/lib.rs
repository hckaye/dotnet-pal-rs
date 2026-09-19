//! `build.rs` support for ports of the .NET NativeAOT runtime onto `dotnet-pal-rs`.
//!
//! A port crate implements the traits in `dotnet_pal_rs::port`, declares its port
//! with `define_pal!`, and from `build.rs` asks this crate to compile the native
//! adapter objects that connect the audited runtime sources to the boundary:
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     dotnet_pal_build::Build::new()
//!         .target(dotnet_pal_build::Target::Wasm32Wasip1)
//!         .adapter(dotnet_pal_build::Adapter::LlvmGcLinear { observer_only: false })
//!         .adapter(dotnet_pal_build::Adapter::MinipalEntropy)
//!         .adapter(dotnet_pal_build::Adapter::P1ErrorText)
//!         .compile()
//!         .expect("native adapters");
//! }
//! ```
//!
//! `compile` produces one static archive in `OUT_DIR`, emits the Cargo link
//! directives for it, and can write an MSBuild `.props` file that a NativeAOT
//! project imports so `dotnet publish` links the port and its adapters.
//! Everything else (the C# compiler, the runtime packs, the WASI SDK) stays the
//! responsibility of the surrounding pipeline; this crate never downloads anything.
use std::{env, fs, io, path::{Path, PathBuf}};

/// Target profile the adapters are compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Native POSIX process with sparse virtual memory (`CAP_VM`).
    Native,
    /// WASI Preview 1 module linked with wasi-libc (experimental NativeAOT LLVM).
    Wasm32Wasip1,
    /// Freestanding wasm32 module without libc; only allocation-free adapters apply.
    Wasm32Freestanding,
}

/// Native adapter sources shipped in this build-only package. Reference
/// POSIX providers live in the separately enabled `dotnet-pal-posix` package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    /// `--wrap` definitions routing the LLVM Wasm GC's virtual-memory and clock
    /// calls to the linear-storage adapter. `observer_only` compiles only the
    /// statistics observers (the baseline negative control).
    LlvmGcLinear { observer_only: bool },
    /// Entropy for `minipal` through the boundary's `random_bytes`.
    MinipalEntropy,
    /// Pure errno-to-text formatter required by the published `System.Native`.
    P1ErrorText,
    /// Storage hooks for `linear-heap` backed by `posix_memalign`/`free`.
    LinearHeapPosix,
    /// Native helper-service provider (`host-support`) backed by POSIX.
    SupportPosix,
    /// The 45 raw WASI thunks of the single-import isolated profile.
    WasiBridge,
}
impl Adapter {
    fn source(self) -> &'static str {
        match self {
            Adapter::LlvmGcLinear { .. } => "integration/llvm-wasi/gc_linear_wrap.cpp",
            Adapter::MinipalEntropy => "native/minipal_entropy_adapter.c",
            Adapter::P1ErrorText => "integration/llvm-wasi/p1_error_text.c",
            Adapter::LinearHeapPosix => "native/linear_heap_posix.c",
            Adapter::SupportPosix => "native/support_posix.c",
            Adapter::WasiBridge => "native/wasi_bridge.c",
        }
    }
    fn supported(self, target: Target) -> bool {
        match self {
            Adapter::LlvmGcLinear { .. } | Adapter::P1ErrorText | Adapter::WasiBridge => target == Target::Wasm32Wasip1,
            Adapter::LinearHeapPosix | Adapter::SupportPosix => matches!(target, Target::Native | Target::Wasm32Wasip1),
            Adapter::MinipalEntropy => true,
        }
    }
}

/// GC and clock definitions of the pinned LLVM Wasm runtime pack that the
/// `LlvmGcLinear` adapter redirects with `--wrap`. Mirrors
/// `integration/llvm-wasi/symbols.py`, which audits them against the pack.
pub const LLVM_GC_WRAP_SYMBOLS: [&str; 9] = [
    "_ZN15GCToOSInterface14VirtualReserveEmmjt",
    "_ZN15GCToOSInterface13VirtualCommitEPvmt",
    "_ZN15GCToOSInterface15VirtualDecommitEPvm",
    "_ZN15GCToOSInterface14VirtualReleaseEPvm",
    "_ZN15GCToOSInterface12VirtualResetEPvmb",
    "_ZN15GCToOSInterface33VirtualReserveAndCommitLargePagesEmt",
    "_ZN15GCToOSInterface23QueryPerformanceCounterEv",
    "_ZN15GCToOSInterface25QueryPerformanceFrequencyEv",
    "_ZN15GCToOSInterface24GetLowPrecisionTimeStampEv",
];

/// Where this build-only package ships its native adapter sources.
/// DOTNET_PAL_SOURCE_ROOT may select another directory with the same asset layout.
pub fn source_root() -> PathBuf {
    if let Some(root) = env::var_os("DOTNET_PAL_SOURCE_ROOT") { return PathBuf::from(root); }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
/// Directory containing `dotnet_pal.h`.
pub fn include_dir() -> PathBuf { PathBuf::from(dotnet_pal_rs::C_INCLUDE_DIR) }

/// Outputs of a successful [`Build::compile`].
#[derive(Debug)]
pub struct Artifacts {
    /// The adapter archive written to `OUT_DIR`.
    pub archive: PathBuf,
    /// The compiled objects, one per adapter, in the order they were requested.
    pub objects: Vec<PathBuf>,
    /// Linker arguments the adapters require (`--wrap` redirections).
    pub linker_args: Vec<String>,
}

/// A configured adapter build.
#[derive(Debug)]
pub struct Build {
    target: Target,
    adapters: Vec<Adapter>,
    defines: Vec<(String, Option<String>)>,
    includes: Vec<PathBuf>,
    name: String,
}
impl Default for Build { fn default() -> Self { Self::new() } }
impl Build {
    pub fn new() -> Self {
        Self { target: Target::Native, adapters: Vec::new(), defines: Vec::new(), includes: Vec::new(), name: "dotnet_pal_adapters".into() }
    }
    pub fn target(mut self, target: Target) -> Self { self.target = target; self }
    pub fn adapter(mut self, adapter: Adapter) -> Self { self.adapters.push(adapter); self }
    pub fn define(mut self, name: &str, value: Option<&str>) -> Self { self.defines.push((name.into(), value.map(String::from))); self }
    pub fn include(mut self, dir: impl Into<PathBuf>) -> Self { self.includes.push(dir.into()); self }
    /// Archive base name (`lib<name>.a`).
    pub fn name(mut self, name: &str) -> Self { self.name = name.into(); self }

    /// Compiles the requested adapters with the `cc` crate into one archive and
    /// emits `cargo:rustc-link-search` / `cargo:rustc-link-lib` for it. The C
    /// compiler comes from the usual `cc` environment (`CC`, `CXX`,
    /// `CC_wasm32-wasip1`, ...); for WASI targets point it at the WASI SDK.
    pub fn compile(self) -> io::Result<Artifacts> {
        let out = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("OUT_DIR is set only inside build scripts"))?);
        let root = source_root();
        for adapter in &self.adapters {
            if !adapter.supported(self.target) {
                return Err(io::Error::other(format!("{adapter:?} is not available for {:?}", self.target)));
            }
        }
        let mut objects = Vec::new();
        let mut c = cc::Build::new();
        let mut cxx = cc::Build::new();
        for build in [&mut c, &mut cxx] {
            build.include(include_dir()).include(root.join("native")).warnings(true).extra_warnings(true)
                .flag_if_supported("-Werror").flag_if_supported("-ffunction-sections").flag_if_supported("-fdata-sections").opt_level(2);
            for dir in &self.includes { build.include(dir); }
            for (name, value) in &self.defines { build.define(name, value.as_deref()); }
            if self.target == Target::Wasm32Freestanding { build.flag("-ffreestanding").flag("-fno-builtin"); }
        }
        cxx.cpp(true).flag_if_supported("-std=c++17").flag("-fno-exceptions").flag("-fno-rtti");
        c.flag_if_supported("-std=c11");
        // The pure errno formatter needs the P2 sysroot's netdb.h constants (the
        // P1 sysroot has none); it performs no OS calls and links into a P1 module.
        let mut p2 = cc::Build::new();
        p2.include(include_dir()).warnings(true).extra_warnings(true).flag_if_supported("-Werror")
            .flag_if_supported("-std=c11").opt_level(2).target("wasm32-wasip2");
        let (mut have_c, mut have_cxx, mut have_p2) = (false, false, false);
        for adapter in &self.adapters {
            let source = match adapter {
                Adapter::LinearHeapPosix | Adapter::SupportPosix => {
                    #[cfg(feature = "posix")]
                    { PathBuf::from(dotnet_pal_posix::SOURCE_DIR).join(Path::new(adapter.source()).file_name().unwrap()) }
                    #[cfg(not(feature = "posix"))]
                    { return Err(io::Error::other("POSIX providers require dotnet-pal-build's explicit posix feature")); }
                }
                _ => root.join(adapter.source()),
            };
            println!("cargo:rerun-if-changed={}", source.display());
            match adapter {
                Adapter::LlvmGcLinear { observer_only } => {
                    if *observer_only { cxx.define("DOTNET_PAL_OBSERVER_ONLY", None); }
                    cxx.file(&source); have_cxx = true;
                }
                Adapter::P1ErrorText => { p2.file(&source); have_p2 = true; }
                _ => { c.file(&source); have_c = true; }
            }
            objects.push(out.join(source.file_name().unwrap()).with_extension("o"));
        }
        println!("cargo:rerun-if-changed={}", include_dir().join("dotnet_pal.h").display());
        println!("cargo:rerun-if-env-changed=DOTNET_PAL_SOURCE_ROOT");
        // Several compiler invocations, one archive: compile separately, then merge objects.
        let mut compiled = Vec::new();
        if have_c { compiled.extend(c.compile_intermediates()); }
        if have_cxx { compiled.extend(cxx.compile_intermediates()); }
        if have_p2 { compiled.extend(p2.compile_intermediates()); }
        let archive = out.join(format!("lib{}.a", self.name));
        let _ = fs::remove_file(&archive);
        let archiver = cc::Build::new().get_archiver();
        let status = std::process::Command::new(archiver.get_program()).args(archiver.get_args()).arg("crs").arg(&archive).args(&compiled).status()?;
        if !status.success() { return Err(io::Error::other("archiver failed")); }
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static={}", self.name);
        println!("cargo:include={}", include_dir().display());
        let mut linker_args = Vec::new();
        if self.adapters.iter().any(|a| matches!(a, Adapter::LlvmGcLinear { observer_only: false })) {
            linker_args.extend(LLVM_GC_WRAP_SYMBOLS.iter().map(|s| format!("-Wl,--wrap={s}")));
        }
        Ok(Artifacts { archive, objects: compiled, linker_args })
    }
}

/// Writes an MSBuild props file that makes a NativeAOT project link the port's
/// static library and the adapter archive (`<NativeLibrary>` items) and exposes
/// the boundary header directory as `$(DotnetPalInclude)`.
pub fn write_nativeaot_props(path: impl AsRef<Path>, port_library: &Path, artifacts: &Artifacts, linker_args: &[&str]) -> io::Result<()> {
    let mut text = String::from("<Project>\n  <PropertyGroup>\n");
    text.push_str(&format!("    <DotnetPalInclude>{}</DotnetPalInclude>\n", include_dir().display()));
    text.push_str("  </PropertyGroup>\n  <ItemGroup>\n");
    text.push_str(&format!("    <NativeLibrary Include=\"{}\" />\n", artifacts.archive.display()));
    text.push_str(&format!("    <NativeLibrary Include=\"{}\" />\n", port_library.display()));
    for arg in artifacts.linker_args.iter().map(String::as_str).chain(linker_args.iter().copied()) {
        text.push_str(&format!("    <LinkerArg Include=\"{arg}\" />\n"));
    }
    text.push_str("  </ItemGroup>\n</Project>\n");
    fs::write(path, text)
}
