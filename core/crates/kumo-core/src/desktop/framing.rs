use super::state::DesktopState;

const ASPECT: f64 = 1.6;
const ZONE_MARGIN: f64 = 1.5;
const PERSON_SPAN: (f64, f64) = (1400.0, 900.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl DesktopState {
    pub fn bounds(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: self.width,
            h: self.height,
        }
    }

    pub fn viewport(&self) -> Rect {
        let full = self.bounds();
        if self.width <= 0.0 || self.height <= 0.0 {
            return full;
        }
        let (cx, cy, base_w, base_h) = match (self.self_zone(), self.self_person()) {
            (Some(z), _) => (
                z.x + z.w / 2.0,
                z.y + z.h / 2.0,
                z.w * ZONE_MARGIN,
                z.h * ZONE_MARGIN,
            ),
            (None, Some(me)) => (me.x, me.y, PERSON_SPAN.0, PERSON_SPAN.1),
            (None, None) => return full,
        };
        let w = base_w.max(base_h * ASPECT).min(self.width);
        let h = (w / ASPECT).min(self.height);
        Rect {
            x: (cx - w / 2.0).clamp(0.0, (self.width - w).max(0.0)),
            y: (cy - h / 2.0).clamp(0.0, (self.height - h).max(0.0)),
            w,
            h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::state::{DesktopPerson, DesktopSelf, DesktopZone};

    fn floor(width: f64, height: f64) -> DesktopState {
        DesktopState {
            floor_id: None,
            width,
            height,
            floors: Vec::new(),
            me: None,
            zones: Vec::new(),
            people: Vec::new(),
            audio: false,
            video: false,
        }
    }

    fn me_at(state: &mut DesktopState, x: f64, y: f64, zone: Option<&str>) {
        state.me = Some(DesktopSelf {
            id: "me".into(),
            x,
            y,
            zone_id: zone.map(String::from),
            availability: None,
            message: String::new(),
        });
        state.people.push(DesktopPerson {
            id: "me".into(),
            uid: String::new(),
            name: "Me".into(),
            avatar: String::new(),
            x,
            y,
            zone_id: zone.map(String::from),
            is_self: true,
            availability: None,
            idle: false,
            message: String::new(),
        });
    }

    fn add_zone(state: &mut DesktopState, id: &str, rect: Rect) {
        state.zones.push(DesktopZone {
            id: id.into(),
            name: id.into(),
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            open: true,
        });
    }

    #[test]
    fn without_anyone_to_follow_the_whole_floor_is_shown() {
        let s = floor(3000.0, 2000.0);
        assert_eq!(s.viewport(), s.bounds());
        assert_eq!(floor(0.0, 0.0).viewport(), floor(0.0, 0.0).bounds());
    }

    #[test]
    fn your_zone_is_centred_with_a_margin_at_the_fixed_aspect() {
        let mut s = floor(4000.0, 3000.0);
        add_zone(
            &mut s,
            "a",
            Rect {
                x: 1000.0,
                y: 1000.0,
                w: 400.0,
                h: 200.0,
            },
        );
        me_at(&mut s, 1100.0, 1100.0, Some("a"));
        assert_eq!(
            s.viewport(),
            Rect {
                x: 900.0,
                y: 912.5,
                w: 600.0,
                h: 375.0,
            }
        );
    }

    #[test]
    fn a_tall_zone_widens_to_keep_the_aspect() {
        let mut s = floor(4000.0, 3000.0);
        add_zone(
            &mut s,
            "a",
            Rect {
                x: 1000.0,
                y: 1000.0,
                w: 100.0,
                h: 400.0,
            },
        );
        me_at(&mut s, 1050.0, 1100.0, Some("a"));
        let v = s.viewport();
        assert_eq!((v.w, v.h), (960.0, 600.0));
        assert_eq!((v.x + v.w / 2.0, v.y + v.h / 2.0), (1050.0, 1200.0));
    }

    #[test]
    fn framing_near_the_edge_is_clamped_inside_the_floor() {
        let mut s = floor(3000.0, 2000.0);
        me_at(&mut s, 100.0, 1950.0, None);
        assert_eq!(
            s.viewport(),
            Rect {
                x: 0.0,
                y: 1100.0,
                w: 1440.0,
                h: 900.0,
            }
        );
    }

    #[test]
    fn a_floor_smaller_than_the_frame_is_shown_whole() {
        let mut s = floor(1000.0, 500.0);
        me_at(&mut s, 500.0, 250.0, None);
        assert_eq!(
            s.viewport(),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 500.0,
            }
        );
    }
}
