//! Native GTK3 screenshot pins. All GTK objects live exclusively on Tauri's main thread.
//! `set_keep_above` is advisory: Wayland compositors (notably GNOME) ignore it;
//! keeping pins above other apps there requires a compositor extension/rule
//! (see `gnome-extension/`, which calls `Meta.Window.make_above()` on map for
//! windows titled "PureShot Pin"). We still call `set_keep_above` for X11 and
//! other compositors that do honor it, and re-assert it after state changes
//! Mutter is known to drop it on (losing then regaining focus).
use gtk::{gdk, gdk_pixbuf, glib, prelude::*};
use std::{cell::RefCell, rc::Rc, sync::mpsc};

/// Logical-pixel geometry of the hover-visible close button, top-right inset.
const CLOSE_BUTTON_MARGIN: f64 = 10.0;
const CLOSE_BUTTON_RADIUS: f64 = 11.0;

thread_local! {
    // GTK owns no Rust reference for an unparented toplevel. Retain each pin until destroy.
    static PINS: RefCell<Vec<gtk::Window>> = const { RefCell::new(Vec::new()) };
}

/// Call from a worker, not the GTK thread: waits only until the window is created.
pub fn create(
    app: &tauri::AppHandle,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    corner_radius: u32,
) -> Result<(), String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if width == 0
        || height == 0
        || width > (i32::MAX / 4) as u32
        || height > i32::MAX as u32
        || expected != Some(rgba.len())
    {
        return Err("Invalid RGBA screenshot dimensions or buffer length".into());
    }
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(create_on_main(rgba, width, height, corner_radius));
    })
    .map_err(|error| error.to_string())?;
    rx.recv()
        .map_err(|_| "Native pin creation did not return a result".to_string())?
}

/// Schedule closure without waiting, so this is also safe to call during main-thread shutdown.
pub fn close_all(app: &tauri::AppHandle) -> Result<(), String> {
    app.run_on_main_thread(|| {
        // Drop the registry borrow before close() emits destroy synchronously.
        let windows = PINS.with(|pins| std::mem::take(&mut *pins.borrow_mut()));
        for window in windows {
            window.close();
        }
    })
    .map_err(|error| error.to_string())
}

/// Uniform scale, rounded to whole logical pixels. Never distort a preview to fill its window.
fn fitted_size(image: (u32, u32), bounds: (i32, i32), requested_scale: f64) -> (i32, i32) {
    let max_scale =
        (bounds.0.max(1) as f64 / image.0 as f64).min(bounds.1.max(1) as f64 / image.1 as f64);
    let scale = requested_scale.max(0.0).min(max_scale);
    (
        (image.0 as f64 * scale).round().max(1.0) as i32,
        (image.1 as f64 * scale).round().max(1.0) as i32,
    )
}

fn monitor_bounds(window: &gtk::Window) -> Option<(i32, i32)> {
    let display = gdk::Display::default()?;
    let monitor = window
        .window()
        .and_then(|w| display.monitor_at_window(&w))
        .or_else(|| display.primary_monitor())
        .or_else(|| display.monitor(0))?;
    let rect = monitor.workarea();
    Some((rect.width().max(1), rect.height().max(1)))
}

fn reset_size(window: &gtk::Window, image: (u32, u32)) {
    if let Some((w, h)) = monitor_bounds(window) {
        let size = fitted_size(
            image,
            ((w as f64 * 0.7) as i32, (h as f64 * 0.7) as i32),
            1.0,
        );
        // For a non-resizable GTK window, the default size also acts as a
        // minimum on Wayland. Update it before requesting a smaller allocation.
        window.set_default_size(size.0, size.1);
        window.resize(size.0, size.1);
    }
}

fn resize_to_scale(window: &gtk::Window, image: (u32, u32), requested: f64) {
    if !requested.is_finite() || requested <= 0.0 {
        return;
    }
    if let Some((w, h)) = monitor_bounds(window) {
        let minimum = (32.0 / image.0.max(image.1) as f64).min(1.0);
        let size = fitted_size(
            image,
            ((w as f64 * 0.95) as i32, (h as f64 * 0.95) as i32),
            requested.max(minimum),
        );
        // For a non-resizable GTK window, the default size also acts as a
        // minimum on Wayland. Update it before requesting a smaller allocation.
        window.set_default_size(size.0, size.1);
        window.resize(size.0, size.1);
    }
}

