use std::{
    fs::OpenOptions,
    io::Write,
    sync::mpsc::{self, Receiver},
    thread,
};

use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

const CAPTURE_REGION_ID: i32 = 0x5053_0001;
const CAPTURE_SAME_REGION_ID: i32 = 0x5053_0002;
const TOGGLE_WORKSPACE_ID: i32 = 0x5053_0003;
const CAPTURE_MONITOR_ID: i32 = 0x5053_0004;
const CAPTURE_ACTIVE_WINDOW_ID: i32 = 0x5053_0005;
const CAPTURE_NOTE_ID: i32 = 0x5053_0006;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    CaptureRegion,
    CaptureSameRegion,
    ToggleWorkspace,
    CaptureMonitor,
    CaptureActiveWindow,
    CaptureNote,
}

pub struct HotkeyReceiver {
    receiver: Receiver<Result<HotkeyEvent, String>>,
}

impl HotkeyReceiver {
    pub fn start(context: egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("captur-hotkeys".into())
            .spawn(move || {
                // Safety: this thread owns both thread-level hotkey registrations and its message
                // loop. The registrations are released before the thread exits.
                unsafe {
                    let modifiers = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
                    let registrations = [
                        (CAPTURE_REGION_ID, b'S', "Ctrl+Alt+S"),
                        (CAPTURE_SAME_REGION_ID, b'R', "Ctrl+Alt+R"),
                        (TOGGLE_WORKSPACE_ID, b'W', "Ctrl+Alt+W"),
                        (CAPTURE_MONITOR_ID, b'M', "Ctrl+Alt+M"),
                        (CAPTURE_ACTIVE_WINDOW_ID, b'A', "Ctrl+Alt+A"),
                        (CAPTURE_NOTE_ID, b'N', "Ctrl+Alt+N"),
                    ];
                    let mut registered_ids = Vec::with_capacity(registrations.len());
                    for (id, key, label) in registrations {
                        match RegisterHotKey(None, id, modifiers, key as u32) {
                            Ok(()) => {
                                diagnostic(&format!("registered {label} id={id:#x}"));
                                registered_ids.push(id);
                            }
                            Err(error) => {
                                diagnostic(&format!("failed {label} id={id:#x}: {error}"));
                                let _ = sender
                                    .send(Err(format!("could not register {label}: {error}")));
                            }
                        }
                    }
                    if registered_ids.is_empty() {
                        context.request_repaint();
                        return;
                    }

                    let mut message = MSG::default();
                    while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                        if message.message == WM_HOTKEY {
                            diagnostic(&format!("received WM_HOTKEY id={:#x}", message.wParam.0));
                            let event = match message.wParam.0 as i32 {
                                CAPTURE_REGION_ID => Some(HotkeyEvent::CaptureRegion),
                                CAPTURE_SAME_REGION_ID => Some(HotkeyEvent::CaptureSameRegion),
                                TOGGLE_WORKSPACE_ID => Some(HotkeyEvent::ToggleWorkspace),
                                CAPTURE_MONITOR_ID => Some(HotkeyEvent::CaptureMonitor),
                                CAPTURE_ACTIVE_WINDOW_ID => Some(HotkeyEvent::CaptureActiveWindow),
                                CAPTURE_NOTE_ID => Some(HotkeyEvent::CaptureNote),
                                _ => None,
                            };
                            if let Some(event) = event {
                                diagnostic(&format!("dispatching {event:?}"));
                                if sender.send(Ok(event)).is_err() {
                                    diagnostic("hotkey receiver disconnected");
                                    break;
                                }
                                if !super::windows::is_workspace_visible() {
                                    if super::windows::wake_for_hotkey() {
                                        diagnostic("posted workspace wake for hotkey");
                                    } else {
                                        diagnostic("could not post workspace wake for hotkey");
                                    }
                                }
                                context.request_repaint();
                            }
                        }
                    }

                    for id in registered_ids {
                        let _ = UnregisterHotKey(None, id);
                    }
                }
            })
            .expect("failed to create hotkey thread");

        Self { receiver }
    }

    pub fn try_recv(&self) -> Option<Result<HotkeyEvent, String>> {
        self.receiver.try_recv().ok()
    }
}

fn diagnostic(message: &str) {
    let Some(path) = std::env::var_os("CAPTUR_HOTKEY_LOG") else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}
