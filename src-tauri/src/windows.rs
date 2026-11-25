use std::path::Path;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};
use windows_sys::Win32::Graphics::Gdi::{
    CombineRgn, CreateEllipticRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn,
    RGN_AND, RGN_DIFF, RGN_OR,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
};

static AUTO_CLOSE_SUPPRESS: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that suppresses focus-loss auto-close while native pickers, drags, or overlays are open.
pub struct AutoCloseGuard;

impl Drop for AutoCloseGuard {
    fn drop(&mut self) {
        AUTO_CLOSE_SUPPRESS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Temporarily suppress auto-close until the returned guard is dropped.
pub fn suppress_auto_close() -> AutoCloseGuard {
    AUTO_CLOSE_SUPPRESS.fetch_add(1, Ordering::SeqCst);
    AutoCloseGuard
}

/// Check whether auto-close is currently suppressed due to an active dialog, overlay, or drag.
pub fn is_auto_close_suppressed() -> bool {
    AUTO_CLOSE_SUPPRESS.load(Ordering::SeqCst) > 0
}

/// Tracks the last active foreground window that does not belong to Peek.
#[derive(Default)]
pub struct ForegroundTracker(AtomicIsize);

impl ForegroundTracker {
    /// Record the current OS foreground window if it is not one of Peek's own windows.
    pub fn record_if_external(&self, peek_hwnds: &[isize]) {
        #[cfg(windows)]
        unsafe {
            let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            let raw = fg.0 as isize;
            if raw != 0 && !peek_hwnds.contains(&raw) {
                self.0.store(raw, Ordering::SeqCst);
            }
        }
        #[cfg(not(windows))]
        let _ = peek_hwnds;
    }

    /// Return the current external foreground window, falling back to the last recorded external window.
    pub fn best_external(&self, peek_hwnds: &[isize]) -> Option<isize> {
        #[cfg(windows)]
        unsafe {
            let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            let raw = fg.0 as isize;
            if raw != 0
                && !peek_hwnds.contains(&raw)
                && windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(fg)).as_bool()
            {
                self.0.store(raw, Ordering::SeqCst);
                return Some(raw);
            }
            let cached = self.0.load(Ordering::SeqCst);
            if cached != 0
                && !peek_hwnds.contains(&cached)
                && windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(
                    windows::Win32::Foundation::HWND(cached as _),
                ))
                .as_bool()
            {
                return Some(cached);
            }
        }
        #[cfg(not(windows))]
        let _ = peek_hwnds;
        None
    }
}

/// Collect raw HWNDs for all of Peek's open windows so foreground/window capture excludes Peek itself.
pub fn peek_hwnds(app: &tauri::AppHandle) -> Vec<isize> {
    use tauri::Manager;
    app.webview_windows()
        .values()
        .filter_map(|win| win.hwnd().ok().map(|h| h.0 as isize))
        .collect()
}

/// Apply a crisp rounded `SetWindowRgn` clip matching the window's physical pixel bounds.
pub fn apply_rounded_window_region(
    window: &WebviewWindow,
    radius_logical: f64,
) -> Result<(), String> {
    let handle = window.hwnd().map_err(|e| e.to_string())?;
    let size = window.inner_size().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let w = (size.width as i32).max(2);
    let h = (size.height as i32).max(2);
    let dia = ((radius_logical * 2.0 * scale).round() as i32)
        .min(w)
        .min(h)
        .max(2);
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, dia, dia);
        if rgn.is_null() {
            return Err("Could not create rounded floating window region".into());
        }
        if SetWindowRgn(handle.0, rgn, 1) == 0 {
            DeleteObject(rgn);
            return Err("SetWindowRgn failed for floating window".into());
        }
    }
    Ok(())
}

/// Read a window's physical bounding rectangle `(left, top, right, bottom)`.
pub fn window_rect(window: &WebviewWindow) -> Result<(i32, i32, i32, i32), String> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let handle = window.hwnd().map_err(|e| e.to_string())?;
    let mut rc = RECT::default();
    unsafe {
        GetWindowRect(HWND(handle.0), &mut rc).map_err(|e| e.to_string())?;
    }
    Ok((rc.left, rc.top, rc.right, rc.bottom))
}

/// Atomically move and resize a frameless floating window in physical coordinates and refresh
/// its `SetWindowRgn` rounded mask without adding `WS_THICKFRAME` border chrome.
pub fn set_rounded_window_rect(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    scale: f64,
    radius_logical: f64,
) -> Result<(), String> {
    let handle = window.hwnd().map_err(|e| e.to_string())?;
    let w = width.max(2);
    let h = height.max(2);
    let dia = ((radius_logical * 2.0 * scale).round() as i32)
        .min(w)
        .min(h)
        .max(2);
    unsafe {
        if SetWindowPos(
            handle.0,
            std::ptr::null_mut(),
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, dia, dia);
        if rgn.is_null() {
            return Err("Could not create rounded floating window region".into());
        }
        if SetWindowRgn(handle.0, rgn, 1) == 0 {
            DeleteObject(rgn);
            return Err("SetWindowRgn failed for floating window".into());
        }
    }
    Ok(())
}

/// Begin native Win32 caption-less window dragging via `WM_NCLBUTTONDOWN` + `HTCAPTION`.
pub fn begin_window_drag(window: &WebviewWindow) -> Result<(), String> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, HTCAPTION, WM_NCLBUTTONDOWN};

    let handle = window.hwnd().map_err(|e| e.to_string())?;
    let hwnd = HWND(handle.0);
    unsafe {
        let _ = ReleaseCapture();
        SendMessageW(
            hwnd,
            WM_NCLBUTTONDOWN,
            Some(WPARAM(HTCAPTION as usize)),
            Some(LPARAM(0)),
        );
    }
    Ok(())
}

