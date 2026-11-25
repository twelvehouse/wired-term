//! Profiles: the programs a new tab can run, and which one is the default.

use std::{borrow::Borrow, path::PathBuf};

use serde::Deserialize;

/// Stable id of a profile: `builtin:<slug>` for detected shells. Settings and key bindings
/// refer to profiles by it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    /// Wraps an id string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as written in the settings.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(&self.0)
    }
}

/// Icon of a built-in profile: the set Tabby ships for its shells and WSL distributions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileIcon {
    /// Windows PowerShell 5.1.
    PowerShell,
    /// PowerShell 7+.
    PowerShellCore,
    /// Command Prompt.
    Cmd,
    /// Command Prompt with Clink.
    Clink,
    /// Git Bash.
    GitBash,
    /// A WSL distribution without its own icon.
    Linux,
    /// AlmaLinux.
    Alma,
    /// Alpine.
    Alpine,
    /// Debian.
    Debian,
    /// Kali Linux.
    Kali,
    /// SUSE / openSUSE.
    Suse,
    /// Ubuntu.
    Ubuntu,
    /// Oracle Linux.
    OracleLinux,
    /// openEuler.
    OpenEuler,
}

/// A program a tab or pane can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Stable id.
    pub id: ProfileId,
    /// Name shown in the profile menu and as the default tab title.
    pub name: String,
    /// Shown before the name in the profile menu and on tabs; `None` = a generic terminal icon.
    pub icon: Option<ProfileIcon>,
    /// Absolute path of the executable.
    pub program: PathBuf,
    /// Arguments, passed as-is.
    pub args: Vec<String>,
    /// Environment variables set on top of the inherited ones.
    pub env: Vec<(String, String)>,
}

/// Built-in ids tried in this order when no default is configured or the configured one is not
/// available.
pub const DEFAULT_ORDER: [&str; 3] = ["builtin:pwsh", "builtin:windows-powershell", "builtin:cmd"];

/// The profile new tabs use: `configured` if it is among `profiles`, else the first of
/// [`DEFAULT_ORDER`] that is, else the first profile. The message says why a configured
/// default was not used. `None` only when `profiles` is empty.
pub fn default_profile<'a, P: Borrow<Profile>>(
    profiles: &'a [P],
    configured: Option<&ProfileId>,
) -> Option<(&'a P, Option<String>)> {
    let find = |id: &str| profiles.iter().find(|p| Borrow::<Profile>::borrow(*p).id.as_str() == id);
    if let Some(profile) = configured.and_then(|id| find(id.as_str())) {
        return Some((profile, None));
    }
    let fallback = DEFAULT_ORDER.iter().find_map(|id| find(id)).or_else(|| profiles.first())?;
    let message = configured
        .map(|id| format!("profile \"{id}\" was not found, using {}", Borrow::<Profile>::borrow(fallback).name));
    Some((fallback, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> Profile {
        Profile {
            id: ProfileId::new(id),
            name: id.into(),
            icon: None,
            program: PathBuf::from(r"C:\x.exe"),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    #[test]
    fn configured_default_wins_when_present() {
        let profiles = [profile("builtin:pwsh"), profile("builtin:git-bash")];
        let (chosen, message) =
            default_profile(&profiles, Some(&ProfileId::new("builtin:git-bash"))).expect("profiles");
        assert_eq!(chosen.id.as_str(), "builtin:git-bash");
        assert!(message.is_none());
    }

    #[test]
    fn falls_back_in_order_and_says_why() {
        let profiles = [profile("builtin:cmd"), profile("builtin:windows-powershell")];
        let (chosen, message) = default_profile(&profiles, Some(&ProfileId::new("builtin:nope"))).expect("profiles");
        assert_eq!(chosen.id.as_str(), "builtin:windows-powershell");
        assert!(message.is_some_and(|m| m.contains("builtin:nope")));
        let (chosen, message) = default_profile(&profiles, None).expect("profiles");
        assert_eq!(chosen.id.as_str(), "builtin:windows-powershell");
        assert!(message.is_none());
    }

    #[test]
    fn no_profiles_no_default() {
        assert!(default_profile::<Profile>(&[], None).is_none());
    }
}
