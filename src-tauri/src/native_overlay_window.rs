//! Native Win32 Quick Access Overlay (the small preview window shown right
//! after a capture, before the user pins/annotates/saves it).
//!
//! This mirrors the layered-window compositing approach in `native_pin.rs`
//! (software composite into a premultiplied BGRA surface, push with
//! `UpdateLayeredWindow`) but adds hover-revealed per-button hit-testing,
//! an auto-close timer with a slide+fade close animation, and a small set of
//! vector icons (pin / pencil / close / download) drawn with the same
//! signed-distance helpers used for the pin window's close button.
//!
//! Scope: this file only renders the *unpinned* floating preview. Pinning
//! still hands off to `native_pin::create`, which keeps its own window.
#![cfg(windows)]

use std::{
    ffi::c_void,
    mem, ptr,
    path::PathBuf,
    sync::{Once, OnceLock},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
            GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER,
            ANTIALIASED_QUALITY, BACKGROUND_MODE, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER,
            DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_SEMIBOLD, HBITMAP, HDC, HGDIOBJ,
            OUT_DEFAULT_PRECIS, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            },
            WindowsAndMessaging::*,
        },
    },
};

const CLASS_NAME: &[u16] = &[
    80, 117, 114, 101, 83, 104, 111, 116, 79, 118, 101, 114, 108, 97, 121, 0,
]; // "PureShotOverlay"

/// Not exposed by the WindowsAndMessaging module we import.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Timer id for the auto-close countdown.
const TIMER_AUTOCLOSE: usize = 1;
/// Timer id driving the slide+fade close animation.
const TIMER_CLOSING: usize = 2;
/// Timer id that restores Copy/Save labels after their success checkmark.
const TIMER_ACTION_FEEDBACK: usize = 3;
const ACTION_FEEDBACK_MS: u32 = 1400;

const CONTENT_W: i32 = 288;
const CONTENT_H: i32 = 180;
// Compact CleanShot/macOS-style controls. These values are logical 96-DPI
// pixels and are converted to physical pixels by the geometry helpers.
const BTN_MARGIN: f32 = 7.0;
const BTN_SIZE: f32 = 22.0;
const BTN_RADIUS: f32 = BTN_SIZE / 2.0;
const PILL_W: f32 = 47.0;
const PILL_H: f32 = 22.0;
const PILL_GAP: f32 = 6.0;
const CLOSE_ANIM_MS: u32 = 260;
/// Radius for each of three box-blur passes. Repeating the separable pass
/// approximates a Gaussian and avoids the hard rectangular bands produced by
/// a single small box blur over text-heavy screenshots.
const HOVER_BLUR_RADIUS: f32 = 2.0;
/// Dark translucent wash used by CleanShot-style hover previews. Applied
/// after the subtle blur and before drawing controls.
const HOVER_IMAGE_BRIGHTNESS: f32 = 0.52;
/// Transparent canvas margin reserved around the 288x180 card for its native
/// drop shadow. The actual preview size remains unchanged.
const SHADOW_MARGIN: f32 = 12.0;
const SHADOW_BLUR: f32 = 9.0;
const SHADOW_OFFSET_Y: f32 = 3.0;

static REGISTER: Once = Once::new();

/// Which button/pill is under a given point, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HitZone {
    Pin,
    Annotate,
    Close,
    Save,
    CopyPill,
    SavePill,
}

/// Callbacks into the app layer. Kept as a plain struct of function pointers
/// (rather than a trait object) so `State` stays `'static` and simple to box.
pub struct OverlayCallbacks {
    pub on_pin: Box<dyn Fn() + Send>,
    pub on_annotate: Box<dyn Fn() + Send>,
    pub on_copy: Box<dyn Fn() + Send>,
    pub on_save: Box<dyn Fn() + Send>,
}

struct NativeWindowHandle(HWND);

impl HasWindowHandle for NativeWindowHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = Win32WindowHandle::new(
            std::num::NonZeroIsize::new(self.0.0 as isize)
                .ok_or(raw_window_handle::HandleError::Unavailable)?,
        );
        // SAFETY: the HWND remains valid for the duration of the synchronous
        // OLE drag operation.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(raw)) })
    }
}

struct State {
    image: Vec<u8>,
    image_width: i32,
    image_height: i32,
    radius: i32,
    /// Monitor DPI scale factor. Button/pill geometry (`BTN_MARGIN`,
    /// `BTN_RADIUS`, ...) is defined in logical (96-DPI) pixels but the
    /// window itself is created at physical pixel size, so every geometry
    /// helper multiplies by this to stay the right size and position.
    scale: f32,
    is_dark: bool,
    show_on_hover: bool,
    timeout_ms: u32,
    timeout_remaining_ms: u32,
    timeout_started_at: Option<std::time::Instant>,
    hovered: bool,
    hovered_zone: Option<HitZone>,
    closing: bool,
    close_started_at: Option<std::time::Instant>,
    pressed: Option<HitZone>,
    action_feedback: Option<HitZone>,
    surface: Vec<u8>,
    surface_w: i32,
    surface_h: i32,
    mem_dc: HDC,
    default_bitmap: HBITMAP,
    dib: HBITMAP,
    dib_bits: *mut u8,
    capture_path: PathBuf,
    drag_start: Option<(i32, i32)>,
    dragging_file: bool,
    callbacks: OverlayCallbacks,
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    (
        ((lparam.0 & 0xffff) as i16) as i32,
        ((((lparam.0 >> 16) & 0xffff) as i16) as i32),
    )
}

/// Signed distance from a point to a rounded rectangle centred on the origin.
fn round_box_sdf(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
}

