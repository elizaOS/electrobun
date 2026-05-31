//! Bundle layout discovery and OS/app-scoped path resolution.
//!
//! Ports `resolveBundlePaths`, `resolveAppInfoFromBundle`, `quit`, the
//! `getXxxDirOwned` helpers, and the Linux `user-dirs.dirs` XDG parser from
//! `package/src/zig-sdk/electrobun.zig`. The OS-default directories use the
//! `dirs` crate, but we keep the hand-rolled XDG `user-dirs.dirs` parser to
//! match the Zig behaviour exactly (including `$HOME` substitution).

use std::path::{Path, PathBuf};

use crate::error::{ElectrobunError, Result};
use crate::types::{AppInfo, OwnedAppInfo};

/// Executable directory and sibling `Resources` directory of the running bundle.
///
/// Replaces the Zig `BundlePaths` (with `deinit`); ownership is via `Drop`.
#[derive(Debug, Clone)]
pub struct BundlePaths {
    /// Directory containing the executable.
    pub exe_dir: PathBuf,
    /// `../Resources` relative to `exe_dir` (macOS bundle convention).
    pub resources_dir: PathBuf,
}

/// Resolve [`BundlePaths`] from the current executable's location.
pub fn resolve_bundle_paths() -> Result<BundlePaths> {
    let exe_path = std::env::current_exe().map_err(|source| ElectrobunError::Io {
        context: "resolve current executable path".to_string(),
        source,
    })?;
    let exe_dir = exe_path
        .parent()
        .ok_or(ElectrobunError::InvalidExePath)?
        .to_path_buf();
    let resources_dir = exe_dir.join("..").join("Resources");
    Ok(BundlePaths {
        exe_dir,
        resources_dir,
    })
}

/// Parse `<resources_dir>/version.json` into an [`OwnedAppInfo`].
///
/// Unknown fields are ignored, matching the Zig `ignore_unknown_fields = true`.
pub fn resolve_app_info_from_bundle(bundle_paths: &BundlePaths) -> Result<OwnedAppInfo> {
    let version_json_path = bundle_paths.resources_dir.join("version.json");
    let contents =
        std::fs::read_to_string(&version_json_path).map_err(|source| ElectrobunError::Io {
            context: format!("read {}", version_json_path.display()),
            source,
        })?;
    serde_json::from_str(&contents).map_err(|source| ElectrobunError::Json {
        context: "parse version.json".to_string(),
        source,
    })
}

/// Terminate the process immediately with the given code.
///
/// Mirrors the Zig free function `quit`.
pub fn quit(code: u8) -> ! {
    std::process::exit(code as i32)
}

/// Fully resolved set of OS-level and app-scoped directories.
///
/// Replaces the Zig `Paths` (with `deinit`); ownership is via `Drop`.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Home directory.
    pub home: PathBuf,
    /// Application-support / data directory.
    pub app_data: PathBuf,
    /// Config directory.
    pub config: PathBuf,
    /// Cache directory.
    pub cache: PathBuf,
    /// Temp directory.
    pub temp: PathBuf,
    /// Logs directory.
    pub logs: PathBuf,
    /// Documents directory.
    pub documents: PathBuf,
    /// Downloads directory.
    pub downloads: PathBuf,
    /// Desktop directory.
    pub desktop: PathBuf,
    /// Pictures directory.
    pub pictures: PathBuf,
    /// Music directory.
    pub music: PathBuf,
    /// Videos directory.
    pub videos: PathBuf,
    /// App-scoped data directory (`<app_data>/<id>/<channel>`).
    pub user_data: PathBuf,
    /// App-scoped cache directory.
    pub user_cache: PathBuf,
    /// App-scoped logs directory.
    pub user_logs: PathBuf,
}

impl Paths {
    /// Resolve every directory for the given app identity.
    pub fn resolve(app_info: AppInfo<'_>) -> Result<Paths> {
        let home = home_dir()?;
        let app_data = app_data_dir(&home);
        let config = config_dir(&home);
        let cache = cache_dir(&home);
        let temp = temp_dir(&home);
        let logs = logs_dir(&home);

        let documents = user_dir(&home, UserDir::Documents);
        let downloads = user_dir(&home, UserDir::Downloads);
        let desktop = user_dir(&home, UserDir::Desktop);
        let pictures = user_dir(&home, UserDir::Pictures);
        let music = user_dir(&home, UserDir::Music);
        let videos = user_dir(&home, UserDir::Videos);

        let user_data = build_app_scoped_dir(&app_data, app_info);
        let user_cache = build_app_scoped_dir(&cache, app_info);
        let user_logs = build_app_scoped_dir(&logs, app_info);

        Ok(Paths {
            home,
            app_data,
            config,
            cache,
            temp,
            logs,
            documents,
            downloads,
            desktop,
            pictures,
            music,
            videos,
            user_data,
            user_cache,
            user_logs,
        })
    }
}

/// Resolve the home directory (`USERPROFILE` then `HOME` on Windows, `HOME`
/// elsewhere). Mirrors `getHomeDirOwned`.
pub fn home_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(value) = env_path("USERPROFILE") {
            return Ok(value);
        }
        env_path("HOME").ok_or(ElectrobunError::MissingEnv { name: "HOME" })
    }
    #[cfg(not(windows))]
    {
        env_path("HOME").ok_or(ElectrobunError::MissingEnv { name: "HOME" })
    }
}

