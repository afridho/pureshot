//! Temporarily hide Explorer's desktop icon views without toggling its saved setting.
//!
//! Keep the guard alive until capture completes (including cancellation/error paths).
//! Dropping it restores only views that this guard found visible and hid.

#[cfg(windows)]
mod platform {
    use windows::{
        core::{w, BOOL, PCWSTR},
        Win32::{
            Foundation::{GetLastError, SetLastError, ERROR_SUCCESS, HWND, LPARAM},
            UI::WindowsAndMessaging::{
                EnumWindows, FindWindowExW, GetClassNameW, GetParent, GetWindowThreadProcessId,
                IsWindow, IsWindowVisible, ShowWindow, SW_HIDE, SW_SHOWNA,
            },
        },
    };

    #[derive(Clone, Copy)]
    struct IconView {
        hwnd: HWND,
        parent: HWND,
        process_id: u32,
        thread_id: u32,
    }

    /// Restores desktop icon views on normal completion, cancellation, or error.
    #[must_use = "keep this guard alive until the screenshot has been captured"]
    #[derive(Default)]
    pub struct DesktopIconsGuard {
        hidden: Vec<IconView>,
    }

    #[derive(Default)]
    struct Enumeration {
        views: Vec<IconView>,
        error: Option<String>,
    }

    // FindWindowExW reports both a missing child and an actual API failure as an
    // Err. Clear last-error first so ordinary missing children are not failures.
    unsafe fn find_child(
        parent: HWND,
        after: Option<HWND>,
        class: PCWSTR,
    ) -> Result<Option<HWND>, String> {
        SetLastError(ERROR_SUCCESS);
        match FindWindowExW(Some(parent), after, class, None) {
            Ok(hwnd) => Ok(Some(hwnd)),
            Err(error) => {
                let code = GetLastError();
                if code == ERROR_SUCCESS {
                    Ok(None)
                } else {
                    Err(format!("Could not find Explorer desktop icons: {error}"))
                }
            }
        }
    }

    unsafe fn has_class(hwnd: HWND, expected: &str) -> bool {
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class);
        len > 0
            && class[..len as usize]
                .iter()
                .copied()
                .eq(expected.encode_utf16())
    }

    unsafe fn collect_views(hwnd: HWND, views: &mut Vec<IconView>) -> Result<(), String> {
        // Depending on the Explorer/Windows version, DefView belongs to either
        // Progman or one of the WorkerW windows. Never hide the host itself.
        if !has_class(hwnd, "Progman") && !has_class(hwnd, "WorkerW") {
            return Ok(());
        }
        let mut previous_def_view = None;
        while let Some(def_view) = find_child(hwnd, previous_def_view, w!("SHELLDLL_DefView"))? {
            previous_def_view = Some(def_view);
            let mut previous_icons = None;
            while let Some(icons) = find_child(def_view, previous_icons, w!("SysListView32"))? {
                previous_icons = Some(icons);
                let mut process_id = 0;
                let thread_id = GetWindowThreadProcessId(icons, Some(&mut process_id));
                if thread_id == 0 {
                    return Err("Explorer desktop icon view disappeared during discovery".into());
                }
                if !views.iter().any(|view| view.hwnd == icons) {
                    views.push(IconView {
                        hwnd: icons,
                        parent: def_view,
                        process_id,
                        thread_id,
                    });
                }
            }
        }
        Ok(())
    }

    unsafe extern "system" fn enumerate(hwnd: HWND, param: LPARAM) -> BOOL {
        let state = &mut *(param.0 as *mut Enumeration);
        match collect_views(hwnd, &mut state.views) {
            Ok(()) => BOOL(1),
            Err(error) => {
                state.error = Some(error);
                BOOL(0)
            }
        }
    }

    impl DesktopIconsGuard {
        pub fn hide(enabled: bool) -> Result<Self, String> {
            let mut guard = Self::default();
            if !enabled {
                return Ok(guard);
            }
            unsafe {
                let mut state = Enumeration::default();
                let result = EnumWindows(
                    Some(enumerate),
                    LPARAM(&mut state as *mut Enumeration as isize),
                );
                if let Some(error) = state.error {
                    return Err(error);
                }
                result.map_err(|error| {
                    format!("Could not enumerate Explorer desktop windows: {error}")
                })?;
                if state.views.is_empty() {
                    return Err("Could not find Explorer's desktop icon view".into());
                }
                for view in state.views {
                    // In particular, do not restore icons that the user had
                    // already hidden via Explorer's Show desktop icons setting.
                    if !IsWindowVisible(view.hwnd).as_bool() {
                        continue;
                    }
                    // Register restoration before changing visibility: every
                    // subsequent early return rolls back through Drop.
                    guard.hidden.push(view);
                    let _ = ShowWindow(view.hwnd, SW_HIDE);
                    if !IsWindow(Some(view.hwnd)).as_bool() || IsWindowVisible(view.hwnd).as_bool()
                    {
                        return Err("Could not hide Explorer desktop icons".into());
                    }
                }
            }
            Ok(guard)
        }
    }

    impl Drop for DesktopIconsGuard {
        fn drop(&mut self) {
            for view in self.hidden.drain(..).rev() {
                unsafe {
                    // Explorer may have restarted during capture. Do not act
                    // on a recycled handle belonging to an unrelated window.
                    let mut process_id = 0;
                    let thread_id = GetWindowThreadProcessId(view.hwnd, Some(&mut process_id));
                    if thread_id == view.thread_id
                        && process_id == view.process_id
                        && GetParent(view.hwnd).ok() == Some(view.parent)
                        && has_class(view.hwnd, "SysListView32")
                    {
                        let _ = ShowWindow(view.hwnd, SW_SHOWNA);
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub use platform::DesktopIconsGuard;
