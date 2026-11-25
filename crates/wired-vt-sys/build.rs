//! Environment:
//! - `WIRED_GHOSTTY_DIR`: Ghostty source tree (default: `<workspace>/.submodules/ghostty-wired`).
//! - `ZIG`: path to `zig.exe` (default: `zig` on PATH, then `%LOCALAPPDATA%\Programs\zig\zig-*-0.17.*`).
//! - `WIRED_GHOSTTY_CPU`: Zig CPU model to build for (default below). Never the building
//!   machine's own CPU: Zig would use every instruction it has (AVX-512, for one), and the
//!   program would crash on machines without them.
//! - `LIBCLANG_PATH`: only for the `regenerate` feature.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

const STATIC_LIB: &str = "ghostty-vt-static";
const DEFAULT_CPU: &str = "baseline";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let ghostty_dir = env::var_os("WIRED_GHOSTTY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../.submodules/ghostty-wired"));
    let out_dir = ghostty_dir.join("zig-out");
    let lib_dir = out_dir.join("lib");

    build_ghostty(&ghostty_dir);

    #[cfg(feature = "regenerate")]
    regenerate_bindings(&manifest_dir, &out_dir.join("include"));

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static={STATIC_LIB}");
    println!("cargo:rerun-if-env-changed=WIRED_GHOSTTY_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=WIRED_GHOSTTY_CPU");
    println!("cargo:rerun-if-changed=build.rs");
    for source in ["src", "include", "build.zig", "build.zig.zon"] {
        println!("cargo:rerun-if-changed={}", ghostty_dir.join(source).display());
    }
    println!("cargo:rerun-if-changed={}", lib_dir.join(format!("{STATIC_LIB}.lib")).display());
}

fn build_ghostty(ghostty_dir: &Path) {
    let zig = find_zig();
    let cpu = env::var("WIRED_GHOSTTY_CPU").unwrap_or_else(|_| DEFAULT_CPU.to_owned());
    let status = Command::new(&zig)
        .args(["build", "-Demit-lib-vt", "-Doptimize=ReleaseFast", &format!("-Dcpu={cpu}")])
        .current_dir(ghostty_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}. Install Zig 0.17.x or set ZIG.", zig.display()));
    assert!(status.success(), "zig build of libghostty-vt failed ({status})");
}

fn find_zig() -> PathBuf {
    if let Some(zig) = env::var_os("ZIG") {
        return PathBuf::from(zig);
    }
    let on_path = Command::new("zig").arg("version").output().is_ok_and(|o| o.status.success());
    if on_path {
        return PathBuf::from("zig");
    }
    let programs = env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("zig"));
    let installed = programs
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().contains("-0.17.")))
        .map(|path| path.join("zig.exe"))
        .find(|zig| zig.exists());
    installed.unwrap_or_else(|| PathBuf::from("zig"))
}

#[cfg(feature = "regenerate")]
fn regenerate_bindings(manifest_dir: &Path, include_dir: &Path) {
    let bindings = bindgen::Builder::default()
        .header(include_dir.join("ghostty/vt.h").to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.display()))
        .clang_arg("-std=c11")
        // Static linking: no dllimport on the declarations.
        .clang_arg("-DGHOSTTY_STATIC")
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .generate()
        .expect("bindgen failed");
    bindings.write_to_file(manifest_dir.join("src/bindings.rs")).expect("write src/bindings.rs");
}