/// Strip all Win32 caption/border/frame styles and DWM frame chrome so a shaped
/// transparent `WS_POPUP` island never draws Windows' legacy frame halo along `SetWindowRgn` edges,
/// while updating `WS_EX_NOACTIVATE` directly without calling Tao's style-resetting `set_focusable`.
pub fn enforce_frameless_popup(window: &WebviewWindow, focusable: bool) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWM_BB_BLURREGION, DWM_BB_ENABLE,
        DWM_BLURBEHIND,
    };
    use windows::Win32::Graphics::Gdi::HRGN;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassLongPtrW, GetWindowLongW, SetClassLongPtrW, SetWindowLongW, GCLP_HBRBACKGROUND,
        GWL_EXSTYLE, GWL_STYLE, WS_BORDER, WS_CAPTION, WS_DLGFRAME, WS_EX_APPWINDOW,
        WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_NOACTIVATE, WS_EX_STATICEDGE,
        WS_EX_TOOLWINDOW, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
        WS_THICKFRAME,
    };

    let handle = window.hwnd().map_err(|error| error.to_string())?;
    let win_hwnd = HWND(handle.0);

    unsafe {
        if GetClassLongPtrW(win_hwnd, GCLP_HBRBACKGROUND) != 0 {
            SetClassLongPtrW(win_hwnd, GCLP_HBRBACKGROUND, 0);
        }
        let cur_style = GetWindowLongW(win_hwnd, GWL_STYLE) as u32;
        let strip_style = (WS_CAPTION
            | WS_THICKFRAME
            | WS_BORDER
            | WS_DLGFRAME
            | WS_SYSMENU
            | WS_MINIMIZEBOX
            | WS_MAXIMIZEBOX)
            .0;
        let next_style = (cur_style & !strip_style) | WS_POPUP.0;

        let cur_ex = GetWindowLongW(win_hwnd, GWL_EXSTYLE) as u32;
        let strip_ex = (WS_EX_WINDOWEDGE
            | WS_EX_DLGMODALFRAME
            | WS_EX_CLIENTEDGE
            | WS_EX_STATICEDGE
            | WS_EX_APPWINDOW)
            .0;
        let mut next_ex = (cur_ex & !strip_ex) | WS_EX_TOOLWINDOW.0;
        if focusable {
            next_ex &= !WS_EX_NOACTIVATE.0;
        } else {
            next_ex |= WS_EX_NOACTIVATE.0;
        }

        let frame_changed = next_style != cur_style
            || (cur_ex & strip_ex) != 0
            || (cur_ex & WS_EX_TOOLWINDOW.0) == 0;

        if next_style != cur_style {
            SetWindowLongW(win_hwnd, GWL_STYLE, next_style as i32);
        }
        if next_ex != cur_ex {
            SetWindowLongW(win_hwnd, GWL_EXSTYLE, next_ex as i32);
        }

        if frame_changed {
            let no_color = DWMWA_COLOR_NONE;
            let _ = DwmSetWindowAttribute(
                win_hwnd,
                DWMWA_BORDER_COLOR,
                &no_color as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
            let corner = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(
                win_hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&corner) as u32,
            );
            let empty_rgn = CreateRectRgn(0, 0, -1, -1);
            if !empty_rgn.is_null() {
                let bb = DWM_BLURBEHIND {
                    dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
                    fEnable: true.into(),
                    hRgnBlur: HRGN(empty_rgn),
                    fTransitionOnMaximized: false.into(),
                };
                let _ = DwmEnableBlurBehindWindow(win_hwnd, &bb);
                DeleteObject(empty_rgn);
            }
            if SetWindowPos(
                handle.0,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
    }
    Ok(())
}

/// Anchor Peek's fixed transparent stage HWND once (startup and monitor/DPI changes only),
/// enforcing `WS_POPUP` frameless styles before positioning.
pub fn anchor_island_stage(
    window: &WebviewWindow,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) -> Result<(), String> {
    enforce_frameless_popup(window, false)?;
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    let w = (size.width as i32).max(1);
    let h = (size.height as i32).max(1);
    unsafe {
        if SetWindowPos(
            handle.0,
            std::ptr::null_mut(),
            position.x,
            position.y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

unsafe fn build_island_region(
    stage_width: u32,
    region_width: u32,
    region_height: u32,
    scale: f64,
    radius_logical: f64,
    ear_px: i32,
    rounded_bottom: bool,
) -> Result<windows_sys::Win32::Graphics::Gdi::HRGN, String> {
    let sw = (stage_width as i32).max(1);
    let rw = (region_width as i32).clamp(1, sw);
    let rh = (region_height as i32).max(1);
    let left = ((sw - rw) / 2).max(0);
    let right = left + rw;
    let ear = ear_px.clamp(0, (rw / 4).max(0));

    if !rounded_bottom {
        let rect_rgn = CreateRectRgn(left, 0, right, rh);
        if rect_rgn.is_null() {
            return Err("Could not create the island's rect region".into());
        }
        return Ok(rect_rgn);
    }

    let inner_w = (rw - 2 * ear).max(2);
    // Slightly outside CSS radius so the 1-bit GDI mask stays ~0.6px inside opaque rounded pixels,
    // preventing semi-transparent edge pixels from picking up DWM's pale-cyan fallback tint.
    let radius = (((radius_logical + 1.5).max(1.0) * scale).round() as i32)
        .min(rh)
        .min(inner_w / 2)
        .max(1);
    let diameter = radius * 2;
    let rounded = CreateRoundRectRgn(
        left + ear,
        -radius,
        right - ear + 1,
        rh + 1,
        diameter,
        diameter,
    );
    if rounded.is_null() {
        return Err("Could not create the island's notch region".into());
    }
    let clip = CreateRectRgn(left + ear, 0, right - ear, rh);
    if clip.is_null() {
        DeleteObject(rounded);
        return Err("Could not create the island's clip region".into());
    }
    CombineRgn(rounded, rounded, clip, RGN_AND);
    DeleteObject(clip);

    if ear > 0 {
        let ear_band = CreateRectRgn(left, 0, right, ear.min(rh).max(1));
        if !ear_band.is_null() {
            let r_cut = ear.max(1);
            let left_cut =
                CreateEllipticRgn(left - r_cut, ear - r_cut, left + r_cut + 1, ear + r_cut + 1);
            if !left_cut.is_null() {
                CombineRgn(ear_band, ear_band, left_cut, RGN_DIFF);
                DeleteObject(left_cut);
            }
            let right_cut = CreateEllipticRgn(
                right - r_cut,
                ear - r_cut,
                right + r_cut + 1,
                ear + r_cut + 1,
            );
            if !right_cut.is_null() {
                CombineRgn(ear_band, ear_band, right_cut, RGN_DIFF);
                DeleteObject(right_cut);
            }
            CombineRgn(rounded, rounded, ear_band, RGN_OR);
            DeleteObject(ear_band);
        }
    }
    Ok(rounded)
}

/// Update only the HWND's centered top-clipped input/paint region inside the fixed stage
/// without calling `SetWindowPos`, eliminating WebView2 resize flicker and bitmap shifts.
pub fn set_island_region(
    window: &WebviewWindow,
    stage_width: u32,
    region_width: u32,
    region_height: u32,
    scale: f64,
    radius_logical: f64,
    ear_px: i32,
    rounded_bottom: bool,
) -> Result<(), String> {
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    // SAFETY: The HWND belongs to this live Tauri window. Windows takes ownership
    // of `region` only when `SetWindowRgn` returns non-zero.
    unsafe {
        let region = build_island_region(
            stage_width,
            region_width,
            region_height,
            scale,
            radius_logical,
            ear_px,
            rounded_bottom,
        )?;
        if SetWindowRgn(handle.0, region, 1) == 0 {
            DeleteObject(region);
            return Err("Could not apply the island's input region".into());
        }
    }
    Ok(())
}

/// Apply an empty HWND region when the notch is fully hidden so zero pixels or hit targets remain.
pub fn clear_island_region(window: &WebviewWindow) -> Result<(), String> {
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        if region.is_null() {
            return Err("Could not create empty island region".into());
        }
        if SetWindowRgn(handle.0, region, 1) == 0 {
            DeleteObject(region);
            return Err("Could not clear the island's region".into());
        }
    }
    Ok(())
}

/// Read the current global cursor position in physical screen coordinates.
pub fn cursor_position() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
        Some((pt.x, pt.y))
    } else {
        None
    }
}

/// Check whether the primary mouse button is currently held down.
pub fn left_mouse_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

/// Check whether the secondary mouse button is currently held down.
pub fn right_mouse_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_RBUTTON};
    unsafe { (GetAsyncKeyState(VK_RBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

const IGNORED_FULLSCREEN_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "DV2ControlHost",
    "Windows.UI.Core.CoreWindow",
];

pub(crate) fn is_rect_fullscreen_on_monitor(
    win_rect: (i32, i32, i32, i32),
    mon_rect: (i32, i32, i32, i32),
    work_rect: (i32, i32, i32, i32),
    style: u32,
    is_zoomed: bool,
    class_name: &str,
) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{WS_CAPTION, WS_THICKFRAME};

    let mon_w = mon_rect.2 - mon_rect.0;
    let mon_h = mon_rect.3 - mon_rect.1;
    if mon_w <= 0 || mon_h <= 0 {
        return false;
    }

    let trimmed = class_name.trim();
    if trimmed.is_empty()
        || IGNORED_FULLSCREEN_CLASSES
            .iter()
            .any(|ignored| trimmed.eq_ignore_ascii_case(ignored))
    {
        return false;
    }

    let covers_monitor = win_rect.0 <= mon_rect.0 + 2
        && win_rect.1 <= mon_rect.1 + 2
        && win_rect.2 >= mon_rect.2 - 2
        && win_rect.3 >= mon_rect.3 - 2;
    if !covers_monitor {
        return false;
    }

    let work_matches_monitor = (work_rect.0 - mon_rect.0).abs() <= 4
        && (work_rect.1 - mon_rect.1).abs() <= 4
        && (work_rect.2 - mon_rect.2).abs() <= 4
        && (work_rect.3 - mon_rect.3).abs() <= 4;
    if work_matches_monitor
        && is_zoomed
        && (style & WS_CAPTION.0) == WS_CAPTION.0
        && (style & WS_THICKFRAME.0) != 0
    {
        return false;
    }

    true
}

/// Check whether an external fullscreen app/game is currently active on `mon_rect`.
pub fn is_fullscreen_app_on_monitor(
    mon_rect: (i32, i32, i32, i32),
    work_rect: (i32, i32, i32, i32),
    peek_hwnds: &[isize],
    fallback_external: Option<isize>,
) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic, IsWindow,
        IsWindowVisible, IsZoomed, GWL_STYLE,
    };

    unsafe {
        let fg = GetForegroundWindow();
        let fg_raw = fg.0 as isize;
        let target = if fg_raw != 0 && !peek_hwnds.contains(&fg_raw) {
            fg
        } else if let Some(ext) = fallback_external {
            if ext == 0 || peek_hwnds.contains(&ext) {
                return false;
            }
            HWND(ext as _)
        } else {
            return false;
        };

        if !IsWindow(Some(target)).as_bool()
            || !IsWindowVisible(target).as_bool()
            || IsIconic(target).as_bool()
        {
            return false;
        }

        let mut rc = RECT::default();
        if GetWindowRect(target, &mut rc).is_ok() {
            let style = GetWindowLongW(target, GWL_STYLE) as u32;
            let zoomed = IsZoomed(target).as_bool();
            let mut class_buf = [0u16; 64];
            let class_len = GetClassNameW(target, &mut class_buf);
            let class_name = if class_len > 0 {
                String::from_utf16_lossy(&class_buf[..class_len as usize])
            } else {
                String::new()
            };
            if is_rect_fullscreen_on_monitor(
                (rc.left, rc.top, rc.right, rc.bottom),
                mon_rect,
                work_rect,
                style,
                zoomed,
                &class_name,
            ) {
                return true;
            }
        }

        if let Ok(state) = SHQueryUserNotificationState() {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN {
                let mon = MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(mon, &mut info).as_bool() {
                    let m = info.rcMonitor;
                    if (m.left - mon_rect.0).abs() <= 2
                        && (m.top - mon_rect.1).abs() <= 2
                        && (m.right - mon_rect.2).abs() <= 2
                        && (m.bottom - mon_rect.3).abs() <= 2
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Toggle raw Win32 HWND visibility without Tao's style-resetting wrappers, optionally
/// re-asserting `HWND_TOPMOST` so Peek renders above fullscreen apps on hover reveal.
pub fn set_island_window_visible_raw(
    window: &WebviewWindow,
    visible: bool,
    assert_topmost: bool,
) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetWindowPos as WinSetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    };

    let handle = window.hwnd().map_err(|error| error.to_string())?;
    let hwnd = HWND(handle.0);
    unsafe {
        let is_vis = IsWindowVisible(hwnd).as_bool();
        if visible {
            if !is_vis {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            if assert_topmost {
                let _ = WinSetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        } else if is_vis {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    Ok(())
}

/// Position a window in exact raw physical virtual-desktop coordinates (used by the capture overlay).
pub fn set_window_bounds_raw(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
            windows::Win32::Foundation::HWND(handle.0),
            Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
            x,
            y,
            width,
            height,
            windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
        )
        .map_err(|e| e.to_string())
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapLayout {
    Left,
    Right,
    Maximize,
    Center,
}

/// Snap an external window within its monitor's work area.
pub fn snap_window(hwnd: isize, layout: SnapLayout) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOWMAXIMIZED,
    };

    if hwnd == 0 {
        return Err("No active window to organize".into());
    }
    unsafe {
        let h = HWND(hwnd as _);
        let monitor = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Err("Could not read monitor bounds".into());
        }
        let work = info.rcWork;
        let w = work.right - work.left;
        let height = work.bottom - work.top;
        if matches!(layout, SnapLayout::Maximize) {
            let _ = ShowWindow(h, SW_SHOWMAXIMIZED);
            return Ok(());
        }
        let _ = ShowWindow(h, SW_RESTORE);
        let (x, y, cx, cy) = match layout {
            SnapLayout::Left => (work.left, work.top, w / 2, height),
            SnapLayout::Right => (work.left + w / 2, work.top, w - w / 2, height),
            SnapLayout::Center => {
                let cw = (w as f64 * 0.72).round() as i32;
                let ch = (height as f64 * 0.82).round() as i32;
                (
                    work.left + (w - cw) / 2,
                    work.top + (height - ch) / 2,
                    cw,
                    ch,
                )
            }
            SnapLayout::Maximize => unreachable!(),
        };
        SetWindowPos(h, None, x, y, cx, cy, SWP_NOZORDER | SWP_NOACTIVATE)
            .map_err(|e| e.to_string())
    }
}

/// Force WebView2's controller and Tauri window surface to true transparent `(0,0,0,0)` so
/// dynamic `SetWindowRgn` expansion never exposes an unpainted white backdrop.
pub fn set_webview_transparent_bg(window: &WebviewWindow) {
    let _ = window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    let _ = window.with_webview(|wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
        };
        use windows_core::Interface;
        if let Ok(ctrl2) = wv.controller().cast::<ICoreWebView2Controller2>() {
            let _ = ctrl2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                A: 0,
                R: 0,
                G: 0,
                B: 0,
            });
        }
    });
}

