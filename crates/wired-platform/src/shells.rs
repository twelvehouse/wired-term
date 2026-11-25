//! Finding the installed shells, as built-in profiles.
//!
//! Every candidate is resolved to an absolute path from known locations, the registry or
//! absolute PATH entries, never from the current directory, so a `pwsh.exe` or `cmd.exe` dropped
//! there cannot be started. Only files and the registry are looked at (no process is started),
//! so detection is fast enough to run before the first window.

use std::path::{Path, PathBuf};

use wired_config::{Profile, ProfileIcon, ProfileId};

/// The installed shells, in menu order. Never empty on Windows: the command prompt is always
/// there.
pub fn detect() -> Vec<Profile> {
    #[cfg(windows)]
    {
        let no_logo = || vec!["-NoLogo".to_owned()];
        let cmd = windows::cmd();
        let mut out = Vec::new();
        if let Some(program) = windows::pwsh() {
            out.push(builtin("pwsh", "PowerShell", ProfileIcon::PowerShellCore, program, no_logo()));
        }
        if let Some(program) = windows::windows_powershell() {
            out.push(builtin("windows-powershell", "Windows PowerShell", ProfileIcon::PowerShell, program, no_logo()));
        }
        out.push(builtin("cmd", "Command Prompt", ProfileIcon::Cmd, cmd.clone(), Vec::new()));
        if let Some(clink) = windows::clink() {
            let mut profile = builtin("cmd-clink", "Command Prompt (Clink)", ProfileIcon::Clink, cmd, clink_args());
            // Tells Clink the terminal handles escape sequences itself, so it does not emulate them.
            profile.env.push(("WT_SESSION".into(), "0".into()));
            profile.env.push(("WIRED_CLINK".into(), format!("\"{}\"", clink.display())));
            out.push(profile);
        }
        if let Some(bash) = windows::git_bash() {
            out.push(builtin("git-bash", "Git Bash", ProfileIcon::GitBash, bash, vec!["--login".into(), "-i".into()]));
        }
        if let Some(wsl) = windows::wsl() {
            let (default, distros) = windows::wsl_distros();
            let distros: Vec<String> = distros.into_iter().filter(|name| is_shell_distro(name)).collect();
            if let Some(default) = default.filter(|name| distros.contains(name)) {
                let args = vec!["--cd".into(), "~".into()];
                out.push(builtin("wsl", "WSL", wsl_icon(&default), wsl.clone(), args));
            }
            out.extend(distros.iter().map(|name| wsl_profile(&wsl, name)));
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![builtin("sh", "sh", ProfileIcon::Linux, PathBuf::from("/bin/sh"), Vec::new())]
    }
}

/// The profile that starts WSL distribution `name` with `wsl` in the user's Linux home.
fn wsl_profile(wsl: &Path, name: &str) -> Profile {
    let args = vec!["-d".into(), name.into(), "--cd".into(), "~".into()];
    builtin(
        &format!("wsl-{}", name.to_ascii_lowercase()),
        &format!("WSL / {name}"),
        wsl_icon(name),
        wsl.to_path_buf(),
        args,
    )
}

/// Icon of WSL distribution `name`: Tabby's table of the names the Store distributions register
/// under, else a generic Linux icon.
fn wsl_icon(name: &str) -> ProfileIcon {
    match name {
        "Alpine" => ProfileIcon::Alpine,
        "Debian" => ProfileIcon::Debian,
        "kali-linux" => ProfileIcon::Kali,
        "SLES-12" | "openSUSE-Leap-15-1" => ProfileIcon::Suse,
        "Ubuntu" | "Ubuntu-16.04" | "Ubuntu-18.04" | "Ubuntu-22.04" => ProfileIcon::Ubuntu,
        "AlmaLinux-8" => ProfileIcon::Alma,
        "OracleLinux_7_9" | "OracleLinux_8_5" => ProfileIcon::OracleLinux,
        "openEuler" => ProfileIcon::OpenEuler,
        _ => ProfileIcon::Linux,
    }
}

fn builtin(slug: &str, name: &str, icon: ProfileIcon, program: PathBuf, args: Vec<String>) -> Profile {
    Profile {
        id: ProfileId::new(format!("builtin:{slug}")),
        name: name.into(),
        icon: Some(icon),
        program,
        args,
        env: Vec::new(),
    }
}

/// Whether a registered WSL distribution is one to offer: a name WSL itself allows, and not one
/// of Docker Desktop's internal ones.
fn is_shell_distro(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !name.to_ascii_lowercase().starts_with("docker-desktop")
}

/// Expanding the quoted path in cmd avoids CRT argument quoting, which does not protect `&`.
fn clink_args() -> Vec<String> {
    vec!["/v:off".into(), "/k".into(), "call".into(), "%WIRED_CLINK%".into(), "inject".into()]
}

/// Whether cmd can be given `path` inside quotes as is: `%` would be expanded and `"` would
/// end the quoting; `call` reparses and strips carets even inside quotes.
fn is_cmd_safe(path: &Path) -> bool {
    path.to_str().is_some_and(|s| !s.contains(['%', '"', '^']))
}

/// Whether `path` names an existing non-directory entry. App execution aliases (Store installs
/// of PowerShell in `WindowsApps`) are reparse points that cannot be followed, so the entry
/// itself is checked.
fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|m| !m.is_dir())
}

