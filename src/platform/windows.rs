use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use windows::{
    Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::{
            Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute},
            Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
                MonitorFromWindow,
            },
        },
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
            WindowsAndMessaging::{
                FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetForegroundWindow, GetPhysicalCursorPos,
                GetWindowRect, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_HIDE, SW_SHOW,
                SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetForegroundWindow,
                SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_POPUP,
            },
        },
    },
    core::w,
};

use crate::capture::region::{PixelPoint, PixelRect};

pub const WINDOW_TITLE: &str = "ProofSnip";

pub struct OverlayBoundsGuard {
    active: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl OverlayBoundsGuard {
    pub fn start(bounds: PixelRect) -> Result<Self, String> {
        let hwnd = app_window()?;
        let raw_window = hwnd.0 as usize;
        let active = Arc::new(AtomicBool::new(true));
        let worker_active = active.clone();
        let worker = thread::Builder::new()
            .name("proofsnip-overlay-bounds".into())
            .spawn(move || {
                let hwnd = HWND(raw_window as *mut core::ffi::c_void);
                while worker_active.load(Ordering::Acquire) {
                    let _ = ensure_overlay_bounds_for(hwnd, bounds);
                    thread::sleep(Duration::from_millis(4));
                }
            })
            .map_err(|error| format!("could not start overlay bounds guard: {error}"))?;
        Ok(Self {
            active,
            worker: Some(worker),
        })
    }
}

impl Drop for OverlayBoundsGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn configure_process() -> eframe::Result<()> {
    // Safety: process DPI awareness is configured before eframe creates a window. COM is kept
    // initialized for the lifetime of the UI thread and is intentionally uninitialized at exit.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    Ok(())
}

fn app_window() -> Result<HWND, String> {
    // Safety: static UTF-16 title string remains valid for the duration of the call.
    unsafe {
        FindWindowW(None, w!("ProofSnip"))
            .map_err(|error| format!("ProofSnip window not found: {error}"))
    }
}

pub fn show_overlay(bounds: PixelRect) -> Result<(), String> {
    let hwnd = app_window()?;
    // Safety: hwnd is our top-level eframe window. Styles and physical coordinates are applied
    // synchronously to make the client area exactly cover the virtual desktop.
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0 as isize,
        );
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            bounds.left,
            bounds.top,
            bounds.width() as i32,
            bounds.height() as i32,
            SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        )
        .map_err(|error| format!("could not position capture overlay: {error}"))?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

pub fn ensure_overlay_bounds(bounds: PixelRect) -> Result<(), String> {
    ensure_overlay_bounds_for(app_window()?, bounds)
}

fn ensure_overlay_bounds_for(hwnd: HWND, bounds: PixelRect) -> Result<(), String> {
    let mut current = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut current)
            .map_err(|error| format!("could not query capture-overlay bounds: {error}"))?;
        if current.left == bounds.left
            && current.top == bounds.top
            && current.right == bounds.right
            && current.bottom == bounds.bottom
        {
            return Ok(());
        }
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            bounds.left,
            bounds.top,
            bounds.width() as i32,
            bounds.height() as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .map_err(|error| format!("could not maintain capture-overlay bounds: {error}"))?;
    }
    Ok(())
}

pub fn hide_window() {
    if let Ok(hwnd) = app_window() {
        // Safety: hwnd is our top-level eframe window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub fn is_workspace_visible() -> bool {
    app_window().is_ok_and(|hwnd| unsafe { IsWindowVisible(hwnd).as_bool() })
}

pub fn hide_for_capture() -> Result<(), String> {
    let hwnd = app_window()?;
    // Safety: hwnd is our top-level window. DwmFlush waits for the hide operation to reach the
    // compositor, avoiding both self-capture and an arbitrary fixed sleep on the hot path.
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        DwmFlush().map_err(|error| format!("could not synchronize desktop composition: {error}"))
    }
}

pub fn show_workspace() -> Result<(), String> {
    let hwnd = app_window()?;
    // Safety: hwnd is our top-level eframe window. eframe handles subsequent client rendering.
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return Err("could not query the workspace monitor".into());
        }

        let work = monitor_info.rcWork;
        let work_width = work.right.saturating_sub(work.left);
        let work_height = work.bottom.saturating_sub(work.top);
        let desired_width = (1040_i32.saturating_mul(dpi) / 96)
            .min(work_width.saturating_sub(48))
            .max(720.min(work_width));
        let desired_height = (720_i32.saturating_mul(dpi) / 96)
            .min(work_height.saturating_sub(48))
            .max(520.min(work_height));
        let x = work.left + (work_width - desired_width) / 2;
        let y = work.top + (work_height - desired_height) / 2;

        SetWindowLongPtrW(hwnd, GWL_STYLE, 0x00CF_0000_u32 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, 0);
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            desired_width,
            desired_height,
            SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        )
        .map_err(|error| format!("could not show evidence workspace: {error}"))?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

pub fn show_toast(anchor: PixelRect) -> Result<(), String> {
    show_compact_window(
        anchor.right.saturating_sub(370),
        anchor.bottom.saturating_sub(84),
        350,
        60,
        false,
    )
}

pub fn show_note_prompt(anchor: PixelRect) -> Result<(), String> {
    show_compact_window(anchor.left, anchor.bottom.saturating_add(8), 560, 150, true)
}

fn show_compact_window(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    activate: bool,
) -> Result<(), String> {
    let hwnd = app_window()?;
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0 as isize,
        );
        let flags = if activate {
            SWP_FRAMECHANGED | SWP_SHOWWINDOW
        } else {
            SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_NOACTIVATE
        };
        SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, flags)
            .map_err(|error| format!("could not show compact window: {error}"))?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        if activate {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    Ok(())
}

pub fn cursor_position() -> Option<PixelPoint> {
    let mut point = POINT::default();
    // Safety: point is a valid out pointer.
    unsafe { GetPhysicalCursorPos(&mut point).ok()? };
    Some(PixelPoint {
        x: point.x,
        y: point.y,
    })
}

pub fn primary_button_down() -> bool {
    // Safety: GetAsyncKeyState reads process-independent input state and requires no pointers.
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

pub fn monitor_under_cursor() -> Result<PixelRect, String> {
    let point = cursor_position().ok_or_else(|| "could not read cursor position".to_owned())?;
    // Safety: MONITORINFO has the required size and is a valid out pointer.
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: point.x,
                y: point.y,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Err("could not query the monitor under the cursor".into());
        }
        Ok(rect_to_pixel(info.rcMonitor))
    }
}

pub fn active_window_rect() -> Result<PixelRect, String> {
    // Safety: the foreground HWND is queried and validated before its bounds are read.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
            return Err("no visible active window is available".into());
        }
        if IsIconic(hwnd).as_bool() {
            return Err("the active window is minimized".into());
        }

        let mut rect = RECT::default();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .is_err()
        {
            GetWindowRect(hwnd, &mut rect)
                .map_err(|error| format!("could not query active-window bounds: {error}"))?;
        }
        let rect = rect_to_pixel(rect);
        if rect.is_empty() {
            return Err("the active window has empty bounds".into());
        }
        Ok(rect)
    }
}

fn rect_to_pixel(rect: RECT) -> PixelRect {
    PixelRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}
