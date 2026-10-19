//! Interactive screenshot library — provides full-screen capture, auto window
//! detection, selection overlay, and RGBA cropping on Windows.
//!
//! # System Requirements
//!
//! - **OS**: Windows 7+
//! - **Dependencies**: `windows` crate (Win32 API), `log` (optional logging facade)
//!
//! # Features
//!
//! - **Full-screen capture**: captures the entire desktop with native Win32 GDI
//! - **Auto window detection**: highlights window borders on mouse hover
//! - **Manual selection**: click and drag to select any region
//! - **DPI aware**: automatically declares DPI awareness for accurate coordinates
//! - **Framework-free**: returns raw RGBA pixels, compatible with any UI framework
//!
//! The entry point is [`start_capture`]; see its documentation for usage examples.

// Friendly compile-time error on non-Windows platforms (skipped during doc builds)
#[cfg(not(any(windows, doc)))]
compile_error!("capture-overlay crate only supports Windows.");

/// Image processing module (RGBA cropping)
mod image;
/// Native Win32 screenshot overlay window
mod native_overlay;
/// Window enumeration and lookup
mod windows;

pub(crate) use image::crop_rgba;
pub(crate) use windows::enumerate_windows;
pub(crate) use windows::find_window_at_point;

use ::windows::Win32::Foundation::{HANDLE, HWND};
use ::windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    GetDeviceCaps, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    LOGPIXELSX, RGBQUAD, SRCCOPY,
};
use ::windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware;
use ::windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SYSTEM_METRICS_INDEX};
use std::ffi::c_void;
use std::mem;
use std::sync::Once;

// Virtual desktop metrics constants
const SM_XVIRTUALSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(76);
const SM_YVIRTUALSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(77);
const SM_CXVIRTUALSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(78);
const SM_CYVIRTUALSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(79);
// Primary monitor metrics constants
const SM_CXSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(0);
const SM_CYSCREEN: SYSTEM_METRICS_INDEX = SYSTEM_METRICS_INDEX(1);

/// Errors that can occur during screenshot capture
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Failed to get the screen device context (DC), usually due to low system resources
    #[error("Failed to get screen device context")]
    GetScreenDcFailed,

    /// Failed to create a compatible memory DC, usually due to low system resources
    #[error("Failed to create compatible memory DC")]
    CreateCompatibleDcFailed,

    /// Failed to create a DIB section (used for screenshot buffer or mask)
    #[error("Failed to create DIB section: {source}")]
    CreateDIBSectionFailed {
        /// The underlying Windows error
        #[source]
        source: ::windows::core::Error,
    },

    /// Failed to create the overlay window (possibly low system resources or window class conflict)
    #[error("Failed to create overlay window")]
    CreateOverlayWindowFailed,

    /// Failed to get the module handle
    #[error("Failed to get module handle: {source}")]
    GetModuleHandleFailed {
        /// The underlying Windows error
        #[source]
        source: ::windows::core::Error,
    },
}

/// Convenience alias for screenshot operation results, defaulting to `CaptureError`
pub type CaptureResult<T> = Result<T, CaptureError>;

/// Full-screen capture data
pub struct CapturedScreen {
    /// RGBA pixel data
    pub rgba: Vec<u8>,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Screen scale factor
    pub scale_factor: f32,
    /// Virtual desktop X origin in screen coordinates (negative = secondary monitor on the left)
    pub virtual_left: i32,
    /// Virtual desktop Y origin in screen coordinates (negative = secondary monitor above)
    pub virtual_top: i32,
}

/// Settings for the native interactive selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureOptions {
    /// Show pointer-aligned guides and use a crosshair cursor (otherwise an arrow).
    pub show_crosshair: bool,
    /// Show a pixel magnifier sampling the unmodified desktop image.
    pub show_magnifier: bool,
    /// Keep the startup snapshot instead of refreshing the desktop during selection.
    pub freeze_screen: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            show_crosshair: true,
            show_magnifier: true,
            freeze_screen: false,
        }
    }
}

#[cfg(test)]
mod option_tests {
    use super::CaptureOptions;

