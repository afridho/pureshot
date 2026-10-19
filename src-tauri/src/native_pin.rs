//! Native Win32 floating screenshot window.
//!
//! The window is composited in software into a premultiplied 32-bit BGRA
//! surface and pushed with `UpdateLayeredWindow`. Doing the compositing here
//! (rather than with GDI primitives) is what lets the rounded corners, the
//! drop shadow and the close button be anti-aliased and translucent.
#![cfg(windows)]

use std::{
    ffi::c_void,
    mem, ptr,
    sync::{Once, OnceLock},
};
use windows::{
    core::{BOOL, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
            GdiFlush, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA,
            AC_SRC_OVER, ANTIALIASED_QUALITY, BACKGROUND_MODE, BITMAPINFO, BITMAPINFOHEADER,
            BI_RGB, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER,
            DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, HBITMAP, HDC, HGDIOBJ,
            OUT_DEFAULT_PRECIS, TRANSPARENT,
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::{
            Input::{
                KeyboardAndMouse::{
                    GetAsyncKeyState, ReleaseCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
                    VK_CONTROL, VK_ESCAPE,
                },
                Pointer::{GetPointerFrameInfoHistory, POINTER_INFO},
            },
            WindowsAndMessaging::*,
        },
    },
};

const CLASS_NAME: &[u16] = &[80, 117, 114, 101, 83, 104, 111, 116, 80, 105, 110, 0];
const MIN_SCALE: f64 = 0.25;
const MAX_SCALE: f64 = 3.0;

/// Not exposed by the WindowsAndMessaging module we import; it lives behind
/// the Controls feature instead, so it's defined here directly.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Transparent padding around the image that holds the drop shadow.
const MARGIN: i32 = 16;
const SHADOW_BLUR: f32 = 10.0;
const SHADOW_STRENGTH: f32 = 0.28;
const SHADOW_OFFSET: f32 = 3.0;
const BUTTON_RADIUS: f32 = 12.5;

static REGISTER: Once = Once::new();

type RegisterTouchpadFn = unsafe extern "system" fn(HWND, BOOL) -> BOOL;

struct State {
    image: Vec<u8>, // RGBA at the original capture resolution
    image_width: i32,
    image_height: i32,
    base_width: i32,
    radius: i32,
    start_distance: Option<f64>,
    last_distance: Option<f64>,
    last_center_y: Option<f64>,
    gesture_width: i32,
    opacity: u8,
    hovered: bool,
    locked: bool,
    click_through: bool,
    surface: Vec<u8>,
    surface_w: i32,
    surface_h: i32,
    mem_dc: HDC,
    default_bitmap: HBITMAP,
    dib: HBITMAP,
    dib_bits: *mut u8,
}

unsafe fn register_touchpad(hwnd: HWND) {
    if let Ok(module) = GetModuleHandleW(windows::core::w!("user32.dll")) {
        if let Some(proc) =
            GetProcAddress(module, windows::core::s!("RegisterTouchpadCapableWindow"))
        {
            let register: RegisterTouchpadFn = mem::transmute(proc);
            let _ = register(hwnd, BOOL(1));
        }
    }
}

fn pointer_id(wparam: WPARAM) -> u32 {
    (wparam.0 & 0xffff) as u32
}

unsafe fn two_finger_metrics(id: u32) -> Option<(f64, f64)> {
    let mut frames = 0;
    let mut count = 0;
    GetPointerFrameInfoHistory(id, &mut frames, &mut count, None).ok()?;
    if frames == 0 || count < 2 {
        return None;
    }
    let mut points = vec![POINTER_INFO::default(); frames.checked_mul(count)? as usize];
    GetPointerFrameInfoHistory(id, &mut frames, &mut count, Some(points.as_mut_ptr())).ok()?;
    let contacts: Vec<_> = points
        .iter()
        .take(count as usize)
        .filter(|p| p.pointerType == PT_TOUCHPAD)
        .take(2)
        .collect();
    if contacts.len() != 2 {
        return None;
    }
    let dx = (contacts[0].ptHimetricLocationRaw.x - contacts[1].ptHimetricLocationRaw.x) as f64;
    let dy = (contacts[0].ptHimetricLocationRaw.y - contacts[1].ptHimetricLocationRaw.y) as f64;
    let distance = dx.hypot(dy);
    let center_y =
        (contacts[0].ptHimetricLocationRaw.y + contacts[1].ptHimetricLocationRaw.y) as f64 / 2.0;
    (distance > 0.0).then_some((distance, center_y))
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    (
        ((lparam.0 & 0xffff) as i16) as i32,
        ((((lparam.0 >> 16) & 0xffff) as i16) as i32),
    )
}