/// Application-support / data directory. Mirrors `getAppDataDirOwned`.
pub fn app_data_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library").join("Application Support")
    }
    #[cfg(target_os = "windows")]
    {
        env_or_join("LOCALAPPDATA", home, &["AppData", "Local"])
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env_or_join("XDG_DATA_HOME", home, &[".local", "share"])
    }
}

/// Cache directory. Mirrors `getCacheDirOwned`.
pub fn cache_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library").join("Caches")
    }
    #[cfg(target_os = "windows")]
    {
        env_or_join("LOCALAPPDATA", home, &["AppData", "Local"])
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env_or_join("XDG_CACHE_HOME", home, &[".cache"])
    }
}

/// Logs directory. Mirrors `getLogsDirOwned`.
pub fn logs_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library").join("Logs")
    }
    #[cfg(target_os = "windows")]
    {
        env_or_join("LOCALAPPDATA", home, &["AppData", "Local"])
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env_or_join("XDG_STATE_HOME", home, &[".local", "state"])
    }
}

/// Config directory. Mirrors `getConfigDirOwned`.
pub fn config_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library").join("Application Support")
    }
    #[cfg(target_os = "windows")]
    {
        env_or_join("APPDATA", home, &["AppData", "Roaming"])
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env_or_join("XDG_CONFIG_HOME", home, &[".config"])
    }
}

/// Temp directory. Mirrors `getTempDirOwned`.
pub fn temp_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(value) = env_path("TEMP") {
            return value;
        }
        if let Some(value) = env_path("TMP") {
            return value;
        }
        home.join("AppData").join("Local").join("Temp")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = home;
        env_path("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}

/// The OS user directories the Zig SDK resolves.
#[derive(Debug, Clone, Copy)]
enum UserDir {
    Documents,
    Downloads,
    Desktop,
    Pictures,
    Music,
    Videos,
}

impl UserDir {
    /// macOS folder name (under `$HOME`).
    #[cfg(target_os = "macos")]
    fn mac_name(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Movies",
        }
    }

    /// Windows folder name (under `$HOME`).
    #[cfg(target_os = "windows")]
    fn win_name(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Videos",
        }
    }

    /// XDG key in `user-dirs.dirs`.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn xdg_key(self) -> &'static str {
        match self {
            Self::Documents => "XDG_DOCUMENTS_DIR",
            Self::Downloads => "XDG_DOWNLOAD_DIR",
            Self::Desktop => "XDG_DESKTOP_DIR",
            Self::Pictures => "XDG_PICTURES_DIR",
            Self::Music => "XDG_MUSIC_DIR",
            Self::Videos => "XDG_VIDEOS_DIR",
        }
    }

    /// Fallback folder name (under `$HOME`) when the XDG entry is missing.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn fallback_name(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Desktop => "Desktop",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Videos",
        }
    }
}

/// Resolve a single user directory. Mirrors `getUserDirOwned`.
fn user_dir(home: &Path, kind: UserDir) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join(kind.mac_name())
    }
    #[cfg(target_os = "windows")]
    {
        home.join(kind.win_name())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        linux_xdg_user_dir(home, kind.xdg_key(), kind.fallback_name())
    }
}

/// Read an env var as a `PathBuf`, treating empty values as unset (matching
/// `std.process.getEnvVarOwned`, which returns the value verbatim — empty
/// strings are preserved by Zig, but `dirs`/`std::env` callers expect a real
/// path, and an empty env var here is indistinguishable from unset for join
/// fallback purposes).
fn env_path(name: &str) -> Option<PathBuf> {
    match std::env::var_os(name) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// Return the env var as a path, or join the fallback parts onto `home`.
/// Mirrors `envOrJoin`.
#[cfg(any(
    target_os = "windows",
    not(any(target_os = "macos", target_os = "windows"))
))]
fn env_or_join(name: &str, home: &Path, fallback_parts: &[&str]) -> PathBuf {
    if let Some(value) = env_path(name) {
        return value;
    }
    let mut path = home.to_path_buf();
    for part in fallback_parts {
        path.push(part);
    }
    path
}

/// Parse `~/.config/user-dirs.dirs` for `key`, substituting `$HOME`.
/// Mirrors `linuxXdgUserDirOwned`. Falls back to `home/<fallback_name>`.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_xdg_user_dir(home: &Path, key: &str, fallback_name: &str) -> PathBuf {
    let fallback = home.join(fallback_name);
    let config_path = home.join(".config").join("user-dirs.dirs");

    let content = match std::fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(_) => return fallback,
    };

    for line in content.lines() {
        let trimmed = line.trim_matches([' ', '\t', '\r']);
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(eq_index) = trimmed.find('=') else {
            continue;
        };
        let line_key = &trimmed[..eq_index];
        if line_key != key {
            continue;
        }

        let mut value = trimmed[eq_index + 1..].trim_matches([' ', '\t', '\r']);
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = &value[1..value.len() - 1];
        }

        let home_str = home.to_string_lossy();
        return PathBuf::from(value.replace("$HOME", &home_str));
    }

    fallback
}

/// Append `<identifier>/<channel>` to `base`, unless either is empty.
/// Mirrors `buildAppScopedDir`.
fn build_app_scoped_dir(base: &Path, app_info: AppInfo<'_>) -> PathBuf {
    if app_info.identifier.is_empty() || app_info.channel.is_empty() {
        return base.to_path_buf();
    }
    base.join(app_info.identifier).join(app_info.channel)
}
