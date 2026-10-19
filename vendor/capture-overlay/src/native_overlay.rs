//! Native Win32 screenshot overlay window.
//!
//! Creates a full-screen semi-transparent overlay that displays the screenshot
//! background, selection mask, and border, while handling mouse drag selection
//! and right-click cancellation.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled;
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    EndPaint, GdiFlush, GetDC, ReleaseDC, SelectObject, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, PAINTSTRUCT, RGBQUAD, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    KillTimer, LoadCursorW, PostQuitMessage, RegisterClassW, SetCursor, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HCURSOR,
    IDC_CROSS, LWA_ALPHA, MA_NOACTIVATE, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW,
    WINDOW_DISPLAY_AFFINITY, WM_CREATE, WM_DESTROY, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

// Raw FFI declarations for user32 functions not exported by the windows crate 0.52
#[link(name = "user32")]
extern "system" {
    fn SetCapture(hWnd: HWND) -> HWND;
    fn ReleaseCapture() -> i32;
    fn InvalidateRect(hWnd: HWND, lpRect: *const RECT, bErase: i32) -> i32;
}

// RtlGetVersion is not affected by the application's compatibility manifest.
// Older Windows accepts 0x11 but treats it as WDA_MONITOR (black capture), so
// SetWindowDisplayAffinity success alone is NOT sufficient to enable live mode.
#[repr(C)]
struct OsVersion {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}
#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(version: *mut OsVersion) -> i32;
}

fn supports_capture_exclusion() -> bool {
    let mut version: OsVersion = unsafe { mem::zeroed() };
    version.size = mem::size_of::<OsVersion>() as u32;
    unsafe {
        RtlGetVersion(&mut version) == 0
            && (version.major > 10 || (version.major == 10 && version.build >= 19041))
            && DwmIsCompositionEnabled()
                .map(|v| v.as_bool())
                .unwrap_or(false)
    }
}

/// Selection rectangle: (x, y, width, height, `is_auto`)
type Selection = Option<(f32, f32, f32, f32, bool)>;
/// Plain rectangle: (x, y, width, height)
type Rect = Option<(f32, f32, f32, f32)>;

/// Neutral gray selection outline, one physical pixel wide.
const BORDER_GRAY: f32 = 160.0;

/// AlphaBlend parameters for the semi-transparent mask
const MASK_BLEND: BLENDFUNCTION = BLENDFUNCTION {
    BlendOp: AC_SRC_OVER as u8,
    BlendFlags: 0,
    SourceConstantAlpha: 38,
    AlphaFormat: 0,
};

/// Internal state for the overlay window
struct OverlayState {
    options: super::CaptureOptions,
    pointer: (i32, i32),
    /// True only after capture exclusion and the refresh timer are enabled.
    live: bool,
    cache_bits: *mut u8,
    /// Raw screenshot BGRA data (directly from BitBlt, avoids RGBA round-trip)
    screenshot_bgra: Vec<u8>,
    /// Screen width in physical pixels
    screen_w: u32,
    /// Screen height in physical pixels
    screen_h: u32,
    /// Virtual desktop X origin in screen coordinates
    virtual_left: i32,
    /// Virtual desktop Y origin in screen coordinates
    virtual_top: i32,
    /// Primary monitor width in physical pixels
    primary_w: u32,
    /// Primary monitor height in physical pixels
    primary_h: u32,
    /// Current selection: (x, y, w, h, is_auto)
    selection: Selection,
    /// Mouse drag start point
    drag_start: Option<(f32, f32)>,
    /// Auto-detected window rectangle
    auto_rect: Rect,
    /// Screenshot result; set to exit the message loop
    result: Option<super::CaptureOutcome>,
    /// Memory DC holding the cached screenshot bitmap (ready for BitBlt)
    hdc_cache: HDC,
    /// Cached screenshot bitmap
    hbmp_cache: HBITMAP,
    /// Memory DC holding the cached full-screen mask (created once, reused)
    hdc_mask: HDC,
    /// Cached full-screen mask bitmap
    hbmp_mask: HBITMAP,
    /// CPU-accessible double buffer for anti-aliased selection edges.
    double_bits: *mut u8,
    /// Rounded corner radius used by the selection border
    corner_radius: u32,
    /// Whether the cache has been initialized
    cache_initialized: bool,
    /// Double-buffer DC (prevents flicker during WM_MOUSEMOVE)
    hdc_double: HDC,
    /// Double-buffer bitmap
    hbmp_double: HBITMAP,
}

