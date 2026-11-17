use tray_icon::Icon;

const SIZE: u32 = 44;

pub fn tray() -> Icon {
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let s = SIZE as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let head = ((px - s * 0.42).powi(2) + (py - s * 0.33).powi(2)).sqrt() <= s * 0.17;
            let body_dx = (px - s * 0.42) / (s * 0.30);
            let body_dy = (py - s * 0.86) / (s * 0.30);
            let body = py <= s * 0.86 && body_dx * body_dx + body_dy * body_dy <= 1.0;
            let dot = ((px - s * 0.76).powi(2) + (py - s * 0.24).powi(2)).sqrt() <= s * 0.11;
            let glyph = head || body || dot;
            if cfg!(target_os = "macos") {
                rgba.extend_from_slice(&[0, 0, 0, if glyph { 255 } else { 0 }]);
            } else if glyph {
                rgba.extend_from_slice(&[255, 255, 255, 255]);
            } else {
                let round = ((px - s / 2.0).powi(2) + (py - s / 2.0).powi(2)).sqrt() <= s / 2.0;
                rgba.extend_from_slice(&[88, 86, 214, if round { 255 } else { 0 }]);
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("tray icon")
}
