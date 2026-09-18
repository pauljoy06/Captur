use std::{
    mem::size_of,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CREATESTRUCTW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyMenu, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos,
                GetMessageW, GetWindowLongPtrW, IDC_ARROW, IDI_APPLICATION, LoadCursorW, LoadIconW,
                MF_SEPARATOR, MF_STRING, MSG, PostQuitMessage, RegisterClassW, SetForegroundWindow,
                SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
                TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_DESTROY,
                WM_LBUTTONDBLCLK, WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
                WS_OVERLAPPED,
            },
        },
    },
    core::{PCWSTR, w},
};

const WINDOW_CLASS: PCWSTR = w!("CapturTrayWindow");
const TRAY_MESSAGE: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const MENU_SHOW: usize = 1;
const MENU_EXIT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    ShowWorkspace,
    Exit,
}

pub struct TrayReceiver {
    receiver: Receiver<TrayEvent>,
}

impl TrayReceiver {
    pub fn start(egui_context: egui::Context) -> Result<Self, String> {
        let (event_sender, event_receiver) = mpsc::channel();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);

        thread::Builder::new()
            .name("captur-tray".into())
            .spawn(move || {
                let result = run_tray_thread(event_sender, egui_context, startup_sender);
                if let Err(error) = result {
                    eprintln!("Captur tray thread stopped: {error}");
                }
            })
            .map_err(|error| format!("could not start tray thread: {error}"))?;

        startup_receiver
            .recv()
            .map_err(|_| "tray thread stopped during startup".to_owned())??;

        Ok(Self {
            receiver: event_receiver,
        })
    }

    pub fn try_recv(&self) -> Result<TrayEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

struct TrayState {
    sender: mpsc::Sender<TrayEvent>,
    egui_context: egui::Context,
    icon_added: bool,
}

impl TrayState {
    fn send(&self, event: TrayEvent) {
        let _ = self.sender.send(event);
        self.egui_context.request_repaint();
    }
}

fn run_tray_thread(
    sender: mpsc::Sender<TrayEvent>,
    egui_context: egui::Context,
    startup_sender: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    // All HWND and Shell_NotifyIconW ownership remains on this thread. The sole raw state
    // pointer is installed during WM_NCCREATE and reclaimed during WM_NCDESTROY.
    unsafe {
        let module = GetModuleHandleW(None)
            .map_err(|error| format!("could not get application module: {error}"))?;
        let instance = HINSTANCE(module.0);
        let window_class = WNDCLASSW {
            hInstance: instance,
            lpszClassName: WINDOW_CLASS,
            lpfnWndProc: Some(window_proc),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };

        if RegisterClassW(&window_class) == 0 {
            let error = std::io::Error::last_os_error();
            let message = format!("could not register tray window class: {error}");
            let _ = startup_sender.send(Err(message.clone()));
            return Err(message);
        }

        let state = Box::new(TrayState {
            sender,
            egui_context,
            icon_added: false,
        });
        let state_pointer = Box::into_raw(state);
        let window = match CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            w!("Captur Tray"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            Some(state_pointer.cast()),
        ) {
            Ok(window) => window,
            Err(error) => {
                // CreateWindowExW may already have delivered WM_NCDESTROY after WM_NCCREATE,
                // in which case the window procedure reclaimed this pointer. Avoid a possible
                // double free here. A failure before WM_NCCREATE can only leak this small state.
                let message = format!("could not create tray window: {error}");
                let _ = startup_sender.send(Err(message.clone()));
                return Err(message);
            }
        };

        if let Err(error) = add_tray_icon(window) {
            let _ = DestroyWindow(window);
            let _ = startup_sender.send(Err(error.clone()));
            return Err(error);
        }
        (*state_pointer).icon_added = true;
        let _ = startup_sender.send(Ok(()));

        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == -1 {
                let _ = DestroyWindow(window);
                return Err(format!(
                    "tray message loop failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if result == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    Ok(())
}

unsafe fn add_tray_icon(window: HWND) -> Result<(), String> {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_MESSAGE,
        hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default(),
        ..Default::default()
    };
    write_wide_buffer(&mut data.szTip, "Captur");

    // Safety: `data` is fully initialized, and `window` is owned by the calling tray thread.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        Ok(())
    } else {
        Err(format!(
            "could not add tray icon: {}",
            std::io::Error::last_os_error()
        ))
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() {
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
            }
        }
    }

    let state_pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut TrayState };
    match message {
        TRAY_MESSAGE if lparam.0 as u32 == WM_LBUTTONDBLCLK => {
            if let Some(state) = unsafe { state_pointer.as_ref() } {
                state.send(TrayEvent::ShowWorkspace);
            }
            LRESULT(0)
        }
        TRAY_MESSAGE if lparam.0 as u32 == WM_RBUTTONUP => {
            unsafe { show_context_menu(window) };
            LRESULT(0)
        }
        WM_COMMAND => {
            match wparam.0 & 0xffff {
                MENU_SHOW => {
                    if let Some(state) = unsafe { state_pointer.as_ref() } {
                        state.send(TrayEvent::ShowWorkspace);
                    }
                }
                MENU_EXIT => {
                    if let Some(state) = unsafe { state_pointer.as_ref() } {
                        state.send(TrayEvent::Exit);
                    }
                    let _ = unsafe { DestroyWindow(window) };
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            if !state_pointer.is_null() {
                // Shell_NotifyIconW does not own the icon or state. Remove the notification
                // before releasing the Box that is exclusively owned by this window/thread.
                let state = unsafe { Box::from_raw(state_pointer) };
                if state.icon_added {
                    let data = NOTIFYICONDATAW {
                        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: window,
                        uID: TRAY_ID,
                        ..Default::default()
                    };
                    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                }
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
            }
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

unsafe fn show_context_menu(window: HWND) {
    let menu = match unsafe { CreatePopupMenu() } {
        Ok(menu) => menu,
        Err(_) => return,
    };

    let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_SHOW, w!("Show Captur")) };
    let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, None) };
    let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit")) };

    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
        let _ = unsafe { SetForegroundWindow(window) };
        let _ = unsafe {
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
                cursor.x,
                cursor.y,
                None,
                window,
                None,
            )
        };
        // Required by the notification-area menu guidance so dismissal is processed correctly.
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(window),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
            )
        };
    }

    let _ = unsafe { DestroyMenu(menu) };
}

fn write_wide_buffer(buffer: &mut [u16], text: &str) {
    for (destination, source) in buffer.iter_mut().zip(text.encode_utf16().chain(Some(0))) {
        *destination = source;
    }
}