// Thread-local storage for accessing state from the window procedure
thread_local! {
    static OVERLAY_STATE: Cell<*mut OverlayState> = const { Cell::new(std::ptr::null_mut()) };
}

/// Registers the window class and creates the overlay window.
fn create_overlay_window() -> Result<HWND, super::CaptureError> {
    let class_name: Vec<u16> = "ScreenCaptureOverlay\0".encode_utf16().collect();

    let hinstance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| super::CaptureError::GetModuleHandleFailed { source: e })?;

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_wnd_proc),
        hInstance: hinstance.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_CROSS) }.unwrap_or(HCURSOR::default()),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };

    // Register the window class. If already registered (atom == 0), ignore the error and proceed.
    let _ = unsafe { RegisterClassW(&wc) };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(std::ptr::null()),
            WS_POPUP,
            0,
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            hinstance,
            None,
        )
    };

    if hwnd.0 == 0 {
        return Err(super::CaptureError::CreateOverlayWindowFailed);
    }

    log::info!("Overlay window created, hwnd={}", hwnd.0);
    Ok(hwnd)
}

/// Initializes the screenshot cache (DIB bitmaps, CPU-accessible double buffer, mask).
/// Called only on the first paint.
fn init_screenshot_cache(state: &mut OverlayState, hdc: HDC) {
    let w = state.screen_w as i32;
    let h = state.screen_h as i32;

    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };

    // Screenshot cache DIB: system memory allocation, compatible with large virtual desktops
    let mut cache_bits: *mut c_void = std::ptr::null_mut();
    state.hdc_cache = unsafe { CreateCompatibleDC(hdc) };
    state.hbmp_cache = match unsafe {
        CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut cache_bits, HANDLE(0), 0)
    } {
        Ok(bmp) => bmp,
        Err(e) => {
            log::error!("Failed to create screenshot cache DIB: {e}");
            return;
        }
    };
    unsafe { SelectObject(state.hdc_cache, state.hbmp_cache) };
    // Direct memcpy of BGRA data into the DIB, one less GDI call than StretchDIBits
    if !cache_bits.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(
                state.screenshot_bgra.as_ptr(),
                cache_bits as *mut u8,
                (w * h * 4) as usize,
            );
        }
    }

    // Double-buffer DIB: prevents flicker during WM_MOUSEMOVE repaints
    let mut db_bits: *mut c_void = std::ptr::null_mut();
    state.hdc_double = unsafe { CreateCompatibleDC(hdc) };
    state.hbmp_double =
        match unsafe { CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut db_bits, HANDLE(0), 0) } {
            Ok(bmp) => bmp,
            Err(e) => {
                log::error!("Failed to create double-buffer DIB: {e}");
                return;
            }
        };
    unsafe { SelectObject(state.hdc_double, state.hbmp_double) };
    state.double_bits = db_bits as *mut u8;

    state.cache_bits = cache_bits as *mut u8;
    state.cache_initialized = !state.cache_bits.is_null() && !state.double_bits.is_null();

    // Full-screen mask bitmap; rounded edges are composited into the double buffer.
    let (mask_dc, mask_bmp) = create_mask_bitmap(hdc, w, h);
    state.hdc_mask = mask_dc;
    state.hbmp_mask = mask_bmp;
}

/// Creates a 32-bit black mask bitmap with an alpha channel.
fn create_mask_bitmap(hdc: HDC, w: i32, h: i32) -> (HDC, HBITMAP) {
    let mask_dc = unsafe { CreateCompatibleDC(hdc) };
    let mask_bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };
    let mut bits = std::ptr::null_mut();
    let mask_bmp = match unsafe {
        CreateDIBSection(
            HDC::default(),
            &mask_bi,
            DIB_RGB_COLORS,
            &mut bits,
            HANDLE(0),
            0,
        )
    } {
        Ok(bmp) => bmp,
        Err(e) => {
            log::warn!("Failed to create mask bitmap: {e}");
            unsafe { DeleteDC(mask_dc) };
            return (mask_dc, HBITMAP::default());
        }
    };
    unsafe { SelectObject(mask_dc, mask_bmp) };
    if !bits.is_null() {
        let pixel_count = (w * h) as usize;
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count) };
        for p in pixels.iter_mut() {
            *p = 0xFF00_0000;
        }
    }
    (mask_dc, mask_bmp)
}

