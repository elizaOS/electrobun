//! Post-extraction fixups for the Windows/Linux install path: executable
//! permissions, CEF symlinks (Linux), and shortcut/registry creation.
//!
//! macOS does not use this module — it replaces the `.app` bundle in place
//! (see `macos.rs`) and never chmods, recreates symlinks, or makes shortcuts.
//!
//! Note: Windows copies (never renames) across directories — the copy helper
//! lives here; the caller drives the relocation.

use std::fs;
use std::io;
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::metadata::AppMetadata;

/// Executables that must be marked `+x` after extraction.
const EXECUTABLES: &[&str] = &[
    "bin/launcher",
    "bin/bun",
    "bin/bspatch",
    "bin/bsdiff",
    "bin/zig-zstd",
];

/// CEF shared libraries that need `lib -> cef/lib` symlinks recreated.
#[cfg(target_os = "linux")]
const CEF_LIBS: &[&str] = &[
    "libcef.so",
    "libEGL.so",
    "libGLESv2.so",
    "libvk_swiftshader.so",
    "libvulkan.so.1",
];

/// Set 0755 on the known executables (Linux). On Windows POSIX modes do not
/// apply, so this is a no-op there.
pub fn fix_executable_permissions(app_dir: &Path) -> io::Result<()> {
    for exe in EXECUTABLES {
        let exe_path = app_dir.join(exe);
        if !exe_path.exists() {
            continue;
        }

        #[cfg(target_os = "linux")]
        set_mode_0755(&exe_path);

        #[cfg(windows)]
        let _ = &exe_path; // permissions are not POSIX-managed on Windows
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn set_mode_0755(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o755)) {
        eprintln!(
            "Warning: could not set executable permissions on {}: {e}",
            path.display()
        );
    }
}

/// Recreate CEF symlinks (`bin/<lib> -> cef/<lib>`) that tar extraction drops.
#[cfg(target_os = "linux")]
pub fn fix_cef_symlinks(app_dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    let bin_dir = app_dir.join("bin");
    let cef_dir = bin_dir.join("cef");
    if !cef_dir.exists() {
        return Ok(());
    }

    for lib in CEF_LIBS {
        let symlink_path = bin_dir.join(lib);
        let target = format!("cef/{lib}");
        let _ = fs::remove_file(&symlink_path);
        if let Err(e) = symlink(&target, &symlink_path) {
            eprintln!("Warning: could not create symlink for {lib}: {e}");
        }
    }
    Ok(())
}

/// Windows ships no CEF `.so` symlinks, so there is nothing to recreate.
#[cfg(windows)]
pub fn fix_cef_symlinks(_app_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Linux: install a `.desktop` shortcut, deriving Exec/Icon from the bundled
/// `.desktop` template and any `.png` icon found in the app root or
/// `Resources/`. Also installs a copy under `~/.local/share/applications` and
/// refreshes the desktop database. All steps are best-effort.
#[cfg(target_os = "linux")]
pub fn create_desktop_shortcut(app_dir: &Path, metadata: &AppMetadata) -> io::Result<()> {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let home = match env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Warning: could not get HOME directory");
            return Ok(());
        }
    };

    let launcher_path = app_dir.join("bin").join("launcher");
    if !launcher_path.exists() {
        eprintln!(
            "Warning: launcher binary not found at {}",
            launcher_path.display()
        );
        return Ok(());
    }

    // Locate the bundled .desktop template.
    let template = fs::read_dir(app_dir)?.flatten().find_map(|e| {
        let p = e.path();
        match p.extension() {
            Some(ext) if ext == "desktop" => Some(p),
            _ => None,
        }
    });
    let Some(template_path) = template else {
        eprintln!("Warning: no desktop file found in extracted app directory");
        return Ok(());
    };

    let content = fs::read_to_string(&template_path)?;
    let icon_path = find_icon(app_dir);

    let rewritten = rewrite_desktop_entry(
        &content,
        &launcher_path.to_string_lossy(),
        icon_path.as_deref(),
    );

    let desktop_filename = format!("{}.desktop", metadata.name);

    // 1. Desktop surface (optional — only if ~/Desktop exists).
    let desktop_dir = Path::new(&home).join("Desktop");
    let mut desktop_shortcut_created = false;
    if desktop_dir.exists() {
        let desktop_file = desktop_dir.join(&desktop_filename);
        match fs::write(&desktop_file, &rewritten) {
            Ok(()) => {
                let _ = fs::set_permissions(&desktop_file, fs::Permissions::from_mode(0o755));
                let _ = Command::new("gio")
                    .args([
                        "set",
                        &desktop_file.to_string_lossy(),
                        "metadata::trusted",
                        "true",
                    ])
                    .status();
                desktop_shortcut_created = true;
            }
            Err(e) => eprintln!("Warning: could not create Desktop shortcut file: {e}"),
        }
    }

    // 2. Application-menu entry under XDG data dir.
    if let Ok(data_dir) = crate::appdata::app_data_dir() {
        let applications_dir = data_dir.join("applications");
        let _ = fs::create_dir_all(&applications_dir);
        let applications_file = applications_dir.join(&desktop_filename);
        match fs::write(&applications_file, &rewritten) {
            Ok(()) => {
                let _ = fs::set_permissions(&applications_file, fs::Permissions::from_mode(0o644));
                let _ = Command::new("update-desktop-database")
                    .arg(&applications_dir)
                    .status();
            }
            Err(e) => eprintln!("Warning: could not write applications desktop file: {e}"),
        }
    }

    if !desktop_shortcut_created {
        eprintln!("Note: Desktop shortcut not created (Desktop directory may be absent)");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn find_icon(app_dir: &Path) -> Option<String> {
    let png_in = |dir: &Path| -> Option<String> {
        fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            match p.extension() {
                Some(ext) if ext == "png" => Some(p.to_string_lossy().into_owned()),
                _ => None,
            }
        })
    };
    png_in(app_dir).or_else(|| png_in(&app_dir.join("Resources")))
}

