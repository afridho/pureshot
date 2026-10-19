use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Local;
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::{fs, io::Cursor, path::PathBuf};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

#[cfg(target_os = "linux")]
mod linux_capture;
#[cfg(target_os = "linux")]
mod linux_selection;
#[cfg(target_os = "linux")]
mod linux_pin;
#[cfg(target_os = "linux")]
mod desktop_appearance;

#[tauri::command]
fn get_desktop_theme(app: tauri::AppHandle) -> Option<String> {
    #[cfg(target_os = "linux")]
    { return app.state::<desktop_appearance::Appearance>().0.lock().ok().and_then(|theme| theme.clone()); }
    #[cfg(not(target_os = "linux"))]
    { let _ = app; None }
}
#[cfg(windows)]
mod native_overlay_window;
#[cfg(windows)]
mod native_pin;

mod desktop_icons;
mod hardware_acceleration;
mod filename;
mod ocr;
mod qr;
mod export_format;
mod quick_access;
use export_format::ExportFormat;

struct DefaultFormat(Mutex<ExportFormat>);

fn default_format_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|dir| dir.join("default-format.json")).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_default_format(app: tauri::AppHandle) -> Result<ExportFormat, String> {
    app.state::<DefaultFormat>().0.lock().map(|value| *value).map_err(|_| "Default format unavailable".into())
}

#[tauri::command]
fn set_default_format(app: tauri::AppHandle, format: ExportFormat) -> Result<(), String> {
    let state = app.state::<DefaultFormat>();
    let mut current = state.0.lock().map_err(|_| "Default format unavailable")?;
    let path = default_format_path(&app)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    fs::write(path, serde_json::to_vec(&format).map_err(|error| error.to_string())?).map_err(|error| format!("Unable to save default format: {error}"))?;
    *current = format;
    Ok(())
}

struct FilenameTemplate(Mutex<String>);

fn filename_template_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|dir| dir.join("filename-template.json")).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_filename_template(app: tauri::AppHandle) -> Result<String, String> {
    app.state::<FilenameTemplate>().0.lock().map(|value| value.clone()).map_err(|_| "Filename setting unavailable".into())
}

#[tauri::command]
fn preview_filename_template(app: tauri::AppHandle, template: String) -> Result<String, String> {
    let name = validate_filename_template(&template)?;
    Ok(format!("{name}.{}", get_default_format(app)?.extension()))
}

fn validate_filename_template(template: &str) -> Result<String, String> {
    let now = Local::now().timestamp_millis();
    filename::render(template, now, 1920, 1080, "area", &format!("{now}-area"))
}