/// Two-pass (horizontal then vertical) box blur over an RGB `f32` buffer of
/// `w`x`h` pixels (3 floats per pixel, 0..1 range), approximating a Gaussian
/// cheaply. `radius` is in physical pixels. Used to blur the cover-fit
/// preview image behind the hover controls, mirroring the WebView version's
/// `backdrop-filter: blur(...)`.
fn box_blur_rgb(src: &[f32], w: i32, h: i32, radius: i32) -> Vec<f32> {
    if radius <= 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0f32; src.len()];
    let mut out = vec![0.0f32; src.len()];
    let win = (radius * 2 + 1) as f32;

    // Horizontal pass: src -> tmp.
    for y in 0..h {
        let row = (y * w) as usize * 3;
        for c in 0..3 {
            let mut acc = 0.0f32;
            for x in -radius..=radius {
                let sx = x.clamp(0, w - 1);
                acc += src[row + sx as usize * 3 + c];
            }
            for x in 0..w {
                tmp[row + x as usize * 3 + c] = acc / win;
                let add_x = (x + radius + 1).min(w - 1);
                let sub_x = (x - radius).max(0);
                acc += src[row + add_x as usize * 3 + c] - src[row + sub_x as usize * 3 + c];
            }
        }
    }

    // Vertical pass: tmp -> out.
    for x in 0..w {
        for c in 0..3 {
            let mut acc = 0.0f32;
            for y in -radius..=radius {
                let sy = y.clamp(0, h - 1);
                acc += tmp[(sy * w + x) as usize * 3 + c];
            }
            for y in 0..h {
                out[(y * w + x) as usize * 3 + c] = acc / win;
                let add_y = (y + radius + 1).min(h - 1);
                let sub_y = (y - radius).max(0);
                acc +=
                    tmp[(add_y * w + x) as usize * 3 + c] - tmp[(sub_y * w + x) as usize * 3 + c];
            }
        }
    }
    out
}

/// Geometry (centre + radius, in physical pixels) for every circular
/// button, matching `.pin-action` / `.annotate-action` / `.close` /
/// `.save-cloud-action` in `styles.css` (9px margin, 27px diameter at
/// 96 DPI). `cw`/`ch` are the physical window size; `scale` converts the
/// logical-pixel CSS constants to physical pixels.
fn circle_geometry(cw: f32, ch: f32, scale: f32, zone: HitZone) -> Option<(f32, f32, f32)> {
    let margin = BTN_MARGIN * scale;
    let r = BTN_RADIUS * scale;
    match zone {
        HitZone::Pin => Some((margin + r, margin + r, r)),
        HitZone::Annotate => Some((margin + r, ch - margin - r, r)),
        HitZone::Close => Some((cw - margin - r, margin + r, r)),
        HitZone::Save => Some((cw - margin - r, ch - margin - r, r)),
        _ => None,
    }
}

/// Bounding rect (x0, y0, x1, y1) for the two centred pill buttons, matching
/// `.primary-actions` (two 60x29 pills stacked with a 7px gap, centred, at
/// 96 DPI). See `circle_geometry` for the `scale` convention.
fn pill_rect(cw: f32, ch: f32, scale: f32, zone: HitZone) -> Option<(f32, f32, f32, f32)> {
    let pill_w = PILL_W * scale;
    let pill_h = PILL_H * scale;
    let gap = PILL_GAP * scale;
    let cx = cw * 0.5;
    let cy = ch * 0.5;
    let total_h = pill_h * 2.0 + gap;
    let top = cy - total_h * 0.5;
    match zone {
        HitZone::CopyPill => Some((cx - pill_w * 0.5, top, cx + pill_w * 0.5, top + pill_h)),
        HitZone::SavePill => {
            let y0 = top + pill_h + gap;
            Some((cx - pill_w * 0.5, y0, cx + pill_w * 0.5, y0 + pill_h))
        }
        _ => None,
    }
}

/// Hit-tests a physical-pixel point against every button/pill, in paint order.
fn hit_test(cw: f32, ch: f32, scale: f32, lx: f32, ly: f32) -> Option<HitZone> {
    for zone in [
        HitZone::Pin,
        HitZone::Annotate,
        HitZone::Close,
        HitZone::Save,
    ] {
        if let Some((cx, cy, r)) = circle_geometry(cw, ch, scale, zone) {
            let dx = lx - cx;
            let dy = ly - cy;
            if dx * dx + dy * dy <= (r + 1.5) * (r + 1.5) {
                return Some(zone);
            }
        }
    }
    for zone in [HitZone::CopyPill, HitZone::SavePill] {
        if let Some((x0, y0, x1, y1)) = pill_rect(cw, ch, scale, zone) {
            if lx >= x0 && lx <= x1 && ly >= y0 && ly <= y1 {
                return Some(zone);
            }
        }
    }
    None
}

/// Half-width of the analytic edge transition in physical pixels. Geometry
/// is already converted to physical pixels before its SDF is evaluated, so
/// scaling this value by monitor DPI a second time makes small icons look
/// swollen and still exposes chunky alpha steps. Roughly half a physical
/// pixel gives the correct one-pixel antialias transition.
const AA_WIDTH_PX: f32 = 0.55;

