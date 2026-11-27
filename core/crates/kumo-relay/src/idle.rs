/// Seconds since the last local keyboard, mouse or trackpad input; `None` when unknown, never a stand-in 0.
pub fn idle_seconds() -> Option<u64> {
    platform::idle_seconds().and_then(whole_seconds)
}

fn whole_seconds(seconds: f64) -> Option<u64> {
    (0.0..1e10).contains(&seconds).then_some(seconds as u64)
}

#[cfg(target_os = "macos")]
mod platform {
    // The HID state is the kernel's physical-input timer (IOHIDSystem's HIDIdleTime) and needs no privacy permission;
    // the combined session state also resets when an app declares user activity (`caffeinate -u`).
    const HID_SYSTEM_STATE: i32 = 1;
    const ANY_INPUT_EVENT: u32 = u32::MAX;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }

    pub fn idle_seconds() -> Option<f64> {
        // SAFETY: takes and returns plain values.
        Some(unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) })
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    pub fn idle_seconds() -> Option<f64> {
        let mut session = 0;
        // SAFETY: `session` is a valid out pointer.
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 || session == 0
        {
            // GetLastInputInfo sees only this process's session, and session 0 (a service, or a task that runs
            // whether or not the user is logged on) never has a user at the keyboard.
            return None;
        }
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a LASTINPUTINFO with `cbSize` set.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // SAFETY: no arguments.
        let now = unsafe { GetTickCount() };
        // Both are 32-bit tick counts, so after 49.7 days without input this wraps back to a small number.
        Some(f64::from(now.wrapping_sub(info.dwTime)) / 1000.0)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    // Linux would need X11 or D-Bus client libraries, and most Wayland compositors expose no idle time at all.
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
}