/// Blends a mask region onto the specified DC.
fn blend_mask(hdc: HDC, mask_dc: HDC, x: i32, y: i32, w: i32, h: i32, blend: BLENDFUNCTION) {
    if w > 0 && h > 0 {
        unsafe { AlphaBlend(hdc, x, y, w, h, mask_dc, x, y, w, h, blend) };
    }
}

/// Composite only the perimeter strips, using signed-distance pixel coverage.
/// The screenshot is opaque, so source-over can be flattened directly into BGRA.
fn draw_smooth_edges(state: &OverlayState, sx: i32, sy: i32, sw: i32, sh: i32) {
    if sw <= 0 || sh <= 0 || state.double_bits.is_null() {
        return;
    }
    // Complete all pending GDI writes before accessing the DIB from the CPU.
    unsafe {
        let _ = GdiFlush();
    }
    let hw = sw as f32 / 2.0;
    let hh = sh as f32 / 2.0;
    let radius = (state.corner_radius as f32).min(hw).min(hh);
    let band = radius.ceil() as i32 + 2;
    let left = sx.max(0);
    let right = (sx + sw).min(state.screen_w as i32);
    let top = sy.max(0);
    let bottom = (sy + sh).min(state.screen_h as i32);
    for y in top..bottom {
        let corner_row = y - sy < band || sy + sh - 1 - y < band;
        let mut x = left;
        while x < right {
            if !corner_row && x >= sx + band && x < sx + sw - band {
                x = (sx + sw - band).max(x + 1);
                continue;
            }
            let qx = (x as f32 + 0.5 - (sx as f32 + hw)).abs() - (hw - radius);
            let qy = (y as f32 + 0.5 - (sy as f32 + hh)).abs() - (hh - radius);
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            // Outer boundary at the capture bounds; one-pixel stroke sits inside.
            let inside = (0.5 - distance).clamp(0.0, 1.0);
            let inner = (-0.5 - distance).clamp(0.0, 1.0);
            let stroke = (inside - inner).clamp(0.0, 1.0);
            let mask = inside * MASK_BLEND.SourceConstantAlpha as f32 / 255.0;
            let index = (y as usize * state.screen_w as usize + x as usize) * 4;
            for channel in 0..3 {
                let source = state.screenshot_bgra[index + channel] as f32;
                let value = source * (1.0 - mask) * (1.0 - stroke) + BORDER_GRAY * stroke;
                unsafe {
                    *state.double_bits.add(index + channel) = value.round() as u8;
                }
            }
            unsafe {
                *state.double_bits.add(index + 3) = 255;
            }
            x += 1;
        }
    }
}

/// Refresh only from a desktop that excludes this HWND. Never hide/show the
/// selector per frame: that flickers and races DWM's asynchronous composition.
fn refresh_desktop(state: &mut OverlayState) {
    if !state.live || !state.cache_initialized {
        return;
    }
    // A remote-session/composition change can invalidate exclusion mid-session.
    if !unsafe { DwmIsCompositionEnabled() }
        .map(|v| v.as_bool())
        .unwrap_or(false)
    {
        state.live = false;
        log::warn!("DWM composition stopped; freezing selector at last clean frame");
        return;
    }
    unsafe {
        let screen = GetDC(HWND::default());
        if screen.0 == 0 {
            return;
        }
        let result = BitBlt(
            state.hdc_cache,
            0,
            0,
            state.screen_w as i32,
            state.screen_h as i32,
            screen,
            state.virtual_left,
            state.virtual_top,
            SRCCOPY,
        );
        let _ = ReleaseDC(HWND::default(), screen);
        if result.is_ok() {
            let _ = GdiFlush();
            std::ptr::copy_nonoverlapping(
                state.cache_bits,
                state.screenshot_bgra.as_mut_ptr(),
                state.screenshot_bgra.len(),
            );
        }
    }
}