    #[test]
    fn selector_defaults_match_settings_ui() {
        assert_eq!(
            CaptureOptions::default(),
            CaptureOptions {
                show_crosshair: true,
                show_magnifier: true,
                freeze_screen: false,
            }
        );
    }
}

/// Outcome of an interactive screenshot session
pub enum CaptureOutcome {
    /// User completed a selection; returns cropped RGBA data and dimensions
    Captured {
        /// RGBA pixel data
        rgba: Vec<u8>,
        /// Cropped region width in pixels
        width: u32,
        /// Cropped region height in pixels
        height: u32,
    },
    /// User cancelled (right-click or empty selection)
    Cancelled,
}

/// Raw virtual desktop capture data (internal; BGRA format to avoid RGBA round-trip conversion)
struct VirtualDesktopCapture {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    scale_factor: f32,
    virtual_left: i32,
    virtual_top: i32,
    primary_w: u32,
    primary_h: u32,
}

/// Captures the current virtual desktop as a full-screen screenshot.
///
/// Uses native Win32 GDI to capture screen pixel data, returning RGBA pixel data
/// along with screen information. On multi-monitor setups, captures the entire
/// virtual desktop (from top-left to bottom-right of the combined display area).
///
/// # Errors
///
/// - [`GetScreenDcFailed`](CaptureError::GetScreenDcFailed) — failed to get screen DC
/// - [`CreateCompatibleDcFailed`](CaptureError::CreateCompatibleDcFailed) — failed to create memory DC
/// - [`CreateDIBSectionFailed`](CaptureError::CreateDIBSectionFailed) — failed to create DIB section
pub fn capture_full_screen() -> CaptureResult<CapturedScreen> {
    let cap = capture_virtual_desktop()?;
    let pixel_count = (cap.width * cap.height) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in cap.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
    }
    Ok(CapturedScreen {
        rgba,
        width: cap.width,
        height: cap.height,
        scale_factor: cap.scale_factor,
        virtual_left: cap.virtual_left,
        virtual_top: cap.virtual_top,
    })
}

/// Captures the virtual desktop and returns raw BGRA data (internal; skips RGBA conversion).
fn capture_virtual_desktop() -> CaptureResult<VirtualDesktopCapture> {
    // SAFETY: All Win32 API calls below receive valid parameters or null handles:
    // - GetDC(HWND::default()) gets the screen DC, always safe
    // - GetDeviceCaps reads properties from a valid DC
    // - GetSystemMetrics reads system metrics, always safe
    // - CreateCompatibleDC/BitBlt operate on standard GDI objects
    // - CreateDIBSection is wrapped by the `windows` crate and returns Err on failure
    // - The `bits` pointer is written by CreateDIBSection and guaranteed to point to valid memory
    // - All resources are released via DeleteDC/DeleteObject/ReleaseDC before the function returns
    unsafe {
        let hdc_screen = GetDC(HWND::default());
        if hdc_screen.0 == 0 {
            return Err(CaptureError::GetScreenDcFailed);
        }

        // Get virtual desktop bounds (covers all monitors)
        let virtual_left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let virtual_top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        // Get primary monitor dimensions (to distinguish mouse on primary vs secondary)
        let primary_w = GetSystemMetrics(SM_CXSCREEN);
        let primary_h = GetSystemMetrics(SM_CYSCREEN);
        let dpi = GetDeviceCaps(hdc_screen, LOGPIXELSX);
        let scale_factor = dpi as f32 / 96.0;

        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.0 == 0 {
            ReleaseDC(HWND::default(), hdc_screen);
            return Err(CaptureError::CreateCompatibleDcFailed);
        }

        // CreateDIBSection returns the pixel buffer pointer directly, avoiding GetDIBits double-copy
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // negative: top-down bitmap
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let hbmp = CreateDIBSection(hdc_mem, &bi, DIB_RGB_COLORS, &mut bits, HANDLE(0), 0)
            .map_err(|e| CaptureError::CreateDIBSectionFailed { source: e })?;

        SelectObject(hdc_mem, hbmp);
        // Capture from the virtual desktop origin, covering all monitors
        let _ = BitBlt(
            hdc_mem,
            0,
            0,
            width,
            height,
            hdc_screen,
            virtual_left,
            virtual_top,
            SRCCOPY,
        );

        // Flush queued GDI writes before reading the DIB from the CPU.
        let _ = GdiFlush();
        // `bits` points to DIB pixel data; keep BGRA format (no RGBA conversion)
        let pixel_count = (width * height) as usize;
        let bgra_slice = std::slice::from_raw_parts(bits as *const u8, pixel_count * 4);
        let bgra = bgra_slice.to_vec();

        DeleteDC(hdc_mem);
        DeleteObject(hbmp);
        ReleaseDC(HWND::default(), hdc_screen);

        log::debug!("Virtual desktop: {width}x{height}, origin=({virtual_left},{virtual_top}), scale={scale_factor}");

        Ok(VirtualDesktopCapture {
            bgra,
            width: width as u32,
            height: height as u32,
            scale_factor,
            virtual_left,
            virtual_top,
            primary_w: primary_w as u32,
            primary_h: primary_h as u32,
        })
    }
}

