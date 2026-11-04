//! Movement for still images, which have no animation of their own. Same choreography as the
//! character.json "motion" values people already use, evaluated directly instead of through CSS.

use crate::i18n::t;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    None,
    Bob,
    Hop,
    Nudge,
    Type,
    Look,
}

pub const ALL: [Motion; 6] = [Motion::None, Motion::Bob, Motion::Hop, Motion::Nudge, Motion::Type, Motion::Look];

/// Offsets around the character's feet, in logical pixels and degrees.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Xform {
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
    pub rot: f32,
}

const REST: Xform = Xform { dy: 0.0, sx: 1.0, sy: 1.0, rot: 0.0 };

/// (time in the cycle 0..1, value)
type Key = (f32, Xform);

const fn k(at: f32, dy: f32, sx: f32, sy: f32, rot: f32) -> Key {
    (at, Xform { dy, sx, sy, rot })
}

const BOB: [Key; 3] = [k(0.0, 0.0, 1.0, 1.0, 0.0), k(0.5, -6.0, 0.98, 1.02, 0.0), k(1.0, 0.0, 1.0, 1.0, 0.0)];
const HOP: [Key; 6] = [
    k(0.0, 0.0, 1.0, 1.0, 0.0),
    k(0.08, 0.0, 1.06, 0.94, 0.0),
    k(0.2, -18.0, 0.97, 1.03, 0.0),
    k(0.32, 0.0, 1.04, 0.96, 0.0),
    k(0.4, 0.0, 1.0, 1.0, 0.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
];
const NUDGE: [Key; 6] = [
    k(0.0, 0.0, 1.0, 1.0, 0.0),
    k(0.7, 0.0, 1.0, 1.0, 0.0),
    k(0.75, 0.0, 1.0, 1.0, -5.0),
    k(0.85, 0.0, 1.0, 1.0, 5.0),
    k(0.95, 0.0, 1.0, 1.0, -2.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
];
const TYPE: [Key; 12] = [
    k(0.0, 0.0, 1.0, 1.0, 0.0),
    k(0.06, 2.0, 1.0, 1.0, -0.6),
    k(0.12, 0.0, 1.0, 1.0, 0.0),
    k(0.18, 1.0, 1.0, 1.0, 0.5),
    k(0.24, 0.0, 1.0, 1.0, 0.0),
    k(0.30, 2.0, 1.0, 1.0, -0.4),
    k(0.36, 0.0, 1.0, 1.0, 0.0),
    k(0.42, 1.0, 1.0, 1.0, 0.6),
    k(0.48, 0.0, 1.0, 1.0, 0.0),
    k(0.54, 2.0, 1.0, 1.0, -0.5),
    k(0.60, 0.0, 1.0, 1.0, 0.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
];
/// Turning sideways is a 2D squeeze: cos(22°) of the width is what a perspective turn looks like here.
const TURNED: f32 = 0.927;
const LOOK: [Key; 13] = [
    k(0.0, 0.0, 1.0, 1.0, 0.0),
    k(0.08, -4.0, 1.0, 1.0, 0.0),
    k(0.16, 0.0, TURNED, 1.0, 0.0),
    k(0.26, -3.0, TURNED, 1.0, 0.0),
    k(0.36, 0.0, TURNED, 1.0, 0.0),
    k(0.5, -4.0, 1.0, 1.0, 0.0),
    k(0.6, 0.0, TURNED, 1.0, 0.0),
    k(0.7, -3.0, TURNED, 1.0, 0.0),
    k(0.8, 0.0, TURNED, 1.0, 0.0),
    k(0.92, -4.0, 1.0, 1.0, 0.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
    k(1.0, 0.0, 1.0, 1.0, 0.0),
];

fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn sample(keys: &[Key], at: f32) -> Xform {
    for w in keys.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if at <= t1 && t1 > t0 {
            let t = ease(((at - t0) / (t1 - t0)).clamp(0.0, 1.0));
            return Xform { dy: mix(a.dy, b.dy, t), sx: mix(a.sx, b.sx, t), sy: mix(a.sy, b.sy, t), rot: mix(a.rot, b.rot, t) };
        }
    }
    REST
}

impl Motion {
    pub fn parse(s: &str) -> Option<Motion> {
        Some(match s {
            "none" => Motion::None,
            "bob" => Motion::Bob,
            "hop" => Motion::Hop,
            "nudge" => Motion::Nudge,
            "type" => Motion::Type,
            "look" => Motion::Look,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Motion::None => "none",
            Motion::Bob => "bob",
            Motion::Hop => "hop",
            Motion::Nudge => "nudge",
            Motion::Type => "type",
            Motion::Look => "look",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Motion::None => t("đứng yên", "still"),
            Motion::Bob => t("nhún", "bob"),
            Motion::Hop => t("nhảy", "hop"),
            Motion::Nudge => t("lắc", "nudge"),
            Motion::Type => t("gõ phím", "typing"),
            Motion::Look => t("nhìn quanh", "look around"),
        }
    }

    pub fn at(self, t: f32) -> Xform {
        let (keys, cycle): (&[Key], f32) = match self {
            Motion::None => return REST,
            Motion::Bob => (&BOB, 0.7),
            Motion::Hop => (&HOP, 1.6),
            Motion::Nudge => (&NUDGE, 1.6),
            Motion::Type => (&TYPE, 1.4),
            Motion::Look => (&LOOK, 4.0),
        };
        sample(keys, t.rem_euclid(cycle) / cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for m in ALL {
            assert_eq!(Motion::parse(m.name()), Some(m));
        }
        assert_eq!(Motion::parse("spin"), None);
    }

    #[test]
    fn hits_the_key_poses() {
        assert_eq!(Motion::Bob.at(0.35).dy, -6.0);
        assert_eq!(Motion::Hop.at(1.6 * 0.2).dy, -18.0);
        assert_eq!(Motion::None.at(3.3), REST);
        let nudge = Motion::Nudge.at(1.6 * 0.85);
        assert!((nudge.rot - 5.0).abs() < 0.001, "{nudge:?}");
    }

    #[test]
    fn loops_without_a_jump_and_stays_in_the_reserved_margin() {
        for m in ALL {
            let (a, b) = (m.at(0.0), m.at(1000.0 * 4.0 * 1.4 * 1.6 * 0.7));
            assert!((a.dy - b.dy).abs() < 2.5, "{m:?}");
            let mut prev = m.at(0.0);
            for i in 1..4000 {
                let x = m.at(i as f32 * 0.002);
                assert!(x.dy.abs() <= 18.0 && x.rot.abs() <= 5.0, "{m:?} {x:?}");
                assert!((x.dy - prev.dy).abs() < 2.0, "{m:?} jumps at {i}");
                prev = x;
            }
        }
    }
}