#[tauri::command]
fn set_filename_template(app: tauri::AppHandle, template: String) -> Result<(), String> {
    validate_filename_template(&template)?;
    let state = app.state::<FilenameTemplate>();
    let mut current = state.0.lock().map_err(|_| "Filename setting unavailable")?;
    let path = filename_template_path(&app)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    fs::write(path, serde_json::to_vec(&template).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    *current = template;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaptureShortcuts {
    fullscreen: String,
    area: String,
    window: String,
    // Older files retain their custom capture bindings; OCR starts unassigned.
    #[serde(default)]
    ocr: String,
}

impl Default for CaptureShortcuts {
    fn default() -> Self {
        Self {
            fullscreen: "Shift+Alt+3".into(),
            area: "Shift+Alt+4".into(),
            window: "Shift+Alt+5".into(),
            ocr: String::new(),
        }
    }
}

impl CaptureShortcuts {
    fn get(&self, mode: &str) -> Option<&str> {
        match mode {
            "fullscreen" => Some(&self.fullscreen),
            "area" => Some(&self.area),
            "window" => Some(&self.window),
            "ocr" => Some(&self.ocr),
            _ => None,
        }
    }

    fn set(&mut self, mode: &str, accelerator: String) -> Result<(), String> {
        match mode {
            "fullscreen" => self.fullscreen = accelerator,
            "area" => self.area = accelerator,
            "window" => self.window = accelerator,
            "ocr" => self.ocr = accelerator,
            _ => return Err(format!("Unknown capture mode: {mode}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod capture_shortcut_tests {
    use super::*;

    #[test]
    fn migrates_old_custom_settings_without_resetting_bindings() {
        let shortcuts: CaptureShortcuts = serde_json::from_str(r#"{"fullscreen":"Ctrl+F3","area":"","window":"Super+W"}"#).unwrap();
        assert_eq!(shortcuts.fullscreen, "Ctrl+F3");
        assert_eq!(shortcuts.area, "");
        assert_eq!(shortcuts.window, "Super+W");
        assert_eq!(shortcuts.ocr, "");
    }

    #[test]
    fn empty_bindings_survive_persistence_for_every_action() {
        let mut shortcuts = CaptureShortcuts::default();
        assert_eq!(shortcuts.ocr, "");
        for action in ["fullscreen", "area", "window", "ocr"] {
            shortcuts.set(action, String::new()).unwrap();
        }
        let restored: CaptureShortcuts = serde_json::from_slice(&serde_json::to_vec(&shortcuts).unwrap()).unwrap();
        for action in ["fullscreen", "area", "window", "ocr"] {
            assert_eq!(restored.get(action), Some(""));
        }
        assert!(shortcuts.set("unknown", String::new()).is_err());
    }

    #[test]
    fn rejects_canonical_duplicates_but_allows_multiple_unassigned() {
        let mut shortcuts = CaptureShortcuts::default();
        shortcuts.area = " Control + Shift + T ".into();
        shortcuts.ocr = "Shift+Ctrl+T".into();
        assert!(validate_capture_shortcuts(&shortcuts).unwrap_err().contains("already assigned"));
        shortcuts.area.clear();
        shortcuts.window.clear();
        assert!(validate_capture_shortcuts(&shortcuts).is_ok());
        shortcuts.fullscreen = "Meta+Minus".into();
        shortcuts.ocr = "Super+-".into();
        assert!(validate_capture_shortcuts(&shortcuts).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn compositor_shortcuts_only_replace_plugin_on_gnome_wayland() {
        assert!(is_gnome_wayland_session("ubuntu:GNOME", "wayland"));
        assert!(is_gnome_wayland_session("gnome", "Wayland"));
        assert!(!is_gnome_wayland_session("GNOME", "x11"));
        assert!(!is_gnome_wayland_session("KDE", "wayland"));
        assert!(!is_gnome_wayland_session("", ""));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_accelerators_preserve_modifiers_and_empty() {
        assert_eq!(to_gnome_accelerator("").unwrap(), "");
        assert_eq!(to_gnome_accelerator("Ctrl+Shift+T").unwrap(), "<Control><Shift>t");
        assert!(to_gnome_accelerator("Ctrl+").is_err());
    }
}

struct CaptureShortcutState(Mutex<CaptureShortcuts>);

struct PendingCliCapture(Mutex<Option<String>>);

#[tauri::command]
fn process_cli_capture(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<PendingCliCapture>();
    let command = state.0.lock().map_err(|_| "CLI capture state unavailable")?.take();
    let Some(command) = command else { return Ok(()); };
    match command.as_str() {
        "area" => start_capture_from_shortcut(app, "area"),
        "fullscreen" => start_capture_from_shortcut(app, "fullscreen"),
        "window" => start_capture_from_shortcut(app, "window"),
        "ocr" => start_capture_from_shortcut(app, "ocr"),
        _ => return Err(format!("Unknown CLI capture action: {command}")),
    }
    Ok(())
}

fn capture_shortcuts_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|dir| dir.join("capture-shortcuts.json")).map_err(|error| error.to_string())
}

fn load_capture_shortcuts(app: &tauri::AppHandle) -> CaptureShortcuts {
    capture_shortcuts_path(app).ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Parses "Shift+Alt+4" into a tauri-plugin-global-shortcut accelerator.
/// Requires exactly one non-modifier key and at least one modifier so a
/// stray key press can never become an accidental global grab.
fn parse_accelerator(accelerator: &str) -> Result<(tauri_plugin_global_shortcut::Modifiers, tauri_plugin_global_shortcut::Code), String> {
    use tauri_plugin_global_shortcut::{Code, Modifiers};
    let mut modifiers = Modifiers::empty();
    let mut code = None;
    for part in accelerator.split('+') {
        let part = part.trim();
        if part.is_empty() { return Err("Shortcut cannot have an empty segment".into()); }
        match part {
            "Ctrl" | "Control" => modifiers |= Modifiers::CONTROL,
            "Shift" => modifiers |= Modifiers::SHIFT,
            "Alt" => modifiers |= Modifiers::ALT,
            "Super" | "Meta" | "Cmd" | "Command" => modifiers |= Modifiers::SUPER,
            key => {
                if code.is_some() { return Err("Shortcut can only have one non-modifier key".into()); }
                code = Some(parse_key_code(key)?);
            }
        }
    }
    if modifiers.is_empty() { return Err("Shortcut needs at least one modifier (Ctrl, Alt, Shift or Super)".into()); }
    let code = code.ok_or("Shortcut needs one non-modifier key")?;
    let _: Code = code;
    Ok((modifiers, code))
}

fn parse_key_code(key: &str) -> Result<tauri_plugin_global_shortcut::Code, String> {
    use tauri_plugin_global_shortcut::Code;
    Ok(match key {
        "0" => Code::Digit0, "1" => Code::Digit1, "2" => Code::Digit2, "3" => Code::Digit3,
        "4" => Code::Digit4, "5" => Code::Digit5, "6" => Code::Digit6, "7" => Code::Digit7,
        "8" => Code::Digit8, "9" => Code::Digit9,
        "A" => Code::KeyA, "B" => Code::KeyB, "C" => Code::KeyC, "D" => Code::KeyD,
        "E" => Code::KeyE, "F" => Code::KeyF, "G" => Code::KeyG, "H" => Code::KeyH,
        "I" => Code::KeyI, "J" => Code::KeyJ, "K" => Code::KeyK, "L" => Code::KeyL,
        "M" => Code::KeyM, "N" => Code::KeyN, "O" => Code::KeyO, "P" => Code::KeyP,
        "Q" => Code::KeyQ, "R" => Code::KeyR, "S" => Code::KeyS, "T" => Code::KeyT,
        "U" => Code::KeyU, "V" => Code::KeyV, "W" => Code::KeyW, "X" => Code::KeyX,
        "Y" => Code::KeyY, "Z" => Code::KeyZ,
        "F1" => Code::F1, "F2" => Code::F2, "F3" => Code::F3, "F4" => Code::F4,
        "F5" => Code::F5, "F6" => Code::F6, "F7" => Code::F7, "F8" => Code::F8,
        "F9" => Code::F9, "F10" => Code::F10, "F11" => Code::F11, "F12" => Code::F12,
        "Space" => Code::Space, "Tab" => Code::Tab, "Escape" => Code::Escape,
        "Minus" | "-" => Code::Minus, "Equal" | "=" => Code::Equal,
        "Comma" | "," => Code::Comma, "Period" | "." => Code::Period,
        "Slash" | "/" => Code::Slash, "Semicolon" | ";" => Code::Semicolon,
        other => return Err(format!("Unsupported key: {other}")),
    })
}

/// GNOME's accelerator syntax, e.g. "<Control><Shift>4", for the companion
/// extension's per-mode GSettings keys.
#[cfg(target_os = "linux")]
fn to_gnome_accelerator(accelerator: &str) -> Result<String, String> {
    if accelerator.is_empty() { return Ok(String::new()); }
    parse_accelerator(accelerator)?;
    let mut gnome = String::new();
    for part in accelerator.split('+') {
        let part = part.trim();
        match part {
            "Ctrl" | "Control" => gnome.push_str("<Control>"),
            "Shift" => gnome.push_str("<Shift>"),
            "Alt" => gnome.push_str("<Alt>"),
            "Super" | "Meta" | "Cmd" | "Command" => gnome.push_str("<Super>"),
            key => gnome.push_str(&key.to_lowercase()),
        }
    }
    Ok(gnome)
}

#[cfg(target_os = "linux")]
fn sync_gnome_shortcut(mode: &str, accelerator: &str) -> Result<(), String> {
    let key = match mode {
        "area" => "capture-area-shortcut",
        "window" => "capture-window-shortcut",
        "fullscreen" => "capture-fullscreen-shortcut",
        "ocr" => "capture-ocr-shortcut",
        _ => return Err(format!("Unknown shortcut action: {mode}")),
    };
    // X11 and other desktops must not depend on the companion extension.
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !desktop.split(':').any(|name| name.eq_ignore_ascii_case("gnome")) {
        return Ok(());
    }
    let required = uses_gnome_shortcuts();
    let value = if accelerator.is_empty() { "[]".to_string() } else {
        format!("[{:?}]", to_gnome_accelerator(accelerator)?)
    };
    // The schema ships inside the extension folder, not the global schema
    // path, so plain `gsettings` can't find it without --schemadir. Both
    // write to the same dconf key the extension's keybinding listens to.
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or("Unable to locate XDG data directory")?;
    let schema_dir = data_home.join("gnome-shell/extensions/pureshot@afridho/schemas");
    let result = std::process::Command::new("gsettings")
        .arg("--schemadir").arg(&schema_dir)
        .args(["set", "org.gnome.shell.extensions.pureshot", key, &value])
        .output()
        .map_err(|error| format!("Unable to run gsettings: {error}"))
        .and_then(|output| {
            if output.status.success() { Ok(()) } else {
                Err(format!("gsettings set failed (install/update the PureShot GNOME extension first): {}", String::from_utf8_lossy(&output.stderr).trim()))
            }
        });
    if let Err(error) = result {
        if required { return Err(error); }
        eprintln!("Optional GNOME shortcut sync skipped: {error}");
    }
    Ok(())
}

fn validate_capture_shortcuts(shortcuts: &CaptureShortcuts) -> Result<(), String> {
    let mut assigned: Vec<((tauri_plugin_global_shortcut::Modifiers, tauri_plugin_global_shortcut::Code), &str)> = Vec::new();
    for action in ["fullscreen", "area", "window", "ocr"] {
        let accelerator = shortcuts.get(action).unwrap_or_default();
        if accelerator.is_empty() { continue; }
        // Parsed modifiers/key normalize order, whitespace and supported aliases.
        let parsed = parse_accelerator(accelerator)?;
        if let Some((_, other)) = assigned.iter().find(|(binding, _)| *binding == parsed) {
            return Err(format!("Shortcut for {action} is already assigned to {other}"));
        }
        assigned.push((parsed, action));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_gnome_wayland_session(desktop: &str, session_type: &str) -> bool {
    desktop.split(':').any(|name| name.eq_ignore_ascii_case("gnome")) &&
        session_type.eq_ignore_ascii_case("wayland")
}

#[cfg(target_os = "linux")]
fn uses_gnome_shortcuts() -> bool {
    is_gnome_wayland_session(
        &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        &std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
    )
}

/// Unregisters every currently bound capture shortcut, then (re)registers
/// the ones in `shortcuts`, wiring each to `start_capture_from_shortcut`.
fn apply_global_shortcuts(app: &tauri::AppHandle, shortcuts: &CaptureShortcuts) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    validate_capture_shortcuts(shortcuts)?;
    // On GNOME Wayland the compositor extension is authoritative. Do not
    // require an X11 grab or register a second callback via XWayland. This
    // same path is used at startup, by the setter and during rollback.
    #[cfg(target_os = "linux")]
    if uses_gnome_shortcuts() { return Ok(()); }
    let manager = app.global_shortcut();
    manager.unregister_all().map_err(|error| format!("Unable to unregister shortcuts: {error}"))?;
    for mode in ["fullscreen", "area", "window", "ocr"] {
        let accelerator = shortcuts.get(mode).unwrap_or_default();
        if accelerator.is_empty() { continue; }
        let (modifiers, code) = parse_accelerator(accelerator)?;
        let shortcut = tauri_plugin_global_shortcut::Shortcut::new(Some(modifiers), code);
        let handle = app.clone();
        let mode: &'static str = mode;
        manager.on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                start_capture_from_shortcut(handle.clone(), mode);
            }
        }).map_err(|error| format!("Global shortcut for {mode} unavailable (use tray or app controls): {error}"))?;
    }
    Ok(())
}

#[tauri::command]
fn get_capture_shortcuts(app: tauri::AppHandle) -> Result<CaptureShortcuts, String> {
    app.state::<CaptureShortcutState>().0.lock().map(|value| value.clone()).map_err(|_| "Shortcut setting unavailable".into())
}

#[tauri::command]
fn set_capture_shortcut(app: tauri::AppHandle, mode: String, accelerator: String) -> Result<(), String> {
    if !accelerator.is_empty() { parse_accelerator(&accelerator)?; }
    let state = app.state::<CaptureShortcutState>();
    let mut current = state.0.lock().map_err(|_| "Shortcut setting unavailable")?;
    let mut next = current.clone();
    next.set(&mode, accelerator.clone())?;
    validate_capture_shortcuts(&next)?;

    // Resolve/prepare persistence before touching working registrations.
    let path = capture_shortcuts_path(&app)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    let bytes = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| format!("Unable to save shortcut setting: {error}"))?;
    let result = (|| -> Result<(), String> {
        apply_global_shortcuts(&app, &next)?;
        #[cfg(target_os = "linux")]
        sync_gnome_shortcut(&mode, &accelerator)?;
        fs::rename(&temporary, &path).map_err(|error| format!("Unable to save shortcut setting: {error}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        if let Err(rollback) = apply_global_shortcuts(&app, &current) { failures.push(rollback); }
        #[cfg(target_os = "linux")]
        if let Err(rollback) = sync_gnome_shortcut(&mode, current.get(&mode).unwrap_or_default()) { failures.push(rollback); }
        let _ = fs::remove_file(&temporary);
        return Err(if failures.is_empty() { error } else { format!("{error}; shortcut rollback failed: {}", failures.join("; ")) });
    }
    *current = next;
    Ok(())
}

static CAPTURE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct CaptureGuard;
impl CaptureGuard {
    fn acquire() -> Result<Self, String> {
        CAPTURE_ACTIVE.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
            .map(|_| Self).map_err(|_| "A screenshot is already in progress".into())
    }
}
impl Drop for CaptureGuard {
    fn drop(&mut self) { CAPTURE_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst); }
}

#[cfg(test)]
mod capture_guard_tests {
    use super::CaptureGuard;

    #[test]
    fn rejects_concurrent_capture_and_releases_after_drop() {
        let first = CaptureGuard::acquire().unwrap();
        assert!(CaptureGuard::acquire().is_err());
        drop(first);
        assert!(CaptureGuard::acquire().is_ok());
    }
}

struct LatestCapture(Mutex<Option<CaptureItem>>);
struct CaptureHistory(Mutex<Vec<CaptureItem>>);

#[derive(Clone, Copy)]
struct CaptureSettings {
    corner_radius: u32,
    preview_timeout_secs: u32,
    copy_after_capture: bool,
    hide_desktop_icons: bool,
    show_quick_access: bool,
    quick_access_hover: bool,
    show_crosshair: bool,
    show_magnifier: bool,
    freeze_screen: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            corner_radius: 13,
            preview_timeout_secs: 10,
            copy_after_capture: false,
            hide_desktop_icons: false,
            show_quick_access: true,
            quick_access_hover: true,
            show_crosshair: true,
            show_magnifier: true,
            freeze_screen: false,
        }
    }
}

#[cfg(test)]
mod capture_settings_tests {
    use super::CaptureSettings;

    #[test]
    fn defaults_match_frontend_capture_preferences() {
        let settings = CaptureSettings::default();
        assert_eq!(settings.corner_radius, 13);
        assert_eq!(settings.preview_timeout_secs, 10);
        assert!(!settings.copy_after_capture);
        assert!(!settings.hide_desktop_icons);
        assert!(settings.show_quick_access);
        assert!(settings.quick_access_hover);
        assert!(settings.show_crosshair);
        assert!(settings.show_magnifier);
        assert!(!settings.freeze_screen);
    }
}

fn capture_settings(app: &tauri::AppHandle) -> CaptureSettings {
    app.state::<RuntimeSettings>().0.lock().map(|value| *value).unwrap_or_default()
}

struct RuntimeSettings(Mutex<CaptureSettings>);
struct TrayVisibility(Mutex<bool>);

fn tray_visibility_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir()
        .map(|dir| dir.join("show-tray-icon.json"))
        .map_err(|error| error.to_string())
}

fn load_tray_visibility(app: &tauri::AppHandle) -> Result<bool, String> {
    let path = tray_visibility_path(app)?;
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn get_show_tray_icon(app: tauri::AppHandle) -> Result<bool, String> {
    app.state::<TrayVisibility>().0.lock()
        .map(|visible| *visible)
        .map_err(|_| "Tray setting unavailable".into())
}

#[tauri::command]
fn set_show_tray_icon(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    let state = app.state::<TrayVisibility>();
    let mut current = state.0.lock().map_err(|_| "Tray setting unavailable")?;
    let tray = app.tray_by_id("main-tray").ok_or("System tray icon unavailable")?;
    let path = tray_visibility_path(&app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    tray.set_visible(visible).map_err(|error| error.to_string())?;
    if let Err(error) = fs::write(path, if visible { b"true".as_slice() } else { b"false".as_slice() }) {
        let _ = tray.set_visible(*current);
        return Err(format!("Unable to save tray setting: {error}"));
    }
    *current = visible;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureItem {
    id: String,
    path: String,
    data_url: String,
    width: u32,
    height: u32,
    created_at: i64,
}

struct ExportDirectory(Mutex<PathBuf>);

fn export_directory_setting_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map(|dir| dir.join("export-directory.json")).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_export_directory(app: tauri::AppHandle) -> Result<String, String> {
    app.state::<ExportDirectory>().0.lock().map(|dir| dir.to_string_lossy().into_owned()).map_err(|_| "Export directory unavailable".into())
}

#[tauri::command]
async fn choose_export_directory(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<Option<String>, String> {
    let current = get_export_directory(app.clone())?;
    let mut dialog = app.dialog().file().set_title("Choose PureShot export folder");
    if std::path::Path::new(&current).is_dir() { dialog = dialog.set_directory(&current); }
    #[cfg(any(windows, target_os = "macos"))]
    { dialog = dialog.set_parent(&window); }
    #[cfg(target_os = "linux")]
    let _ = &window;
    let Some(selected) = dialog.blocking_pick_folder() else { return Ok(None) };
    let path = selected.into_path().map_err(|error| error.to_string())?;
    if !path.is_absolute() || !path.is_dir() { return Err("Choose an existing folder.".into()); }
    // Check write access without touching any existing user files.
    let probe = path.join(format!(".pureshot-write-test-{}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap_or_default()));
    let file = fs::OpenOptions::new().write(true).create_new(true).open(&probe)
        .map_err(|error| format!("Cannot write to the selected folder: {error}"))?;
    drop(file);
    fs::remove_file(&probe).map_err(|error| format!("Unable to remove folder write test: {error}"))?;
    let state = app.state::<ExportDirectory>();
    let mut current = state.0.lock().map_err(|_| "Export directory unavailable")?;
    let setting_path = export_directory_setting_path(&app)?;
    if let Some(parent) = setting_path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    fs::write(setting_path, serde_json::to_vec(&path).map_err(|error| error.to_string())?).map_err(|error| format!("Unable to save export folder: {error}"))?;
    let display = path.to_string_lossy().into_owned();
    *current = path;
    Ok(Some(display))
}

fn export_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.state::<ExportDirectory>().0.lock().map_err(|_| "Export directory unavailable")?.clone();
    fs::create_dir_all(&dir).map_err(|error| format!("Unable to access export folder {}: {error}", dir.display()))?;
    Ok(dir)
}

fn capture_dir(app: &tauri::AppHandle) -> PathBuf {
    let base = app.path().picture_dir().or_else(|_| app.path().home_dir())
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("PureShot");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn capture_cache_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "linux"))]
    let base = std::env::temp_dir();
    let dir = base.join("PureShot").join("captures");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn export_filename(app: &tauri::AppHandle, item: &CaptureItem, annotated: bool, format: ExportFormat) -> Result<String, String> {
    let template = get_filename_template(app.clone())?;
    let capture_type = item.id.split_once('-').map(|(_, mode)| mode).unwrap_or("capture");
    let name = filename::render(&template, item.created_at, item.width, item.height, capture_type, &item.id)?;
    Ok(format!("{name}{}.{}", if annotated { " Annotated" } else { "" }, format.extension()))
}

fn unique_annotation_path(app: &tauri::AppHandle, item: &CaptureItem, format: ExportFormat) -> Result<PathBuf, String> {
    let dir = export_directory(app)?;
    let preferred = dir.join(export_filename(app, item, true, format)?);
    if !preferred.exists() {
        return Ok(preferred);
    }
    let stem = preferred
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("PureShot Annotated");
    for suffix in 2.. {
        let candidate = dir.join(format!("{stem} ({suffix}).{}", format.extension()));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!()
}

fn unique_export_path(app: &tauri::AppHandle, item: &CaptureItem, format: ExportFormat) -> Result<PathBuf, String> {
    let dir = export_directory(app)?;
    let preferred = dir.join(export_filename(app, item, false, format)?);
    if !preferred.exists() {
        return Ok(preferred);
    }

    let stem = preferred
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("PureShot");
    for suffix in 2.. {
        let candidate = dir.join(format!("{stem} ({suffix}).{}", format.extension()));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!()
}

fn copy_capture_to_clipboard(app: &tauri::AppHandle, item: &CaptureItem) -> Result<(), String> {
    let decoded = image::open(&item.path)
        .map_err(|error| format!("Unable to decode screenshot for clipboard: {error}"))?
        .to_rgba8();
    let clipboard_image = Image::new(decoded.as_raw(), decoded.width(), decoded.height());
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|error| error.to_string())
}

fn remember_capture(app: &tauri::AppHandle, item: &CaptureItem) -> Result<(), String> {
    if let Some(state) = app.try_state::<CaptureHistory>() {
        let mut history = state.0.lock().map_err(|_| "Capture history unavailable")?;
        history.retain(|existing| existing.id != item.id && std::path::Path::new(&existing.path).exists());
        history.insert(0, item.clone());
        history.truncate(50);
    }
    Ok(())
}

fn load_cached_history() -> Vec<CaptureItem> {
    let mut items = Vec::new();
    let Ok(entries) = fs::read_dir(capture_cache_dir()) else { return items };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("png") { continue; }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else { continue };
        let created_at = stem.split('-').next().and_then(|value| value.parse::<i64>().ok()).unwrap_or(0);
        if let Ok((width, height)) = image::image_dimensions(&path) {
            items.push(CaptureItem { id: stem.to_string(), path: path.to_string_lossy().into_owned(), data_url: String::new(), width, height, created_at });
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
    items.truncate(50);
    items
}

fn maybe_copy_after_capture(app: &tauri::AppHandle, item: &CaptureItem) {
    let enabled = app
        .try_state::<RuntimeSettings>()
        .and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|settings| settings.copy_after_capture)
        })
        .unwrap_or(false);
    if enabled {
        if let Err(error) = copy_capture_to_clipboard(app, item) {
            eprintln!("Failed to copy capture to clipboard: {error}");
        }
    }
}

fn save_image(
    app: &tauri::AppHandle,
    dynamic: image::DynamicImage,
    mode: &str,
) -> Result<CaptureItem, String> {
    let item = save_image_without_copy(app, dynamic, mode)?;
    maybe_copy_after_capture(app, &item);
    Ok(item)
}

fn save_image_without_copy(
    app: &tauri::AppHandle,
    dynamic: image::DynamicImage,
    mode: &str,
) -> Result<CaptureItem, String> {
    let width = dynamic.width();
    let height = dynamic.height();
    let mut png = Vec::new();
    dynamic
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let stamp = Local::now();
    let id = format!("{}-{}", stamp.timestamp_millis(), mode);
    let path = capture_cache_dir().join(format!("{id}.png"));
    fs::write(&path, &png).map_err(|e| e.to_string())?;
    let item = CaptureItem {
        id,
        path: path.to_string_lossy().to_string(),
        data_url: format!("data:image/png;base64,{}", STANDARD.encode(&png)),
        width,
        height,
        created_at: stamp.timestamp_millis(),
    };
    if let Some(state) = app.try_state::<LatestCapture>() {
        *state.0.lock().map_err(|_| "Capture state unavailable")? = Some(item.clone());
    }
    remember_capture(app, &item)?;
    let _ = app.emit("capture-created", &item);
    Ok(item)
}

#[tauri::command]
fn start_area_capture(
    app: tauri::AppHandle,
    corner_radius: u32,
    preview_timeout_secs: u32,
) -> Result<(), String> {
    start_area_capture_inner(app, corner_radius, preview_timeout_secs, false)
}

fn start_area_capture_inner(app: tauri::AppHandle, corner_radius: u32, preview_timeout_secs: u32, ocr: bool) -> Result<(), String> {
    let guard = CaptureGuard::acquire()?;
    let settings = capture_settings(&app);
    // Selection is a blocking native Win32 message loop, so it must never run
    // on Tauri's UI thread. The native library snapshots the virtual desktop
    // before drawing its overlay, preventing white/transparent WebView frames.
    #[cfg(target_os = "linux")]
    let main_was_visible = app.get_webview_window("main")
        .map(|main| main.is_visible().unwrap_or(false)).unwrap_or(false);
    if let Some(main) = app.get_webview_window("main") {
        main.hide().map_err(|e| e.to_string())?;
    }
    std::thread::spawn(move || {
        let _guard = guard;
        #[cfg(windows)]
        {
            // WebView2 hide returns before DWM has committed the new desktop
            // frame. Capturing immediately would bake the PureShot dashboard
            // into the screenshot and therefore into the pinned image.
            std::thread::sleep(std::time::Duration::from_millis(280));
            #[cfg(windows)]
            let icons = match desktop_icons::DesktopIconsGuard::hide(settings.hide_desktop_icons) {
                Ok(guard) => guard,
                Err(error) => { report_capture_error(&app, &error); return; }
            };
            // Let Explorer's visibility change reach the compositor before sampling.
            #[cfg(windows)]
            if settings.hide_desktop_icons { std::thread::sleep(std::time::Duration::from_millis(120)); }
            use capture_overlay::{start_capture_with_options, CaptureOptions, CaptureOutcome};
            let outcome = start_capture_with_options(corner_radius, CaptureOptions {
                show_crosshair: settings.show_crosshair,
                show_magnifier: settings.show_magnifier,
                freeze_screen: settings.freeze_screen,
            });
            #[cfg(windows)]
            drop(icons);
            match outcome {
                Ok(CaptureOutcome::Captured {
                    rgba,
                    width,
                    height,
                }) => {
                    if let Some(image) = image::RgbaImage::from_raw(width, height, rgba) {
                        let dynamic = image::DynamicImage::ImageRgba8(image);
                        let saved = if ocr { save_image_without_copy(&app, dynamic, "ocr") } else { save_image(&app, dynamic, "area") };
                        match saved {
                            Ok(item) => {
                                let result = if ocr { recognize_and_copy_capture(&app, &item) } else {
                                    show_automatic_capture_overlay(app.clone(), item.id, corner_radius, preview_timeout_secs)
                                };
                                if let Err(error) = result { if error != "Screenshot cancelled" { report_capture_error(&app, &error); } }
                            }
                            Err(error) => report_capture_error(&app, &error),
                        }
                    }
                }
                Ok(CaptureOutcome::Cancelled) => {}
                Err(error) => report_capture_error(&app, &error.to_string()),
            }
        }
        #[cfg(target_os = "linux")]
        {
            if main_was_visible {
                std::thread::sleep(std::time::Duration::from_millis(280));
            }
            // The companion applies selector preferences and returns either the
            // selected frozen frame or a clean live capture after modal teardown.
            let result = tauri::async_runtime::block_on(linux_capture::capture_area(corner_radius, linux_capture::SelectionOptions {
                show_crosshair: settings.show_crosshair,
                show_magnifier: settings.show_magnifier,
                freeze_screen: settings.freeze_screen,
                hide_desktop_icons: settings.hide_desktop_icons,
            }))
                .and_then(|image| image.map(|image| {
                    if ocr { save_image_without_copy(&app, image, "ocr") }
                    else { save_image(&app, linux_selection::round_capture(image, corner_radius), "area") }
                }).transpose());
            match result {
                Ok(Some(item)) => {
                    let result = if ocr { recognize_and_copy_capture(&app, &item) } else {
                        show_automatic_capture_overlay(app.clone(), item.id, corner_radius, preview_timeout_secs)
                    };
                    if let Err(error) = result {
                        report_capture_error(&app, &error);
                    }
                }
                Ok(None) => {}
                Err(error) => report_capture_error(&app, &error),
            }
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        report_capture_error(&app, "Capture is not supported on this platform");
    });
    Ok(())
}

fn notify_ocr(app: &tauri::AppHandle, title: &str, body: &str) {
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        eprintln!("Unable to show OCR notification: {error}");
    }
}

fn recognize_and_copy_capture(app: &tauri::AppHandle, item: &CaptureItem) -> Result<(), String> {
    #[cfg(debug_assertions)]
    let started = std::time::Instant::now();
    let result = (|| -> Result<(), String> {
        let text = app.state::<ocr::OcrState>().recognize(&ocr_model_dir(app)?, PathBuf::from(&item.path))?;
        #[cfg(debug_assertions)]
        eprintln!("OCR completed in {:.2}s", started.elapsed().as_secs_f64());
        if text.is_empty() {
            notify_ocr(app, "No text or QR code found", "Capture clearer text or the entire QR code, including its border. Your clipboard was not changed.");
            return Ok(());
        }
        app.clipboard().write_text(&text).map_err(|e| format!("Unable to copy OCR text: {e}"))?;
        // Escape markup interpreted by Linux notification daemons.
        let mut preview: String = text.chars().take(240).collect();
        if text.chars().count() > 240 { preview.push('…'); }
        let preview = preview.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        notify_ocr(app, "Text or QR content copied", &preview);
        Ok(())
    })();
    if let Err(error) = &result { notify_ocr(app, "OCR failed", error); }
    result
}

fn ocr_model_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map(|dir| dir.join("ocr-models")).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_ocr_status(app: tauri::AppHandle) -> Result<bool, String> {
    Ok(ocr::ready(&ocr_model_dir(&app)?))
}

#[tauri::command]
async fn install_ocr_models(app: tauri::AppHandle) -> Result<(), String> {
    let dir = ocr_model_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<ocr::OcrState>().install(&dir))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn extract_capture_text(app: tauri::AppHandle, id: String) -> Result<String, String> {
    let path = app.state::<CaptureHistory>().0.lock().map_err(|_| "Capture history unavailable")?
        .iter().find(|item| item.id == id).map(|item| PathBuf::from(&item.path)).ok_or("Capture not found")?;
    let dir = ocr_model_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<ocr::OcrState>().recognize(&dir, path))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn copy_ocr_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    if text.trim().is_empty() { return Err("No text to copy".into()); }
    app.clipboard().write_text(&text).map_err(|e| e.to_string())?;
    let preview: String = text.chars().take(240).collect();
    let preview = preview.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    notify_ocr(&app, "Text copied to clipboard", &preview);
    Ok(())
}

#[tauri::command]
fn start_ocr_capture(app: tauri::AppHandle) -> Result<(), String> {
    let settings = capture_settings(&app);
    start_area_capture_inner(app, settings.corner_radius, 0, true)
}

#[tauri::command(async)]
fn show_ocr_capture(app: tauri::AppHandle, id: String) -> Result<(), String> {
    if !ocr::ready(&ocr_model_dir(&app)?) { return open_ocr_window(&app, &id); }
    let item = app.state::<CaptureHistory>().0.lock().map_err(|_| "Capture history unavailable")?
        .iter().find(|item| item.id == id).cloned().ok_or("Capture not found")?;
    std::thread::spawn(move || {
        if let Err(error) = recognize_and_copy_capture(&app, &item) { report_capture_error(&app, &error); }
    });
    Ok(())
}

fn open_ocr_window(app: &tauri::AppHandle, id: &str) -> Result<(), String> {
    if !app.state::<CaptureHistory>().0.lock().map_err(|_| "Capture history unavailable")?.iter().any(|item| item.id == id) {
        return Err("Capture not found".into());
    }
    let label = format!("ocr-{id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html?view=ocr".into()))
        .title("PureShot — Extract Text").inner_size(900.0, 650.0)
        .min_inner_size(600.0, 440.0).center().build().map_err(|e| e.to_string())?;
    Ok(())
}

fn restore_main(app: &tauri::AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
}

fn report_capture_error(app: &tauri::AppHandle, error: &str) {
    restore_main(app);
    eprintln!("Capture failed: {error}");
    let _ = app.emit("capture-error", error);
}

#[tauri::command]
async fn capture_screen(app: tauri::AppHandle, mode: String) -> Result<CaptureItem, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || capture_screen_blocking(handle, mode))
        .await.map_err(|error| error.to_string())?
}

fn capture_screen_blocking(app: tauri::AppHandle, mode: String) -> Result<CaptureItem, String> {
    let _guard = CaptureGuard::acquire()?;
    if !matches!(mode.as_str(), "fullscreen" | "window") {
        return Err("Unknown capture mode".into());
    }
    let result = capture_screen_inner(&app, &mode);
    if let Err(error) = &result {
        if error != "Screenshot cancelled" { report_capture_error(&app, error); }
    }
    result
}

fn capture_screen_inner(app: &tauri::AppHandle, mode: &str) -> Result<CaptureItem, String> {
    // Fullscreen is the stable engine used by the first milestone. Area/window selectors
    // layer on top of this frozen frame in the next native capture module.
    if let Some(main) = app.get_webview_window("main") {
        let was_visible = main.is_visible().unwrap_or(false);
        if was_visible {
            main.hide().map_err(|e| e.to_string())?;
            std::thread::sleep(std::time::Duration::from_millis(280));
        }
    }
    #[cfg(windows)]
    let dynamic = {
        let settings = capture_settings(app);
        #[cfg(windows)]
        let _icons = desktop_icons::DesktopIconsGuard::hide(settings.hide_desktop_icons)?;
        if settings.hide_desktop_icons { std::thread::sleep(std::time::Duration::from_millis(120)); }
        let shot = capture_overlay::capture_full_screen().map_err(|e| e.to_string())?;
        let rgba = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba)
            .ok_or("Invalid capture buffer")?;
        image::DynamicImage::ImageRgba8(rgba)
    };
    #[cfg(target_os = "linux")]
    let dynamic = {
        let settings = capture_settings(app);
        tauri::async_runtime::block_on(linux_capture::capture_with_options(mode, linux_capture::SelectionOptions {
            show_crosshair: settings.show_crosshair,
            show_magnifier: settings.show_magnifier,
            freeze_screen: settings.freeze_screen,
            hide_desktop_icons: settings.hide_desktop_icons,
        }))?.ok_or("Screenshot cancelled")?
    };
    #[cfg(any(windows, target_os = "linux"))]
    { save_image(app, dynamic, mode) }
    #[cfg(not(any(windows, target_os = "linux")))]
    { Err("Capture is not supported on this platform".into()) }
}

#[tauri::command]
fn get_capture_history(app: tauri::AppHandle) -> Result<Vec<CaptureItem>, String> {
    let history_state = app.state::<CaptureHistory>();
    let mut items = history_state.0.lock().map_err(|_| "Capture history unavailable")?.clone();
    for item in &mut items {
        if item.data_url.is_empty() {
            if let Ok(bytes) = fs::read(&item.path) {
                item.data_url = format!("data:image/png;base64,{}", STANDARD.encode(bytes));
            }
        }
    }
    Ok(items)
}

#[tauri::command(async)]
fn restore_history_capture(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let history_state = app.state::<CaptureHistory>();
    let item = history_state.0.lock().map_err(|_| "Capture history unavailable")?.iter().find(|item| item.id == id).cloned().ok_or("Capture not found")?;
    if let Some(state) = app.try_state::<LatestCapture>() { *state.0.lock().map_err(|_| "Capture state unavailable")? = Some(item.clone()); }
    let settings = capture_settings(&app);
    if let Some(window) = app.get_webview_window("history") { let _ = window.close(); }
    show_capture_overlay(app, item.id, settings.corner_radius, settings.preview_timeout_secs)
}

#[tauri::command]
fn delete_history_capture(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let history_state = app.state::<CaptureHistory>();
    let mut history = history_state.0.lock().map_err(|_| "Capture history unavailable")?;
    if let Some(index) = history.iter().position(|item| item.id == id) {
        let item = history.remove(index);
        fs::remove_file(item.path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn get_latest_capture(app: tauri::AppHandle) -> Result<Option<CaptureItem>, String> {
    Ok(app
        .state::<LatestCapture>()
        .0
        .lock()
        .map_err(|_| "Capture state unavailable")?
        .clone())
}

#[tauri::command]
fn get_launch_at_startup(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
fn set_launch_at_startup(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|error| error.to_string())?;
    } else if autostart.is_enabled().map_err(|error| error.to_string())? {
        autostart.disable().map_err(|error| error.to_string())?;
    }
    autostart.is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
fn update_capture_settings(
    app: tauri::AppHandle,
    corner_radius: u32,
    preview_timeout_secs: u32,
    copy_after_capture: bool,
    hide_desktop_icons: bool,
    show_quick_access: bool,
    quick_access_hover: bool,
    show_crosshair: bool,
    show_magnifier: bool,
    freeze_screen: bool,
) -> Result<(), String> {
    let state = app.state::<RuntimeSettings>();
    let mut settings = state.0.lock().map_err(|_| "Capture settings unavailable")?;
    settings.corner_radius = corner_radius;
    settings.preview_timeout_secs = preview_timeout_secs;
    settings.copy_after_capture = copy_after_capture;
    settings.hide_desktop_icons = hide_desktop_icons;
    settings.show_quick_access = show_quick_access;
    settings.quick_access_hover = quick_access_hover;
    settings.show_crosshair = show_crosshair;
    settings.show_magnifier = show_magnifier;
    settings.freeze_screen = freeze_screen;
    Ok(())
}

#[tauri::command(async)]
fn show_automatic_capture_overlay(app: tauri::AppHandle, id: String, corner_radius: u32, preview_timeout_secs: u32) -> Result<(), String> {
    if !capture_settings(&app).show_quick_access { return Ok(()); }
    show_capture_overlay(app, id, corner_radius, preview_timeout_secs)
}

/// Shows the Quick Access Overlay for the capture with the given `id`.
///
/// This used to spawn a small transparent WebView window; it now renders a
/// native Win32 layered window instead (see `native_overlay_window.rs`), so
/// there is no `index.html?view=overlay` route involved anymore on Windows.
/// `corner_radius`/`preview_timeout_secs` come from the frontend because the
/// corresponding settings live in `localStorage`, not on the Rust side.
#[tauri::command(async)]
fn show_capture_overlay(
    app: tauri::AppHandle,
    id: String,
    corner_radius: u32,
    preview_timeout_secs: u32,
) -> Result<(), String> {
    let item = get_capture_history(app.clone())?.into_iter()
        .find(|item| item.id == id).ok_or("No capture available to preview")?;
    if item.id != id {
        return Err("The selected capture is no longer available".into());
    }

    #[cfg(windows)]
    {
        let (monitor_x, monitor_y, monitor_w, monitor_h, scale) = app
            .get_webview_window("main")
            .and_then(|main| main.current_monitor().ok().flatten())
            .map(|monitor| {
                let area = monitor.work_area();
                (
                    area.position.x,
                    area.position.y,
                    area.size.width as i32,
                    area.size.height as i32,
                    monitor.scale_factor(),
                )
            })
            .unwrap_or((0, 0, 1920, 1080, 1.0));

        let is_dark = app
            .get_webview_window("main")
            .and_then(|main| main.theme().ok())
            .map(|theme| matches!(theme, tauri::Theme::Dark))
            .unwrap_or(false);

        let decoded = image::open(&item.path)
            .map_err(|error| format!("Unable to decode preview screenshot: {error}"))?
            .to_rgba8();
        let (width, height) = (item.width, item.height);
        let rgba = decoded.into_raw();

        let pin_path = item.path.clone();
        let annotate_app = app.clone();
        let annotate_item = item.clone();
        let copy_app = app.clone();
        let copy_item = item.clone();
        let save_app = app.clone();
        let save_item = item.clone();

        native_overlay_window::create(
            rgba,
            item.path.clone().into(),
            width,
            height,
            corner_radius,
            preview_timeout_secs,
            monitor_x,
            monitor_y,
            monitor_w,
            monitor_h,
            scale,
            is_dark,
            capture_settings(&app).quick_access_hover,
            native_overlay_window::OverlayCallbacks {
                on_pin: Box::new(move || {
                    if let Ok(decoded) = image::open(&pin_path) {
                        let rgba = decoded.to_rgba8();
                        let (w, h) = (rgba.width(), rgba.height());
                        let _ = native_pin::create(rgba.into_raw(), w, h, corner_radius);
                    }
                }),
                on_annotate: Box::new(move || {
                    let _ = capture_action(
                        annotate_app.clone(),
                        "annotate".into(),
                        annotate_item.clone(),
                    );
                }),
                on_copy: Box::new(move || {
                    let _ = capture_action(copy_app.clone(), "copy".into(), copy_item.clone());
                }),
                on_save: Box::new(move || {
                    let _ = capture_action(save_app.clone(), "save".into(), save_item.clone());
                }),
            },
        )
    }
    #[cfg(target_os = "linux")]
    {
        quick_access::show(&app, &item, corner_radius, preview_timeout_secs)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = corner_radius;
        open_capture_webview(&app, &item, false, preview_timeout_secs)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_capture_webview(app: &tauri::AppHandle, item: &CaptureItem, pinned: bool, timeout: u32) -> Result<(), String> {
    let view = if pinned { "pin" } else { "overlay" };
    let label = format!("{view}-{}", item.id);
    if let Some(window) = app.get_webview_window(&label) {
        return window.set_focus().map_err(|error| error.to_string());
    }
    let ratio = item.height as f64 / item.width.max(1) as f64;
    let width = if pinned { (item.width as f64).clamp(240.0, 800.0) } else { 336.0 };
    // Quick Access is a fixed 16:10 canvas, independent of capture dimensions.
    // Native pins keep the original aspect ratio instead.
    let height = if pinned { (width * ratio + 40.0).clamp(180.0, 800.0) } else { width * 10.0 / 16.0 };
    let hover = capture_settings(app).quick_access_hover;
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("index.html?view={view}&timeout={timeout}&hover={hover}").into()))
        .title(if pinned { "PureShot Pin" } else { "PureShot Preview" })
        .inner_size(width, height)
        .skip_taskbar(!pinned)
        .decorations(pinned).resizable(pinned).always_on_top(true);
    if pinned {
        builder = builder.center();
    } else if let Some(monitor) = app.get_webview_window("main")
        .and_then(|main| main.current_monitor().ok().flatten()) {
        let work = monitor.work_area();
        let scale = monitor.scale_factor();
        let x = work.position.x as f64 / scale + (work.size.width as f64 / scale - width - 24.0).max(0.0);
        let y = work.position.y as f64 / scale + (work.size.height as f64 / scale - height - 24.0).max(0.0);
        builder = builder.position(x, y);
    }
    // Configure auxiliary window hints and parentage before mapping. The
    // utility/skip-taskbar hints cover X11; Wayland needs the parent below.
    #[cfg(target_os = "linux")]
    if !pinned {
        builder = builder.visible(false);
        // xdg-shell has no skip-taskbar or utility window role. Supply an
        // actual parent relationship so Mutter sees an auxiliary window.
        if let Some(main) = app.get_webview_window("main") {
            builder = builder.transient_for(&main).map_err(|error| error.to_string())?;
        }
    }
    let window = builder.build().map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    if !pinned {
        use gtk::prelude::*;
        let native = window.gtk_window().map_err(|error| error.to_string())?;
        native.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
        native.set_skip_taskbar_hint(true);
        native.set_skip_pager_hint(true);
        window.show().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn get_window_capture(window: tauri::WebviewWindow, app: tauri::AppHandle) -> Result<Option<CaptureItem>, String> {
    let label = window.label();
    let id = label.strip_prefix("overlay-").or_else(|| label.strip_prefix("pin-")).or_else(|| label.strip_prefix("annotate-")).or_else(|| label.strip_prefix("ocr-"));
    if let Some(id) = id {
        return Ok(get_capture_history(app)?.into_iter().find(|item| item.id == id));
    }
    get_latest_capture(app)
}

#[tauri::command(async)]
fn pin_capture(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    id: String,
    corner_radius: u32,
) -> Result<(), String> {
    let item = get_capture_history(app.clone())?.into_iter()
        .find(|item| item.id == id).ok_or("No capture available to pin")?;
    if item.id != id {
        return Err("The selected capture is no longer available".into());
    }

    #[cfg(windows)]
    {
        let decoded = image::open(&item.path)
            .map_err(|error| format!("Unable to decode pinned screenshot: {error}"))?
            .to_rgba8();
        native_pin::create(decoded.into_raw(), item.width, item.height, corner_radius)?;
        window.close().map_err(|error| error.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let decoded = image::open(&item.path)
            .map_err(|error| format!("Unable to decode pinned screenshot: {error}"))?
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        linux_pin::create(&app, decoded.into_raw(), width, height, corner_radius)?;
        if window.label() == "overlay-stack" {
            quick_access::dismiss_quick_access(app.clone(), id)?;
        } else if window.label().starts_with("overlay-") {
            window.close().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = corner_radius;
        open_capture_webview(&app, &item, true, 0)?;
        if window.label().starts_with("overlay-") {
            window.close().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[cfg(windows)]
fn disable_window_animations(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_TRANSITIONS_FORCEDISABLED, DWMSBT_TRANSIENTWINDOW,
    };
    let disabled: i32 = 1;
    let backdrop = DWMSBT_TRANSIENTWINDOW;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disabled as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&backdrop) as u32,
        );
    }
}

#[cfg(windows)]
fn install_history_escape(hwnd: windows::Win32::Foundation::HWND) -> Result<(), String> {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_ESCAPE},
            Shell::{DefSubclassProc, SetWindowSubclass, SUBCLASSPROC},
            WindowsAndMessaging::{DestroyWindow, WM_HOTKEY, WM_NCDESTROY},
        },
    };
    const SUBCLASS_ID: usize = 0x505348;
    const ESCAPE_HOTKEY_ID: i32 = 0x5053;
    unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if message == WM_HOTKEY && wparam.0 == ESCAPE_HOTKEY_ID as usize {
            // Close the native HWND immediately, bypassing system window
            // animation. This is the same direct destruction used by the
            // previous native history window.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        if message == WM_NCDESTROY {
            let _ = unsafe { UnregisterHotKey(Some(hwnd), ESCAPE_HOTKEY_ID) };
        }
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
    let proc: SUBCLASSPROC = Some(subclass);
    if !unsafe { SetWindowSubclass(hwnd, proc, SUBCLASS_ID, 0) }.as_bool() {
        return Err("Unable to install native Escape handler for history window".into());
    }
    if let Err(error) = unsafe { RegisterHotKey(Some(hwnd), ESCAPE_HOTKEY_ID, MOD_NOREPEAT, VK_ESCAPE.0 as u32) } {
        return Err(format!("Unable to register Escape for history window: {error}"));
    }
    Ok(())
}

fn show_history(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("history") {
        window.show().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }
    let (width, height, x, y) = app.get_webview_window("main").and_then(|window| window.current_monitor().ok().flatten()).map(|monitor| {
        // Use the monitor work area directly. Oversizing the window with a
        // negative x made Windows clamp the transparent WebView and exposed
        // the resize-frame width as a gap on the left.
        let area = monitor.work_area();
        // Fixed content-sized shelf, not a percentage of a tall monitor.
        let shelf_height = (220.0 * monitor.scale_factor()).round() as u32;
        (
            area.size.width as f64,
            shelf_height as f64,
            area.position.x as f64,
            (area.position.y + area.size.height as i32 - shelf_height as i32) as f64,
        )

    }).unwrap_or((1920.0, 324.0, 0.0, 756.0));
    #[cfg(windows)]
    let window = {
        let webview = WebviewWindowBuilder::new(app, "history", WebviewUrl::App("index.html?view=history".into()))
            .title("PureShot History").inner_size(width, height).position(x, y).decorations(false).resizable(false).transparent(true).always_on_top(true).skip_taskbar(true)
            .visible(false).background_color(tauri::window::Color(0, 0, 0, 0)).build().map_err(|error| error.to_string())?;
        disable_window_animations(webview.hwnd().map_err(|error| error.to_string())?);
        webview
    };
    #[cfg(not(windows))]
    let window = WebviewWindowBuilder::new(app, "history", WebviewUrl::App("index.html?view=history".into()))
        .title("PureShot History").inner_size(width, height).position(x, y).decorations(false).resizable(false).transparent(true).always_on_top(true).skip_taskbar(true)
        .background_color(tauri::window::Color(0, 0, 0, 0)).build().map_err(|error| error.to_string())?;
    #[cfg(windows)]
    install_history_escape(window.hwnd().map_err(|error| error.to_string())?)?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn open_annotation_window(app: &tauri::AppHandle, item: &CaptureItem) -> Result<(), String> {
    let label = format!("annotate-{}", item.id);
    if let Some(window) = app.get_webview_window(&label) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App("index.html?view=annotate".into()),
    )
    .title("PureShot Annotate")
    .visible(!cfg!(target_os = "linux"))
    .inner_size(1180.0, 780.0)
    .min_inner_size(760.0, 520.0)
    .center()
    .decorations(false)
    .background_color(tauri::window::Color(24, 25, 28, 255))
    .build()
    .map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    desktop_appearance::decorate(&window);
    #[cfg(not(target_os = "linux"))]
    let _ = window;
    Ok(())
}

#[tauri::command]
fn close_annotation_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.close().map_err(|error| error.to_string())
}

#[tauri::command(async)]
fn pin_annotation(app: tauri::AppHandle, data_url: String, corner_radius: u32) -> Result<(), String> {
    let encoded = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or("Annotation data must be a PNG data URL")?;
    let png = STANDARD
        .decode(encoded)
        .map_err(|error| format!("Unable to decode annotation: {error}"))?;
    let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|error| format!("Invalid annotation image: {error}"))?
        .to_rgba8();
    #[cfg(windows)]
    {
        let (width, height) = decoded.dimensions();
        native_pin::create(decoded.into_raw(), width, height, corner_radius)
    }
    #[cfg(target_os = "linux")]
    {
        let (width, height) = decoded.dimensions();
        linux_pin::create(&app, decoded.into_raw(), width, height, corner_radius)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = corner_radius;
        let item = save_image_without_copy(&app, image::DynamicImage::ImageRgba8(decoded), "annotation")?;
        open_capture_webview(&app, &item, true, 0)
    }
}

#[tauri::command]
fn copy_annotation(app: tauri::AppHandle, data_url: String) -> Result<(), String> {
    let encoded = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or("Annotation data must be a PNG data URL")?;
    let png = STANDARD
        .decode(encoded)
        .map_err(|error| format!("Unable to decode annotation: {error}"))?;
    let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|error| format!("Invalid annotation image: {error}"))?
        .to_rgba8();
    let clipboard_image = Image::new(decoded.as_raw(), decoded.width(), decoded.height());
    app.clipboard()
        .write_image(&clipboard_image)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_annotation(app: tauri::AppHandle, item: CaptureItem, data_url: String) -> Result<String, String> {
    let encoded = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or("Annotation data must be a PNG data URL")?;
    let png = STANDARD
        .decode(encoded)
        .map_err(|error| format!("Unable to decode annotation: {error}"))?;
    let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|error| format!("Invalid annotation image: {error}"))?;
    let format = get_default_format(app.clone())?;
    let bytes = if format == ExportFormat::Png { png } else { format.encode(&decoded)? };
    let destination = unique_annotation_path(&app, &item, format)?;
    fs::write(&destination, bytes).map_err(|error| {
        format!(
            "Unable to save annotation to {}: {error}",
            destination.display()
        )
    })?;
    Ok(destination.to_string_lossy().into_owned())
}

#[tauri::command(async)]
fn capture_action(app: tauri::AppHandle, action: String, item: CaptureItem) -> Result<(), String> {
    match action.as_str() {
        "delete" => fs::remove_file(&item.path).map_err(|e| e.to_string()),
        "close-overlay" => {
            quick_access::clear(&app)?;
            for (label, window) in app.webview_windows() {
                if label.starts_with("overlay-") && label != "overlay-stack" {
                    let _ = window.close();
                }
            }
            Ok(())
        }
        "close-pin" => {
            #[cfg(target_os = "linux")]
            linux_pin::close_all(&app)?;
            for (label, window) in app.webview_windows() {
                if label.starts_with("pin-") {
                    let _ = window.close();
                }
            }
            Ok(())
        }
        "copy" => copy_capture_to_clipboard(&app, &item),
        "save" => {
            let format = get_default_format(app.clone())?;
            let bytes = if format == ExportFormat::Png {
                fs::read(&item.path).map_err(|error| format!("Unable to read screenshot: {error}"))?
            } else {
                let decoded = image::open(&item.path).map_err(|error| format!("Unable to decode screenshot: {error}"))?;
                format.encode(&decoded)?
            };
            let destination = unique_export_path(&app, &item, format)?;
            fs::write(&destination, bytes)
                .map_err(|error| {
                    format!(
                        "Unable to save screenshot to {}: {error}",
                        destination.display()
                    )
                })
        }
        "annotate" => open_annotation_window(&app, &item),
        _ => Err(format!("Unknown action: {action}")),
    }
}

fn start_capture_from_shortcut(app: tauri::AppHandle, mode: &'static str) {
    if mode == "ocr" {
        // OCR is a shortcut action, not a screenshot CaptureMode/frontend event.
        if let Err(error) = start_ocr_capture(app.clone()) { report_capture_error(&app, &error); }
        return;
    }
    if mode == "area" {
        #[cfg(target_os = "linux")]
        {
            let settings = app.state::<RuntimeSettings>().0.lock()
                .map(|settings| *settings).ok();
            if let Some(settings) = settings {
                if let Err(error) = start_area_capture(app.clone(), settings.corner_radius, settings.preview_timeout_secs) {
                    eprintln!("Area shortcut ignored: {error}");
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
            let _ = window.emit("shortcut-capture", mode);
        }
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("shortcut-capture", mode);
    }
}

pub fn run() {
    let context = tauri::generate_context!();
    hardware_acceleration::configure(&context);
    tauri::Builder::default()
        .manage(ocr::OcrState::default())
        .manage(LatestCapture(Mutex::new(None)))
        .manage(CaptureHistory(Mutex::new(load_cached_history())))
        .manage(RuntimeSettings(Mutex::new(CaptureSettings::default())))
        .manage(PendingCliCapture(Mutex::new(None)))
        .manage(quick_access::State::default())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(command) = args.iter().find(|arg| arg.starts_with("capture-")) {
                let action = match command.as_str() {
                    "capture-region" => Some("area"),
                    "capture-fullscreen" => Some("fullscreen"),
                    "capture-window" => Some("window"),
                    "capture-ocr" => Some("ocr"),
                    _ => None,
                };
                if let Some(action) = action {
                    if let Ok(mut state) = app.state::<PendingCliCapture>().0.lock() { *state = Some(action.into()); }
                    let _ = app.emit("cli-capture", ());
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::Builder::new().app_name("PureShot").args(["--autostart"]).build())
        .invoke_handler(tauri::generate_handler![
            get_desktop_theme,
            quick_access::get_quick_access,
            quick_access::dismiss_quick_access,
            get_ocr_status,
            install_ocr_models,
            extract_capture_text,
            copy_ocr_text,
            start_ocr_capture,
            show_ocr_capture,
            capture_screen,
            start_area_capture,
            get_latest_capture,
            get_window_capture,
            get_capture_history,
            restore_history_capture,
            delete_history_capture,
            update_capture_settings,
            get_launch_at_startup,
            set_launch_at_startup,
            get_show_tray_icon,
            set_show_tray_icon,
            get_default_format,
            set_default_format,
            get_filename_template,
            set_filename_template,
            preview_filename_template,
            get_capture_shortcuts,
            set_capture_shortcut,
            get_export_directory,
            choose_export_directory,
            show_capture_overlay,
            show_automatic_capture_overlay,
            hardware_acceleration::get_hardware_acceleration,
            hardware_acceleration::set_hardware_acceleration,
            pin_capture,
            close_annotation_window,
            pin_annotation,
            copy_annotation,
            save_annotation,
            capture_action,
            process_cli_capture
        ])
        .setup(|app| {
            if let Some(command) = std::env::args().find(|arg| arg.starts_with("capture-")) {
                let action = match command.as_str() {
                    "capture-region" => Some("area"),
                    "capture-fullscreen" => Some("fullscreen"),
                    "capture-window" => Some("window"),
                    "capture-ocr" => Some("ocr"),
                    _ => None,
                };
                if let Some(action) = action {
                    if let Ok(mut state) = app.state::<PendingCliCapture>().0.lock() { *state = Some(action.into()); }
                }
            }
            #[cfg(target_os = "linux")]
            {
                app.manage(desktop_appearance::Appearance(Mutex::new(None)));
                desktop_appearance::start(app.handle());
                if let Some(window) = app.get_webview_window("main") {
                    desktop_appearance::decorate(&window);
                    let _ = window.set_background_color(Some(tauri::window::Color(255, 255, 255, 255)));
                }
            }
            let export_dir = export_directory_setting_path(app.handle())
                .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
                .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).map_err(|error| error.to_string()))
                .ok().filter(|path| path.is_absolute()).unwrap_or_else(|| capture_dir(app.handle()));
            app.manage(ExportDirectory(Mutex::new(export_dir)));
            let format = default_format_path(app.handle())
                .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
                .and_then(|bytes| serde_json::from_slice::<ExportFormat>(&bytes).map_err(|error| error.to_string()))
                .unwrap_or_default();
            app.manage(DefaultFormat(Mutex::new(format)));
            let template = filename_template_path(app.handle())
                .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
                .and_then(|bytes| serde_json::from_slice::<String>(&bytes).map_err(|error| error.to_string()))
                .and_then(|template| validate_filename_template(&template).map(|_| template))
                .unwrap_or_else(|_| filename::DEFAULT_TEMPLATE.to_string());
            app.manage(FilenameTemplate(Mutex::new(template)));
            let capture_area = MenuItem::with_id(
                app,
                "capture-area",
                "Capture Area",
                true,
                None::<&str>,
            )?;
            let capture_window = MenuItem::with_id(
                app,
                "capture-window",
                "Capture Window",
                true,
                None::<&str>,
            )?;
            let capture_fullscreen = MenuItem::with_id(
                app,
                "capture-fullscreen",
                "Capture Fullscreen",
                true,
                None::<&str>,
            )?;
            let capture_text = MenuItem::with_id(app, "capture-text", "Capture OCR", true, None::<&str>)?;
            let history = MenuItem::with_id(app, "history", "History", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Open PureShot", true, None::<&str>)?;
            let open_folder = MenuItem::with_id(
                app,
                "open-folder",
                "Open Capture Folder",
                true,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Quit PureShot", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &capture_area,
                    &capture_window,
                    &capture_fullscreen,
                    &capture_text,
                    &tauri::menu::PredefinedMenuItem::separator(app)?,
                    &history,
                    &show,
                    &open_folder,
                    &tauri::menu::PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;
            let tray = TrayIconBuilder::with_id("main-tray")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .expect("application icon"),
                )
                .tooltip("PureShot")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "capture-area" => {
                        let settings = capture_settings(app);
                        let app = app.clone();
                        let _ = start_area_capture(
                            app,
                            settings.corner_radius,
                            settings.preview_timeout_secs,
                        );
                    }
                    "capture-window" => {
                        let settings = capture_settings(app);
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let result = capture_screen_blocking(app.clone(), "window".into()).and_then(|item| {
                                show_automatic_capture_overlay(
                                    app.clone(),
                                    item.id,
                                    settings.corner_radius,
                                    settings.preview_timeout_secs,
                                )
                            });
                            if let Err(error) = result { if error != "Screenshot cancelled" { report_capture_error(&app, &error); } }
                        });
                    }
                    "capture-fullscreen" => {
                        let settings = capture_settings(app);
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let result =
                                capture_screen_blocking(app.clone(), "fullscreen".into()).and_then(|item| {
                                    show_automatic_capture_overlay(
                                        app.clone(),
                                        item.id,
                                        settings.corner_radius,
                                        settings.preview_timeout_secs,
                                    )
                                });
                            if let Err(error) = result { if error != "Screenshot cancelled" { report_capture_error(&app, &error); } }
                        });
                    }
                    "capture-text" => {
                        if let Err(error) = start_ocr_capture(app.clone()) { report_capture_error(app, &error); }
                    }
                    "history" => {
                        if let Err(error) = show_history(app) { eprintln!("Unable to show history: {error}"); }
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "open-folder" => {
                        match export_directory(app) {
                            Ok(dir) => {
                                #[cfg(windows)]
                                let opener = "explorer";
                                #[cfg(target_os = "linux")]
                                let opener = "xdg-open";
                                #[cfg(not(any(windows, target_os = "linux")))]
                                let opener = "open";
                                if let Err(error) = std::process::Command::new(opener).arg(dir).spawn() {
                                    eprintln!("Unable to open export folder: {error}");
                                }
                            }
                            Err(error) => eprintln!("Unable to open export folder: {error}"),
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app);
            let show_tray_icon = load_tray_visibility(app.handle()).unwrap_or_else(|error| {
                eprintln!("Unable to load tray setting: {error}");
                true
            });
            match tray {
                Ok(tray) => { if let Err(error) = tray.set_visible(show_tray_icon) { eprintln!("Tray visibility unavailable: {error}"); } }
                Err(error) => eprintln!("System tray unavailable; use app capture controls: {error}"),
            }
            app.manage(TrayVisibility(Mutex::new(show_tray_icon)));

            // Under Wayland (GNOME, etc.) the compositor owns key grabs, so a
            // client-side X11 grab like this plugin uses never sees the event.
            // Area capture gets a working Wayland path below; fullscreen/window
            // keep this best-effort registration (it still works under X11).
            let shortcuts = load_capture_shortcuts(app.handle());
            if let Err(error) = apply_global_shortcuts(app.handle(), &shortcuts) {
                eprintln!("Global shortcut registration unavailable (use tray or app controls): {error}");
            }
            // Push saved shortcuts to the extension so a custom combo set
            // before it was installed (or before window/fullscreen were
            // wired) still takes effect. Missing extension is non-fatal.
            #[cfg(target_os = "linux")]
            for mode in ["area", "window", "fullscreen", "ocr"] {
                if let Some(accelerator) = shortcuts.get(mode) {
                    if let Err(error) = sync_gnome_shortcut(mode, accelerator) {
                        eprintln!("GNOME shortcut sync for {mode} skipped: {error}");
                    }
                }
            }
            app.manage(CaptureShortcutState(Mutex::new(shortcuts)));
            // Wayland-reliable fallback for area capture: the GNOME companion
            // extension registers the real compositor keybinding and forwards
            // activation over D-Bus, since this process cannot grab keys itself.
            #[cfg(target_os = "linux")]
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    linux_capture::watch_shortcut(move |mode| {
                        let mode: &'static str = match mode.as_str() {
                            "fullscreen" => "fullscreen",
                            "window" => "window",
                            "area" => "area",
                            "ocr" => "ocr",
                            _ => return,
                        };
                        start_capture_from_shortcut(handle.clone(), mode);
                    }).await;
                });
            }
            // The main window starts hidden to avoid flashing settings at login.
            // Normal launches still open settings; autostart launches stay in the tray.
            if !std::env::args_os().any(|arg| arg == "--autostart") {
                if let Some(window) = app.get_webview_window("main") {
                    window.show()?;
                    window.set_focus()?;
                }
            }
            Ok(())
        })
        .run(context)
        .expect("error while running PureShot");
}