/// Hint Edge WebView2 to release V8/Blink caches whenever Peek collapses into idle or hidden state.
pub fn set_webview_memory_low(window: &WebviewWindow, low: bool) {
    let _ = window.with_webview(move |wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        };
        use windows_core::Interface;
        if let Ok(core) = wv.controller().CoreWebView2() {
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                let level = if low {
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                } else {
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                };
                let _ = wv19.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartStatus {
    pub enabled_in_prefs: bool,
    pub registered: bool,
    pub path_matches: bool,
    pub target_exists: bool,
    pub disabled_by_windows: bool,
    pub registered_path: String,
}

pub(crate) fn parse_run_exe_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

pub(crate) fn startup_approved_disabled(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes[0] != 2 && (bytes[0] == 3 || (bytes[0] & 1) == 1)
}

fn read_registry_string(
    subkey: windows::core::PCWSTR,
    value_name: windows::core::PCWSTR,
) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_VALUE_TYPE,
    };

    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, None, KEY_READ, &mut key) != ERROR_SUCCESS {
            return None;
        }
        let mut typ = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let query = RegQueryValueExW(key, value_name, None, Some(&mut typ), None, Some(&mut size));
        let result = if query == ERROR_SUCCESS && size >= 2 {
            let mut buf = vec![0u16; (size as usize).div_ceil(2)];
            let mut actual = size;
            if RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut typ),
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut actual),
            ) == ERROR_SUCCESS
            {
                let len = (actual as usize / 2).min(buf.len());
                let trimmed = buf[..len]
                    .iter()
                    .take_while(|&&ch| ch != 0)
                    .copied()
                    .collect::<Vec<u16>>();
                Some(String::from_utf16_lossy(&trimmed))
            } else {
                None
            }
        } else {
            None
        };
        let _ = RegCloseKey(key);
        result
    }
}

