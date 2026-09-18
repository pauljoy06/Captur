use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

const CAPTURE_REGION_ID: i32 = 0x5053_0001;
const CAPTURE_SAME_REGION_ID: i32 = 0x5053_0002;
const SHOW_WORKSPACE_ID: i32 = 0x5053_0003;
const CAPTURE_MONITOR_ID: i32 = 0x5053_0004;
const CAPTURE_ACTIVE_WINDOW_ID: i32 = 0x5053_0005;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    CaptureRegion,
    CaptureSameRegion,
    ShowWorkspace,
    CaptureMonitor,
    CaptureActiveWindow,
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
                    let modifiers = MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
                    if let Err(error) =
                        RegisterHotKey(None, CAPTURE_REGION_ID, modifiers, b'4' as u32)
                    {
                        let _ =
                            sender.send(Err(format!("could not register Ctrl+Shift+4: {error}")));
                        context.request_repaint();
                        return;
                    }
                    if let Err(error) =
                        RegisterHotKey(None, CAPTURE_SAME_REGION_ID, modifiers, b'5' as u32)
                    {
                        let _ = UnregisterHotKey(None, CAPTURE_REGION_ID);
                        let _ =
                            sender.send(Err(format!("could not register Ctrl+Shift+5: {error}")));
                        context.request_repaint();
                        return;
                    }
                    if let Err(error) =
                        RegisterHotKey(None, SHOW_WORKSPACE_ID, modifiers, b'6' as u32)
                    {
                        let _ = UnregisterHotKey(None, CAPTURE_REGION_ID);
                        let _ = UnregisterHotKey(None, CAPTURE_SAME_REGION_ID);
                        let _ =
                            sender.send(Err(format!("could not register Ctrl+Shift+6: {error}")));
                        context.request_repaint();
                        return;
                    }
                    if let Err(error) =
                        RegisterHotKey(None, CAPTURE_MONITOR_ID, modifiers, b'7' as u32)
                    {
                        unregister_all();
                        let _ =
                            sender.send(Err(format!("could not register Ctrl+Shift+7: {error}")));
                        context.request_repaint();
                        return;
                    }
                    if let Err(error) =
                        RegisterHotKey(None, CAPTURE_ACTIVE_WINDOW_ID, modifiers, b'8' as u32)
                    {
                        unregister_all();
                        let _ =
                            sender.send(Err(format!("could not register Ctrl+Shift+8: {error}")));
                        context.request_repaint();
                        return;
                    }

                    let mut message = MSG::default();
                    while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                        if message.message == WM_HOTKEY {
                            let event = match message.wParam.0 as i32 {
                                CAPTURE_REGION_ID => Some(HotkeyEvent::CaptureRegion),
                                CAPTURE_SAME_REGION_ID => Some(HotkeyEvent::CaptureSameRegion),
                                SHOW_WORKSPACE_ID => Some(HotkeyEvent::ShowWorkspace),
                                CAPTURE_MONITOR_ID => Some(HotkeyEvent::CaptureMonitor),
                                CAPTURE_ACTIVE_WINDOW_ID => Some(HotkeyEvent::CaptureActiveWindow),
                                _ => None,
                            };
                            if let Some(event) = event {
                                if sender.send(Ok(event)).is_err() {
                                    break;
                                }
                                context.request_repaint();
                            }
                        }
                    }

                    unregister_all();
                }
            })
            .expect("failed to create hotkey thread");

        Self { receiver }
    }

    pub fn try_recv(&self) -> Option<Result<HotkeyEvent, String>> {
        self.receiver.try_recv().ok()
    }
}

unsafe fn unregister_all() {
    unsafe {
        let _ = UnregisterHotKey(None, CAPTURE_REGION_ID);
        let _ = UnregisterHotKey(None, CAPTURE_SAME_REGION_ID);
        let _ = UnregisterHotKey(None, SHOW_WORKSPACE_ID);
        let _ = UnregisterHotKey(None, CAPTURE_MONITOR_ID);
        let _ = UnregisterHotKey(None, CAPTURE_ACTIVE_WINDOW_ID);
    }
}
