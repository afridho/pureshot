//! Linux desktop integration lives on GTK's main thread. No subprocess polling,
//! extra runtime, or persisted application theme preference is needed.
use gtk::{gio, glib, prelude::*};
use tauri::{Emitter, Manager};
use std::{cell::RefCell, sync::Mutex};

pub struct Appearance(pub Mutex<Option<String>>);

thread_local! {
    // Keep signal sources alive for the application's lifetime.
    static WATCHERS: RefCell<Option<Watchers>> = const { RefCell::new(None) };
}
struct Watchers {
    _interface: Option<gio::Settings>,
    _portal: Option<gio::DBusProxy>,
}

fn settings(schema: &str) -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(schema, true)?;
    Some(gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None))
}

fn fallback(interface: Option<&gio::Settings>) -> Option<String> {
    if let Some(settings) = interface {
        if settings.settings_schema().is_some_and(|schema| schema.has_key("color-scheme")) {
            match settings.string("color-scheme").as_str() {
                "prefer-dark" => return Some("dark".into()),
                "prefer-light" => return Some("light".into()),
                _ => {}
            }
        }
        if settings.settings_schema().is_some_and(|schema| schema.has_key("gtk-theme")) {
            return Some(if settings.string("gtk-theme").to_lowercase().contains("dark") { "dark" } else { "light" }.into());
        }
    }
    gtk::Settings::default().map(|s| if s.is_gtk_application_prefer_dark_theme() { "dark" } else { "light" }.into())
}

fn apply(app: &tauri::AppHandle, theme: Option<String>) {
    // Observe the desktop preference; never override GTK's process-wide theme.
    // Doing so also restyles libappindicator's tray menu and can break custom
    // GTK themes. Native decorations already inherit the desktop GTK theme.
    if let Some(state) = app.try_state::<Appearance>() {
        if let Ok(mut current) = state.0.lock() { *current = theme.clone(); }
    }
    let _ = app.emit("desktop-theme-changed", theme);
}

/// The settings window uses a single SVG minimize button in the WebView.
/// Keep native desktop decorations only for secondary editor windows.
pub fn decorate(window: &tauri::WebviewWindow) {
    if window.label() == "main" { return; }
    let target = window.clone();
    if let Err(error) = window.run_on_main_thread(move || decorate_on_main(&target)) {
        eprintln!("Unable to install desktop title bar: {error}");
    }
}

fn decorate_on_main(window: &tauri::WebviewWindow) {
    if let Ok(native) = window.gtk_window() {
        let visible = native.is_visible();
        // Tao realizes even initially hidden windows. GTK only accepts a new
        // titlebar before realization; remap this regular window, not overlays.
        native.hide();
        native.unrealize();
        native.set_decorated(true);
        let header = gtk::HeaderBar::new();
        header.set_show_close_button(true);
        header.set_title(Some(&window.title().unwrap_or_else(|_| "PureShot".into())));
        if let Some(wm) = settings("org.gnome.desktop.wm.preferences") {
            if wm.settings_schema().is_some_and(|schema| schema.has_key("button-layout")) {
                header.set_decoration_layout(Some(wm.string("button-layout").as_str()));
                let weak = header.downgrade();
                wm.connect_changed(Some("button-layout"), move |settings, _| {
                    if let Some(header) = weak.upgrade() {
                        header.set_decoration_layout(Some(settings.string("button-layout").as_str()));
                    }
                });
                // Retain the Settings object with the header without unsafe data.
                header.connect_destroy(move |_| { let _ = &wm; });
            }
        }
        native.set_titlebar(Some(&header));
        header.show_all();
        if visible || window.label().starts_with("annotate-") { native.show(); }
    }
}

pub fn start(app: &tauri::AppHandle) {
    let interface = settings("org.gnome.desktop.interface");
    apply(app, fallback(interface.as_ref()));
    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        // The portal is the authoritative preference on Wayland, including
        // sandboxed desktops. GNOME GSettings remains a clean fallback.
        let portal = gio::DBusProxy::for_bus_future(
            gio::BusType::Session, gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
            None, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        ).await.ok();
        let preferred = std::rc::Rc::new(RefCell::new(None::<String>));
        if let Some(ref settings) = interface {
            let app = app.clone();
            let preferred = preferred.clone();
            settings.connect_changed(None, move |settings, key| {
                if key == "color-scheme" || key == "gtk-theme" {
                    apply(&app, preferred.borrow().clone().or_else(|| fallback(Some(settings))));
                }
            });
        }
        let Some(portal) = portal else {
            // Retain fallback notifications even without a session bus/portal.
            WATCHERS.with(|watchers| *watchers.borrow_mut() = Some(Watchers { _interface: interface, _portal: None }));
            return;
        };
        let events = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let signal_events = events.clone();
        let app_signal = app.clone();
        let preference_signal = preferred.clone();
        let interface_signal = interface.clone();
        // GObject's local signal connection keeps GTK objects on this thread.
        portal.connect_local("g-signal", false, move |args| {
            let signal = args[2].get::<String>().ok()?;
            if signal != "SettingChanged" { return None; }
            let parameters = args[3].get::<glib::Variant>().ok()?;
            if let Some((namespace, key, value)) = parameters.get::<(String, String, glib::Variant)>() {
                if namespace == "org.freedesktop.appearance" && key == "color-scheme" {
                    signal_events.set(signal_events.get() + 1);
                    let theme = portal_theme(&value);
                    *preference_signal.borrow_mut() = theme.clone();
                    apply(&app_signal, theme.or_else(|| fallback(interface_signal.as_ref())));
                }
            }
            None
        });
        let parameters = ("org.freedesktop.appearance", "color-scheme").to_variant();
        let revision = events.get();
        if let Ok(result) = portal.call_future("Read", Some(&parameters), gio::DBusCallFlags::NONE, 1500).await {
            if let Some((value,)) = result.get::<(glib::Variant,)>().filter(|_| events.get() == revision) {
                let theme = portal_theme(&value);
                *preferred.borrow_mut() = theme.clone();
                apply(&app, theme.or_else(|| fallback(interface.as_ref())));
            }
        }
        WATCHERS.with(|watchers| *watchers.borrow_mut() = Some(Watchers { _interface: interface, _portal: Some(portal) }));
    });
}

fn portal_theme(value: &glib::Variant) -> Option<String> {
    // Some portal backends wrap Read's variant once more than the signature.
    let mut value = value.clone();
    // as_variant() calls g_variant_get_variant(), which emits a GLib critical
    // for non-variant types; None is not a safe end-of-unwrapping check.
    while value.is::<glib::Variant>() {
        value = value.as_variant()?;
    }
    match value.get::<u32>() {
        Some(1) => Some("dark".into()),
        Some(2) => Some("light".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::portal_theme;
    use gtk::{glib, prelude::*};

    #[test]
    fn portal_color_scheme_accepts_plain_and_nested_variants() {
        for (scheme, expected) in [(0u32, None), (1, Some("dark")), (2, Some("light")), (3, None)] {
            let mut value = scheme.to_variant();
            for _ in 0..3 {
                assert_eq!(portal_theme(&value).as_deref(), expected);
                value = glib::Variant::from_variant(&value);
            }
        }
    }

    #[test]
    fn portal_color_scheme_rejects_unexpected_types() {
        for value in ["dark".to_variant(), true.to_variant(), (-1i32).to_variant(), ().to_variant()] {
            assert_eq!(portal_theme(&value), None);
            assert_eq!(portal_theme(&glib::Variant::from_variant(&value)), None);
        }
    }
}
