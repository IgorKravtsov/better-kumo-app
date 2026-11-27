//! Seconds since the last keyboard, mouse or trackpad input on this computer, read with one system call per request.
//! `None` means unknown (unsupported platform or a failed call), never "just now".

/// The user's input idle time in whole seconds, or `None` when this platform can't tell.
pub fn idle_seconds() -> Option<u64> {
    platform::idle_seconds().and_then(whole_seconds)
}

fn whole_seconds(seconds: f64) -> Option<u64> {
    // NaN, negative and absurd values (a failed call) are unknown rather than 0.
    (0.0..1e10).contains(&seconds).then_some(seconds as u64)
}

#[cfg(target_os = "macos")]
mod platform {
    // The HID system's idle timer (the same as IOHIDSystem's `HIDIdleTime`): it reports when the last keyboard, mouse or
    // trackpad event happened, not what it was, so it needs no Accessibility or Input Monitoring permission, and it
    // works for a login item or a launchd agent in the session. The combined session state would also count apps
    // declaring user activity (`caffeinate -u`, players keeping the screen on), which isn't the user at the keyboard.
    const HID_SYSTEM_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
    const ANY_INPUT_EVENT: u32 = u32::MAX; // kCGAnyInputEventType

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }

    pub fn idle_seconds() -> Option<f64> {
        // SAFETY: a plain C function taking two integers and returning a double.
        Some(unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) })
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    pub fn idle_seconds() -> Option<f64> {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a valid LASTINPUTINFO with `cbSize` set, as the call requires.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // SAFETY: no arguments. Both are 32-bit millisecond tick counts, so the difference survives the 49.7-day wrap.
        let now = unsafe { GetTickCount() };
        Some(f64::from(now.wrapping_sub(info.dwTime)) / 1000.0)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    // Linux has no session-wide idle time without X11 or D-Bus client libraries (and none at all on most Wayland
    // compositors), so it stays unknown.
    pub fn idle_seconds() -> Option<f64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_readings_are_unknown_not_zero() {
        assert_eq!(whole_seconds(f64::NAN), None);
        assert_eq!(whole_seconds(f64::INFINITY), None);
        assert_eq!(whole_seconds(-1.0), None);
        assert_eq!(whole_seconds(1e12), None);
    }

    #[test]
    fn readings_round_down_to_whole_seconds() {
        assert_eq!(whole_seconds(0.0), Some(0));
        assert_eq!(whole_seconds(0.9), Some(0));
        assert_eq!(whole_seconds(299.99), Some(299));
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn supported_platforms_report_a_value() {
        assert!(idle_seconds().is_some());
    }
}