/// Rewrite the `Exec=` line to point at the launcher and, when an icon is
/// available, the `Icon=` line to its absolute path.
#[cfg(target_os = "linux")]
fn rewrite_desktop_entry(content: &str, launcher: &str, icon: Option<&str>) -> String {
    let mut out = String::with_capacity(content.len() + 64);
    for line in content.split('\n') {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec=\"{launcher}\"\n"));
        } else if line.starts_with("Icon=") {
            match icon {
                Some(icon) => out.push_str(&format!("Icon={icon}\n")),
                None => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Windows: create Desktop + Start-Menu `.lnk` shortcuts pointing at
/// `bin/launcher.exe`, then drop a `.reg` file that registers an uninstall
/// entry. All steps are best-effort.
#[cfg(target_os = "windows")]
pub fn create_windows_shortcut(app_dir: &Path, metadata: &AppMetadata) -> io::Result<()> {
    use std::env;

    let userprofile = match env::var("USERPROFILE") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Warning: could not get USERPROFILE directory");
            return Ok(());
        }
    };

    let target_path = app_dir.join("bin").join("launcher.exe");
    if !target_path.exists() {
        eprintln!(
            "Warning: could not find launcher.exe at {}",
            target_path.display()
        );
        return Ok(());
    }
    let working_dir = app_dir.join("bin");

    let desktop_dir = Path::new(&userprofile).join("Desktop");
    create_windows_lnk(
        &desktop_dir,
        &metadata.name,
        &target_path,
        &working_dir,
        &target_path,
    );

    let start_menu_dir = Path::new(&userprofile)
        .join("AppData")
        .join("Roaming")
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    let _ = fs::create_dir_all(&start_menu_dir);
    create_windows_lnk(
        &start_menu_dir,
        &metadata.name,
        &target_path,
        &working_dir,
        &target_path,
    );

    add_windows_uninstall_entry(app_dir, metadata);
    Ok(())
}

#[cfg(target_os = "windows")]
fn create_windows_lnk(
    shortcut_dir: &Path,
    app_name: &str,
    target_path: &Path,
    working_dir: &Path,
    icon_path: &Path,
) {
    use std::process::Command;

    let lnk_path = shortcut_dir.join(format!("{app_name}.lnk"));
    let ps = format!(
        "$WshShell = New-Object -ComObject WScript.Shell\n\
         $Shortcut = $WshShell.CreateShortcut(\"{lnk}\")\n\
         $Shortcut.TargetPath = \"{target}\"\n\
         $Shortcut.WorkingDirectory = \"{wd}\"\n\
         $Shortcut.IconLocation = \"{icon}\"\n\
         $Shortcut.WindowStyle = 1\n\
         $Shortcut.Save()\n",
        lnk = lnk_path.display(),
        target = target_path.display(),
        wd = working_dir.display(),
        icon = icon_path.display(),
    );

    let result = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &ps,
        ])
        .status();
    if let Err(e) = result {
        eprintln!("Warning: could not create Windows shortcut: {e}");
    }
}

#[cfg(target_os = "windows")]
fn add_windows_uninstall_entry(app_dir: &Path, metadata: &AppMetadata) {
    let reg_path = app_dir.join(format!("{}_uninstall.reg", metadata.name));
    let app_dir_str = app_dir.display();
    let display_name = format!("{} ({})", metadata.name, metadata.channel);

    let reg_content = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n\
         [HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{id}]\r\n\
         @=\"{display}\"\r\n\
         \"DisplayName\"=\"{display}\"\r\n\
         \"DisplayVersion\"=\"1.0\"\r\n\
         \"Publisher\"=\"Electrobun\"\r\n\
         \"InstallLocation\"=\"{dir}\"\r\n\
         \"UninstallString\"=\"cmd.exe /c rmdir /s /q \\\"{dir}\\\"\"\r\n\
         \"NoModify\"=dword:00000001\r\n\
         \"NoRepair\"=dword:00000001\r\n",
        id = metadata.identifier,
        display = display_name,
        dir = app_dir_str,
    );

    if let Err(e) = fs::write(&reg_path, reg_content) {
        eprintln!("Warning: could not write uninstall registry file: {e}");
    }
}

/// Recursively copy a directory tree. Windows cannot reliably `rename` across
/// directories, so the install step copies instead. Symlinks are skipped
/// (matching the Zig `copyDirectory`, which only handled files + dirs).
#[cfg(target_os = "windows")]
pub fn copy_directory(src: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
        // Other kinds (symlinks) are intentionally skipped.
    }
    Ok(())
}