/// Draw guides and a nearest-neighbor 5x pixel loupe into the presentation DIB
/// only. Sampling always uses the clean source, never the dimmed/annotated frame.
fn draw_pointer_aids(state: &OverlayState) {
    if !state.cache_initialized {
        return;
    }
    unsafe {
        let _ = GdiFlush();
    }
    let w = state.screen_w as i32;
    let h = state.screen_h as i32;
    let (px, py) = state.pointer;
    let pixel = |x: i32, y: i32, color: [u8; 4]| {
        if x >= 0 && x < w && y >= 0 && y < h {
            let index = (y as usize * w as usize + x as usize) * 4;
            unsafe {
                std::ptr::copy_nonoverlapping(color.as_ptr(), state.double_bits.add(index), 4);
            }
        }
    };
    if state.options.show_crosshair {
        for x in 0..w {
            if (x - px).abs() > 5 {
                pixel(
                    x,
                    py,
                    if x % 8 < 4 {
                        [240, 240, 240, 255]
                    } else {
                        [35, 35, 35, 255]
                    },
                );
            }
        }
        for y in 0..h {
            if (y - py).abs() > 5 {
                pixel(
                    px,
                    y,
                    if y % 8 < 4 {
                        [240, 240, 240, 255]
                    } else {
                        [35, 35, 35, 255]
                    },
                );
            }
        }
    }
    if state.options.show_magnifier {
        const SIZE: i32 = 115; // 23 source pixels at 5x
        let left = if px + 24 + SIZE + 2 < w {
            px + 24
        } else {
            px - SIZE - 24
        }
        .clamp(0, (w - SIZE).max(0));
        let top = if py + 24 + SIZE + 2 < h {
            py + 24
        } else {
            py - SIZE - 24
        }
        .clamp(0, (h - SIZE).max(0));
        for y in -2..SIZE + 2 {
            for x in -2..SIZE + 2 {
                let color = if x < 0 || y < 0 || x >= SIZE || y >= SIZE {
                    [235, 235, 235, 255]
                } else if (55..60).contains(&x) && (y == 55 || y == 59)
                    || (55..60).contains(&y) && (x == 55 || x == 59)
                {
                    [40, 100, 255, 255]
                } else {
                    let sx = (px + x / 5 - 11).clamp(0, w - 1);
                    let sy = (py + y / 5 - 11).clamp(0, h - 1);
                    let i = (sy as usize * w as usize + sx as usize) * 4;
                    [
                        state.screenshot_bgra[i],
                        state.screenshot_bgra[i + 1],
                        state.screenshot_bgra[i + 2],
                        255,
                    ]
                };
                pixel(left + x, top + y, color);
            }
        }
    }
}

