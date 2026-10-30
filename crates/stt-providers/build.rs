//! Build script for medical-stt-providers.
//!
//! macOS: supply the toolchain's compiler-rt availability archive so native
//! objects linked through this crate (whisper.cpp's ggml-metal-device.m and
//! ORT's CoreML host_utils.mm/model.mm, which use `@available` checks) can
//! resolve `___isPlatformVersionAtLeast`.
//!
//! Why this is needed (evidence from the failed v0.82.0 release run
//! 41077937853 and local reproduction on Xcode 27 / macOS 26):
//!
//! 1. Apple clang lowers `@available(...)` into calls to
//!    `___isPlatformVersionAtLeast`, defined only in the toolchain's
//!    `libclang_rt.osx.a` (the clang driver appends it automatically — but
//!    see 3). macOS 26 no longer ships `/usr/lib/libclang_rt.osx.dylib`, the
//!    old implicit fallback, so there is no other source for the symbol.
//! 2. The failing references come from ggml-metal-device.m.o inside the
//!    whisper-rs-sys rlib (compiled on the build machine by the current
//!    clang) and from the prebuilt ORT CoreML objects inside the ort-sys
//!    rlib — both are pulled into every final link of this crate's
//!    dependents.
//! 3. rustc drives the final link through `cc` WITH `-nodefaultlibs`, which
//!    suppresses the clang driver's automatic `libclang_rt.osx.a`.
//! 4. Dev/test-profile links still succeed because current rustc (1.99)
//!    synthesizes a weak `___isPlatformVersionAtLeast` into non-LTO final
//!    links. The workspace `[profile.release]` uses `lto = true` +
//!    `codegen-units = 1`, which drops that shim — so release-profile links
//!    fail with `Undefined symbols for architecture arm64` exactly as in
//!    run 41077937853. (Raising MACOSX_DEPLOYMENT_TARGET does NOT help: the
//!    references live in prebuilt ORT objects regardless.)
//!
//! The fix mirrors the clang driver: pass the archive explicitly. It is
//! static and multi-arch (i386/x86_64/x86_64h/arm64/arm64e), adds no runtime
//! dylib dependency, and does not change MACOSX_DEPLOYMENT_TARGET (still
//! 11.0 — no OS-support drop).
//!
//! Scope: `cargo:rustc-link-lib`/`-search` from this build script propagate
//! through the crate's rlib metadata to every dependent final link — most
//! importantly the `rust-medical-assistant` app binary built by release.yml.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // The whisper/ort objects are macOS-only Metal/CoreML code paths; never
    // emit Apple toolchain paths for other targets (CI also builds Linux).
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("-apple-") {
        return;
    }

    match apple_clang_rt_dir() {
        Some(dir) => {
            let archive = dir.join("libclang_rt.osx.a");
            if archive.exists() {
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            } else {
                // Don't fail the build here: dev-profile links survive via
                // rustc's std shim. If the archive is genuinely required
                // (release profile), the link error follows with this
                // warning as context.
                println!(
                    "cargo:warning=compiler-rt archive {} not found; \
                     macOS release-profile links may fail with undefined \
                     ___isPlatformVersionAtLeast",
                    archive.display()
                );
            }
        }
        None => println!(
            "cargo:warning=could not locate a clang resource dir \
             (clang/cc --print-resource-dir both failed); macOS \
             release-profile links may fail with undefined \
             ___isPlatformVersionAtLeast"
        ),
    }
}

/// Locate `<clang resource dir>/lib/darwin`, preferring `$CC` (set by cargo
/// for cross builds), then `clang`, then `cc`. On dev machines and CI macOS
/// runners one of these always exists — whisper-rs-sys's CMake build already
/// requires a working clang.
fn apple_clang_rt_dir() -> Option<PathBuf> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(cc) = env::var("CC")
        && !cc.trim().is_empty()
    {
        candidates.push(cc);
    }
    candidates.push("clang".to_string());
    candidates.push("cc".to_string());

    for cc in candidates {
        let Ok(output) = Command::new(&cc).arg("--print-resource-dir").output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if dir.is_empty() {
            continue;
        }
        let darwin = PathBuf::from(dir).join("lib").join("darwin");
        if darwin.is_dir() {
            return Some(darwin);
        }
    }
    None
}