/// `name` in the absolute directories of `PATH`, first match.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)).find(|p| exists(p))
}

/// Picks the newest stable install directory name (`7`, `7.4`), falling back to previews.
fn newest_version<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let key = |name: &str| {
        let is_stable = !name.contains('-');
        let numbers: Vec<u32> = name.split(['-', '.']).map_while(|part| part.parse().ok()).collect();
        (is_stable, numbers)
    };
    names.filter(|name| name.starts_with(|c: char| c.is_ascii_digit())).max_by_key(|name| key(name))
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
        RegOpenKeyExW,
    };

    use super::{exists, find_on_path, is_cmd_safe, newest_version};

    const LXSS: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn env_dir(name: &str) -> Option<PathBuf> {
        std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute())
    }

    fn system_root() -> PathBuf {
        env_dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    /// A string value of the registry (`""` = the key's default value), `None` if missing or
    /// not a string.
    fn registry_string(root: HKEY, key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut buf = vec![0u16; 1024];
        let mut len = u32::try_from(buf.len() * 2).ok()?;
        // SAFETY: key and value are NUL-terminated UTF-16; buf is writable for `len` bytes, and
        // the call writes at most that many and stores the written size in `len`.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &raw mut len,
            )
        };
        if status != 0 {
            return None;
        }
        let units = (len as usize / 2).min(buf.len());
        let text = String::from_utf16(&buf[..units]).ok()?;
        Some(text.trim_end_matches('\0').to_owned())
    }

    /// Names of the subkeys of `key`, empty if it cannot be opened.
    fn subkeys(root: HKEY, key: &str) -> Vec<String> {
        let key = wide(key);
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: key is NUL-terminated UTF-16 and handle is a valid out pointer.
        if unsafe { RegOpenKeyExW(root, key.as_ptr(), 0, KEY_READ, &raw mut handle) } != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        // Registry key names are at most 255 characters.
        let mut name = [0u16; 256];
        for index in 0..1024 {
            let mut len = name.len() as u32;
            // SAFETY: handle is open; name is writable for `len` characters, and the call stores
            // the written length (without the NUL) in `len`. The other pointers may be null.
            let status = unsafe {
                RegEnumKeyExW(
                    handle,
                    index,
                    name.as_mut_ptr(),
                    &raw mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status != 0 {
                break;
            }
            out.extend(String::from_utf16(&name[..(len as usize).min(name.len())]).ok());
        }
        // SAFETY: handle was opened above and is closed once.
        unsafe { RegCloseKey(handle) };
        out
    }

    /// An absolute path to an existing file from the registry.
    fn registry_file(root: HKEY, key: &str, value: &str) -> Option<PathBuf> {
        let path = PathBuf::from(registry_string(root, key, value)?.trim_matches('"'));
        (path.is_absolute() && exists(&path)).then_some(path)
    }

    /// PowerShell 7+: the installer's App Paths entry, `%ProgramFiles%\PowerShell\<version>`
    /// (newest), then PATH (which covers other install locations and the Store alias).
    pub(super) fn pwsh() -> Option<PathBuf> {
        let app_path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\pwsh.exe";
        let registered =
            [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER].into_iter().find_map(|root| registry_file(root, app_path, ""));
        let installed = || {
            let root = env_dir("ProgramFiles")?.join("PowerShell");
            let names: Vec<String> = std::fs::read_dir(&root)
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| exists(&entry.path().join("pwsh.exe")))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            newest_version(names.iter().map(String::as_str)).map(|name| root.join(name).join("pwsh.exe"))
        };
        registered.or_else(installed).or_else(|| find_on_path("pwsh.exe"))
    }

    pub(super) fn windows_powershell() -> Option<PathBuf> {
        let path = system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        exists(&path).then_some(path)
    }

    /// `%ComSpec%` if it is an absolute path to an existing file, else System32's `cmd.exe`.
    pub(super) fn cmd() -> PathBuf {
        env_dir("ComSpec").filter(|p| exists(p)).unwrap_or_else(|| system_root().join(r"System32\cmd.exe"))
    }

    /// `clink.bat` of a Clink install: the installer's and Scoop's folders, then PATH.
    pub(super) fn clink() -> Option<PathBuf> {
        let under = |var: &str, rest: &str| env_dir(var).map(|dir| dir.join(rest).join("clink.bat"));
        [
            under("ProgramFiles(x86)", "clink"),
            under("ProgramFiles", "clink"),
            under("LOCALAPPDATA", r"Programs\clink"),
            under("USERPROFILE", r"scoop\apps\clink\current"),
        ]
        .into_iter()
        .flatten()
        .chain(find_on_path("clink.bat"))
        .find(|bat| exists(bat) && is_cmd_safe(bat))
    }

    /// `bin\bash.exe` of Git for Windows (not `git-bash.exe`, which opens its own window).
    pub(super) fn git_bash() -> Option<PathBuf> {
        let registered = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
            .into_iter()
            .filter_map(|root| registry_string(root, r"SOFTWARE\GitForWindows", "InstallPath"))
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        registered
            .chain(env_dir("ProgramFiles").map(|dir| dir.join("Git")))
            .map(|dir| dir.join(r"bin\bash.exe"))
            .find(|bash| exists(bash))
    }

    pub(super) fn wsl() -> Option<PathBuf> {
        let path = system_root().join(r"System32\wsl.exe");
        exists(&path).then_some(path)
    }

    /// The default WSL distribution and all registered ones, by name, as WSL records them for
    /// the current user.
    pub(super) fn wsl_distros() -> (Option<String>, Vec<String>) {
        let name_of = |guid: &str| {
            let key = format!(r"{LXSS}\{guid}");
            registry_string(HKEY_CURRENT_USER, &key, "BasePath")?;
            registry_string(HKEY_CURRENT_USER, &key, "DistributionName")
        };
        let default = registry_string(HKEY_CURRENT_USER, LXSS, "DefaultDistribution").and_then(|guid| name_of(&guid));
        let distros = subkeys(HKEY_CURRENT_USER, LXSS).iter().filter_map(|guid| name_of(guid)).collect();
        (default, distros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_newest_stable_version() {
        assert_eq!(newest_version(["7", "7.5-preview", "6"].into_iter()), Some("7"));
        assert_eq!(newest_version(["7.4", "7.10", "x"].into_iter()), Some("7.10"));
        assert_eq!(newest_version(["7-preview"].into_iter()), Some("7-preview"));
        assert_eq!(newest_version(["tools"].into_iter()), None);
    }

    #[test]
    fn detected_shells_are_absolute_and_exist() {
        let profiles = detect();
        assert!(!profiles.is_empty());
        for profile in &profiles {
            assert!(profile.program.is_absolute(), "{profile:?}");
            #[cfg(windows)]
            assert!(exists(&profile.program), "{profile:?}");
        }
    }

    #[test]
    fn offers_real_distros_only() {
        assert!(is_shell_distro("Ubuntu-22.04"));
        assert!(!is_shell_distro("docker-desktop-data"));
        assert!(!is_shell_distro("my distro"));
        assert!(!is_shell_distro(""));
    }

    #[test]
    fn wsl_profile_id_preserves_punctuation() {
        let profile = wsl_profile(Path::new(r"C:\Windows\System32\wsl.exe"), "Ubuntu-22.04");
        assert_eq!(profile.id.as_str(), "builtin:wsl-ubuntu-22.04");
        assert_eq!(profile.args, ["-d", "Ubuntu-22.04", "--cd", "~"]);
        assert_eq!(profile.icon, Some(ProfileIcon::Ubuntu));
        assert_eq!(wsl_icon("Ubuntu-24.04"), ProfileIcon::Linux);
    }

    #[test]
    fn clink_paths_reject_expansion_and_quotes() {
        let bat = Path::new(r"C:\Program Files (x86)\clink\clink.bat");
        assert!(is_cmd_safe(bat));
        assert!(!is_cmd_safe(Path::new(r"C:\100%\clink.bat")));
        assert!(!is_cmd_safe(Path::new(r"C:\Clink^Test\clink.bat")));
    }

    #[test]
    fn distinct_distro_names_select_the_requested_default() {
        let wsl = Path::new(r"C:\Windows\System32\wsl.exe");
        let profiles: Vec<_> =
            ["Ubuntu-22.04", "Ubuntu-22-04", "Ubuntu_22_04"].into_iter().map(|name| wsl_profile(wsl, name)).collect();
        for expected in &profiles {
            let (actual, message) = wired_config::default_profile(&profiles, Some(&expected.id)).expect("profile");
            assert_eq!(actual.args, expected.args);
            assert!(message.is_none());
        }
    }

    #[cfg(windows)]
    #[test]
    fn clink_loads_from_paths_with_shell_metacharacters() {
        use std::os::windows::process::CommandExt;
        let root = std::env::temp_dir().join(format!("wired-clink-test-{}", std::process::id()));
        for name in ["Clink&Test", "Program Files (x86)", "Clink & Test", "Clink!Test"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).expect("fixture directory");
            let bat = dir.join("clink.bat");
            std::fs::write(&bat, "@echo WIRED_CLINK_LOADED\r\n").expect("fixture batch");
            let mut args = clink_args();
            args[1] = "/c".into();
            // These fixed arguments contain no CRT quoting characters, as in portable-pty.
            let output = std::process::Command::new(windows::cmd())
                .raw_arg("/d")
                .raw_arg(args.join(" "))
                .env("WIRED_CLINK", format!("\"{}\"", bat.display()))
                .creation_flags(0x0800_0000)
                .output()
                .expect("cmd starts");
            assert!(output.status.success(), "{name}: {:?}", output.status);
            assert!(String::from_utf8_lossy(&output.stdout).contains("WIRED_CLINK_LOADED"), "{name}");
            std::fs::remove_file(&bat).expect("remove fixture batch");
            std::fs::remove_dir(&dir).expect("remove fixture directory");
        }
        std::fs::remove_dir(root).expect("remove fixture root");
    }
}