/// Signed distance from a point to a rounded rectangle centred on the origin.
/// Negative inside, positive outside, measured in pixels.
fn round_box_sdf(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
}

struct IconBitmap {
    alpha: Vec<u8>,
    width: u32,
    height: u32,
}

static CLOSE_ICON: OnceLock<IconBitmap> = OnceLock::new();
static LOCK_ICON: OnceLock<IconBitmap> = OnceLock::new();
static UNLOCK_ICON: OnceLock<IconBitmap> = OnceLock::new();
static CLICK_THROUGH_ICON: OnceLock<IconBitmap> = OnceLock::new();
const CONTROL_SAMPLE_OFFSETS: [f32; 4] = [-0.375, -0.125, 0.125, 0.375];

fn decode_icon(bytes: &[u8], description: &str) -> IconBitmap {
    let rgba = image::load_from_memory(bytes)
        .unwrap_or_else(|_| panic!("embedded {description} must be a valid PNG"))
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    IconBitmap {
        alpha: rgba.pixels().map(|pixel| pixel[3]).collect(),
        width,
        height,
    }
}

fn close_icon() -> &'static IconBitmap {
    CLOSE_ICON
        .get_or_init(|| decode_icon(include_bytes!("assets/overlay-icons/x.png"), "close icon"))
}

fn lock_icon(locked: bool) -> &'static IconBitmap {
    if locked {
        UNLOCK_ICON.get_or_init(|| {
            decode_icon(
                include_bytes!("assets/overlay-icons/lock-open.png"),
                "unlock icon",
            )
        })
    } else {
        LOCK_ICON.get_or_init(|| {
            decode_icon(include_bytes!("assets/overlay-icons/lock.png"), "lock icon")
        })
    }
}

fn click_through_icon() -> &'static IconBitmap {
    CLICK_THROUGH_ICON.get_or_init(|| {
        decode_icon(
            include_bytes!("assets/overlay-icons/mouse-pointer-ban.png"),
            "click-through icon",
        )
    })
}

/// Close control geometry in image-local coordinates. Its 22px diameter and
/// 7px inset match the native Quick Access Overlay control.
fn button_geometry(cw: f32, ch: f32, right: bool) -> (f32, f32, f32) {
    let r = BUTTON_RADIUS.min(cw * 0.2).min(ch * 0.2);
    let margin = 7.0;
    let cx = if right { cw - margin - r } else { margin + r };
    (cx, margin + r, r)
}

fn click_through_geometry(cw: f32, ch: f32) -> (f32, f32, f32) {
    let (lock_x, lock_y, r) = button_geometry(cw, ch, true);
    (lock_x - r * 2.0 - 6.0, lock_y, r)
}

fn zoom_label_pixel(lx: f32, ly: f32, cw: f32) -> (f32, f32, f32, f32) {
    let pill_w = 48.0;
    let pill_h = 19.0;
    let sdf = round_box_sdf(
        lx - cw * 0.5,
        ly - 6.0 - pill_h * 0.5,
        pill_w * 0.5,
        pill_h * 0.5,
        pill_h * 0.5,
    );
    let fill = (0.5 - sdf).clamp(0.0, 1.0) * 0.78;
    (0.13 * fill, 0.13 * fill, 0.14 * fill, fill)
}

