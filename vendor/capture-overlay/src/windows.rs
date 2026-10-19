use std::cell::RefCell;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowTextW, IsWindowVisible,
    GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

// DWMWA_EXTENDED_FRAME_BOUNDS = 9
const DWMWA_EXTENDED_FRAME_BOUNDS: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(9i32);
// DWMWA_CLOAKED = 14
const DWMWA_CLOAKED: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(14i32);

/// Information about a window
#[derive(Clone, Debug)]
pub(crate) struct WindowInfo {
    /// Window rectangle in screen coordinates
    pub rect: RECT,
}

thread_local! {
    static WINDOWS: RefCell<Vec<WindowInfo>> = const { RefCell::new(Vec::new()) };
}

/// EnumWindows callback: enumerates visible windows, filtering out cloaked,
/// tool windows, empty titles, and overly small windows.
///
/// # Safety
///
/// This function is called by the Windows kernel via `EnumWindows` and must
/// conform to the `unsafe extern "system"` ABI convention. `hwnd` is provided
/// by the operating system and guaranteed to be valid.
unsafe extern "system" fn enum_windows_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
    // SAFETY: `IsWindowVisible` is provided by Windows; `hwnd` is guaranteed valid by EnumWindows.
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        // Exclude windows that are "cloaked" (Windows 10+: invisible, on other virtual desktops,
        // or suspended in the background)
        // SAFETY: DwmGetWindowAttribute writes to a BOOL; hwnd is guaranteed valid by EnumWindows.
        let mut cloaked: BOOL = BOOL(0);
        let _ = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut BOOL as *mut std::ffi::c_void,
                std::mem::size_of::<BOOL>() as u32,
            )
        };
        if cloaked.0 != 0 {
            return BOOL(1);
        }

        let mut class_buf = [0u16; 256];
        let class_len = unsafe { GetClassNameW(hwnd, &mut class_buf) };
        let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);

        // Special exemption for the taskbar (Shell_TrayWnd) and secondary taskbar
        // (Shell_SecondaryTrayWnd) — skip the WS_EX_TOOLWINDOW and size checks.
        let is_taskbar = class_name == "Shell_TrayWnd" || class_name == "Shell_SecondaryTrayWnd";

        if !is_taskbar {
            // SAFETY: `GetWindowLongW` queries the extended window style; `hwnd` is valid.
            let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
            if ex_style & WS_EX_TOOLWINDOW.0 as i32 != 0 {
                return BOOL(1);
            }
        }
        let mut title_buf = [0u16; 256];
        // SAFETY: `GetWindowTextW` writes into `title_buf`; the buffer size is the limit.
        let len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
        let title = String::from_utf16_lossy(&title_buf[..len as usize]);

        // Skip windows with no title, unless it's the taskbar
        if !is_taskbar && title.is_empty() {
            return BOOL(1);
        }

        log::debug!(
            "Enumerated window: class={class_name}, title={title}, hwnd={}",
            hwnd.0
        );

        let mut rect = RECT::default();
        // Prefer DwmGetWindowAttribute for the visible region (excludes shadows and rounded corners)
        // SAFETY: `rect` is a valid stack-allocated RECT; `DwmGetWindowAttribute` writes into it.
        let got_rect = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut RECT as *mut std::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok()
        };
        // Fall back to GetWindowRect
        if !got_rect {
            log::warn!(
                "DwmGetWindowAttribute(EXTENDED_FRAME_BOUNDS) failed, falling back to GetWindowRect, hwnd={}",
                hwnd.0
            );
            // SAFETY: `GetWindowRect` writes into `rect`; `hwnd` is valid.
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                return BOOL(1);
            }
        }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if (w > 50 && h > 50) || is_taskbar {
            WINDOWS.with(|w| w.borrow_mut().push(WindowInfo { rect }));
        }
    }
    BOOL(1)
}

/// Enumerates all currently visible windows.
pub(crate) fn enumerate_windows() {
    WINDOWS.with(|w| w.borrow_mut().clear());
    // SAFETY: `EnumWindows` is a safe Windows API; `enum_windows_proc` matches its callback signature.
    unsafe {
        let _ = EnumWindows(Some(enum_windows_proc), LPARAM(0));
    }
    WINDOWS.with(|w| log::debug!("Found {} windows", w.borrow().len()));
}

/// Finds the topmost (highest Z-order) window at the given coordinates.
///
/// Must be called after `enumerate_windows()`.
pub(crate) fn find_window_at_point(x: i32, y: i32) -> Option<WindowInfo> {
    WINDOWS.with(|w| {
        let w = w.borrow();
        for win in w.iter() {
            if x >= win.rect.left
                && x <= win.rect.right
                && y >= win.rect.top
                && y <= win.rect.bottom
            {
                return Some(win.clone());
            }
        }
        None
    })
}