fn read_registry_bytes(
    subkey: windows::core::PCWSTR,
    value_name: windows::core::PCWSTR,
) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_VALUE_TYPE,
    };

    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, None, KEY_READ, &mut key) != ERROR_SUCCESS {
            return None;
        }
        let mut typ = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let query = RegQueryValueExW(key, value_name, None, Some(&mut typ), None, Some(&mut size));
        let result = if query == ERROR_SUCCESS && size > 0 {
            let mut buf = vec![0u8; size as usize];
            let mut actual = size;
            if RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut typ),
                Some(buf.as_mut_ptr()),
                Some(&mut actual),
            ) == ERROR_SUCCESS
            {
                buf.truncate(actual as usize);
                Some(buf)
            } else {
                None
            }
        } else {
            None
        };
        let _ = RegCloseKey(key);
        result
    }
}

/// Inspect the HKCU Run + StartupApproved status for Peek relative to the running executable.
pub fn inspect_autostart(enabled_in_prefs: bool) -> AutostartStatus {
    use windows::core::w;

    let raw = read_registry_string(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
        w!("Peek"),
    );
    let registered_path = raw.as_deref().map(parse_run_exe_path).unwrap_or_default();
    let registered = !registered_path.is_empty();
    let current = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let path_matches =
        registered && !current.is_empty() && registered_path.eq_ignore_ascii_case(&current);
    let target_exists = registered && Path::new(&registered_path).is_file();
    let disabled_by_windows = registered
        && read_registry_bytes(
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
            w!("Peek"),
        )
        .map(|bytes| startup_approved_disabled(&bytes))
        .unwrap_or(false);

    AutostartStatus {
        enabled_in_prefs,
        registered,
        path_matches,
        target_exists,
        disabled_by_windows,
        registered_path,
    }
}

/// Self-heal HKCU Run\Peek on launch when startup is enabled and not running under an isolated profile.
pub fn sync_autostart_if_needed(enabled_in_prefs: bool) {
    if !enabled_in_prefs || std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }
    let st = inspect_autostart(true);
    if !st.registered || !st.path_matches || !st.target_exists {
        let _ = set_autostart(true);
    }
}

/// Guard holding Peek's named single-instance Windows mutex for the process lifetime.
pub struct SingleInstanceGuard(isize);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(
                    windows::Win32::Foundation::HANDLE(self.0 as _),
                );
            }
        }
    }
}

pub(crate) fn try_acquire_named_mutex(name: &str) -> Option<SingleInstanceGuard> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let wide = HSTRING::from(name);
    unsafe {
        let Ok(handle) = CreateMutexW(None, false, PCWSTR(wide.as_ptr())) else {
            return Some(SingleInstanceGuard(0));
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(SingleInstanceGuard(handle.0 as isize))
    }
}

/// Acquire Peek's per-user single-instance mutex so duplicate launches exit cleanly instead
/// of rendering overlapping notches (bypassed when `WEBVIEW2_USER_DATA_FOLDER` is overridden).
pub fn acquire_single_instance_guard() -> Option<SingleInstanceGuard> {
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return Some(SingleInstanceGuard(0));
    }
    try_acquire_named_mutex(r"Local\com.mellowlaw.peek.single_instance")
}

/// Configure Windows startup entry in HKCU Run without admin rights.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use windows::core::w;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_SZ,
    };

    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
            None,
            KEY_SET_VALUE,
            &mut key,
        );
        if status != ERROR_SUCCESS {
            return Err(format!("Could not open Run registry key: {:?}", status));
        }
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let command = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            let res = RegSetValueExW(key, w!("Peek"), None, REG_SZ, Some(bytes));
            if res == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("Failed to write startup entry: {:?}", res))
            }
        } else {
            let res = RegDeleteValueW(key, w!("Peek"));
            if res == ERROR_SUCCESS || res == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("Failed to remove startup entry: {:?}", res))
            }
        };
        let _ = RegCloseKey(key);
        result
    }
}

