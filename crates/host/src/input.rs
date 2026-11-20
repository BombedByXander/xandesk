use arcdesk_protocol::Input;
use std::collections::HashSet;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

pub struct Controller {
    keys: HashSet<String>,
    buttons: HashSet<u8>,
    clipboard: bool,
    last_event: std::time::Instant,
}
impl Controller {
    pub fn new(clipboard: bool) -> Self {
        Self {
            keys: HashSet::new(),
            buttons: HashSet::new(),
            clipboard,
            last_event: std::time::Instant::now(),
        }
    }
    pub fn apply(&mut self, event: Input) {
        self.last_event = std::time::Instant::now();
        match event {
            Input::Move { x, y } if x.is_finite() && y.is_finite() => mouse(
                (x.clamp(0.0, 1.0) * 65535.0) as i32,
                (y.clamp(0.0, 1.0) * 65535.0) as i32,
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            ),
            Input::Relative { x, y } => mouse(
                x.clamp(-32767, 32767),
                y.clamp(-32767, 32767),
                0,
                MOUSEEVENTF_MOVE,
            ),
            Input::Button { button, down } if button <= 2 => {
                let changed = if down {
                    self.buttons.insert(button)
                } else {
                    self.buttons.remove(&button)
                };
                if changed {
                    mouse(0, 0, 0, button_flag(button, down));
                }
            }
            Input::Key { code, down } => {
                if let Some((scan, extended)) = scan_code(&code) {
                    let changed = if down {
                        self.keys.insert(code)
                    } else {
                        self.keys.remove(&code)
                    };
                    if changed {
                        key(scan, extended, down);
                    }
                }
            }
            Input::Wheel { delta } => {
                mouse(0, 0, delta.clamp(-1200, 1200) as u32, MOUSEEVENTF_WHEEL)
            }
            Input::Release => self.release(),
            Input::Clipboard { text } if self.clipboard && text.len() <= 64000 => {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(text);
                }
            }
            _ => {}
        }
    }
    pub fn release(&mut self) {
        for code in self.keys.drain() {
            if let Some((scan, extended)) = scan_code(&code) {
                key(scan, extended, false);
            }
        }
        for button in self.buttons.drain() {
            mouse(0, 0, 0, button_flag(button, false));
        }
    }
    pub fn release_if_stale(&mut self) {
        if self.last_event.elapsed() > std::time::Duration::from_secs(3) {
            self.release();
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.release();
    }
}
fn emit(input: INPUT) {
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}
fn mouse(x: i32, y: i32, data: u32, flags: u32) {
    emit(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
}
fn key(scan: u16, extended: bool, down: bool) {
    emit(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: KEYEVENTF_SCANCODE
                    | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }
                    | if down { 0 } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
}
fn button_flag(button: u8, down: bool) -> u32 {
    match (button, down) {
        (0, true) => MOUSEEVENTF_LEFTDOWN,
        (0, false) => MOUSEEVENTF_LEFTUP,
        (1, true) => MOUSEEVENTF_MIDDLEDOWN,
        (1, false) => MOUSEEVENTF_MIDDLEUP,
        (2, true) => MOUSEEVENTF_RIGHTDOWN,
        _ => MOUSEEVENTF_RIGHTUP,
    }
}
fn scan_code(code: &str) -> Option<(u16, bool)> {
    let scan = match code {
        "Escape" => 0x01,
        "Digit1" => 0x02,
        "Digit2" => 0x03,
        "Digit3" => 0x04,
        "Digit4" => 0x05,
        "Digit5" => 0x06,
        "Digit6" => 0x07,
        "Digit7" => 0x08,
        "Digit8" => 0x09,
        "Digit9" => 0x0a,
        "Digit0" => 0x0b,
        "Minus" => 0x0c,
        "Equal" => 0x0d,
        "Backspace" => 0x0e,
        "Tab" => 0x0f,
        "KeyQ" => 0x10,
        "KeyW" => 0x11,
        "KeyE" => 0x12,
        "KeyR" => 0x13,
        "KeyT" => 0x14,
        "KeyY" => 0x15,
        "KeyU" => 0x16,
        "KeyI" => 0x17,
        "KeyO" => 0x18,
        "KeyP" => 0x19,
        "BracketLeft" => 0x1a,
        "BracketRight" => 0x1b,
        "Enter" => 0x1c,
        "ControlLeft" => 0x1d,
        "KeyA" => 0x1e,
        "KeyS" => 0x1f,
        "KeyD" => 0x20,
        "KeyF" => 0x21,
        "KeyG" => 0x22,
        "KeyH" => 0x23,
        "KeyJ" => 0x24,
        "KeyK" => 0x25,
        "KeyL" => 0x26,
        "Semicolon" => 0x27,
        "Quote" => 0x28,
        // Treat the browser's backtick key as the Windows key so viewers can
        // open the remote Start menu even on compact Chromebook keyboards.
        "Backquote" => return Some((0x5b, true)),
        "ShiftLeft" => 0x2a,
        "Backslash" => 0x2b,
        "KeyZ" => 0x2c,
        "KeyX" => 0x2d,
        "KeyC" => 0x2e,
        "KeyV" => 0x2f,
        "KeyB" => 0x30,
        "KeyN" => 0x31,
        "KeyM" => 0x32,
        "Comma" => 0x33,
        "Period" => 0x34,
        "Slash" => 0x35,
        "ShiftRight" => 0x36,
        "NumpadMultiply" => 0x37,
        "AltLeft" => 0x38,
        "Space" => 0x39,
        "CapsLock" => 0x3a,
        "F1" => 0x3b,
        "F2" => 0x3c,
        "F3" => 0x3d,
        "F4" => 0x3e,
        "F5" => 0x3f,
        "F6" => 0x40,
        "F7" => 0x41,
        "F8" => 0x42,
        "F9" => 0x43,
        "F10" => 0x44,
        "NumLock" => 0x45,
        "ScrollLock" => 0x46,
        "Numpad7" => 0x47,
        "Numpad8" => 0x48,
        "Numpad9" => 0x49,
        "NumpadSubtract" => 0x4a,
        "Numpad4" => 0x4b,
        "Numpad5" => 0x4c,
        "Numpad6" => 0x4d,
        "NumpadAdd" => 0x4e,
        "Numpad1" => 0x4f,
        "Numpad2" => 0x50,
        "Numpad3" => 0x51,
        "Numpad0" => 0x52,
        "NumpadDecimal" => 0x53,
        "F11" => 0x57,
        "F12" => 0x58,
        "ControlRight" => return Some((0x1d, true)),
        "AltRight" => return Some((0x38, true)),
        "ArrowUp" => return Some((0x48, true)),
        "ArrowDown" => return Some((0x50, true)),
        "ArrowLeft" => return Some((0x4b, true)),
        "ArrowRight" => return Some((0x4d, true)),
        "Home" => return Some((0x47, true)),
        "End" => return Some((0x4f, true)),
        "PageUp" => return Some((0x49, true)),
        "PageDown" => return Some((0x51, true)),
        "Insert" => return Some((0x52, true)),
        "Delete" => return Some((0x53, true)),
        "MetaLeft" => return Some((0x5b, true)),
        "MetaRight" => return Some((0x5c, true)),
        "NumpadEnter" => return Some((0x1c, true)),
        "NumpadDivide" => return Some((0x35, true)),
        _ => return None,
    };
    Some((scan, false))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extended_keys_and_untrusted_codes() {
        assert_eq!(scan_code("KeyW"), Some((0x11, false)));
        assert_eq!(scan_code("ControlRight"), Some((0x1d, true)));
        assert_eq!(scan_code("ArrowLeft"), Some((0x4b, true)));
        assert_eq!(scan_code("shell:calc"), None);
    }
}
