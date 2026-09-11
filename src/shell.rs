use std::env;
use std::path::{Path, PathBuf};

use nix::unistd::{AccessFlags, access};

use crate::Result;

/// An explicit override must fail visibly; only the implicit login shell falls back.
pub(super) fn resolve_shell(configured: Option<&str>) -> Result<String> {
    let override_shell = env::var("RUSTMUX_SHELL").ok();
    let login_shell = env::var("SHELL").ok();
    select_shell(
        override_shell.as_deref(),
        configured,
        login_shell.as_deref(),
        |name| executable(name, env::var_os("PATH").as_deref()),
    )
}

fn select_shell(
    override_shell: Option<&str>,
    configured: Option<&str>,
    login_shell: Option<&str>,
    resolve: impl Fn(&str) -> Option<PathBuf>,
) -> Result<String> {
    if let Some(name) = override_shell.or(configured) {
        return resolve(name)
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| {
                format!("cannot start shell '{name}': executable not found or not executable")
                    .into()
            });
    }
    login_shell
        .filter(|name| !name.is_empty())
        .and_then(&resolve)
        .or_else(|| resolve("/bin/sh"))
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| {
            "cannot find a usable shell; set RUSTMUX_SHELL or shell in config.toml".into()
        })
}

fn executable(name: &str, search_path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let usable = |path: &Path| path.is_file() && access(path, AccessFlags::X_OK).is_ok();
    if name.contains('/') {
        let path = PathBuf::from(name);
        return usable(&path)
            .then(|| std::fs::canonicalize(&path).ok())
            .flatten();
    }
    if name.is_empty() {
        return None;
    }
    env::split_paths(search_path?)
        .map(|directory| directory.join(name))
        .find(|path| usable(path))
        .and_then(|path| std::fs::canonicalize(path).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_shells_take_precedence_and_do_not_silently_fall_back() {
        let resolve = |name: &str| match name {
            "fish" | "zsh" | "/bin/sh" => Some(PathBuf::from(name)),
            _ => None,
        };
        assert_eq!(
            select_shell(Some("fish"), Some("zsh"), None, resolve).unwrap(),
            "fish"
        );
        assert_eq!(
            select_shell(None, Some("zsh"), Some("fish"), resolve).unwrap(),
            "zsh"
        );
        assert!(select_shell(Some("missing"), Some("zsh"), None, resolve).is_err());
        assert_eq!(
            select_shell(None, None, Some("missing"), resolve).unwrap(),
            "/bin/sh"
        );
        assert_eq!(
            select_shell(None, None, Some("fish"), resolve).unwrap(),
            "fish"
        );
    }

    #[test]
    fn resolves_executable_paths_and_rejects_directories() {
        assert!(executable("/bin/sh", None).is_some());
        assert!(executable("/tmp", None).is_none());
        assert!(executable("", Some(std::ffi::OsStr::new("/bin"))).is_none());
        assert!(executable("sh", Some(std::ffi::OsStr::new("/bin"))).is_some());
    }
}
