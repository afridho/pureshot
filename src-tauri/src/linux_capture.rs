//! Still-image capture through the desktop's permission-aware portal.
//! Area capture selects on the live desktop inside the trusted GNOME companion extension.
//! Window capture uses the trusted companion's live window picker and camera cursor.
use ashpd::desktop::{screenshot::Screenshot, ResponseError};

#[zbus::proxy(
    interface = "org.pureshot.Capture",
    default_service = "org.pureshot.Capture",
    default_path = "/org/pureshot/Capture"
)]
trait NativeCapture {
    fn select_area_with_options(&self, corner_radius: u32, options_json: &str) -> zbus::Result<String>;
    fn select_window_with_options(&self, options_json: &str) -> zbus::Result<String>;
    fn capture_with_options(&self, options_json: &str) -> zbus::Result<String>;

    #[zbus(signal)]
    fn shortcut_activated(&self, mode: &str) -> zbus::Result<()>;
}

/// Global shortcuts don't reach app windows under Wayland (the compositor
/// owns key grabs, not individual clients). The GNOME companion extension
/// runs inside Mutter itself, registers the real keybinding there, and
/// forwards activations over this signal instead.
pub async fn watch_shortcut<F>(on_activated: F)
where
    F: Fn(String) + Send + 'static,
{
    loop {
        if let Err(error) = watch_shortcut_once(&on_activated).await {
            eprintln!("PureShot extension shortcut listener unavailable, retrying: {error}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn watch_shortcut_once<F>(on_activated: &F) -> Result<(), String>
where
    F: Fn(String),
{
    let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
    let proxy = NativeCaptureProxy::new(&connection).await.map_err(|error| error.to_string())?;
    let mut stream = proxy.receive_shortcut_activated().await.map_err(|error| error.to_string())?;
    use futures_util::StreamExt;
    while let Some(signal) = stream.next().await {
        if let Ok(args) = signal.args() {
            on_activated(args.mode().to_string());
        }
    }
    Err("Signal stream ended".into())
}

/// Per-request compositor preferences. No settings are persisted in the extension.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct SelectionOptions {
    pub show_crosshair: bool,
    pub show_magnifier: bool,
    pub freeze_screen: bool,
    pub hide_desktop_icons: bool,
}

impl Default for SelectionOptions {
    fn default() -> Self {
        Self {
            show_crosshair: true,
            show_magnifier: true,
            freeze_screen: false,
            hide_desktop_icons: false,
        }
    }
}

/// Do not silently fall back to a picker or an older extension that ignores preferences.
pub async fn capture_area(corner_radius: u32, options: SelectionOptions) -> Result<Option<image::DynamicImage>, String> {
    let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
    let proxy = NativeCaptureProxy::new(&connection).await.map_err(|error| error.to_string())?;
    let options_json = serde_json::to_string(&options).map_err(|error| error.to_string())?;
    let path = proxy.select_area_with_options(corner_radius, &options_json).await.map_err(|error| format!(
        "PureShot GNOME extension unavailable or access denied: {error}. Install/update and enable gnome-extension/pureshot@afridho, authorize this PureShot executable using its installer, then log out/in to load updated extension code. No desktop screenshot picker was opened."
    ))?;
    read_extension_capture(path)
}

fn read_extension_capture(path: String) -> Result<Option<image::DynamicImage>, String> {
    if path.is_empty() { return Ok(None); }
    let path = std::path::PathBuf::from(path);
    let cache = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cache")))
        .ok_or("Unable to locate extension screenshot cache")?
        .join("pureshot-extension-captures");
    if !path.is_absolute() || path.parent() != Some(cache.as_path()) ||
        path.extension().and_then(|extension| extension.to_str()) != Some("png") {
        return Err("PureShot extension returned an invalid screenshot path".into());
    }
    let result = image::open(&path).map(Some)
        .map_err(|error| format!("Unable to read native screenshot: {error}"));
    // The extension owns a private temporary frame; the shared pipeline saves its own copy.
    if let Err(error) = std::fs::remove_file(&path) {
        eprintln!("Unable to remove temporary extension screenshot: {error}");
    }
    result
}

/// Window selection and icon suppression use the explicitly trusted companion.
/// Area-only preferences do not apply to fullscreen/window captures.
pub async fn capture_with_options(mode: &str, options: SelectionOptions) -> Result<Option<image::DynamicImage>, String> {
    if mode == "window" {
        let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
        let proxy = NativeCaptureProxy::new(&connection).await.map_err(|error| error.to_string())?;
        let json = serde_json::to_string(&options).map_err(|error| error.to_string())?;
        let path = proxy.select_window_with_options(&json).await.map_err(|error| format!(
            "PureShot GNOME window selection unavailable or access denied: {error}. Update/enable and authorize the companion extension, then log out/in. No desktop screenshot picker was opened."
        ))?;
        return read_extension_capture(path);
    }
    if options.hide_desktop_icons {
        if mode != "fullscreen" {
            return Err("Use capture_area for area selection".into());
        }
        let connection = zbus::Connection::session().await.map_err(|error| error.to_string())?;
        let proxy = NativeCaptureProxy::new(&connection).await.map_err(|error| error.to_string())?;
        let json = serde_json::to_string(&options).map_err(|error| error.to_string())?;
        let path = proxy.capture_with_options(&json).await.map_err(|error| format!(
            "PureShot GNOME fullscreen capture failed: {error}. Update/enable and authorize the companion extension; no portal fallback was opened."
        ))?;
        return read_extension_capture(path);
    }
    capture_fullscreen(mode).await
}

async fn capture_fullscreen(mode: &str) -> Result<Option<image::DynamicImage>, String> {
    if mode != "fullscreen" {
        return Err("Use capture_area with the configured radius for area selection".into());
    }
    let request = Screenshot::request()
        .interactive(false)
        .modal(true)
        .send()
        .await
        .map_err(|error| format!("Screenshot portal unavailable: {error}. Install xdg-desktop-portal and the backend for your desktop."))?;
    let response = match request.response() {
        Ok(response) => response,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(format!("Screenshot portal failed: {error}")),
    };
    let path = response.uri().to_file_path()
        .map_err(|_| "The screenshot portal returned a non-local image URI")?;
    // Copy pixels into PureShot's own cache; never delete a portal-owned file.
    image::open(path).map(Some).map_err(|error| format!("Unable to read portal screenshot: {error}"))
}