/// Rasterize regular Segoe UI into a grayscale mask, then composite it with
/// our own alpha. GDI text drawing itself does not preserve layered-window alpha.
unsafe fn draw_zoom_text(state: &mut State, w: i32, h: i32) {
    if !state.hovered {
        return;
    }
    let cw = w - 2 * MARGIN;
    let scale = ((cw as f32 / state.base_width.max(1) as f32) * 100.0).round() as u32;
    let mut text: Vec<u16> = format!("{scale}%").encode_utf16().collect();
    let mut rect = RECT {
        left: MARGIN + (cw - 48) / 2,
        top: MARGIN + 6,
        right: MARGIN + (cw - 48) / 2 + 48,
        bottom: MARGIN + 25,
    };
    let font = CreateFontW(
        -12,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
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
    if font.is_invalid() {
        return;
    }
    ptr::write_bytes(state.dib_bits, 0, state.surface.len());
    let old_font = SelectObject(state.mem_dc, HGDIOBJ(font.0));
    let old_mode = BACKGROUND_MODE(SetBkMode(state.mem_dc, TRANSPARENT) as u32);
    let old_color = SetTextColor(state.mem_dc, COLORREF(0x00ffffff));
    DrawTextW(
        state.mem_dc,
        &mut text,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    let _ = GdiFlush();
    SelectObject(state.mem_dc, old_font);
    SetBkMode(state.mem_dc, old_mode);
    SetTextColor(state.mem_dc, old_color);
    let _ = DeleteObject(HGDIOBJ(font.0));
    let opacity = state.opacity as f32 / 100.0;
    for y in rect.top.max(0)..rect.bottom.min(h) {
        for x in rect.left.max(0)..rect.right.min(w) {
            let idx = ((y * w + x) * 4) as usize;
            let coverage = *state.dib_bits.add(idx) as f32 / 255.0;
            for channel in 0..3 {
                state.surface[idx + channel] = (240.0 * coverage * opacity
                    + state.surface[idx + channel] as f32 * (1.0 - coverage))
                    as u8;
            }
            state.surface[idx + 3] = (255.0 * coverage * opacity
                + state.surface[idx + 3] as f32 * (1.0 - coverage))
                as u8;
        }
    }
}

fn icon_coverage(icon: &IconBitmap, lx: f32, ly: f32, cx: f32, cy: f32) -> f32 {
    let size = 14.0;
    let sample_at = |px: f32, py: f32| {
        let u = (px - (cx - size * 0.5)) / size;
        let v = (py - (cy - size * 0.5)) / size;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 0.0;
        }
        let sx = u * (icon.width - 1) as f32;
        let sy = v * (icon.height - 1) as f32;
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

/// Same graphite circle and official Lucide X used by the preview overlay.
fn button_pixel(
    icon: &IconBitmap,
    lx: f32,
    ly: f32,
    cx: f32,
    cy: f32,
    r: f32,
) -> (f32, f32, f32, f32) {
    let mut fill = 0.0;
    for &oy in &CONTROL_SAMPLE_OFFSETS {
        for &ox in &CONTROL_SAMPLE_OFFSETS {
            let dx = lx + ox - cx;
            let dy = ly + oy - cy;
            fill += (r + 0.55 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
        }
    }
    fill /= 16.0;
    if fill <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let base_alpha = fill * 0.82;
    let mut pr = 0.52 * base_alpha;
    let mut pg = 0.52 * base_alpha;
    let mut pb = 0.52 * base_alpha;
    let mut pa = base_alpha;
    let icon = icon_coverage(icon, lx, ly, cx, cy);
    if icon > 0.0 {
        pr = 0.18 * icon + pr * (1.0 - icon);
        pg = 0.18 * icon + pg * (1.0 - icon);
        pb = 0.19 * icon + pb * (1.0 - icon);
        pa = icon + pa * (1.0 - icon);
    }
    (pr, pg, pb, pa)
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

/// Renders the whole window (shadow, image, close control) into `state.surface`.
unsafe fn render_surface(state: &mut State, w: i32, h: i32) {
    let m = MARGIN as f32;
    let cw = (w as f32 - 2.0 * m).max(1.0);
    let ch = (h as f32 - 2.0 * m).max(1.0);
    let hw = cw * 0.5;
    let hh = ch * 0.5;
    let r = (state.radius as f32).max(0.0).min(hw).min(hh);
    let op = (state.opacity as f32 / 100.0).clamp(0.0, 1.0);
    let iw = state.image_width.max(1);
    let ih = state.image_height.max(1);
    let hovered = state.hovered;

    // Precompute the source-image mapping for every output column/row so the
    // bilinear sampling inside the loop stays cheap.
    let mut xs = Vec::with_capacity(w as usize);
    for x in 0..w {
        let lx = x as f32 - m + 0.5;
        let sx = (lx / cw) * iw as f32 - 0.5;
        let x0 = sx.floor().clamp(0.0, (iw - 1) as f32) as i32;
        let x1 = (x0 + 1).min(iw - 1);
        xs.push((x0, x1, (sx - x0 as f32).clamp(0.0, 1.0)));
    }
    let mut ys = Vec::with_capacity(h as usize);
    for y in 0..h {
        let ly = y as f32 - m + 0.5;
        let sy = (ly / ch) * ih as f32 - 0.5;
        let y0 = sy.floor().clamp(0.0, (ih - 1) as f32) as i32;
        let y1 = (y0 + 1).min(ih - 1);
        ys.push((y0, y1, (sy - y0 as f32).clamp(0.0, 1.0)));
    }

    let (close_x, close_y, close_r) = button_geometry(cw, ch, false);
    let (lock_x, lock_y, lock_r) = button_geometry(cw, ch, true);
    let (through_x, through_y, through_r) = click_through_geometry(cw, ch);
    let control_bounds =
        |cx: f32, cy: f32, r: f32| (cx - r - 1.5, cy - r - 1.5, cx + r + 1.5, cy + r + 1.5);
    let close_bounds = control_bounds(close_x, close_y, close_r);
    let lock_bounds = control_bounds(lock_x, lock_y, lock_r);
    let through_bounds = control_bounds(through_x, through_y, through_r);

    let image = &state.image;
    let surf = &mut state.surface;
    for y in 0..h {
        let (y0, y1, fy) = ys[y as usize];
        for x in 0..w {
            let lx = x as f32 - m + 0.5;
            let ly = y as f32 - m + 0.5;
            let idx = ((y * w + x) * 4) as usize;

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
            let rr = (top_r + (bot_r - top_r) * fy) / 255.0;
            let gg = (top_g + (bot_g - top_g) * fy) / 255.0;
            let bb = (top_b + (bot_b - top_b) * fy) / 255.0;

            let in_close = hovered
                && lx >= close_bounds.0
                && lx <= close_bounds.2
                && ly >= close_bounds.1
                && ly <= close_bounds.3;
            let in_lock = (hovered || state.locked)
                && lx >= lock_bounds.0
                && lx <= lock_bounds.2
                && ly >= lock_bounds.1
                && ly <= lock_bounds.3;
            let in_through = (hovered || state.click_through)
                && lx >= through_bounds.0
                && lx <= through_bounds.2
                && ly >= through_bounds.1
                && ly <= through_bounds.3;
            let in_button = in_close || in_lock || in_through;
            let in_zoom = hovered
                && lx >= cw * 0.5 - 38.0
                && lx <= cw * 0.5 + 38.0
                && ly >= 4.0
                && ly <= 28.0;

            // Interior away from the rounded corners: the image is fully opaque
            // and hides the shadow, so the distance fields can be skipped.
            if !in_button
                && !in_zoom
                && lx >= r + 1.0
                && lx <= cw - r - 1.0
                && ly >= r + 1.0
                && ly <= ch - r - 1.0
            {
                surf[idx] = (bb * op * 255.0) as u8;
                surf[idx + 1] = (gg * op * 255.0) as u8;
                surf[idx + 2] = (rr * op * 255.0) as u8;
                surf[idx + 3] = (op * 255.0) as u8;
                continue;
            }

            // Beyond the shadow's reach: nothing to draw.
            if lx < -SHADOW_BLUR - 1.0
                || lx > cw + SHADOW_BLUR + 1.0
                || ly < -SHADOW_BLUR - 1.0
                || ly > ch + SHADOW_OFFSET + SHADOW_BLUR + 1.0
            {
                surf[idx] = 0;
                surf[idx + 1] = 0;
                surf[idx + 2] = 0;
                surf[idx + 3] = 0;
                continue;
            }

            // Drop shadow: same rounded rectangle pushed down, faded out.
            let sdf_sh = round_box_sdf(lx - hw, ly - SHADOW_OFFSET - hh, hw, hh, r);
            let t = (1.0 - sdf_sh.max(0.0) / SHADOW_BLUR).clamp(0.0, 1.0);
            let mut pa = t * t * SHADOW_STRENGTH;
            let mut pr = 0.0f32;
            let mut pg = 0.0f32;
            let mut pb = 0.0f32;

            // Image with a hard, anti-aliased rounded edge on top of the shadow.
            let ca = (0.5 - round_box_sdf(lx - hw, ly - hh, hw, hh, r)).clamp(0.0, 1.0);
            if ca > 0.0 {
                pa = ca + pa * (1.0 - ca);
                pr = rr * ca;
                pg = gg * ca;
                pb = bb * ca;
            }

            let overlay = if in_close {
                Some(button_pixel(
                    close_icon(),
                    lx,
                    ly,
                    close_x,
                    close_y,
                    close_r,
                ))
            } else if in_lock {
                Some(button_pixel(
                    lock_icon(state.locked),
                    lx,
                    ly,
                    lock_x,
                    lock_y,
                    lock_r,
                ))
            } else if in_through {
                Some(button_pixel(
                    click_through_icon(),
                    lx,
                    ly,
                    through_x,
                    through_y,
                    through_r,
                ))
            } else if in_zoom {
                Some(zoom_label_pixel(lx, ly, cw))
            } else {
                None
            };
            if let Some((sr, sg, sb, sa)) = overlay {
                if sa > 0.0 {
                    pr = sr + pr * (1.0 - sa);
                    pg = sg + pg * (1.0 - sa);
                    pb = sb + pb * (1.0 - sa);
                    pa = sa + pa * (1.0 - sa);
                }
            }

            surf[idx] = (pb * op * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 1] = (pg * op * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 2] = (pr * op * 255.0).clamp(0.0, 255.0) as u8;
            surf[idx + 3] = (pa * op * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

/// Re-renders and pushes the surface to the layered window.
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
    draw_zoom_text(state, w, h);

    ptr::copy_nonoverlapping(state.surface.as_ptr(), state.dib_bits, state.surface.len());

    let mut window = RECT::default();
    let _ = GetWindowRect(hwnd, &mut window);
    let dst = POINT {
        x: window.left,
        y: window.top,
    };
    let src = POINT { x: 0, y: 0 };
    let size = SIZE { cx: w, cy: h };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
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

unsafe fn set_opacity(hwnd: HWND, state: &mut State, opacity: i32) {
    state.opacity = opacity.clamp(20, 100) as u8;
    refresh(hwnd, state);
}

unsafe fn resize_for_pinch(hwnd: HWND, state: &State, width: i32) {
    let min_w = (state.base_width as f64 * MIN_SCALE) as i32;
    let max_w = (state.base_width as f64 * MAX_SCALE) as i32;
    let content_w = width.clamp(min_w, max_w);
    let content_h =
        (content_w as f64 * state.image_height as f64 / state.image_width as f64).round() as i32;
    let win_w = content_w + 2 * MARGIN;
    let win_h = content_h + 2 * MARGIN;
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return;
    }
    let cx = (rect.left + rect.right) / 2;
    let cy = (rect.top + rect.bottom) / 2;
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        cx - win_w / 2,
        cy - win_h / 2,
        win_w,
        win_h,
        SWP_NOACTIVATE,
    );
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
        WM_CREATE => {
            register_touchpad(hwnd);
            return LRESULT(0);
        }
        WM_SIZE if !raw.is_null() => {
            refresh(hwnd, &mut *raw);
        }
        WM_MOUSEACTIVATE => {
            // Keep this tool window inactive without eating the click that
            // should toggle lock/unlock after another window gains focus.
            return LRESULT(MA_NOACTIVATE as isize);
        }
        WM_NCHITTEST if !raw.is_null() => {
            let (x, y) = lparam_point(lparam);
            let mut window = RECT::default();
            let _ = GetWindowRect(hwnd, &mut window);
            let cw = (window.right - window.left - 2 * MARGIN) as f32;
            let ch = (window.bottom - window.top - 2 * MARGIN) as f32;
            let lx = (x - window.left - MARGIN) as f32;
            let ly = (y - window.top - MARGIN) as f32;
            let in_control = |(cx, cy, r): (f32, f32, f32)| {
                (lx - cx).powi(2) + (ly - cy).powi(2) <= (r + 3.0).powi(2)
            };
            // Recovery controls stay clickable even in click-through mode.
            // Lock only disables dragging; it must never disable its own button.
            let recovery_control = in_control(button_geometry(cw, ch, true))
                || in_control(click_through_geometry(cw, ch));
            if (*raw).click_through
                && !recovery_control
                && GetAsyncKeyState(VK_CONTROL.0 as i32) >= 0
            {
                return LRESULT(HTTRANSPARENT as isize);
            }
            return LRESULT(HTCLIENT as isize);
        }
        WM_MOUSEMOVE if !raw.is_null() => {
            if !(*raw).hovered {
                (*raw).hovered = true;
                refresh(hwnd, &mut *raw);
            }
            // Ask for a WM_MOUSELEAVE as soon as the cursor actually exits the
            // window, instead of guessing with a timer.
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tracking);
        }
        WM_MOUSELEAVE if !raw.is_null() => {
            if (*raw).hovered {
                (*raw).hovered = false;
                refresh(hwnd, &mut *raw);
            }
        }
        WM_LBUTTONDOWN if !raw.is_null() => {
            let (x, y) = lparam_point(lparam);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let cw = (client.right - 2 * MARGIN) as f32;
            let ch = (client.bottom - 2 * MARGIN) as f32;
            let (close_x, close_y, close_r) = button_geometry(cw, ch, false);
            let (lock_x, lock_y, lock_r) = button_geometry(cw, ch, true);
            let (through_x, through_y, through_r) = click_through_geometry(cw, ch);
            let local_x = x as f32 - MARGIN as f32;
            let local_y = y as f32 - MARGIN as f32;
            let close_dx = local_x - close_x;
            let close_dy = local_y - close_y;
            let lock_dx = local_x - lock_x;
            let lock_dy = local_y - lock_y;
            let through_dx = local_x - through_x;
            let through_dy = local_y - through_y;
            if close_dx * close_dx + close_dy * close_dy <= (close_r + 2.0) * (close_r + 2.0) {
                let _ = DestroyWindow(hwnd);
            } else if lock_dx * lock_dx + lock_dy * lock_dy <= (lock_r + 3.0) * (lock_r + 3.0) {
                (*raw).locked = !(*raw).locked;
                refresh(hwnd, &mut *raw);
            } else if through_dx * through_dx + through_dy * through_dy
                <= (through_r + 3.0) * (through_r + 3.0)
            {
                (*raw).click_through = !(*raw).click_through;
                refresh(hwnd, &mut *raw);
            } else if !(*raw).locked {
                let _ = ReleaseCapture();
                let _ = SendMessageW(
                    hwnd,
                    WM_NCLBUTTONDOWN,
                    Some(WPARAM(HTCAPTION as usize)),
                    Some(lparam),
                );
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP | WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_POINTERDOWN | WM_POINTERUPDATE if !raw.is_null() => {
            if let Some((distance, center_y)) = two_finger_metrics(pointer_id(wparam)) {
                let state = &mut *raw;
                match (state.last_distance, state.last_center_y) {
                    (Some(last_distance), Some(last_y)) => {
                        // Compare how much the finger separation changed against
                        // how much the pair moved together. Pinching changes the
                        // first; a two-finger scroll changes the second.
                        let separation_change = (distance - last_distance).abs();
                        let translation = (center_y - last_y).abs();
                        if separation_change > translation {
                            let start = state.start_distance.unwrap_or(distance);
                            resize_for_pinch(
                                hwnd,
                                state,
                                (state.gesture_width as f64 * distance / start).round() as i32,
                            );
                        } else if translation > 1.0 {
                            let opacity = state.opacity as i32 - (center_y - last_y) as i32 / 60;
                            set_opacity(hwnd, state, opacity);
                        }
                    }
                    _ => {
                        let mut client = RECT::default();
                        let _ = GetClientRect(hwnd, &mut client);
                        state.start_distance = Some(distance);
                        state.gesture_width = (client.right - 2 * MARGIN).max(1);
                    }
                }
                state.last_distance = Some(distance);
                state.last_center_y = Some(center_y);
                return LRESULT(0);
            }
        }
        WM_POINTERUP if !raw.is_null() => {
            (*raw).start_distance = None;
            (*raw).last_distance = None;
            (*raw).last_center_y = None;
        }
        WM_NCDESTROY if !raw.is_null() => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
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
            error = Some("Unable to register native pin window class".into());
        }
    });
    error.map_or(Ok(()), Err)
}

pub fn create(
    rgba: Vec<u8>,
    image_width: u32,
    image_height: u32,
    radius: u32,
) -> Result<(), String> {
    register_class()?;
    // Keep the caller alive until the first fully composited frame is visible.
    // The overlay uses this as a hand-off barrier, so it cannot disappear a
    // frame before the replacement pin window is ready.
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || unsafe {
        let aspect = image_width as f64 / image_height.max(1) as f64;
        let (content_w, content_h) = if aspect >= 1.0 {
            (480, (480.0 / aspect).round() as i32)
        } else {
            ((480.0 * aspect).round() as i32, 480)
        };
        let win_w = content_w + 2 * MARGIN;
        let win_h = content_h + 2 * MARGIN;
        let state = Box::new(State {
            image: rgba,
            image_width: image_width as i32,
            image_height: image_height as i32,
            base_width: content_w,
            radius: radius as i32,
            start_distance: None,
            last_distance: None,
            last_center_y: None,
            gesture_width: content_w,
            opacity: 100,
            hovered: false,
            locked: false,
            click_through: false,
            surface: Vec::new(),
            surface_w: 0,
            surface_h: 0,
            mem_dc: HDC(ptr::null_mut()),
            default_bitmap: HBITMAP(ptr::null_mut()),
            dib: HBITMAP(ptr::null_mut()),
            dib_bits: ptr::null_mut(),
        });
        let raw = Box::into_raw(state);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            PCWSTR(CLASS_NAME.as_ptr()),
            windows::core::w!("PureShot Pin"),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            win_w,
            win_h,
            None,
            None,
            None,
            Some(raw as *const c_void),
        );
        let Ok(hwnd) = hwnd else {
            drop(Box::from_raw(raw));
            let _ = ready_tx.send(());
            return;
        };
        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        let x = ((screen_width - win_w) / 2).max(0);
        let y = ((screen_height - win_h) / 2).max(0);
        // Render while hidden, then reveal the already-populated layered
        // surface. Creating it with WS_VISIBLE previously let DWM show one
        // empty/transparent frame before UpdateLayeredWindow completed.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, win_w, win_h, SWP_NOACTIVATE);
        refresh(hwnd, &mut *raw);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            win_w,
            win_h,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        let _ = ready_tx.send(());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    ready_rx
        .recv()
        .map_err(|_| "Unable to create native pin window".to_string())
}