/// Launch a strictly allowlisted ms-settings URI.
pub fn open_ms_settings(key: &str) -> Result<(), String> {
    let uri = match key {
        "sound" => "ms-settings:sound",
        "display" => "ms-settings:display",
        "bluetooth" => "ms-settings:bluetooth",
        "network-wifi" => "ms-settings:network-wifi",
        "network-airplanemode" => "ms-settings:network-airplanemode",
        "powersleep" => "ms-settings:powersleep",
        "notifications" => "ms-settings:notifications",
        "easeofaccess" => "ms-settings:easeofaccess",
        "privacy-microphone" => "ms-settings:privacy-microphone",
        _ => return Err("Unsupported Windows settings target".into()),
    };
    shell_open(uri)
}

/// Open a URL or file path using ShellExecuteW without invoking cmd.exe.
pub fn shell_open(target: &str) -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = HSTRING::from(target);
    let ret = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (ret.0 as usize) > 32 {
        Ok(())
    } else {
        Err(format!("Could not open target (code {})", ret.0 as usize))
    }
}

/// Launch a file or application shortcut path via ShellExecuteW.
pub fn shell_launch_path(path: &Path) -> Result<(), String> {
    let text = path
        .to_str()
        .ok_or_else(|| "Path contains invalid characters".to_string())?;
    shell_open(text)
}

/// Restore and focus an existing top-level window matching exact title.
pub fn focus_window_by_title(title: &str) -> bool {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let wide = HSTRING::from(title);
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) {
            if !hwnd.0.is_null() {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                return SetForegroundWindow(hwnd).as_bool();
            }
        }
    }
    false
}

/// Generate a cryptographically random lowercase hex token.
pub fn generate_random_hex(byte_len: usize) -> String {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut buf = vec![0u8; byte_len];
    let status = unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.0 != 0 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (idx, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> ((idx % 16) * 8)) & 0xFF) as u8 ^ (idx as u8).wrapping_mul(73);
        }
    }
    let mut out = String::with_capacity(byte_len * 2);
    for b in buf {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn open_clipboard_with_retry() -> Result<(), String> {
    use windows::Win32::System::DataExchange::OpenClipboard;

    let mut last_err = String::new();
    for attempt in 0..3 {
        match unsafe { OpenClipboard(None) } {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = err.to_string();
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            }
        }
    }
    if last_err.is_empty() {
        Err("Clipboard is busy with another application".into())
    } else {
        Err(last_err)
    }
}

/// Copy plain UTF-8 text to the Windows clipboard as CF_UNICODETEXT.
pub fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * 2;
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            return Err("Could not lock clipboard memory".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(hmem);

        if open_clipboard_with_retry().is_err() {
            let _ = GlobalFree(Some(hmem));
            return Err("Clipboard is busy with another application".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0)))
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        if result.is_err() {
            let _ = GlobalFree(Some(hmem));
        }
        result
    }
}

/// Copy an RGBA image buffer to the Windows clipboard as CF_DIB (32-bit bottom-up BGRX).
pub fn copy_rgba_to_clipboard(width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_DIB;

    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|p| p.checked_mul(4))
        .ok_or_else(|| "Image dimensions overflow".to_string())?;
    if rgba.len() < expected || width == 0 || height == 0 {
        return Err("Invalid image buffer".into());
    }

    let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
    let total_size = header_size + expected;
    let header = BITMAPINFOHEADER {
        biSize: header_size as u32,
        biWidth: width as i32,
        biHeight: height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: expected as u32,
        ..Default::default()
    };

    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, total_size).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(hmem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            return Err("Could not allocate clipboard image".into());
        }
        std::ptr::copy_nonoverlapping(&header as *const _ as *const u8, ptr, header_size);
        let dst_pixels = std::slice::from_raw_parts_mut(ptr.add(header_size), expected);
        let row_bytes = width as usize * 4;
        for row in 0..height as usize {
            let src_row = (height as usize - 1 - row) * row_bytes;
            let dst_row = row * row_bytes;
            for col in 0..width as usize {
                let s = src_row + col * 4;
                let d = dst_row + col * 4;
                dst_pixels[d] = rgba[s + 2];
                dst_pixels[d + 1] = rgba[s + 1];
                dst_pixels[d + 2] = rgba[s];
                dst_pixels[d + 3] = rgba[s + 3];
            }
        }
        let _ = GlobalUnlock(hmem);

        if open_clipboard_with_retry().is_err() {
            let _ = GlobalFree(Some(hmem));
            return Err("Clipboard is busy with another application".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hmem.0))).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        if result.is_err() {
            let _ = GlobalFree(Some(hmem));
        }
        result
    }
}

/// Read UTF-8 text from the Windows clipboard (`CF_UNICODETEXT`), bounded by `max_bytes` UTF-16 byte size.
pub fn read_clipboard_text(max_bytes: usize) -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    if open_clipboard_with_retry().is_err() {
        return None;
    }
    let res = unsafe {
        (|| -> Option<String> {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hmem = HGLOBAL(handle.0);
            let bytes = GlobalSize(hmem);
            if bytes == 0 || bytes > max_bytes.saturating_mul(2) {
                return None;
            }
            let ptr = GlobalLock(hmem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let slice = std::slice::from_raw_parts(ptr, bytes / 2);
            let len = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
            let text = String::from_utf16_lossy(&slice[..len]);
            let _ = GlobalUnlock(hmem);
            Some(text)
        })()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    res
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(slice))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(slice))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(i32::from_le_bytes(slice))
}

fn extract_bitfield_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let shifted_mask = mask >> shift;
    let bits = shifted_mask.count_ones();
    if bits == 0 {
        return 0;
    }
    let val = (pixel & mask) >> shift;
    if bits >= 8 {
        (val >> (bits - 8)) as u8
    } else {
        let max_val = (1u32 << bits) - 1;
        ((val * 255) / max_val) as u8
    }
}