/// Draws the selection mask and border on the given DC.
fn draw_selection(target: HDC, state: &OverlayState) {
    if !state.cache_initialized {
        return;
    }
    let hdc = state.hdc_double;
    let w = state.screen_w as i32;
    let h = state.screen_h as i32;

    if state.cache_initialized {
        unsafe {
            let _ = BitBlt(hdc, 0, 0, w, h, state.hdc_cache, 0, 0, SRCCOPY);
        };
    }

    if state.hbmp_mask.0 == 0 {
        return;
    }

    // Auto-detection can select the whole desktop before any drag. Do not
    // shade that implicit selection: the idle desktop must remain unchanged.
    // Only an explicit dragged region gets the selection tint and outline.
    if let Some(&(sel_x, sel_y, sel_w, sel_h, false)) = state.selection.as_ref() {
        let sx = sel_x as i32;
        let sy = sel_y as i32;
        let sw = sel_w as i32;
        let sh = sel_h as i32;
        // Shade only the selected rectangle, then restore/antialias its
        // rounded cutouts from the clean desktop in draw_smooth_edges.
        blend_mask(hdc, state.hdc_mask, sx, sy, sw, sh, MASK_BLEND);
        draw_smooth_edges(state, sx, sy, sw, sh);
    }
    draw_pointer_aids(state);
    unsafe {
        let _ = BitBlt(target, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
    }
}

/// Handles manual drag selection: computes the selection rectangle from the
/// drag start point and the current mouse position.
fn handle_drag(state: &mut OverlayState, x: f32, y: f32) {
    if let Some((sx, sy)) = state.drag_start {
        let rx = sx.min(x);
        let ry = sy.min(y);
        let rw = (x - sx).abs();
        let rh = (y - sy).abs();
        state.selection = Some((rx, ry, rw, rh, false));
    }
}

/// Handles auto window detection: finds the window under the cursor and
/// highlights its border. Falls back to a default region based on which
/// monitor (primary/secondary) the cursor is on.
fn handle_auto_detect(state: &mut OverlayState, x: f32, y: f32) {
    // (x, y) are client-area coordinates. The overlay window is at the virtual
    // desktop origin, so client-area + virtual_left/top = screen coordinates.
    let screen_x = x as i32 + state.virtual_left;
    let screen_y = y as i32 + state.virtual_top;

    if let Some(win) = super::find_window_at_point(screen_x, screen_y) {
        // win.rect is in screen coordinates; convert to client-area coordinates
        let rx = (win.rect.left - state.virtual_left) as f32;
        let ry = (win.rect.top - state.virtual_top) as f32;
        let rw = (win.rect.right - win.rect.left) as f32;
        let rh = (win.rect.bottom - win.rect.top) as f32;
        let sf = state.screen_w as f32;
        let shf = state.screen_h as f32;
        let cx = rx.max(0.0).min(sf);
        let cy = ry.max(0.0).min(shf);
        let cw = (rx + rw).min(sf) - cx;
        let ch = (ry + rh).min(shf) - cy;
        state.auto_rect = Some((cx, cy, cw, ch));
        state.selection = Some((cx, cy, cw, ch, true));
    } else {
        // No window found; default region depends on which monitor the cursor is on
        let sf = state.screen_w as f32;
        let shf = state.screen_h as f32;
        let is_on_primary = screen_x >= 0
            && screen_x < state.primary_w as i32
            && screen_y >= 0
            && screen_y < state.primary_h as i32;
        if is_on_primary {
            // Cursor on primary monitor → select only the primary area
            let px = ((-state.virtual_left as f32).max(0.0)).min(sf);
            let py = ((-state.virtual_top as f32).max(0.0)).min(shf);
            let pw = (state.primary_w as f32).min(sf - px);
            let ph = (state.primary_h as f32).min(shf - py);
            let primary = (px, py, pw, ph);
            state.auto_rect = Some(primary);
            state.selection = Some((primary.0, primary.1, primary.2, primary.3, true));
        } else {
            // Cursor on secondary monitor → full virtual desktop
            let full = (0.0, 0.0, sf, shf);
            state.auto_rect = Some(full);
            state.selection = Some((full.0, full.1, full.2, full.3, true));
        }
    }
}

/// Double-buffered redraw to screen (draw_selection includes BitBlt, no extra copy needed).
fn double_buffer_redraw(hwnd: HWND, state: &OverlayState) {
    let hdc = unsafe { GetDC(hwnd) };
    draw_selection(hdc, state);
    unsafe {
        let _ = ReleaseDC(hwnd, hdc);
    }
}

/// Overlay window procedure: handles mouse events (drag selection / auto window
/// detection), keyboard events (ESC to cancel), painting, and resource cleanup.
unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = OVERLAY_STATE.with(std::cell::Cell::get);

        match msg {
            WM_SETCURSOR => {
                SetCursor(
                    LoadCursorW(None, IDC_CROSS)
                        .unwrap_or_default(),
                );
                LRESULT(1)
            }
            WM_TIMER if wparam.0 == 1 => {
                if let Some(state) = state_ptr.as_mut() {
                    refresh_desktop(state);
                    super::enumerate_windows();
                    if state.drag_start.is_none() {
                        handle_auto_detect(state, state.pointer.0 as f32, state.pointer.1 as f32);
                    }
                    double_buffer_redraw(hwnd, state);
                }
                LRESULT(0)
            }
            WM_HOTKEY if wparam.0 == 0x5343 => {
                if let Some(state) = state_ptr.as_mut() {
                    state.result = Some(super::CaptureOutcome::Cancelled);
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => {
                // Prevent the overlay from activating, so the target window doesn't lose focus
                LRESULT(MA_NOACTIVATE as isize)
            }
            WM_CREATE => LRESULT(0),
            WM_PAINT => {
                if let Some(state) = state_ptr.as_mut() {
                    let mut ps = PAINTSTRUCT::default();
                    let hdc = BeginPaint(hwnd, &mut ps);
                    if !state.cache_initialized {
                        init_screenshot_cache(state, hdc);
                    }
                    draw_selection(hdc, state);
                    EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(state) = state_ptr.as_mut() {
                    let x = (lparam.0 as i16 as i32).clamp(0, state.screen_w as i32) as f32;
                    let y = ((lparam.0 >> 16) as i16 as i32).clamp(0, state.screen_h as i32) as f32;
                    state.pointer = (x as i32, y as i32);
                    log::debug!("WM_LBUTTONDOWN: ({x:.0}, {y:.0})");
                    state.drag_start = Some((x, y));
                    if let Some((rx, ry, rw, rh)) = state.auto_rect {
                        state.selection = Some((rx, ry, rw, rh, true));
                    }
                    SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(state) = state_ptr.as_mut() {
                    let x = (lparam.0 as i16 as i32).clamp(0, state.screen_w as i32) as f32;
                    let y = ((lparam.0 >> 16) as i16 as i32).clamp(0, state.screen_h as i32) as f32;
                    state.pointer = (x as i32, y as i32);
                    if state.drag_start.is_some() {
                        handle_drag(state, x, y);
                    } else {
                        handle_auto_detect(state, x, y);
                    }
                    double_buffer_redraw(hwnd, state);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(state) = state_ptr.as_mut() {
                    refresh_desktop(state);
                    state.drag_start = None;
                    ReleaseCapture();

                    let final_sel = state.selection.or_else(|| {
                        state
                            .auto_rect
                            .map(|(rx, ry, rw, rh)| (rx, ry, rw, rh, true))
                    });

                    match final_sel {
                        Some((sx, sy, sw, sh, _)) if sw > 0.0 && sh > 0.0 => {
                            // Clamp the selection to screen bounds to prevent cropping errors
                            // when windows extend beyond the display
                            let screen_w_f = state.screen_w as f32;
                            let screen_h_f = state.screen_h as f32;
                            let cx = sx.max(0.0).min(screen_w_f);
                            let cy = sy.max(0.0).min(screen_h_f);
                            let cw = (sx + sw).min(screen_w_f) - cx;
                            let ch = (sy + sh).min(screen_h_f) - cy;
                            if cw > 0.0 && ch > 0.0 {
                                let px = cx as u32;
                                let py = cy as u32;
                                let pw = cw as u32;
                                let ph = ch as u32;
                                log::debug!("WM_LBUTTONUP: selection ({px}, {py}, {pw}, {ph})");
                                log::info!("Screenshot complete: {pw}x{ph}");
                                let cropped_bgra = super::crop_rgba(
                                    &state.screenshot_bgra,
                                    state.screen_w,
                                    px,
                                    py,
                                    pw,
                                    ph,
                                );
                                // The cropped data is BGRA; convert to RGBA for the public API
                                let mut rgba = Vec::with_capacity(cropped_bgra.len());
                                for chunk in cropped_bgra.chunks_exact(4) {
                                    rgba.extend_from_slice(&[
                                        chunk[2], chunk[1], chunk[0], chunk[3],
                                    ]);
                                }
                                state.result = Some(super::CaptureOutcome::Captured {
                                    rgba,
                                    width: pw,
                                    height: ph,
                                });
                            } else {
                                log::info!("User cancelled: selection empty after cropping");
                                state.result = Some(super::CaptureOutcome::Cancelled);
                            }
                        }
                        _ => {
                            log::info!("User cancelled: no valid selection");
                            state.result = Some(super::CaptureOutcome::Cancelled);
                        }
                    }

                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                // Don't destroy the window immediately: capture the mouse and wait for
                // WM_RBUTTONUP to prevent the right-click from reaching the target window
                log::info!("User cancelled: right-click");
                if let Some(state) = state_ptr.as_mut() {
                    state.result = Some(super::CaptureOutcome::Cancelled);
                    SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                ReleaseCapture();
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if wparam.0 == 27 {
                    log::info!("User cancelled: ESC");
                    if let Some(state) = state_ptr.as_mut() {
                        state.result = Some(super::CaptureOutcome::Cancelled);
                        let _ = DestroyWindow(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(hwnd, 1);
                let _ = UnregisterHotKey(hwnd, 0x5343);
                log::debug!("Overlay window destroyed");
                if let Some(state) = state_ptr.as_mut() {
                    if state.cache_initialized {
                        let _ = DeleteDC(state.hdc_cache);
                        let _ = DeleteObject(state.hbmp_cache);
                        let _ = DeleteDC(state.hdc_double);
                        let _ = DeleteObject(state.hbmp_double);
                    }
                    if state.hbmp_mask.0 != 0 {
                        let _ = DeleteDC(state.hdc_mask);
                        let _ = DeleteObject(state.hbmp_mask);
                    }
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Captures an interactive screenshot via the overlay.
pub(crate) fn capture_interactive(
    bgra: Vec<u8>,
    screen_w: u32,
    screen_h: u32,
    virtual_left: i32,
    virtual_top: i32,
    primary_w: u32,
    primary_h: u32,
    corner_radius: u32,
    options: super::CaptureOptions,
) -> Result<super::CaptureOutcome, super::CaptureError> {
    super::enumerate_windows();

    let hwnd = create_overlay_window()?;

    let mut pointer = windows::Win32::Foundation::POINT::default();
    let _ = unsafe { GetCursorPos(&mut pointer) };
    let mut state = Box::new(OverlayState {
        options,
        pointer: (
            (pointer.x - virtual_left).clamp(0, screen_w as i32 - 1),
            (pointer.y - virtual_top).clamp(0, screen_h as i32 - 1),
        ),
        live: false,
        cache_bits: std::ptr::null_mut(),
        screenshot_bgra: bgra,
        screen_w,
        screen_h,
        virtual_left,
        virtual_top,
        primary_w,
        primary_h,
        selection: None,
        drag_start: None,
        auto_rect: None,
        result: None,
        hdc_cache: HDC::default(),
        hbmp_cache: HBITMAP::default(),
        hdc_mask: HDC::default(),
        hbmp_mask: HBITMAP::default(),
        double_bits: std::ptr::null_mut(),
        corner_radius,
        cache_initialized: false,
        hdc_double: HDC::default(),
        hbmp_double: HBITMAP::default(),
    });

    if !options.freeze_screen {
        let excluded = supports_capture_exclusion()
            && unsafe { SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(0x11)) }.is_ok();
        if excluded {
            state.live = unsafe { SetTimer(hwnd, 1, 33, None) } != 0;
        }
        if !state.live {
            log::warn!("Live selector unavailable (requires Windows 10 2004+, DWM and capture exclusion); using frozen snapshot");
        }
    }
    handle_auto_detect(
        &mut state,
        (pointer.x - virtual_left) as f32,
        (pointer.y - virtual_top) as f32,
    );
    let state_ptr = Box::into_raw(state);
    OVERLAY_STATE.with(|s| s.set(state_ptr));
    let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };

    set_window_pos(
        hwnd,
        HWND::default(),
        virtual_left,
        virtual_top,
        screen_w as i32,
        screen_h as i32,
    );
    unsafe { ShowWindow(hwnd, SW_SHOW) };
    unsafe {
        let _ = RegisterHotKey(hwnd, 0x5343, MOD_NOREPEAT, VK_ESCAPE.0 as u32);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    };

    // Force an initial paint
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };

    let mut msg = MSG::default();
    unsafe {
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 || ret.0 == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = DestroyWindow(hwnd);
    };

    let state = unsafe { Box::from_raw(state_ptr) };
    OVERLAY_STATE.with(|s| s.set(std::ptr::null_mut()));

    Ok(state.result.unwrap_or(super::CaptureOutcome::Cancelled))
}

/// Sets the overlay window position and size.
fn set_window_pos(hwnd: HWND, hwnd_insert_after: HWND, x: i32, y: i32, cx: i32, cy: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            hwnd_insert_after,
            x,
            y,
            cx,
            cy,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}
