//! Shared Linux preview shelf. The queue survives WebView startup so captures
//! cannot be lost between creating the window and installing its listener.
use std::sync::Mutex;
use serde::Serialize;
use tauri::{Emitter, Manager};
use crate::CaptureItem;

const LABEL: &str = "overlay-stack";
const WIDTH: f64 = 336.0;
const SHADOW_PADDING: f64 = 24.0;
const WINDOW_WIDTH: f64 = WIDTH + SHADOW_PADDING * 2.0;
const CARD_HEIGHT: f64 = 210.0;
const GAP: f64 = 12.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    item: CaptureItem,
    timeout: u32,
    corner_radius: u32,
    hover: bool,
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct Shelf {
    revision: u64,
    entries: Vec<Preview>,
    capacity: usize,
}

impl Shelf {
    fn push(&mut self, preview: Preview) {
        self.entries.retain(|entry| entry.item.id != preview.item.id);
        self.entries.push(preview);
        while self.entries.len() > self.capacity.max(1) { self.entries.remove(0); }
        self.revision += 1;
    }
}

#[derive(Default)]
pub(crate) struct State(Mutex<Shelf>);

#[tauri::command]
pub(crate) fn get_quick_access(app: tauri::AppHandle) -> Result<Shelf, String> {
    app.state::<State>().0.lock().map(|shelf| shelf.clone()).map_err(|_| "Preview shelf unavailable".into())
}

#[tauri::command(async)]
pub(crate) fn dismiss_quick_access(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<State>();
    let mut shelf = state.0.lock().map_err(|_| "Preview shelf unavailable")?;
    shelf.entries.retain(|entry| entry.item.id != id);
    shelf.revision += 1;
    publish(&app, &shelf)
}

pub(crate) fn clear(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<State>();
    let mut shelf = state.0.lock().map_err(|_| "Preview shelf unavailable")?;
    shelf.entries.clear();
    shelf.revision += 1;
    publish(app, &shelf)
}