/// Decode a Windows `CF_DIB` or `CF_DIBV5` byte buffer (24bpp or 32bpp `BI_RGB`/`BI_BITFIELDS`) into RGBA.
pub fn decode_dib_to_rgba(dib_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    const MAX_DIM: i32 = 8192;
    const MAX_PIXELS: u64 = 33_554_432;
    const BI_RGB_CODE: u32 = 0;
    const BI_BITFIELDS_CODE: u32 = 3;

    if dib_bytes.len() < 40 {
        return None;
    }

    let bi_size = read_u32_le(dib_bytes, 0)? as usize;
    if bi_size < 40 || bi_size > dib_bytes.len() {
        return None;
    }
    let width_i = read_i32_le(dib_bytes, 4)?;
    let height_i = read_i32_le(dib_bytes, 8)?;
    if width_i <= 0 || width_i > MAX_DIM || height_i == 0 || height_i == i32::MIN {
        return None;
    }
    let height_u = height_i.unsigned_abs();
    if height_u > MAX_DIM as u32 {
        return None;
    }
    let width = width_i as u32;
    let total_pixels = (width as u64).checked_mul(height_u as u64)?;
    if total_pixels == 0 || total_pixels > MAX_PIXELS {
        return None;
    }

    let bpp = read_u16_le(dib_bytes, 14)?;
    let compression = read_u32_le(dib_bytes, 16)?;
    let clr_used = read_u32_le(dib_bytes, 32)? as usize;
    if clr_used > 4096 {
        return None;
    }

    let extra_masks = if bi_size == 40 && compression == BI_BITFIELDS_CODE {
        12usize
    } else {
        0usize
    };
    let palette_bytes = clr_used.checked_mul(4)?;
    let pixel_offset = bi_size
        .checked_add(extra_masks)?
        .checked_add(palette_bytes)?;
    if pixel_offset > dib_bytes.len() {
        return None;
    }

    let w = width as usize;
    let h = height_u as usize;
    let top_down = height_i < 0;
    let mut rgba = vec![0u8; w.checked_mul(h)?.checked_mul(4)?];

    match (bpp, compression) {
        (32, BI_RGB_CODE) | (32, BI_BITFIELDS_CODE) => {
            let row_bytes = w.checked_mul(4)?;
            let needed = pixel_offset.checked_add(row_bytes.checked_mul(h)?)?;
            if needed > dib_bytes.len() {
                return None;
            }

            let masks = if compression == BI_BITFIELDS_CODE {
                let r_mask = read_u32_le(dib_bytes, 40)?;
                let g_mask = read_u32_le(dib_bytes, 44)?;
                let b_mask = read_u32_le(dib_bytes, 48)?;
                let a_mask = if bi_size >= 56 {
                    read_u32_le(dib_bytes, 52)?
                } else {
                    0
                };
                Some((r_mask, g_mask, b_mask, a_mask))
            } else {
                None
            };

            let mut has_nonzero_alpha = false;
            for dst_y in 0..h {
                let src_y = if top_down { dst_y } else { h - 1 - dst_y };
                let src_row = pixel_offset + src_y * row_bytes;
                let dst_row = dst_y * row_bytes;
                for x in 0..w {
                    let s = src_row + x * 4;
                    let d = dst_row + x * 4;
                    if let Some((rm, gm, bm, am)) = masks {
                        let px = u32::from_le_bytes([
                            dib_bytes[s],
                            dib_bytes[s + 1],
                            dib_bytes[s + 2],
                            dib_bytes[s + 3],
                        ]);
                        let a = if am == 0 {
                            255
                        } else {
                            extract_bitfield_channel(px, am)
                        };
                        if a != 0 {
                            has_nonzero_alpha = true;
                        }
                        rgba[d] = extract_bitfield_channel(px, rm);
                        rgba[d + 1] = extract_bitfield_channel(px, gm);
                        rgba[d + 2] = extract_bitfield_channel(px, bm);
                        rgba[d + 3] = a;
                    } else {
                        let a = dib_bytes[s + 3];
                        if a != 0 {
                            has_nonzero_alpha = true;
                        }
                        rgba[d] = dib_bytes[s + 2];
                        rgba[d + 1] = dib_bytes[s + 1];
                        rgba[d + 2] = dib_bytes[s];
                        rgba[d + 3] = a;
                    }
                }
            }
            if !has_nonzero_alpha {
                for px in rgba.chunks_exact_mut(4) {
                    px[3] = 255;
                }
            }
        }
        (24, BI_RGB_CODE) => {
            let stride = (w.checked_mul(3)?.checked_add(3)?) & !3usize;
            let needed = pixel_offset.checked_add(stride.checked_mul(h)?)?;
            if needed > dib_bytes.len() {
                return None;
            }
            let dst_row_bytes = w.checked_mul(4)?;
            for dst_y in 0..h {
                let src_y = if top_down { dst_y } else { h - 1 - dst_y };
                let src_row = pixel_offset + src_y * stride;
                let dst_row = dst_y * dst_row_bytes;
                for x in 0..w {
                    let s = src_row + x * 3;
                    let d = dst_row + x * 4;
                    rgba[d] = dib_bytes[s + 2];
                    rgba[d + 1] = dib_bytes[s + 1];
                    rgba[d + 2] = dib_bytes[s];
                    rgba[d + 3] = 255;
                }
            }
        }
        _ => return None,
    }

    Some((width, height_u, rgba))
}

/// Read an image (`PNG`, `CF_DIBV5`, or `CF_DIB`) from the Windows clipboard into tight RGBA bytes.
pub fn read_clipboard_image_rgba() -> Option<(u32, u32, Vec<u8>)> {
    use windows::core::w;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};

    const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
    const MAX_DIB_BYTES: usize = 64 * 1024 * 1024;

    enum RawClipboardImage {
        Png(Vec<u8>),
        Dib(Vec<u8>),
    }

    let png_fmt = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if open_clipboard_with_retry().is_err() {
        return None;
    }

    let raw = unsafe {
        let copy_handle_bytes = |fmt: u32, min_len: usize, max_len: usize| -> Option<Vec<u8>> {
            let handle = GetClipboardData(fmt).ok()?;
            let hmem = HGLOBAL(handle.0);
            let len = GlobalSize(hmem);
            if len < min_len || len > max_len {
                return None;
            }
            let ptr = GlobalLock(hmem) as *const u8;
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
            let _ = GlobalUnlock(hmem);
            Some(bytes)
        };

        let picked = if png_fmt != 0 {
            copy_handle_bytes(png_fmt, 8, MAX_PNG_BYTES).map(RawClipboardImage::Png)
        } else {
            None
        }
        .or_else(|| {
            copy_handle_bytes(CF_DIBV5.0 as u32, 40, MAX_DIB_BYTES).map(RawClipboardImage::Dib)
        })
        .or_else(|| {
            copy_handle_bytes(CF_DIB.0 as u32, 40, MAX_DIB_BYTES).map(RawClipboardImage::Dib)
        });

        let _ = CloseClipboard();
        picked
    }?;

    match raw {
        RawClipboardImage::Png(bytes) => {
            let dyn_img =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
            let rgba = dyn_img.into_rgba8();
            let (w, h) = rgba.dimensions();
            if w == 0 || h == 0 || w > 8192 || h > 8192 {
                return None;
            }
            Some((w, h, rgba.into_raw()))
        }
        RawClipboardImage::Dib(bytes) => decode_dib_to_rgba(&bytes),
    }
}

pub(crate) fn firewall_rule_blocks_exe(rule: &str, exe_path: &str) -> bool {
    let mut is_block = false;
    let mut is_active = true;
    let mut is_inbound = false;
    let mut proto_tcp_or_any = true;
    let mut app_matches = false;

    for part in rule.split('|') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        if key.eq_ignore_ascii_case("Action") {
            is_block = val.eq_ignore_ascii_case("Block");
        } else if key.eq_ignore_ascii_case("Active") {
            is_active = !val.eq_ignore_ascii_case("FALSE");
        } else if key.eq_ignore_ascii_case("Dir") {
            is_inbound = val.eq_ignore_ascii_case("In");
        } else if key.eq_ignore_ascii_case("Protocol") {
            proto_tcp_or_any = val == "6" || val.is_empty();
        } else if key.eq_ignore_ascii_case("App") {
            app_matches = val.eq_ignore_ascii_case(exe_path);
        }
    }

    is_block && is_active && is_inbound && proto_tcp_or_any && app_matches
}

