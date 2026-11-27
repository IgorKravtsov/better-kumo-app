/// Seconds since the last local keyboard, mouse or trackpad input; `None` when unknown, never a stand-in 0.
pub fn idle_seconds() -> Option<u64> {
    platform::idle_seconds().and_then(whole_seconds)
}

fn whole_seconds(seconds: f64) -> Option<u64> {
    (0.0..1e10).contains(&seconds).then_some(seconds as u64)
}

/// Milliseconds from `last` to `now`, both 32-bit tick counts. A wrapped difference can't tell 49.7 days of idle from
/// none, so the count restarts then; an input stamped up to a minute ahead (`SendInput` sets its own time) is now.
#[cfg_attr(not(windows), allow(dead_code))]
fn elapsed_ms(now: u32, last: u32) -> u32 {
    const AHEAD_MS: u32 = 60_000;
    match now.wrapping_sub(last) {
        elapsed if elapsed > u32::MAX - AHEAD_MS => 0,
        elapsed => elapsed,
    }
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
    use windows_sys::Win32::System::RemoteDesktop::{
        ProcessIdToSessionId, WTSGetActiveConsoleSessionId,
    };
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    pub fn idle_seconds() -> Option<f64> {
        let mut session = 0;
        // SAFETY: `session` is a valid out pointer.
        let known = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } != 0;
        // GetLastInputInfo sees only this process's session: session 0 (a service) has no keyboard, and a Remote
        // Desktop session's input comes from another computer.
        // SAFETY: no arguments.
        if !known || session != unsafe { WTSGetActiveConsoleSessionId() } {
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
        Some(f64::from(super::elapsed_ms(now, info.dwTime)) / 1000.0)
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
    fn tick_counts_survive_the_wrap_and_inputs_stamped_ahead() {
        assert_eq!(elapsed_ms(5_000, 2_000), 3_000);
        assert_eq!(elapsed_ms(1_000, u32::MAX - 999), 2_000);
        assert_eq!(elapsed_ms(1_000, 1_500), 0);
        assert_eq!(elapsed_ms(10, u32::MAX - 5), 16);
        assert_eq!(elapsed_ms(0, 59_999), 0);
        assert_eq!(elapsed_ms(0, 60_001), u32::MAX - 60_000);
    }

    #[test]
    fn readings_round_down_to_whole_seconds() {
        assert_eq!(whole_seconds(0.0), Some(0));
        assert_eq!(whole_seconds(0.9), Some(0));
        assert_eq!(whole_seconds(299.99), Some(299));
    }
}