/// Declares the process as DPI-aware (only takes effect on first call).
///
/// Ensures `GetDeviceCaps` returns physical pixels and `WM_MOUSEMOVE` coordinates
/// correspond one-to-one with captured pixels, preventing cropping misalignment on
/// high-DPI displays.
fn enable_dpi_aware() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let _ = SetProcessDPIAware();
    });
}

/// Starts a complete interactive screenshot session.
///
/// Blocks the current thread until the user completes or cancels the operation.
///
/// Internally performs: enumerate windows → capture full screen → show selection overlay →
/// auto window detection / manual drag selection → crop image.
///
/// # Returns
///
/// - `Ok(CaptureOutcome::Captured { rgba, width, height })` — the user made a valid selection
/// - `Ok(CaptureOutcome::Cancelled)` — the user right-clicked or the selection was empty
///
/// # Errors
///
/// - [`GetScreenDcFailed`](CaptureError::GetScreenDcFailed) — failed to get screen DC
/// - [`CreateCompatibleDcFailed`](CaptureError::CreateCompatibleDcFailed) — failed to create memory DC
/// - [`CreateDIBSectionFailed`](CaptureError::CreateDIBSectionFailed) — failed to create DIB section
/// - [`CreateOverlayWindowFailed`](CaptureError::CreateOverlayWindowFailed) — failed to create overlay window
/// - [`GetModuleHandleFailed`](CaptureError::GetModuleHandleFailed) — failed to get module handle
///
/// # Example
///
/// ```no_run
/// use capture_overlay::start_capture;
///
/// match start_capture() {
///     Ok(capture_overlay::CaptureOutcome::Captured { rgba, width, height }) => {
///         // Process the captured screenshot
///     }
///     Ok(capture_overlay::CaptureOutcome::Cancelled) => {
///         // User cancelled
///     }
///     Err(e) => {
///         eprintln!("Screenshot failed: {e}");
///     }
/// }
/// ```
pub fn start_capture() -> CaptureResult<CaptureOutcome> {
    start_capture_with_radius(13)
}

/// Starts interactive capture with a rounded selection border radius.
pub fn start_capture_with_radius(corner_radius: u32) -> CaptureResult<CaptureOutcome> {
    start_capture_with_options(corner_radius, CaptureOptions::default())
}

/// Starts interactive capture with explicit selector settings.
///
/// Blocks the calling thread; call from a worker thread in UI applications.
/// Live selection requires Windows 10 version 2004+ and DWM composition. If
/// capture exclusion is unavailable, the selector safely uses the startup snapshot
/// and logs a warning rather than capturing its own overlay.
pub fn start_capture_with_options(
    corner_radius: u32,
    options: CaptureOptions,
) -> CaptureResult<CaptureOutcome> {
    // Enable DPI awareness so physical pixels match screenshot resolution
    enable_dpi_aware();

    log::info!("Starting screenshot capture");
    let cap = capture_virtual_desktop()?;

    // Launch the native overlay (blocks until the user completes or cancels)
    let outcome = native_overlay::capture_interactive(
        cap.bgra,
        cap.width,
        cap.height,
        cap.virtual_left,
        cap.virtual_top,
        cap.primary_w,
        cap.primary_h,
        corner_radius,
        options,
    )?;

    Ok(outcome)
}
