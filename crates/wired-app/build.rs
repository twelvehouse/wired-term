//! Places files wired ships next to `wired.exe`:
//!
//! - A newer ConPTY (`conpty.dll` + `OpenConsole.exe`). The inbox ConPTY of Windows is much slower
//!   on scroll-heavy output, so wired uses Microsoft's release package
//!   `Microsoft.Windows.Console.ConPTY` (MIT, signed by Microsoft). Without it wired falls back to
//!   the system ConPTY.
//! - The color themes in `themes\`: the iTerm2-Color-Schemes collection (MIT) in Ghostty's theme
//!   format, the release Ghostty itself pins, with its license as `themes\LICENSE.txt`.
//!
//! Environment:
//! - `WIRED_CONPTY_PACKAGE`: path of an already downloaded `.nupkg` (still verified).
//! - `WIRED_CONPTY=system`: skip it and use the system ConPTY.
//! - `WIRED_THEMES=skip`: ship no themes.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

const CONPTY_VERSION: &str = "1.25.260930003";
const CONPTY_SHA256: &str = "02b07b349af66d801159bdf9e440d4a1ce78bb951f37fc8609731665afdae7ee";

const THEMES_RELEASE: &str = "ghostty-themes-release-20261102-150615-3d8a6a1";
const THEMES_SHA256: &str = "b61b297e4245e0a5e46ac1e8d92ec8f7390d4394db4bafc93cfa3209f215435e";
const THEMES_LICENSE: &str = "iterm2-color-schemes-LICENSE.txt";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={THEMES_LICENSE}");
    println!("cargo:rerun-if-env-changed=WIRED_CONPTY_PACKAGE");
    println!("cargo:rerun-if-env-changed=WIRED_CONPTY");
    println!("cargo:rerun-if-env-changed=WIRED_THEMES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    println!("cargo:rerun-if-changed=assets/icons");
    embed_resource::compile("assets/icons/wired.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed wired app icons");
    // OUT_DIR is <target>/<profile>/build/<crate>-<hash>/out, or .../build/<crate>/<hash>/out since
    // Rust 1.100; the exe goes to <target>/<profile>.
    let build_dir = out_dir.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == "build"));
    let Some(exe_dir) = build_dir.and_then(Path::parent) else { return };
    let target_dir = exe_dir.parent().unwrap_or(exe_dir);

    if env::var("WIRED_CONPTY").as_deref() != Ok("system") {
        install_conpty(exe_dir, &target_dir.join("conpty-cache"));
    }
    if env::var("WIRED_THEMES").as_deref() != Ok("skip") {
        install_themes(exe_dir, &target_dir.join("themes-cache"));
    }
}

fn install_conpty(exe_dir: &Path, cache: &Path) {
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("aarch64") => "arm64",
        _ => return,
    };
    let package = match env::var_os("WIRED_CONPTY_PACKAGE") {
        Some(path) => PathBuf::from(path),
        None => {
            let url = format!(
                "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/{CONPTY_VERSION}/microsoft.windows.console.conpty.{CONPTY_VERSION}.nupkg"
            );
            let file = format!("microsoft.windows.console.conpty.{CONPTY_VERSION}.nupkg");
            match download(&url, &cache.join(file)) {
                Ok(path) => path,
                Err(error) => {
                    println!("cargo:warning=ConPTY package not available ({error}); the system ConPTY will be used");
                    return;
                }
            }
        }
    };
    verify(&package, CONPTY_SHA256, "ConPTY package");

    let extract_dir = cache.join(format!("{CONPTY_VERSION}-{arch}"));
    let dll = format!("runtimes/win-{arch}/native/conpty.dll");
    let host = format!("build/native/runtimes/{arch}/OpenConsole.exe");
    fs::create_dir_all(&extract_dir).expect("create ConPTY cache directory");
    run(Command::new(system32("tar.exe")).arg("-xf").arg(&package).arg("-C").arg(&extract_dir).args([&dll, &host]));

    for (from, name) in [(dll, "conpty.dll"), (host, "OpenConsole.exe")] {
        copy_if_changed(&extract_dir.join(from), &exe_dir.join(name));
    }
}

fn install_themes(exe_dir: &Path, cache: &Path) {
    let url = format!("https://deps.files.ghostty.org/{THEMES_RELEASE}.tgz");
    let archive = match download(&url, &cache.join(format!("{THEMES_RELEASE}.tgz"))) {
        Ok(path) => path,
        Err(error) => {
            println!("cargo:warning=color themes not available ({error}); wired is built without them");
            return;
        }
    };
    verify(&archive, THEMES_SHA256, "color theme archive");

    let extract_dir = cache.join(THEMES_RELEASE);
    fs::create_dir_all(&extract_dir).expect("create theme cache directory");
    // tar.exe writes ':' in file names (not allowed on Windows) as '_'; wired looks them up so.
    run(Command::new(system32("tar.exe")).arg("-xzf").arg(&archive).arg("-C").arg(&extract_dir).arg("ghostty"));

    let themes = exe_dir.join("themes");
    fs::create_dir_all(&themes).expect("create themes directory");
    let entries = fs::read_dir(extract_dir.join("ghostty")).expect("read extracted themes");
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            copy_if_changed(&entry.path(), &themes.join(entry.file_name()));
        }
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    copy_if_changed(&manifest_dir.join(THEMES_LICENSE), &themes.join("LICENSE.txt"));
}

fn download(url: &str, path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let partial = path.with_extension("part");
    let status = Command::new(system32("curl.exe"))
        .args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--output"])
        .arg(&partial)
        .arg(url)
        .status()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("download of {url} failed ({status})"));
    }
    fs::rename(&partial, path).map_err(|e| e.to_string())?;
    Ok(path.to_path_buf())
}

fn verify(path: &Path, expected: &str, what: &str) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let digest = hex(&Sha256::digest(&bytes));
    assert!(
        digest == expected,
        "{what} {} has SHA-256 {digest}, expected {expected}; refusing to use it",
        path.display()
    );
}

fn copy_if_changed(from: &Path, to: &Path) {
    // A running wired.exe keeps the old files locked; identical files need no copy.
    if fs::read(to).is_ok_and(|old| fs::read(from).is_ok_and(|new| old == new)) {
        return;
    }
    if let Err(error) = fs::copy(from, to) {
        println!("cargo:warning=cannot copy {} to {} ({error})", from.display(), to.display());
    }
}

/// Tools from System32 by absolute path, so nothing on PATH can stand in for them.
fn system32(tool: &str) -> PathBuf {
    let root = env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(tool)
}

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"));
    assert!(status.success(), "{command:?} failed ({status})");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