/// Smooth 0..1 coverage for a signed distance in physical pixels (negative
/// inside). `scale` is intentionally accepted for call-site consistency but
/// does not affect the physical-pixel AA width.
fn aa_coverage(sdf: f32, _scale: f32) -> f32 {
    let t = ((AA_WIDTH_PX - sdf) / (2.0 * AA_WIDTH_PX)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Fill color and alpha for a CleanShot-style graphite control background,
/// given a signed distance to its shape (negative inside).
fn pill_fill(
    sdf: f32,
    scale: f32,
    is_dark: bool,
    hot: bool,
    pressed: bool,
) -> (f32, f32, f32, f32) {
    let aa = aa_coverage(sdf, scale);
    if aa <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let (shade, opacity) = if is_dark {
        if pressed {
            (0.12, 0.96)
        } else if hot {
            (0.22, 0.96)
        } else {
            (0.16, 0.92)
        }
    } else if pressed {
        (0.84, 0.96)
    } else if hot {
        (1.0, 0.98)
    } else {
        (0.96, 0.96)
    };
    (shade, shade, shade, aa * opacity)
}

const CONTROL_SAMPLE_OFFSETS: [f32; 4] = [-0.375, -0.125, 0.125, 0.375];

fn circle_coverage(lx: f32, ly: f32, cx: f32, cy: f32, radius: f32, scale: f32) -> f32 {
    let mut sum = 0.0;
    for &oy in &CONTROL_SAMPLE_OFFSETS {
        for &ox in &CONTROL_SAMPLE_OFFSETS {
            let dx = lx + ox - cx;
            let dy = ly + oy - cy;
            sum += aa_coverage((dx * dx + dy * dy).sqrt() - radius, scale);
        }
    }
    sum / 16.0
}

fn segment_distance(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let vx = bx - ax;
    let vy = by - ay;
    let length_sq = vx * vx + vy * vy;
    let t = if length_sq > 0.0 {
        (((px - ax) * vx + (py - ay) * vy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = px - (ax + vx * t);
    let dy = py - (ay + vy * t);
    (dx * dx + dy * dy).sqrt()
}

fn check_coverage(lx: f32, ly: f32, cx: f32, cy: f32, scale: f32) -> f32 {
    let stroke = 1.35 * scale;
    let first = segment_distance(
        lx,
        ly,
        cx - 5.0 * scale,
        cy,
        cx - 1.2 * scale,
        cy + 3.8 * scale,
    );
    let second = segment_distance(
        lx,
        ly,
        cx - 1.2 * scale,
        cy + 3.8 * scale,
        cx + 5.5 * scale,
        cy - 4.0 * scale,
    );
    aa_coverage(first.min(second) - stroke, scale)
}

fn rounded_rect_coverage(
    lx: f32,
    ly: f32,
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    radius: f32,
    scale: f32,
) -> f32 {
    let mut sum = 0.0;
    for &oy in &CONTROL_SAMPLE_OFFSETS {
        for &ox in &CONTROL_SAMPLE_OFFSETS {
            let sdf = round_box_sdf(lx + ox - cx, ly + oy - cy, hw, hh, radius);
            sum += aa_coverage(sdf, scale);
        }
    }
    sum / 16.0
}

fn card_drop_shadow(
    lx: f32,
    ly: f32,
    card_x: f32,
    card_y: f32,
    card_w: f32,
    card_h: f32,
    radius: f32,
    scale: f32,
    is_dark: bool,
) -> f32 {
    let blur = (SHADOW_BLUR * scale).max(1.0);
    let offset_y = SHADOW_OFFSET_Y * scale;
    let sdf = round_box_sdf(
        lx - (card_x + card_w * 0.5),
        ly - (card_y + card_h * 0.5 + offset_y),
        card_w * 0.5,
        card_h * 0.5,
        radius,
    );
    let distance = sdf.max(0.0);
    let falloff = (1.0 - distance / blur).clamp(0.0, 1.0);
    let strength = if is_dark { 0.30 } else { 0.16 };
    falloff * falloff * strength
}

struct IconBitmap {
    alpha: Vec<u8>,
    width: u32,
    height: u32,
}

static PIN_ICON: OnceLock<IconBitmap> = OnceLock::new();
static PENCIL_ICON: OnceLock<IconBitmap> = OnceLock::new();
static CLOSE_ICON: OnceLock<IconBitmap> = OnceLock::new();
static DOWNLOAD_ICON: OnceLock<IconBitmap> = OnceLock::new();

fn decode_icon(bytes: &[u8]) -> IconBitmap {
    let rgba = image::load_from_memory(bytes)
        .expect("embedded overlay icon must be a valid PNG")
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let alpha = rgba.pixels().map(|pixel| pixel[3]).collect();
    IconBitmap {
        alpha,
        width,
        height,
    }
}

fn icon_bitmap(zone: HitZone) -> Option<&'static IconBitmap> {
    match zone {
        HitZone::Pin => Some(
            PIN_ICON.get_or_init(|| decode_icon(include_bytes!("assets/overlay-icons/pin.png"))),
        ),
        HitZone::Annotate => Some(
            PENCIL_ICON
                .get_or_init(|| decode_icon(include_bytes!("assets/overlay-icons/pencil.png"))),
        ),
        HitZone::Close => Some(
            CLOSE_ICON.get_or_init(|| decode_icon(include_bytes!("assets/overlay-icons/x.png"))),
        ),
        HitZone::Save => Some(
            DOWNLOAD_ICON
                .get_or_init(|| decode_icon(include_bytes!("assets/overlay-icons/download.png"))),
        ),
        _ => None,
    }
}

/// Bilinearly samples a high-resolution rasterization of the exact Lucide SVG
/// used by the React overlay. The 192px source is downsampled to 14 logical
/// pixels here, avoiding the stair-stepping and inaccurate hand-built SDF
/// geometry of the previous native icons.
fn icon_coverage(zone: HitZone, lx: f32, ly: f32, cx: f32, cy: f32, scale: f32) -> f32 {
    let Some(icon) = icon_bitmap(zone) else {
        return 0.0;
    };
    let size = 14.0 * scale;
    let sample_at = |px: f32, py: f32| {
        let u = (px - (cx - size * 0.5)) / size;
        let v = (py - (cy - size * 0.5)) / size;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 0.0;
        }
        let sx = (u * (icon.width - 1) as f32).clamp(0.0, (icon.width - 1) as f32);
        let sy = (v * (icon.height - 1) as f32).clamp(0.0, (icon.height - 1) as f32);
        let x0 = sx.floor() as u32;
        let y0 = sy.floor() as u32;
        let x1 = (x0 + 1).min(icon.width - 1);
        let y1 = (y0 + 1).min(icon.height - 1);
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;
        let sample = |x: u32, y: u32| icon.alpha[(y * icon.width + x) as usize] as f32 / 255.0;
        let top = sample(x0, y0) + (sample(x1, y0) - sample(x0, y0)) * fx;
        let bottom = sample(x0, y1) + (sample(x1, y1) - sample(x0, y1)) * fx;
        top + (bottom - top) * fy
    };

    let mut sum = 0.0;
    for &oy in &CONTROL_SAMPLE_OFFSETS {
        for &ox in &CONTROL_SAMPLE_OFFSETS {
            sum += sample_at(lx + ox, ly + oy);
        }
    }
    sum / 16.0
}

/// (Re)creates the compositing surface when the window size changes.
unsafe fn ensure_surface(state: &mut State, w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    if state.surface_w == w && state.surface_h == h {
        return true;
    }

    if state.mem_dc.is_invalid() {
        state.mem_dc = CreateCompatibleDC(None);
        if state.mem_dc.is_invalid() {
            return false;
        }
    }
    if !state.dib.is_invalid() {
        SelectObject(state.mem_dc, HGDIOBJ(state.default_bitmap.0));
        let _ = DeleteObject(HGDIOBJ(state.dib.0));
        state.dib = HBITMAP(ptr::null_mut());
    }

    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: (w * h * 4) as u32,
        ..Default::default()
    };
    let mut bits: *mut c_void = ptr::null_mut();
    let Ok(dib) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
        return false;
    };
    let old = SelectObject(state.mem_dc, HGDIOBJ(dib.0));
    if state.default_bitmap.is_invalid() {
        state.default_bitmap = HBITMAP(old.0);
    }

    state.dib = dib;
    state.dib_bits = bits as *mut u8;
    state.surface_w = w;
    state.surface_h = h;
    state.surface.resize((w * h * 4) as usize, 0);
    true
}

/// Renders the preview card (cover-fit image, rounded clip, hover buttons
/// and pills) into `state.surface`. `w`/`h` are the full window size; the
/// card fills the window exactly (no shadow margin, unlike the pin window).
unsafe fn render_surface(state: &mut State, w: i32, h: i32) {
    let dpi_scale = state.scale;
    let margin = (SHADOW_MARGIN * dpi_scale).round();
    let cw = (w as f32 - margin * 2.0).max(1.0);
    let ch = (h as f32 - margin * 2.0).max(1.0);
    let r = (state.radius as f32).max(0.0).min(cw * 0.5).min(ch * 0.5);
    let iw = state.image_width.max(1);
    let ih = state.image_height.max(1);
    // Visibility is independent of real pointer hover, which still controls
    // the auto-close countdown and per-button highlights.
    let actions_visible = !state.show_on_hover || state.hovered;

    // object-fit: cover mapping from window-local pixels to source image
    // pixels: scale to cover both dimensions, then centre-crop. This
    // `img_scale` is unrelated to `dpi_scale` above (image resolution vs.
    // monitor DPI) and must not be confused with it.
    let img_scale = (cw / iw as f32).max(ch / ih as f32);
    let draw_w = iw as f32 * img_scale;
    let draw_h = ih as f32 * img_scale;
    let offset_x = (cw - draw_w) * 0.5;
    let offset_y = (ch - draw_h) * 0.5;

    let card_w = cw.round() as i32;
    let card_h = ch.round() as i32;
    let mut xs = Vec::with_capacity(card_w as usize);
    for x in 0..card_w {
        let sx = ((x as f32 + 0.5 - offset_x) / img_scale - 0.5).clamp(0.0, (iw - 1) as f32);
        let x0 = sx.floor() as i32;
        let x1 = (x0 + 1).min(iw - 1);
        xs.push((x0, x1, sx - x0 as f32));
    }
    let mut ys = Vec::with_capacity(card_h as usize);
    for y in 0..card_h {
        let sy = ((y as f32 + 0.5 - offset_y) / img_scale - 0.5).clamp(0.0, (ih - 1) as f32);
        let y0 = sy.floor() as i32;
        let y1 = (y0 + 1).min(ih - 1);
        ys.push((y0, y1, sy - y0 as f32));
    }

    let image = &state.image;

    // While hovered, pre-resample the cover-fit image into a window-sized
    // RGB buffer and box-blur it, approximating the WebView version's
    // `backdrop-filter: blur(...)` on the card behind the hover controls.
    // Precomputed once per frame (not per-button) since the blur covers the
    // whole card, same as the CSS version.
    let blurred: Option<Vec<f32>> = if actions_visible {
        let mut linear = vec![0.0f32; (card_w * card_h * 3) as usize];
        for y in 0..card_h {
            let (y0, y1, fy) = ys[y as usize];
            for x in 0..card_w {
                let (x0, x1, fx) = xs[x as usize];
                let i00 = ((y0 * iw + x0) * 4) as usize;
                let i10 = ((y0 * iw + x1) * 4) as usize;
                let i01 = ((y1 * iw + x0) * 4) as usize;
                let i11 = ((y1 * iw + x1) * 4) as usize;
                let top_r = image[i00] as f32 + (image[i10] as f32 - image[i00] as f32) * fx;
                let top_g =
                    image[i00 + 1] as f32 + (image[i10 + 1] as f32 - image[i00 + 1] as f32) * fx;
                let top_b =
                    image[i00 + 2] as f32 + (image[i10 + 2] as f32 - image[i00 + 2] as f32) * fx;
                let bot_r = image[i01] as f32 + (image[i11] as f32 - image[i01] as f32) * fx;
                let bot_g =
                    image[i01 + 1] as f32 + (image[i11 + 1] as f32 - image[i01 + 1] as f32) * fx;
                let bot_b =
                    image[i01 + 2] as f32 + (image[i11 + 2] as f32 - image[i01 + 2] as f32) * fx;
                let out = ((y * card_w + x) * 3) as usize;
                linear[out] = (top_r + (bot_r - top_r) * fy) / 255.0;
                linear[out + 1] = (top_g + (bot_g - top_g) * fy) / 255.0;
                linear[out + 2] = (top_b + (bot_b - top_b) * fy) / 255.0;
            }
        }
        let radius = (HOVER_BLUR_RADIUS * dpi_scale).round().max(1.0) as i32;
        let first = box_blur_rgb(&linear, card_w, card_h, radius);
        let second = box_blur_rgb(&first, card_w, card_h, radius);
        Some(box_blur_rgb(&second, card_w, card_h, radius))
    } else {
        None
    };

    let surf = &mut state.surface;
    for y in 0..h {
        for x in 0..w {
            let canvas_x = x as f32 + 0.5;
            let canvas_y = y as f32 + 0.5;
            let lx = canvas_x - margin;
            let ly = canvas_y - margin;
            let idx = ((y * w + x) * 4) as usize;

            let shadow_alpha = card_drop_shadow(
                canvas_x,
                canvas_y,
                margin,
                margin,
                cw,
                ch,
                r,
                dpi_scale,
                state.is_dark,
            );
            let edge = round_box_sdf(lx - cw * 0.5, ly - ch * 0.5, cw * 0.5, ch * 0.5, r);
            let coverage = aa_coverage(edge, dpi_scale);
            if coverage <= 0.0 {
                // Neutral black premultiplied shadow in the transparent margin.
                surf[idx] = 0;
                surf[idx + 1] = 0;
                surf[idx + 2] = 0;
                surf[idx + 3] = (shadow_alpha * 255.0).clamp(0.0, 255.0) as u8;
                continue;
            }

            let local_x = lx.floor().clamp(0.0, (card_w - 1) as f32) as i32;
            let local_y = ly.floor().clamp(0.0, (card_h - 1) as f32) as i32;
            let (y0, y1, fy) = ys[local_y as usize];
            let (mut rr, mut gg, mut bb);
            if let Some(blur) = &blurred {
                let bi = ((local_y * card_w + local_x) * 3) as usize;
                rr = blur[bi];
                gg = blur[bi + 1];
                bb = blur[bi + 2];
            } else {
                let (x0, x1, fx) = xs[local_x as usize];
                let i00 = ((y0 * iw + x0) * 4) as usize;
                let i10 = ((y0 * iw + x1) * 4) as usize;
                let i01 = ((y1 * iw + x0) * 4) as usize;
                let i11 = ((y1 * iw + x1) * 4) as usize;
                let top_r = image[i00] as f32 + (image[i10] as f32 - image[i00] as f32) * fx;
                let top_g =
                    image[i00 + 1] as f32 + (image[i10 + 1] as f32 - image[i00 + 1] as f32) * fx;
                let top_b =
                    image[i00 + 2] as f32 + (image[i10 + 2] as f32 - image[i00 + 2] as f32) * fx;
                let bot_r = image[i01] as f32 + (image[i11] as f32 - image[i01] as f32) * fx;
                let bot_g =
                    image[i01 + 1] as f32 + (image[i11 + 1] as f32 - image[i01 + 1] as f32) * fx;
                let bot_b =
                    image[i01 + 2] as f32 + (image[i11 + 2] as f32 - image[i01 + 2] as f32) * fx;
                rr = (top_r + (bot_r - top_r) * fy) / 255.0;
                gg = (top_g + (bot_g - top_g) * fy) / 255.0;
                bb = (top_b + (bot_b - top_b) * fy) / 255.0;
            }
            let mut aa = coverage;

            if actions_visible {
                // Keep controls legible without forcing the same dark wash in
                // both Windows themes. Light mode lifts the image toward white;
                // dark mode retains the graphite dimming treatment.
                if state.is_dark {
                    rr *= HOVER_IMAGE_BRIGHTNESS;
                    gg *= HOVER_IMAGE_BRIGHTNESS;
                    bb *= HOVER_IMAGE_BRIGHTNESS;
                } else {
                    let wash = 1.0 - HOVER_IMAGE_BRIGHTNESS;
                    rr = rr * HOVER_IMAGE_BRIGHTNESS + wash;
                    gg = gg * HOVER_IMAGE_BRIGHTNESS + wash;
                    bb = bb * HOVER_IMAGE_BRIGHTNESS + wash;
                }
            }

            if actions_visible {
                if let Some(zone) = hit_test(cw, ch, dpi_scale, lx, ly) {
                    let hot = state.hovered_zone == Some(zone);
                    let pressed = state.pressed == Some(zone);
                    if let Some((bcx, bcy, br)) = circle_geometry(cw, ch, dpi_scale, zone) {
                        let coverage = circle_coverage(lx, ly, bcx, bcy, br, dpi_scale);
                        let (pr, pg, pb, base_alpha) =
                            pill_fill(-1.0, dpi_scale, state.is_dark, hot, pressed);
                        let pa = coverage * base_alpha;
                        if pa > 0.0 {
                            rr = pr * pa + rr * (1.0 - pa);
                            gg = pg * pa + gg * (1.0 - pa);
                            bb = pb * pa + bb * (1.0 - pa);
                            aa = pa + aa * (1.0 - pa);
                        }
                        let icon_a = icon_coverage(zone, lx, ly, bcx, bcy, dpi_scale);
                        if icon_a > 0.0 {
                            let shade = if state.is_dark {
                                if hot {
                                    (0.85, 0.88, 0.92)
                                } else {
                                    (0.95, 0.96, 0.98)
                                }
                            } else if hot {
                                (0.05, 0.32, 0.68)
                            } else {
                                (0.18, 0.18, 0.19)
                            };
                            rr = shade.0 * icon_a + rr * (1.0 - icon_a);
                            gg = shade.1 * icon_a + gg * (1.0 - icon_a);
                            bb = shade.2 * icon_a + bb * (1.0 - icon_a);
                            aa = icon_a + aa * (1.0 - icon_a);
                        }
                    } else if let Some((x0, y0, x1, y1)) = pill_rect(cw, ch, dpi_scale, zone) {
                        let hw = (x1 - x0) * 0.5;
                        let hh = (y1 - y0) * 0.5;
                        let radius = hh.min(16.0 * dpi_scale);
                        let coverage = rounded_rect_coverage(
                            lx,
                            ly,
                            x0 + hw,
                            y0 + hh,
                            hw,
                            hh,
                            radius,
                            dpi_scale,
                        );
                        let (pr, pg, pb, base_alpha) =
                            pill_fill(-1.0, dpi_scale, state.is_dark, hot, pressed);
                        let pa = coverage * base_alpha;
                        if pa > 0.0 {
                            rr = pr * pa + rr * (1.0 - pa);
                            gg = pg * pa + gg * (1.0 - pa);
                            bb = pb * pa + bb * (1.0 - pa);
                            aa = pa + aa * (1.0 - pa);
                        }
                        if state.action_feedback == Some(zone) {
                            let check_a = check_coverage(lx, ly, x0 + hw, y0 + hh, dpi_scale);
                            if check_a > 0.0 {
                                let shade = if hot { 0.10 } else { 0.18 };
                                rr = shade * check_a + rr * (1.0 - check_a);
                                gg = shade * check_a + gg * (1.0 - check_a);
                                bb = shade * check_a + bb * (1.0 - check_a);
                                aa = check_a + aa * (1.0 - check_a);
                            }
                        }
                    }
                }
            }

            surf[idx] = (bb * aa * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 1] = (gg * aa * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 2] = (rr * aa * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 3] = (aa * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

/// Draws the "Copy" / "Save" labels with GDI's antialiased text renderer,
/// then copies only the changed RGB pixels back into the software surface
/// while preserving the pill's existing alpha channel.
unsafe fn draw_pill_labels(state: &mut State, w: i32, h: i32) {
    if state.show_on_hover && !state.hovered {
        return;
    }
    let dpi_scale = state.scale;
    let margin = (SHADOW_MARGIN * dpi_scale).round();
    let cw = w as f32 - margin * 2.0;
    let ch = h as f32 - margin * 2.0;
    let Some((mut x0, mut y0, mut x1, mut y1)) = pill_rect(cw, ch, dpi_scale, HitZone::CopyPill)
    else {
        return;
    };
    let Some((mut x2, mut y2, mut x3, mut y3)) = pill_rect(cw, ch, dpi_scale, HitZone::SavePill)
    else {
        return;
    };
    x0 += margin;
    y0 += margin;
    x1 += margin;
    y1 += margin;
    x2 += margin;
    y2 += margin;
    x3 += margin;
    y3 += margin;

    let old_mode = BACKGROUND_MODE(SetBkMode(state.mem_dc, TRANSPARENT) as u32);
    let text_color = if state.is_dark {
        COLORREF(0x00f5f3f1)
    } else {
        COLORREF(0x00302e2e)
    };
    let old_color = SetTextColor(state.mem_dc, text_color);
    let font_height = -(11.0 * dpi_scale).round() as i32;
    let font = CreateFontW(
        font_height,
        0,
        0,
        0,
        FW_SEMIBOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        FF_DONTCARE.0 as u32,
        windows::core::w!("Segoe UI"),
    );
    let old_font = SelectObject(state.mem_dc, HGDIOBJ(font.0));

    for (zone, rect, text) in [
        (HitZone::CopyPill, (x0, y0, x1, y1), "Copy"),
        (HitZone::SavePill, (x2, y2, x3, y3), "Save"),
    ] {
        if state.action_feedback == Some(zone) {
            continue;
        }
        let mut r = RECT {
            left: rect.0 as i32,
            top: rect.1 as i32,
            right: rect.2 as i32,
            bottom: rect.3 as i32,
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            state.mem_dc,
            &mut wide.clone(),
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }

    SelectObject(state.mem_dc, old_font);
    let _ = DeleteObject(HGDIOBJ(font.0));
    SetTextColor(state.mem_dc, old_color);
    SetBkMode(state.mem_dc, old_mode);

    // Copy only pixels changed by DrawTextW back into the software surface.
    // Preserve the existing card alpha instead of thresholding glyphs to
    // fully opaque pixels; GDI's RGB already contains its antialias coverage.
    for (rx0f, ry0f, rx1f, ry1f) in [(x0, y0, x1, y1), (x2, y2, x3, y3)] {
        let (rx0, ry0, rx1, ry1) = (
            rx0f.max(0.0) as i32,
            ry0f.max(0.0) as i32,
            (rx1f as i32).min(w),
            (ry1f as i32).min(h),
        );
        for y in ry0..ry1 {
            for x in rx0..rx1 {
                let idx = ((y * w + x) * 4) as usize;
                if idx + 3 >= state.surface.len() {
                    continue;
                }
                let b = *state.dib_bits.add(idx);
                let g = *state.dib_bits.add(idx + 1);
                let r = *state.dib_bits.add(idx + 2);
                if b != state.surface[idx]
                    || g != state.surface[idx + 1]
                    || r != state.surface[idx + 2]
                {
                    state.surface[idx] = b;
                    state.surface[idx + 1] = g;
                    state.surface[idx + 2] = r;
                }
            }
        }
    }
}

/// Re-renders and pushes the surface to the layered window, honouring the
/// slide+fade close animation while it is in progress.
unsafe fn refresh(hwnd: HWND, state: &mut State) {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_err() {
        return;
    }
    let w = client.right;
    let h = client.bottom;
    if !ensure_surface(state, w, h) {
        return;
    }
    render_surface(state, w, h);
    // Push the composited surface into the DIB first so GDI's `DrawTextW`
    // (used by `draw_pill_labels`) paints text on top of the real pixels,
    // then let that function edit `surface` in place using `dib_bits` as
    // its reference for which pixels changed.
    ptr::copy_nonoverlapping(state.surface.as_ptr(), state.dib_bits, state.surface.len());
    draw_pill_labels(state, w, h);
    ptr::copy_nonoverlapping(state.surface.as_ptr(), state.dib_bits, state.surface.len());

    let mut window = RECT::default();
    let _ = GetWindowRect(hwnd, &mut window);
    let mut dst_x = window.left;
    let dst_y = window.top;
    let mut global_alpha: u8 = 255;

    if let Some(started) = state.close_started_at {
        let elapsed = started.elapsed().as_millis().min(CLOSE_ANIM_MS as u128) as f32;
        let t = elapsed / CLOSE_ANIM_MS as f32;
        // Matches `.is-closing { opacity:0; transform:translateX(115%) }`.
        dst_x += (t * w as f32 * 1.15) as i32;
        global_alpha = ((1.0 - t) * 255.0).clamp(0.0, 255.0) as u8;
    }

    let dst = POINT { x: dst_x, y: dst_y };
    let src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: global_alpha,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let screen = GetDC(None);
    let _ = UpdateLayeredWindow(
        hwnd,
        Some(screen),
        Some(&dst as *const POINT),
        Some(&size as *const SIZE),
        Some(state.mem_dc),
        Some(&src as *const POINT),
        COLORREF(0),
        Some(&blend as *const BLENDFUNCTION),
        ULW_ALPHA,
    );
    ReleaseDC(None, screen);
}

/// Starts the slide+fade close animation (mirrors `.is-closing` in CSS) and
/// arms a timer to keep repainting until it finishes, then destroys the
/// window. A no-op if the animation is already running.
unsafe fn begin_close(hwnd: HWND, state: &mut State) {
    if state.closing {
        return;
    }
    state.closing = true;
    state.close_started_at = Some(std::time::Instant::now());
    let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
    let _ = SetTimer(Some(hwnd), TIMER_CLOSING, 15, None);
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    match message {
        WM_CREATE if !raw.is_null() => {
            let state = &mut *raw;
            if state.timeout_remaining_ms > 0 {
                state.timeout_started_at = Some(std::time::Instant::now());
                let _ = SetTimer(
                    Some(hwnd),
                    TIMER_AUTOCLOSE,
                    state.timeout_remaining_ms,
                    None,
                );
            }
            return LRESULT(0);
        }
        WM_SIZE if !raw.is_null() => {
            refresh(hwnd, &mut *raw);
        }
        WM_MOUSEMOVE if !raw.is_null() => {
            let state = &mut *raw;
            let mut needs_refresh = false;
            if !state.hovered {
                state.hovered = true;
                needs_refresh = true;
                // Pause the countdown while hovered, preserving elapsed time
                // so leaving resumes rather than restarting the full timeout.
                let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
                if let Some(started) = state.timeout_started_at.take() {
                    let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
                    state.timeout_remaining_ms = state.timeout_remaining_ms.saturating_sub(elapsed);
                }
            }
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tracking);
            let (x, y) = lparam_point(lparam);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let margin = (SHADOW_MARGIN * state.scale).round();
            let zone = hit_test(
                client.right as f32 - margin * 2.0,
                client.bottom as f32 - margin * 2.0,
                state.scale,
                x as f32 + 0.5 - margin,
                y as f32 + 0.5 - margin,
            );
            if state.hovered_zone != zone {
                state.hovered_zone = zone;
                needs_refresh = true;
            }
            if state.pressed.is_some() && state.pressed != zone {
                // Button-press highlight should not stick if the cursor
                // drags off the button without releasing.
                state.pressed = None;
                needs_refresh = true;
            }
            if !state.dragging_file {
                if let Some((start_x, start_y)) = state.drag_start {
                    let threshold_x = GetSystemMetrics(SM_CXDRAG).max(4);
                    let threshold_y = GetSystemMetrics(SM_CYDRAG).max(4);
                    if (x - start_x).abs() >= threshold_x || (y - start_y).abs() >= threshold_y {
                        state.dragging_file = true;
                        state.drag_start = None;
                        let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
                        let handle = NativeWindowHandle(hwnd);
                        let path = state.capture_path.clone();
                        let preview = path.clone();
                        if let Err(error) = drag::start_drag(
                            &handle,
                            drag::DragItem::Files(vec![path]),
                            drag::Image::File(preview),
                            |_, _| {},
                            drag::Options::default(),
                        ) {
                            eprintln!("Unable to start screenshot drag: {error}");
                        }
                        state.dragging_file = false;
                        state.hovered = false;
                        state.hovered_zone = None;
                        if !state.closing && state.timeout_remaining_ms > 0 {
                            state.timeout_started_at = Some(std::time::Instant::now());
                            let _ = SetTimer(Some(hwnd), TIMER_AUTOCLOSE, state.timeout_remaining_ms, None);
                        }
                        refresh(hwnd, state);
                        return LRESULT(0);
                    }
                }
            }
            if needs_refresh {
                refresh(hwnd, state);
            }
        }
        WM_MOUSELEAVE if !raw.is_null() => {
            let state = &mut *raw;
            if state.hovered {
                state.hovered = false;
                state.hovered_zone = None;
                state.pressed = None;
                if !state.closing && state.timeout_remaining_ms > 0 {
                    state.timeout_started_at = Some(std::time::Instant::now());
                    let _ = SetTimer(
                        Some(hwnd),
                        TIMER_AUTOCLOSE,
                        state.timeout_remaining_ms,
                        None,
                    );
                } else if !state.closing && state.timeout_ms > 0 {
                    begin_close(hwnd, state);
                }
                refresh(hwnd, state);
            }
        }
        WM_LBUTTONDOWN if !raw.is_null() => {
            let state = &mut *raw;
            let (x, y) = lparam_point(lparam);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let margin = (SHADOW_MARGIN * state.scale).round();
            let zone = hit_test(
                client.right as f32 - margin * 2.0,
                client.bottom as f32 - margin * 2.0,
                state.scale,
                x as f32 + 0.5 - margin,
                y as f32 + 0.5 - margin,
            );
            if let Some(zone) = zone {
                state.pressed = Some(zone);
                refresh(hwnd, state);
            } else {
                // Dragging anywhere on the preview exports the screenshot as a
                // real file (CF_HDROP), so browsers and other apps can accept it.
                state.drag_start = Some((x, y));
                let _ = SetCapture(hwnd);
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP if !raw.is_null() => {
            let state = &mut *raw;
            state.drag_start = None;
            let _ = ReleaseCapture();
            let (x, y) = lparam_point(lparam);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let margin = (SHADOW_MARGIN * state.scale).round();
            let zone = hit_test(
                client.right as f32 - margin * 2.0,
                client.bottom as f32 - margin * 2.0,
                state.scale,
                x as f32 + 0.5 - margin,
                y as f32 + 0.5 - margin,
            );
            let pressed = state.pressed.take();
            let activated = if pressed.is_some() && pressed == zone {
                zone
            } else {
                None
            };
            if let Some(HitZone::Pin) = activated {
                // `on_pin` must run before `DestroyWindow`: once that call
                // returns, `WM_NCDESTROY` has already freed `state` (it
                // frees it synchronously, before `DestroyWindow` itself
                // returns), so touching `state` afterwards would be a
                // use-after-free. Return immediately instead of falling
                // through to the shared `refresh` call below.
                (state.callbacks.on_pin)();
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            match activated {
                Some(HitZone::Close) => {
                    begin_close(hwnd, state);
                }
                Some(HitZone::Annotate) => {
                    // Hand the capture to the editor, then dismiss this topmost
                    // preview so the annotation window can receive focus.
                    (state.callbacks.on_annotate)();
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                Some(HitZone::Save) => {
                    (state.callbacks.on_save)();
                }
                Some(HitZone::SavePill) => {
                    (state.callbacks.on_save)();
                    state.action_feedback = Some(HitZone::SavePill);
                    let _ = SetTimer(Some(hwnd), TIMER_ACTION_FEEDBACK, ACTION_FEEDBACK_MS, None);
                }
                Some(HitZone::CopyPill) => {
                    (state.callbacks.on_copy)();
                    state.action_feedback = Some(HitZone::CopyPill);
                    let _ = SetTimer(Some(hwnd), TIMER_ACTION_FEEDBACK, ACTION_FEEDBACK_MS, None);
                }
                _ => {}
            }
            refresh(hwnd, state);
            return LRESULT(0);
        }
        WM_TIMER if !raw.is_null() => {
            let state = &mut *raw;
            if wparam.0 == TIMER_AUTOCLOSE {
                let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
                begin_close(hwnd, state);
            } else if wparam.0 == TIMER_ACTION_FEEDBACK {
                let _ = KillTimer(Some(hwnd), TIMER_ACTION_FEEDBACK);
                state.action_feedback = None;
                refresh(hwnd, state);
            } else if wparam.0 == TIMER_CLOSING {
                let elapsed = state
                    .close_started_at
                    .map(|t| t.elapsed().as_millis())
                    .unwrap_or(0);
                if elapsed >= CLOSE_ANIM_MS as u128 {
                    let _ = KillTimer(Some(hwnd), TIMER_CLOSING);
                    let _ = DestroyWindow(hwnd);
                } else {
                    refresh(hwnd, state);
                }
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP | WM_CLOSE if !raw.is_null() => {
            begin_close(hwnd, &mut *raw);
            return LRESULT(0);
        }
        WM_NCDESTROY if !raw.is_null() => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
            let _ = KillTimer(Some(hwnd), TIMER_CLOSING);
            let _ = KillTimer(Some(hwnd), TIMER_ACTION_FEEDBACK);
            if !(*raw).dib.is_invalid() {
                SelectObject((*raw).mem_dc, HGDIOBJ((*raw).default_bitmap.0));
                let _ = DeleteObject(HGDIOBJ((*raw).dib.0));
            }
            if !(*raw).mem_dc.is_invalid() {
                let _ = DeleteDC((*raw).mem_dc);
            }
            drop(Box::from_raw(raw));
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn register_class() -> Result<(), String> {
    let mut error = None;
    REGISTER.call_once(|| unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(CLASS_NAME.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            error = Some("Unable to register native overlay window class".into());
        }
    });
    error.map_or(Ok(()), Err)
}

/// Creates and shows the Quick Access Overlay window, positioned at the
/// bottom-right corner of the monitor at `(monitor_x, monitor_y)` sized
/// `monitor_w` x `monitor_h` (physical pixels), scaled by `scale` the same
/// way `show_capture_overlay` used to position the WebView window.
///
/// `timeout_secs` is the `previewTimeout` setting (0 disables auto-close).
/// `show_on_hover` reveals actions only while hovered when true; false keeps
/// actions visible at all times. Hover still pauses auto-close in either mode.
/// Runs its own Win32 message loop on a dedicated thread, same pattern as
/// `native_pin::create`.
pub fn create(
    rgba: Vec<u8>,
    capture_path: PathBuf,
    image_width: u32,
    image_height: u32,
    radius: u32,
    timeout_secs: u32,
    monitor_x: i32,
    monitor_y: i32,
    monitor_w: i32,
    monitor_h: i32,
    scale: f64,
    is_dark: bool,
    show_on_hover: bool,
    callbacks: OverlayCallbacks,
) -> Result<(), String> {
    register_class()?;
    std::thread::spawn(move || unsafe {
        let card_w = (CONTENT_W as f64 * scale).round() as i32;
        let card_h = (CONTENT_H as f64 * scale).round() as i32;
        let shadow_margin = (SHADOW_MARGIN as f64 * scale).round() as i32;
        let win_w = card_w + shadow_margin * 2;
        let win_h = card_h + shadow_margin * 2;
        let offset = (22.0 * scale).round() as i32;
        // Position by the card's bottom-right edge; the transparent shadow
        // canvas extends around it without moving the visible preview.
        let x = monitor_x + monitor_w - card_w - offset - shadow_margin;
        let y = monitor_y + monitor_h - card_h - offset - shadow_margin;

        let state = Box::new(State {
            image: rgba,
            image_width: image_width as i32,
            image_height: image_height as i32,
            radius: (radius as f64 * scale).round() as i32,
            scale: scale as f32,
            is_dark,
            show_on_hover,
            timeout_ms: timeout_secs.saturating_mul(1000),
            timeout_remaining_ms: timeout_secs.saturating_mul(1000),
            timeout_started_at: None,
            hovered: false,
            hovered_zone: None,
            closing: false,
            close_started_at: None,
            pressed: None,
            action_feedback: None,
            surface: Vec::new(),
            surface_w: 0,
            surface_h: 0,
            mem_dc: HDC(ptr::null_mut()),
            default_bitmap: HBITMAP(ptr::null_mut()),
            dib: HBITMAP(ptr::null_mut()),
            dib_bits: ptr::null_mut(),
            capture_path,
            drag_start: None,
            dragging_file: false,
            callbacks,
        });
        let raw = Box::into_raw(state);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR(CLASS_NAME.as_ptr()),
            windows::core::w!("PureShot Preview"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            None,
            Some(raw as *const c_void),
        );
        let Ok(hwnd) = hwnd else {
            drop(Box::from_raw(raw));
            return;
        };
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            win_w,
            win_h,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        refresh(hwnd, &mut *raw);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    Ok(())
}