pub fn is_exe_inbound_firewall_blocked() -> bool {
    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe_str = exe
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();

    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!(
                r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules"
            ),
            None,
            KEY_READ,
            &mut key,
        );
        if status != ERROR_SUCCESS {
            return false;
        }

        let mut blocked = false;
        let mut index = 0u32;
        let mut name_buf = vec![0u16; 512];
        let mut data_buf = vec![0u8; 4096];

        loop {
            let mut name_len = name_buf.len() as u32;
            let mut val_type = 0u32;
            let mut data_len = data_buf.len() as u32;
            let res = RegEnumValueW(
                key,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut val_type),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            );
            if res == ERROR_NO_MORE_ITEMS {
                break;
            }
            if res == ERROR_MORE_DATA {
                let mut grew = false;
                if name_len as usize >= name_buf.len() && name_buf.len() < 4096 {
                    name_buf.resize(((name_len as usize) + 1).min(4096), 0);
                    grew = true;
                }
                if data_len as usize > data_buf.len() && data_buf.len() < 65536 {
                    data_buf.resize((data_len as usize).min(65536), 0);
                    grew = true;
                }
                if !grew {
                    index += 1;
                }
                continue;
            }
            if res != ERROR_SUCCESS {
                break;
            }
            index += 1;

            if val_type == REG_SZ.0 && data_len >= 2 {
                let slice_bytes = &data_buf[..((data_len as usize) & !1)];
                let units: Vec<u16> = slice_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                let rule = String::from_utf16_lossy(&units[..end]);
                if firewall_rule_blocks_exe(&rule, &exe_str) {
                    blocked = true;
                    break;
                }
            }
        }

        let _ = RegCloseKey(key);
        blocked
    }
}