fn publish(app: &tauri::AppHandle, shelf: &Shelf) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.emit("quick-access-changed", shelf).map_err(|e| e.to_string())?;
        #[cfg(target_os = "linux")]
        {
            let handle = window.clone();
            let count = shelf.entries.len();
            let revision = shelf.revision;
            window.run_on_main_thread(move || {
                use gtk::prelude::*;
                let height = (count.max(1) as f64 * (CARD_HEIGHT + GAP) - GAP + SHADOW_PADDING * 2.0) as i32;
                let shrinking = handle.gtk_window().map(|native| native.allocated_height() > height).unwrap_or(false);
                let update = move || {
                // A newer capture/dismiss supersedes a deferred resize.
                if handle.app_handle().state::<State>().0.lock().map(|shelf| shelf.revision != revision).unwrap_or(true) { return; }
                // Restrict native input to cards, not the transparent space
                // above them or the gaps between them (also works on Wayland).
                if let Ok(native) = handle.gtk_window() {
                    // Do not reserve a maximum-height transparent surface:
                    // compositors may still apply shadows/blur to its bounds.
                    // GTK's default size is also a minimum for non-resizable
                    // Wayland windows, so update both when the shelf shrinks.
                    native.set_default_size(WINDOW_WIDTH as i32, height);
                    native.resize(WINDOW_WIDTH as i32, height);
                    if let Ok(Some(monitor)) = handle.current_monitor() {
                        let work = monitor.work_area();
                        let scale = monitor.scale_factor();
                        let x = work.position.x as f64 / scale + (work.size.width as f64 / scale - WINDOW_WIDTH - 24.0).max(0.0);
                        let y = work.position.y as f64 / scale + (work.size.height as f64 / scale - height as f64 - 24.0).max(0.0);
                        let _ = handle.set_position(tauri::LogicalPosition::new(x, y));
                    }
                    native.realize();
                    if let Some(surface) = native.window() {
                        let region = gtk::cairo::Region::create();
                        for index in 0..count {
                            let y = height as f64 - SHADOW_PADDING - CARD_HEIGHT - index as f64 * (CARD_HEIGHT + GAP);
                            let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(SHADOW_PADDING as i32, y as i32, WIDTH as i32, CARD_HEIGHT as i32));
                        }
                        surface.input_shape_combine_region(&region, 0, 0);
                    }
                }
                if count == 0 { let _ = handle.hide(); }
                else { let _ = handle.show(); }
                };
                if shrinking && count > 0 {
                    // Keep the old surface until the cards finish moving;
                    // shrinking immediately clips their transition mid-frame.
                    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), update);
                } else {
                    update();
                }
            }).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn show(app: &tauri::AppHandle, item: &CaptureItem, radius: u32, timeout: u32) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let state = app.state::<State>();
    let mut shelf = state.0.lock().map_err(|_| "Preview shelf unavailable")?;
    if app.get_webview_window(LABEL).is_none() {
        shelf.entries.clear();
        let monitor = app.get_webview_window("main").and_then(|main| main.current_monitor().ok().flatten());
        let (x, y, w, h) = monitor.map(|monitor| {
            let work = monitor.work_area();
            let scale = monitor.scale_factor();
            (work.position.x as f64 / scale, work.position.y as f64 / scale,
             work.size.width as f64 / scale, work.size.height as f64 / scale)
        }).unwrap_or((0.0, 0.0, 1920.0, 1080.0));
        shelf.capacity = (((h - 48.0 - SHADOW_PADDING * 2.0 + GAP) / (CARD_HEIGHT + GAP)).floor() as usize).clamp(1, 5);
        let height = CARD_HEIGHT + SHADOW_PADDING * 2.0;
        let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?view=overlay-stack".into()))
            .title("PureShot Preview").inner_size(WINDOW_WIDTH, height)
            .position(x + (w - WINDOW_WIDTH - 24.0).max(0.0), y + (h - height - 24.0).max(0.0))
            .decorations(false).resizable(false).transparent(true)
            .background_color(tauri::window::Color(0, 0, 0, 0))
            .always_on_top(true).skip_taskbar(true).focused(false).visible(false);
        if let Some(main) = app.get_webview_window("main") {
            builder = builder.transient_for(&main).map_err(|e| e.to_string())?;
        }
        let window = builder.build().map_err(|e| e.to_string())?;
        let native_window = window.clone();
        window.run_on_main_thread(move || {
            use gtk::prelude::*;
            if let Ok(native) = native_window.gtk_window() {
                // Transparent WebKit content alone does not remove GTK theme
                // borders/shadows: those outline the entire rectangular shelf,
                // including the otherwise transparent gaps between cards.
                native.set_widget_name("pureshot-preview-shelf");
                native.set_app_paintable(true);
                let css = gtk::CssProvider::new();
                if let Err(error) = css.load_from_data(b"#pureshot-preview-shelf, #pureshot-preview-shelf decoration { background-color: transparent; background-image: none; border: none; box-shadow: none; outline: none; }") {
                    eprintln!("Unable to style preview shelf: {error}");
                } else {
                    native.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                }
                // Let WebKit own painting. Clearing the parent surface on
                // every GTK draw can expose a blank frame while WebKit keeps
                // its child surface cached during a shelf resize.
                native.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
                native.set_skip_taskbar_hint(true);
                native.set_skip_pager_hint(true);
                native.set_accept_focus(false);
                // Wayland app_id is per surface, unlike GtkApplication's
                // process-wide ID. Set it before the first map so extension
                // rules can distinguish previews from Settings.
                native.realize();
                if let Some(surface) = native.window() {
                    if surface.display().type_().name().contains("Wayland") {
                        use gtk::glib::translate::ToGlibPtr;
                        unsafe extern "C" {
                            fn gdk_wayland_window_set_application_id(window: *mut gtk::gdk::ffi::GdkWindow, application_id: *const std::ffi::c_char);
                        }
                        unsafe {
                            gdk_wayland_window_set_application_id(surface.to_glib_none().0, c"com.pureshot.preview".as_ptr());
                        }
                    }
                }
                // WebKit can retain hover across transparent input-region
                // boundaries without delivering DOM leave events. Query the
                // actual native pointer, not keyboard focus or cached CSS state.
                let weak = native.downgrade();
                let events = native_window.clone();
                gtk::glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                    let Some(native) = weak.upgrade() else { return gtk::glib::ControlFlow::Break; };
                    let hovered = (|| {
                        if !native.is_visible() { return None; }
                        let surface = native.window()?;
                        let pointer = surface.display().default_seat()?.pointer()?;
                        // device_position's returned child can be None even
                        // inside WebKit's own GDK surface. Check the pointer's
                        // toplevel instead, then use shelf-relative coordinates.
                        let (under_pointer, _, _) = pointer.window_at_position();
                        if under_pointer?.toplevel() != surface.toplevel() { return None; }
                        let (_, x, y, _) = surface.device_position(&pointer);
                        if x < SHADOW_PADDING as i32 || x >= (SHADOW_PADDING + WIDTH) as i32 || y < SHADOW_PADDING as i32 || y >= native.allocated_height() - SHADOW_PADDING as i32 { return None; }
                        let from_bottom = native.allocated_height() - SHADOW_PADDING as i32 - 1 - y;
                        let stride = (CARD_HEIGHT + GAP) as i32;
                        if from_bottom % stride >= CARD_HEIGHT as i32 { return None; }
                        let shelf = events.app_handle().state::<State>();
                        let shelf = shelf.0.lock().ok()?;
                        shelf.entries.get((from_bottom / stride) as usize).map(|entry| entry.item.id.clone())
                    })();
                    // Reassert native state even if a delayed DOM event has
                    // since reintroduced stale hover after a window resize.
                    let _ = events.emit("quick-access-pointer-card", &hovered);
                    gtk::glib::ControlFlow::Continue
                });
            }
        }).map_err(|e| e.to_string())?;
    }
    let settings = crate::capture_settings(app);
    shelf.push(Preview { item: item.clone(), timeout, corner_radius: radius, hover: settings.quick_access_hover });
    publish(app, &shelf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(id: &str, timeout: u32) -> Preview {
        Preview {
            item: CaptureItem { id: id.into(), path: String::new(), data_url: String::new(), width: 100, height: 100, created_at: 0 },
            timeout, corner_radius: 13, hover: true,
        }
    }

    #[test]
    fn newest_is_last_and_only_oldest_overflow_is_evicted() {
        let mut shelf = Shelf { capacity: 2, ..Shelf::default() };
        shelf.push(preview("first", 0));
        shelf.push(preview("second", 5));
        shelf.push(preview("third", 10));
        assert_eq!(shelf.entries.iter().map(|entry| entry.item.id.as_str()).collect::<Vec<_>>(), ["second", "third"]);
        assert_eq!(shelf.entries[0].timeout, 5);
        assert_eq!(shelf.entries[1].timeout, 10);
        assert_eq!(shelf.revision, 3);
    }

    #[test]
    fn restoring_same_capture_makes_it_newest_without_duplicate() {
        let mut shelf = Shelf { capacity: 3, ..Shelf::default() };
        shelf.push(preview("first", 5));
        shelf.push(preview("second", 10));
        shelf.push(preview("first", 0));
        assert_eq!(shelf.entries.len(), 2);
        assert_eq!(shelf.entries[1].item.id, "first");
        assert_eq!(shelf.entries[1].timeout, 0);
    }
}
