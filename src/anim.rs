use crate::themes::Rgb;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ease {
    Linear,
    InOutCubic,
    OutCubic,
}

pub struct Tween {
    from: f64,
    to: f64,
    start: f64,
    dur: f64,
    ease: Ease,
}

impl Tween {
    pub fn new(from: f64, to: f64, start: f64, dur: f64, ease: Ease) -> Self {
        Tween {
            from,
            to,
            start,
            dur,
            ease,
        }
    }
    pub fn value(&self, now: f64) -> f64 {
        if self.done(now) {
            return self.to;
        }
        let t = ((now - self.start) / self.dur).clamp(0.0, 1.0);
        let e = match self.ease {
            Ease::Linear => t,
            Ease::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Ease::OutCubic => 1.0 - (1.0 - t).powi(3),
        };
        self.from + (self.to - self.from) * e
    }
    pub fn done(&self, now: f64) -> bool {
        now >= self.start + self.dur
    }
    pub fn target(&self) -> f64 {
        self.to
    }
}

pub fn lerp_rgb(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let ch = |x: u8, y: u8| {
        (f64::from(x) + (f64::from(y) - f64::from(x)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb(ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tween_endpoints_and_easing() {
        let t = Tween::new(10.0, 20.0, 1.0, 2.0, Ease::InOutCubic);
        assert_eq!(t.value(0.0), 10.0);
        assert_eq!(t.value(1.0), 10.0);
        assert!((t.value(2.0) - 15.0).abs() < 1e-9);
        assert_eq!(t.value(3.0), 20.0);
        assert_eq!(t.value(9.0), 20.0);
        assert!(!t.done(2.0));
        assert!(t.done(3.0));
        assert_eq!(t.target(), 20.0);
        let l = Tween::new(0.0, 10.0, 0.0, 1.0, Ease::Linear);
        assert!((l.value(0.25) - 2.5).abs() < 1e-9);
        let o = Tween::new(0.0, 1.0, 0.0, 1.0, Ease::OutCubic);
        assert!((o.value(0.5) - 0.875).abs() < 1e-9);
        let z = Tween::new(0.0, 5.0, 0.0, 0.0, Ease::Linear);
        assert_eq!(z.value(0.0), 5.0);
        assert!(z.done(0.0));
    }

    #[test]
    fn lerp_rgb_midpoint() {
        assert_eq!(
            lerp_rgb(Rgb(0, 100, 255), Rgb(100, 200, 255), 0.5),
            Rgb(50, 150, 255)
        );
        assert_eq!(lerp_rgb(Rgb(0, 0, 0), Rgb(10, 10, 10), 0.0), Rgb(0, 0, 0));
        assert_eq!(
            lerp_rgb(Rgb(0, 0, 0), Rgb(10, 10, 10), 1.0),
            Rgb(10, 10, 10)
        );
    }
}