fn create_on_main(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    corner_radius: u32,
) -> Result<(), String> {
    let display = gdk::Display::default().ok_or("No GTK display available")?;
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let screen = gtk::prelude::WidgetExt::screen(&window).ok_or("No GTK screen available")?;
    let visual = screen
        .rgba_visual()
        .ok_or("No RGBA visual available for native pin")?;
    window.set_visual(Some(&visual));
    window.set_app_paintable(true);
    window.set_title("PureShot Pin");
    window.set_decorated(false);
    window.set_keep_above(true);
    window.set_skip_taskbar_hint(true);
    window.set_type_hint(gdk::WindowTypeHint::Utility);
    // Resizing is deliberately controlled by scroll, rather than a compositor's free-aspect resize.
    window.set_resizable(false);
    let image = (width, height);
    let bounds = monitor_bounds(&window).ok_or("No monitor available")?;
    let initial = fitted_size(
        image,
        (
            (bounds.0 as f64 * 0.7) as i32,
            (bounds.1 as f64 * 0.7) as i32,
        ),
        1.0,
    );
    window.set_default_size(initial.0, initial.1);
    window.set_position(gtk::WindowPosition::Center);

    // Opacity is baked into every pixel we paint, not left to
    // `gtk_widget_set_opacity`'s per-surface alpha: the latter is a no-op on
    // several Wayland compositors (GTK3 only forwards it on backends that
    // implement `gdk_window_set_opacity`), while compositing it ourselves
    // works identically on every backend, matching the Windows pin.
    let opacity = Rc::new(RefCell::new(1.0_f64));
    // Image hover controls visibility; button hover controls its emphasis.
    // Drag/resize already
    // trigger their own `connect_draw` via GTK's size-allocate machinery.
    let hovered_pin = Rc::new(RefCell::new(false));
    let hovered_close = Rc::new(RefCell::new(false));

    let bytes = glib::Bytes::from_owned(rgba);
    let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
        &bytes,
        gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        width as i32,
        height as i32,
        width as i32 * 4,
    );
    let area = gtk::DrawingArea::new();
    // A zero natural size invokes GTK's 200px toplevel fallback. An explicit
    // one-pixel requisition lets small zoom levels allocate their actual size.
    area.set_size_request(1, 1);
    area.set_app_paintable(true);
    area.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::ENTER_NOTIFY_MASK
            | gdk::EventMask::LEAVE_NOTIFY_MASK
            | gdk::EventMask::SCROLL_MASK
            | gdk::EventMask::SMOOTH_SCROLL_MASK,
    );
    // The transient HUD stays legible outside the image's alpha compositing,
    // and passes input through so it never interferes with dragging or gestures.
    let overlay = gtk::Overlay::new();
    overlay.add(&area);
    let opacity_label = gtk::Label::new(Some("Opacity 100%"));
    opacity_label.set_halign(gtk::Align::Center);
    opacity_label.set_valign(gtk::Align::End);
    opacity_label.set_margin_bottom(8);
    opacity_label.set_no_show_all(true);
    overlay.add_overlay(&opacity_label);
    overlay.set_overlay_pass_through(&opacity_label, true);
    window.add(&overlay);

    // A translucent image cannot reveal the desktop if GTK first paints an
    // opaque themed background underneath it. Scope transparency to this pin.
    let css = gtk::CssProvider::new();
    css.load_from_data(b"* { background-color: transparent; background-image: none; }\n#pin-opacity-hud { background-color: rgba(24, 25, 28, 0.94); color: #ffffff; border-radius: 8px; padding: 4px 8px; }")
        .map_err(|error| error.to_string())?;
    opacity_label.set_widget_name("pin-opacity-hud");
    for widget in [
        window.clone().upcast::<gtk::Widget>(),
        overlay.clone().upcast(),
        area.clone().upcast(),
        opacity_label.clone().upcast(),
    ] {
        widget
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    window.connect_draw(|_, cr| {
        cr.set_operator(gtk::cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        let _ = cr.paint();
        cr.set_operator(gtk::cairo::Operator::Over);
        glib::Propagation::Proceed // GTK must still draw the overlay and controls.
    });
    let hud_timeout = Rc::new(RefCell::new(None::<glib::SourceId>));
    let update_opacity: Rc<dyn Fn(f64)> = {
        let opacity = opacity.clone();
        let area = area.downgrade();
        let label = opacity_label.downgrade();
        let hud_timeout = hud_timeout.clone();
        Rc::new(move |value| {
            if !value.is_finite() {
                return;
            }
            let value = value.clamp(0.2, 1.0);
            *opacity.borrow_mut() = value;
            if let Some(area) = area.upgrade() {
                area.queue_draw();
            }
            if let Some(label) = label.upgrade() {
                label.set_text(&format!("Opacity {:.0}%", value * 100.0));
                label.show();
                if let Some(source) = hud_timeout.borrow_mut().take() {
                    source.remove();
                }
                let label = label.downgrade();
                let pending = hud_timeout.clone();
                *hud_timeout.borrow_mut() = Some(glib::timeout_add_local_once(
                    std::time::Duration::from_millis(1200),
                    move || {
                        pending.borrow_mut().take();
                        if let Some(label) = label.upgrade() {
                            label.hide();
                        }
                    },
                ));
            }
        })
    };
    {
        let hud_timeout = hud_timeout.clone();
        window.connect_destroy(move |_| {
            if let Some(source) = hud_timeout.borrow_mut().take() {
                source.remove();
            }
        });
    }
    {
        let pixbuf = pixbuf.clone();
        let opacity = opacity.clone();
        let hovered_pin = hovered_pin.clone();
        let hovered_close = hovered_close.clone();
        area.connect_draw(move |area, cr| {
            // Clear previous contents, including antialiased rounded corners, with real transparency.
            cr.set_operator(gtk::cairo::Operator::Source);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            let _ = cr.paint();
            cr.set_operator(gtk::cairo::Operator::Over);
            let vw = area.allocated_width() as f64;
            let vh = area.allocated_height() as f64;
            let scale = (vw / width as f64).min(vh / height as f64);
            let (w, h) = (width as f64 * scale, height as f64 * scale);
            let (x, y) = ((vw - w) / 2.0, (vh - h) / 2.0);
            let radius = (corner_radius as f64 * scale).min(w / 2.0).min(h / 2.0);
            let op = *opacity.borrow();
            let _ = cr.save();
            rounded_rect(cr, x, y, w, h, radius);
            cr.clip();
            cr.translate(x, y);
            cr.scale(scale, scale);
            cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
            cr.source().set_filter(gtk::cairo::Filter::Best);
            let _ = cr.paint_with_alpha(op);
            let _ = cr.restore();
            if *hovered_pin.borrow() {
                draw_close_button(cr, x, y, w, h, *hovered_close.borrow());
            }
            glib::Propagation::Stop
        });
    }

    // Text-only menu: no icon assets or WebView are needed.
    let menu = gtk::Menu::new();
    let copy = gtk::MenuItem::with_label("Copy image");
    let reset = gtk::MenuItem::with_label("Reset size");
    let opacity_item = gtk::MenuItem::with_label("Opacity");
    let opacity_menu = gtk::Menu::new();
    for percent in [100, 80, 60, 40] {
        let item = gtk::MenuItem::with_label(&format!("{percent}%"));
        let update_opacity = update_opacity.clone();
        item.connect_activate(move |_| {
            update_opacity(percent as f64 / 100.0);
        });
        opacity_menu.append(&item);
    }
    opacity_item.set_submenu(Some(&opacity_menu));
    let close = gtk::MenuItem::with_label("Close");
    menu.append(&copy);
    menu.append(&reset);
    menu.append(&opacity_item);
    menu.append(&close);
    copy.connect_activate(move |_| {
        let clipboard = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
        clipboard.set_image(&pixbuf); // Original full-resolution RGBA, not the scaled preview.
        clipboard.store();
    });
    {
        let weak = window.downgrade();
        reset.connect_activate(move |_| {
            if let Some(window) = weak.upgrade() {
                reset_size(&window, image);
            }
        });
    }
    {
        let weak = window.downgrade();
        close.connect_activate(move |_| {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
    }
    menu.show_all();
    {
        let weak = window.downgrade();
        let hovered_pin = hovered_pin.clone();
        area.connect_button_press_event(move |area, event| {
            if let Some(window) = weak.upgrade() {
                if event.button() == 1
                    && *hovered_pin.borrow()
                    && close_button_hit(area, event.position(), image)
                {
                    // Hit-test on press, not release: matches the Windows pin
                    // and avoids a drag starting the moment the button is pressed.
                    window.close();
                    return glib::Propagation::Stop;
                }
                match event.button() {
                    1 if event.event_type() == gdk::EventType::DoubleButtonPress => {
                        reset_size(&window, image)
                    }
                    1 => {
                        let (x, y) = event.root();
                        window.begin_move_drag(1, x as i32, y as i32, event.time());
                    }
                    3 => menu.popup_at_pointer(Some(event)),
                    _ => return glib::Propagation::Proceed,
                }
            }
            glib::Propagation::Stop
        });
    }
    {
        let hovered_pin = hovered_pin.clone();
        let hovered_close = hovered_close.clone();
        area.connect_enter_notify_event(move |area, event| {
            *hovered_pin.borrow_mut() = true;
            *hovered_close.borrow_mut() = close_button_hit(area, event.position(), image);
            area.queue_draw();
            glib::Propagation::Proceed
        });
    }
    {
        let hovered_pin = hovered_pin.clone();
        let hovered_close = hovered_close.clone();
        area.connect_motion_notify_event(move |area, event| {
            // Motion is also a fallback when the compositor skips enter on map.
            let was_inside = std::mem::replace(&mut *hovered_pin.borrow_mut(), true);
            let now_hovered = close_button_hit(area, event.position(), image);
            let mut was_hovered = hovered_close.borrow_mut();
            if !was_inside || *was_hovered != now_hovered {
                *was_hovered = now_hovered;
                area.queue_draw();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let hovered_pin = hovered_pin.clone();
        let hovered_close = hovered_close.clone();
        area.connect_leave_notify_event(move |area, _| {
            let was_inside = std::mem::take(&mut *hovered_pin.borrow_mut());
            let was_close = std::mem::take(&mut *hovered_close.borrow_mut());
            if was_inside || was_close {
                area.queue_draw();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let weak = window.downgrade();
        let opacity = opacity.clone();
        let update_opacity = update_opacity.clone();
        area.connect_scroll_event(move |_, event| {
            if event.direction() == gdk::ScrollDirection::Smooth {
                let delta = event.delta().1;
                if delta.is_finite() && delta != 0.0 {
                    let current = *opacity.borrow();
                    update_opacity(current - delta.clamp(-4.0, 4.0) * 0.025);
                }
            } else if let Some(window) = weak.upgrade() {
                // Discrete wheel events retain the traditional resize behavior.
                let delta: f64 = match event.direction() {
                    gdk::ScrollDirection::Up => -1.0,
                    gdk::ScrollDirection::Down => 1.0,
                    _ => 0.0,
                };
                if delta != 0.0 {
                    let current = window.size().0 as f64 / width as f64;
                    resize_to_scale(&window, image, current * (-delta * 0.1).exp());
                }
            }
            glib::Propagation::Stop
        });
    }
    // GTK3 delivers touchpad pinch through GestureZoom on supporting backends
    // (notably Wayland); scale-changed is relative to the start of each gesture.
    let zoom = gtk::GestureZoom::new(&area);
    zoom.set_propagation_phase(gtk::PropagationPhase::Capture);
    let zoom_start = Rc::new(RefCell::new(1.0_f64));
    {
        let weak = window.downgrade();
        let zoom_start = zoom_start.clone();
        zoom.connect_begin(move |_, _| {
            if let Some(window) = weak.upgrade() {
                *zoom_start.borrow_mut() = window.size().0 as f64 / width as f64;
            }
        });
    }
    {
        let weak = window.downgrade();
        zoom.connect_scale_changed(move |_, scale| {
            if scale.is_finite() && scale > 0.0 {
                if let Some(window) = weak.upgrade() {
                    resize_to_scale(&window, image, *zoom_start.borrow() * scale);
                }
            }
        });
    }
    // GTK3 controllers need an owned reference for the widget's lifetime.
    window.connect_destroy(move |_| {
        zoom.set_propagation_phase(gtk::PropagationPhase::None);
    });
    window.connect_key_press_event(|window, event| {
        if event.keyval() == gdk::keys::constants::Escape {
            window.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    // Some X11 window managers drop the above-state on focus changes or
    // workspace switches; GTK does not re-assert it on our behalf. This is a
    // no-op where `set_keep_above` was already a no-op (e.g. GNOME Wayland),
    // which instead relies on the companion extension's `make_above()`.
    window.connect_window_state_event(|window, event| {
        if !event.new_window_state().contains(gdk::WindowState::ABOVE) {
            window.set_keep_above(true);
        }
        glib::Propagation::Proceed
    });
    window.connect_destroy(|window| {
        PINS.with(|pins| pins.borrow_mut().retain(|pin| pin != window));
    });
    PINS.with(|pins| pins.borrow_mut().push(window.clone()));
    window.show_all();
    window.present();
    // Re-assert keep-above once the window is realized and mapped. Several
    // X11 window managers only honor `_NET_WM_STATE_ABOVE` once the window
    // has a real X11 surface; setting it before `present()` is sometimes
    // silently dropped. A short idle redo (after the WM has processed the
    // map) catches that case without delaying window creation itself.
    {
        let weak = window.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(window) = weak.upgrade() {
                window.set_keep_above(true);
            }
        });
    }
    Ok(())
}

/// Logical-pixel bounds of the close button over the fitted (letterboxed)
/// image rectangle, matching where `draw_close_button` paints it.
fn close_button_bounds(image_x: f64, image_y: f64, image_w: f64) -> (f64, f64, f64) {
    let cx = image_x + image_w - CLOSE_BUTTON_MARGIN - CLOSE_BUTTON_RADIUS;
    let cy = image_y + CLOSE_BUTTON_MARGIN + CLOSE_BUTTON_RADIUS;
    (cx, cy, CLOSE_BUTTON_RADIUS)
}

/// Mirrors the fitted-rectangle math in `connect_draw` so the hit target
/// always matches what is actually drawn, at any zoom level.
fn close_button_hit(area: &gtk::DrawingArea, (px, py): (f64, f64), image: (u32, u32)) -> bool {
    let vw = area.allocated_width() as f64;
    let vh = area.allocated_height() as f64;
    if vw <= 0.0 || vh <= 0.0 {
        return false;
    }
    let scale = (vw / image.0 as f64).min(vh / image.1 as f64);
    let (w, h) = (image.0 as f64 * scale, image.1 as f64 * scale);
    let (x, y) = ((vw - w) / 2.0, (vh - h) / 2.0);
    let (cx, cy, r) = close_button_bounds(x, y, w);
    // A slightly larger hit radius than the drawn radius forgives imprecise clicks.
    (px - cx).powi(2) + (py - cy).powi(2) <= (r + 2.0).powi(2)
}

fn draw_close_button(
    cr: &gtk::cairo::Context,
    image_x: f64,
    image_y: f64,
    image_w: f64,
    image_h: f64,
    hovered: bool,
) {
    if image_w <= 0.0 || image_h <= 0.0 {
        return;
    }
    let (cx, cy, r) = close_button_bounds(image_x, image_y, image_w);
    // Called only while the pin is hovered; emphasize the button itself on hover.
    let alpha = if hovered { 0.72 } else { 0.28 };
    cr.save().ok();
    cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
    cr.set_source_rgba(0.08, 0.08, 0.09, alpha);
    let _ = cr.fill();
    let inset = r * 0.42;
    cr.set_source_rgba(1.0, 1.0, 1.0, alpha.min(0.95));
    cr.set_line_width(1.6);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.move_to(cx - inset, cy - inset);
    cr.line_to(cx + inset, cy + inset);
    let _ = cr.stroke();
    cr.move_to(cx + inset, cy - inset);
    cr.line_to(cx - inset, cy + inset);
    let _ = cr.stroke();
    cr.restore().ok();
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::fitted_size;
    use gtk::prelude::GtkWindowExt;

    #[test]
    #[ignore = "requires a live Linux desktop; run explicitly with --ignored"]
    fn desktop_pin_can_shrink_below_original_size() {
        gtk::init().expect("GTK desktop");
        super::create_on_main(vec![255; 400 * 200 * 4], 400, 200, 13).unwrap();
        let window = super::PINS.with(|pins| pins.borrow()[0].clone());
        let settle = || {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while std::time::Instant::now() < until {
                while gtk::glib::MainContext::default().pending() {
                    gtk::glib::MainContext::default().iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        settle();
        for scale in [0.5, 0.25, 0.1, 1.0, 0.5] {
            super::resize_to_scale(&window, (400, 200), scale);
            settle();
            let actual = gtk::prelude::GtkWindowExt::size(&window);
            let expected = ((400.0 * scale) as i32, (200.0 * scale) as i32);
            assert_eq!(actual, expected, "pin zoom {scale}");
        }
        window.close();
    }

    #[test]
    fn initial_fit_preserves_aspect_and_never_enlarges() {
        assert_eq!(fitted_size((1920, 1080), (700, 700), 1.0), (700, 394));
        assert_eq!(fitted_size((40, 20), (700, 700), 1.0), (40, 20));
        assert_eq!(fitted_size((100, 400), (700, 700), 1.0), (100, 400));
    }

    #[test]
    fn zoom_is_monitor_bounded_in_both_orientations() {
        assert_eq!(fitted_size((100, 400), (600, 600), 100.0), (150, 600));
        assert_eq!(fitted_size((400, 100), (600, 600), 100.0), (600, 150));
        assert_eq!(fitted_size((200, 100), (600, 600), 0.5), (100, 50));
    }

    #[test]
    fn extreme_aspect_and_small_bounds_remain_nonzero() {
        assert_eq!(fitted_size((10000, 1), (700, 700), 1.0), (700, 1));
        assert_eq!(fitted_size((1, 10000), (0, 0), 1.0), (1, 1));
    }
}