pub fn allow_exe_in_windows_firewall() -> Result<(), String> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\'', "''");
    let script = format!(
        "$p = '{exe_str}'; Get-NetFirewallApplicationFilter -ErrorAction SilentlyContinue | Where-Object {{ $_.Program -ieq $p }} | ForEach-Object {{ Remove-NetFirewallRule -AssociatedNetFirewallApplicationFilter $_ -ErrorAction SilentlyContinue }}; New-NetFirewallRule -DisplayName 'Peek LAN Transfer' -Direction Inbound -Action Allow -Protocol TCP -Profile Private -Program $p | Out-Null"
    );
    let args = format!("-NoProfile -WindowStyle Hidden -Command \"{script}\"");
    let params_w = HSTRING::from(args);
    let _suppress = suppress_auto_close();

    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: w!("powershell.exe"),
            lpParameters: PCWSTR(params_w.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(err) = ShellExecuteExW(&mut info) {
            if err.code() == ERROR_CANCELLED.to_hresult() {
                return Err("Canceled".into());
            }
            return Err(format!("Could not update Windows Firewall: {err}"));
        }
        if !info.hProcess.is_invalid() && !info.hProcess.0.is_null() {
            let _ = WaitForSingleObject(info.hProcess, 15_000);
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetRgnBox, PtInRegion, COMPLEXREGION};

    fn make_test_dib_header(width: i32, height: i32, bpp: u16, compression: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 40];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes());
        buf[4..8].copy_from_slice(&width.to_le_bytes());
        buf[8..12].copy_from_slice(&height.to_le_bytes());
        buf[12..14].copy_from_slice(&1u16.to_le_bytes());
        buf[14..16].copy_from_slice(&bpp.to_le_bytes());
        buf[16..20].copy_from_slice(&compression.to_le_bytes());
        buf
    }

    #[test]
    fn decodes_32bpp_bottom_up_dib_and_fixes_all_zero_alpha() {
        // 2x2 bottom-up BGRA: bottom row is red & blue, top row is green & white, all alpha = 0
        let mut dib = make_test_dib_header(2, 2, 32, 0);
        dib.extend_from_slice(&[
            0, 0, 255, 0, 255, 0, 0, 0, // bottom row: red (BGR=0,0,255), blue (255,0,0)
            0, 255, 0, 0, 255, 255, 255, 0, // top row: green, white
        ]);
        let (w, h, rgba) = decode_dib_to_rgba(&dib).expect("32bpp bottom-up DIB decodes");
        assert_eq!((w, h), (2, 2));
        assert_eq!(
            rgba,
            vec![
                0, 255, 0, 255, 255, 255, 255, 255, // top row RGBA
                255, 0, 0, 255, 0, 0, 255, 255, // bottom row RGBA
            ]
        );
    }

    #[test]
    fn decodes_32bpp_top_down_dib_preserving_nonzero_alpha() {
        let mut dib = make_test_dib_header(2, -1, 32, 0);
        dib.extend_from_slice(&[
            10, 20, 30, 128, // BGRA -> RGBA(30,20,10,128)
            40, 50, 60,
            0, // BGRA -> RGBA(60,50,40,0) preserved because another pixel has alpha
        ]);
        let (w, h, rgba) = decode_dib_to_rgba(&dib).expect("32bpp top-down DIB decodes");
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![30, 20, 10, 128, 60, 50, 40, 0]);
    }

    #[test]
    fn decodes_24bpp_bottom_up_dib_with_row_padding() {
        // Width 1 at 24bpp = 3 bytes pixel + 1 padding byte = 4-byte stride per row
        let mut dib = make_test_dib_header(1, 2, 24, 0);
        dib.extend_from_slice(&[
            0, 0, 200, 99, // bottom pixel: RGB(200,0,0) + padding 99
            10, 20, 30, 77, // top pixel: RGB(30,20,10) + padding 77
        ]);
        let (w, h, rgba) = decode_dib_to_rgba(&dib).expect("24bpp padded DIB decodes");
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgba, vec![30, 20, 10, 255, 200, 0, 0, 255]);
    }

    #[test]
    fn concave_ear_region_carves_transparent_overhang_symmetrically() {
        for (logical_w, logical_h, radius_logical, ear_logical) in [
            (108.0_f64, 30.0_f64, 16.0_f64, 10.0_f64),
            (504.0_f64, 320.0_f64, 36.0_f64, 10.0_f64),
        ] {
            for scale in [1.0_f64, 1.25, 1.5, 2.0] {
                let stage_w = ((620.0 * scale).round() as u32).next_multiple_of(2);
                let rw = (((logical_w + 2.0 * ear_logical) * scale).round() as u32)
                    .next_multiple_of(2)
                    .min(stage_w);
                let rh = (logical_h * scale).round() as u32;
                let ear = (ear_logical * scale).round() as i32;
                let left = ((stage_w as i32 - rw as i32) / 2).max(0);
                let right = left + rw as i32;

                unsafe {
                    let rgn =
                        build_island_region(stage_w, rw, rh, scale, radius_logical, ear, true)
                            .expect("region should build");
                    let mut rc = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    let rgn_type = GetRgnBox(rgn, &mut rc);
                    assert_eq!(rgn_type, COMPLEXREGION);
                    assert_eq!(rc.top, 0);
                    assert_eq!(rc.left, left + 1);
                    assert_eq!(rc.right, right - 1);
                    assert_eq!(rc.bottom, rh as i32);

                    // Points beneath the concave top ears must be excluded from the HRGN.
                    assert_eq!(PtInRegion(rgn, left + 1, ear - 1), 0);
                    assert_eq!(PtInRegion(rgn, left + 2, ear - 2), 0);
                    assert_eq!(PtInRegion(rgn, right - 2, ear - 1), 0);
                    assert_eq!(PtInRegion(rgn, right - 3, ear - 2), 0);

                    // Points inside the concave ear tops and notch center must be included.
                    assert_ne!(PtInRegion(rgn, left + ear - 1, 1), 0);
                    assert_ne!(PtInRegion(rgn, right - ear, 1), 0);
                    assert_ne!(PtInRegion(rgn, (left + right) / 2, rh as i32 / 2), 0);

                    // Outer bottom corners must be carved by CreateRoundRectRgn.
                    assert_eq!(PtInRegion(rgn, left + ear, rh as i32 - 1), 0);
                    assert_eq!(PtInRegion(rgn, right - ear - 1, rh as i32 - 1), 0);

                    // Exact left/right pixel symmetry across sampled coordinates.
                    for y in [0, 1, ear / 2, ear - 1, rh as i32 / 2, rh as i32 - 1] {
                        for dx in 0..(rw as i32 / 2) {
                            assert_eq!(
                                PtInRegion(rgn, left + dx, y) != 0,
                                PtInRegion(rgn, (right - 1) - dx, y) != 0,
                                "asymmetry at scale={scale} w={logical_w} dx={dx} y={y}"
                            );
                        }
                    }

                    DeleteObject(rgn);
                }
            }
        }
    }

    #[test]
    fn detects_inbound_tcp_firewall_block_rules() {
        let exe = r"C:\Users\Lawrence\Peek\peek.exe";
        assert!(firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=TRUE|Dir=In|Protocol=6|Profile=Private|App=c:\\users\\lawrence\\peek\\peek.exe|Name=peek.exe|",
            exe,
        ));
        assert!(firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=TRUE|Dir=In|App=C:\\Users\\Lawrence\\Peek\\peek.exe|",
            exe,
        ));
        assert!(!firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=TRUE|Dir=In|Protocol=17|App=C:\\Users\\Lawrence\\Peek\\peek.exe|",
            exe,
        ));
        assert!(!firewall_rule_blocks_exe(
            "v2.10|Action=Allow|Active=TRUE|Dir=In|Protocol=6|App=C:\\Users\\Lawrence\\Peek\\peek.exe|",
            exe,
        ));
        assert!(!firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=TRUE|Dir=Out|Protocol=6|App=C:\\Users\\Lawrence\\Peek\\peek.exe|",
            exe,
        ));
        assert!(!firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=FALSE|Dir=In|Protocol=6|App=C:\\Users\\Lawrence\\Peek\\peek.exe|",
            exe,
        ));
        assert!(!firewall_rule_blocks_exe(
            "v2.10|Action=Block|Active=TRUE|Dir=In|Protocol=6|App=C:\\Other\\app.exe|",
            exe,
        ));
    }

    #[test]
    fn parses_startup_approved_disabled_flag_and_run_quotes() {
        assert!(!startup_approved_disabled(&[2, 0, 0, 0]));
        assert!(startup_approved_disabled(&[3, 0, 0, 0]));
        assert!(startup_approved_disabled(&[1, 0, 0, 0]));
        assert!(!startup_approved_disabled(&[]));
        assert_eq!(
            parse_run_exe_path(r#""C:\Users\Test\AppData\Local\Peek\peek.exe" --bg"#),
            r"C:\Users\Test\AppData\Local\Peek\peek.exe"
        );
        assert_eq!(parse_run_exe_path(r"C:\Peek\peek.exe"), r"C:\Peek\peek.exe");
    }

    #[test]
    fn named_mutex_blocks_duplicate_instance_until_dropped() {
        let name = format!(
            "Local\\com.mellowlaw.peek.test_mutex_{}",
            std::process::id()
        );
        let first = try_acquire_named_mutex(&name);
        assert!(first.is_some());
        assert!(try_acquire_named_mutex(&name).is_none());
        drop(first);
        let third = try_acquire_named_mutex(&name);
        assert!(third.is_some());
    }

    #[test]
    fn fullscreen_rect_detection_distinguishes_games_maximized_and_desktop() {
        use windows::Win32::UI::WindowsAndMessaging::{WS_CAPTION, WS_POPUP, WS_THICKFRAME};

        let mon = (0, 0, 1920, 1080);
        let work_with_taskbar = (0, 0, 1920, 1032);
        let popup_style = WS_POPUP.0;
        let desktop_max_style = (WS_CAPTION | WS_THICKFRAME).0;

        // Borderless fullscreen game covers full monitor.
        assert!(is_rect_fullscreen_on_monitor(
            mon,
            mon,
            work_with_taskbar,
            popup_style,
            false,
            "UnityWndClass",
        ));
        // Maximized desktop app stops above taskbar -> not fullscreen.
        assert!(!is_rect_fullscreen_on_monitor(
            work_with_taskbar,
            mon,
            work_with_taskbar,
            desktop_max_style,
            true,
            "Chrome_WidgetWin_1",
        ));
        // Desktop and shell classes never trigger fullscreen hiding.
        for shell_cls in ["Progman", "WorkerW", "Shell_TrayWnd", ""] {
            assert!(!is_rect_fullscreen_on_monitor(
                mon,
                mon,
                work_with_taskbar,
                popup_style,
                false,
                shell_cls,
            ));
        }
        // Auto-hide taskbar: normal maximized desktop app shouldn't count as fullscreen,
        // while a borderless app/game covering the monitor still counts.
        assert!(!is_rect_fullscreen_on_monitor(
            mon,
            mon,
            mon,
            desktop_max_style,
            true,
            "Chrome_WidgetWin_1",
        ));
        assert!(is_rect_fullscreen_on_monitor(
            mon,
            mon,
            mon,
            popup_style,
            false,
            "UnrealWindow",
        ));
        // Window on a secondary monitor doesn't trigger primary monitor fullscreen.
        assert!(!is_rect_fullscreen_on_monitor(
            (1920, 0, 3840, 1080),
            mon,
            work_with_taskbar,
            popup_style,
            false,
            "SDL_app",
        ));
    }
}
