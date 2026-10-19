//! Restart-only WebView GPU preference. Call `configure` at the very start of
//! `run`, before constructing the Tauri builder (not from its setup callback).
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::Manager;

const FILE_NAME: &str = "hardware-acceleration.json";
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

fn load(path: &Path) -> Result<bool, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<bool>(&bytes).map_err(|error| {
            format!(
                "Invalid hardware acceleration setting in {}: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("Unable to read {}: {error}", path.display())),
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(FILE_NAME))
        .map_err(|error| format!("Unable to resolve application config directory: {error}"))
}

/// Returns the saved preference, which can differ from the running WebView's
/// configuration until the application is fully restarted.
#[tauri::command]
pub fn get_hardware_acceleration(app: tauri::AppHandle) -> Result<bool, String> {
    let _guard = SETTINGS_LOCK.lock().map_err(|error| error.to_string())?;
    load(&settings_path(&app)?)
}

/// Persists only; deliberately does not mutate the process environment while
/// WebKit/WebView2 and other application threads are running. Tauri rejects
/// missing or non-boolean `enabled` arguments before calling this function.
#[tauri::command]
pub fn set_hardware_acceleration(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let _guard = SETTINGS_LOCK.lock().map_err(|error| error.to_string())?;
    let path = settings_path(&app)?;
    let directory = path
        .parent()
        .ok_or("Invalid hardware acceleration settings path")?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Unable to create {}: {error}", directory.display()))?;
    let bytes = serde_json::to_vec(&enabled).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("Unable to save {}: {error}", path.display()))
}

// Tauri 2.12.1's path/desktop.rs uses dirs::config_dir().join(identifier).
// Neither that resolver (which requires AppHandle) nor dirs is publicly
// available before the runtime starts. Resolve the same OS directory here
// without starting GTK, an event loop, or a WebView and without a new dependency.
fn startup_settings_path(context: &tauri::Context<tauri::Wry>) -> Result<PathBuf, String> {
    // The current app uses the default directories. Refuse a future override
    // rather than silently reading a different file than the commands write.
    if context.config().app.app_directories_override.is_some() {
        return Err(
            "Early hardware acceleration configuration does not support appDirectoriesOverride"
                .into(),
        );
    }
    Ok(platform_config_dir()?
        .join(&context.config().identifier)
        .join(FILE_NAME))
}

#[cfg(target_os = "linux")]
fn platform_config_dir() -> Result<PathBuf, String> {
    // dirs ignores relative XDG_CONFIG_HOME values, per the XDG specification.
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path);
    }
    // GLib's home lookup falls back to the passwd database when HOME is unset.
    let home = gtk::glib::home_dir();
    if !home.is_absolute() {
        return Err("Unable to resolve the home directory".into());
    }
    Ok(home.join(".config"))
}

#[cfg(windows)]
fn platform_config_dir() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::UI::Shell::{SHGetFolderPathW, CSIDL_APPDATA, SHGFP_TYPE_CURRENT};
    let mut buffer = [0u16; 260];
    // CSIDL_APPDATA is the same roaming AppData known folder used by dirs.
    unsafe {
        SHGetFolderPathW(
            None,
            CSIDL_APPDATA as i32,
            None,
            SHGFP_TYPE_CURRENT.0 as u32,
            &mut buffer,
        )
    }
    .map_err(|error| format!("Unable to resolve roaming AppData: {error}"))?;
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length],
    )))
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("Unable to resolve the home directory")?;
    Ok(home.join("Library/Application Support"))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn platform_config_dir() -> Result<PathBuf, String> {
    Err("Unsupported platform for early hardware acceleration configuration".into())
}

/// Apply the saved preference before any WebViews or application threads start.
/// Missing settings default to enabled; malformed/unreadable settings are logged
/// and also default to enabled. Enabled leaves externally supplied GPU flags
/// untouched; it does not force-enable GPU support against the user's environment.
pub fn configure(context: &tauri::Context<tauri::Wry>) {
    let enabled = startup_settings_path(context)
        .and_then(|path| load(&path))
        .unwrap_or_else(|error| {
            eprintln!(
                "Unable to load hardware acceleration preference; defaulting to enabled: {error}"
            );
            true
        });
    if enabled {
        return;
    }

    #[cfg(windows)]
    {
        // OsString preserves even non-Unicode externally supplied arguments.
        let mut arguments =
            std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        if !arguments
            .to_string_lossy()
            .split_whitespace()
            .any(|argument| argument == "--disable-gpu")
        {
            if !arguments.is_empty() {
                arguments.push(" ");
            }
            arguments.push("--disable-gpu");
        }
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", arguments);
    }
    #[cfg(target_os = "linux")]
    {
        // Verified in WebKit UIProcess/gtk/HardwareAccelerationManager.cpp:
        // WEBKIT_DISABLE_COMPOSITING_MODE values other than "0" disable it.
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn persisted_preference_is_strictly_boolean() {
        for enabled in [true, false] {
            let bytes = serde_json::to_vec(&enabled).unwrap();
            assert_eq!(serde_json::from_slice::<bool>(&bytes).unwrap(), enabled);
        }
        for invalid in [
            "null",
            "0",
            "\"false\"",
            "{}",
            "{\"enabled\":false}",
            "true false",
            "",
        ] {
            assert!(serde_json::from_str::<bool>(invalid).is_err());
        }
    }
}
