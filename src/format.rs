/// Format seconds as MM:SS, or HH:MM:SS from one hour up (hours unbounded).
pub fn fmt_seconds(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, rem) = (s / 3600, s % 3600);
    let (m, sec) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod test_fmt_seconds {
        use super::*;

        #[test]
        fn under_a_minute() {
            assert_eq!(fmt_seconds(59.0), "00:59");
        }

        #[test]
        fn minutes() {
            assert_eq!(fmt_seconds(61.0), "01:01");
            assert_eq!(fmt_seconds(300.0), "05:00");
        }

        #[test]
        fn hour_boundary() {
            assert_eq!(fmt_seconds(3599.0), "59:59");
            assert_eq!(fmt_seconds(3600.0), "01:00:00");
        }

        #[test]
        fn large() {
            assert_eq!(fmt_seconds(86399.0), "23:59:59");
        }

        #[test]
        fn negative_clamps_to_zero() {
            assert_eq!(fmt_seconds(-3.0), "00:00");
        }

        #[test]
        fn fmt_seconds_over_100_hours() {
            assert_eq!(fmt_seconds(360000.0), "100:00:00");
        }
    }
}
