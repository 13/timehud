#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub const POSITIONS: [&str; 6] = [
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
    "top-center",
    "bottom-center",
];

/// Half a window counts as "centered" on an axis within this many px.
const CENTER_BAND: f64 = 24.0;
const SNAP: i32 = 20;

pub fn preset_position(preset: &str, area: Rect, w: i32, h: i32, margin: i32) -> (i32, i32) {
    let m = margin;
    let right = area.w - w - m;
    let bottom = area.h - h - m;
    let center = (area.w - w).div_euclid(2);
    let (x, y) = match preset {
        "top-left" => (m, m),
        "bottom-left" => (m, bottom),
        "bottom-right" => (right, bottom),
        "top-center" => (center, m),
        "bottom-center" => (center, bottom),
        _ => (right, m), // "top-right" and unknown
    };
    (area.x + x, area.y + y)
}

/// Move delta that keeps a dragged window's anchor fixed on resize.
///
/// `cx/cy` are the window center *after* the grow (the top-left corner stays
/// put), so the pre-resize center is `cx - dw/2`. Per axis: centered windows
/// keep their center, otherwise the near edge stays put.
pub fn resize_anchor_delta(
    cx: f64,
    cy: f64,
    scr_cx: f64,
    scr_cy: f64,
    dw: i32,
    dh: i32,
) -> (i32, i32) {
    let axis = |c: f64, scr_c: f64, d: i32| {
        let old_c = c - f64::from(d) / 2.0;
        if (old_c - scr_c).abs() <= CENTER_BAND {
            // Python round(): ties to even
            -(f64::from(d) / 2.0).round_ties_even() as i32
        } else if old_c > scr_c {
            -d
        } else {
            0
        }
    };
    (axis(cx, scr_cx, dw), axis(cy, scr_cy, dh))
}

/// Magnetic edge/center snapping while dragging. Uses Qt rect conventions:
/// `right = x + w - 1`, `bottom = y + h - 1`, `center = (first + last) / 2`.
pub fn snap_drag(x: i32, y: i32, w: i32, h: i32, area: Rect, margin: i32) -> (i32, i32) {
    let left = area.x;
    let right = area.x + area.w - 1;
    let top = area.y;
    let bottom = area.y + area.h - 1;
    let ccx = (left + right) / 2;
    let ccy = (top + bottom) / 2;

    let nx = if (x - left).abs() < SNAP {
        left + margin
    } else if (x + w - right).abs() < SNAP {
        right - w - margin
    } else if (x + w / 2 - ccx).abs() < SNAP {
        ccx - w / 2
    } else {
        x
    };
    let ny = if (y - top).abs() < SNAP {
        top + margin
    } else if (y + h - bottom).abs() < SNAP {
        bottom - h - margin
    } else if (y + h / 2 - ccy).abs() < SNAP {
        ccy - h / 2
    } else {
        y
    };
    (nx, ny)
}

/// Keep the window fully inside `area` (pinned to the top-left if too big).
pub fn clamp_into(x: i32, y: i32, w: i32, h: i32, area: Rect) -> (i32, i32) {
    let max_x = (area.x + area.w - w).max(area.x);
    let max_y = (area.y + area.h - h).max(area.y);
    (x.clamp(area.x, max_x), y.clamp(area.y, max_y))
}

/// Where a `w`×`h` tooltip for `anchor` goes: centred under it, or above
/// it when there is no room below, `gap` px away, kept inside `area`.
pub fn tooltip_pos(anchor: Rect, w: i32, h: i32, area: Rect, gap: i32) -> (i32, i32) {
    let x = anchor.x + (anchor.w - w) / 2;
    let below = anchor.y + anchor.h + gap;
    let y = if below + h <= area.y + area.h {
        below
    } else {
        anchor.y - gap - h
    };
    clamp_into(x, y, w, h, area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_below_or_above_the_anchor_inside_the_area() {
        // Work area below a 24 px top panel; icon in that panel.
        let area = Rect {
            x: 0,
            y: 24,
            w: 1920,
            h: 1056,
        };
        let icon = Rect {
            x: 1800,
            y: 0,
            w: 24,
            h: 24,
        };
        assert_eq!(tooltip_pos(icon, 200, 30, area, 4), (1712, 28));
        // Near the right edge: shifted left to stay inside.
        let corner = Rect { x: 1900, ..icon };
        assert_eq!(tooltip_pos(corner, 200, 30, area, 4), (1720, 28));
        // Bottom panel: no room below, so above.
        let area = Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1056,
        };
        let icon = Rect { y: 1056, ..icon };
        assert_eq!(tooltip_pos(icon, 200, 30, area, 4), (1712, 1022));
    }

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    // scr center at (1000, 500); band comfortably covers by-eye centering.
    #[test]
    fn centered_window_keeps_center() {
        let (dx, dy) = resize_anchor_delta(1040.0, 100.0, 1000.0, 500.0, 80, 0);
        assert_eq!(dx, -40);
        assert_eq!(dy, 0);
    }

    #[test]
    fn right_window_keeps_right_edge() {
        let (dx, _) = resize_anchor_delta(1800.0, 100.0, 1000.0, 500.0, 60, 0);
        assert_eq!(dx, -60);
    }

    #[test]
    fn left_window_keeps_left_edge() {
        let (dx, _) = resize_anchor_delta(200.0, 100.0, 1000.0, 500.0, 60, 0);
        assert_eq!(dx, 0);
    }

    #[test]
    fn center_detection_survives_large_growth() {
        let (dx, _) = resize_anchor_delta(1100.0, 100.0, 1000.0, 500.0, 200, 0);
        assert_eq!(dx, -100);
    }

    #[test]
    fn bottom_window_keeps_bottom_edge() {
        let (_, dy) = resize_anchor_delta(1000.0, 900.0, 1000.0, 500.0, 0, 40);
        assert_eq!(dy, -40);
    }

    #[test]
    fn preset_positions() {
        assert_eq!(preset_position("top-right", AREA, 200, 100, 15), (1705, 15));
        assert_eq!(
            preset_position("bottom-center", AREA, 200, 100, 15),
            (860, 965)
        );
        assert_eq!(preset_position("top-left", AREA, 200, 100, 15), (15, 15));
        assert_eq!(
            preset_position("bottom-right", AREA, 200, 100, 15),
            (1705, 965)
        );
        assert_eq!(preset_position("bogus", AREA, 200, 100, 15), (1705, 15));
        let a = Rect { x: 1920, ..AREA };
        assert_eq!(preset_position("top-right", a, 200, 100, 15), (3625, 15));
    }

    #[test]
    fn snap_to_left_edge_uses_margin() {
        assert_eq!(snap_drag(10, 400, 200, 100, AREA, 15), (15, 400));
    }

    #[test]
    fn snap_to_right_and_bottom_edges() {
        // right = 1919, so x + w - right < 20 snaps to right - w - margin
        assert_eq!(snap_drag(1715, 300, 200, 100, AREA, 15), (1704, 300));
        assert_eq!(snap_drag(500, 975, 200, 100, AREA, 15), (500, 964));
    }

    #[test]
    fn snap_to_horizontal_center() {
        // center.x = 959 (Qt convention); x + 100 near 959 -> x = 859
        assert_eq!(snap_drag(865, 400, 200, 100, AREA, 15), (859, 400));
    }

    #[test]
    fn no_snap_far_from_edges() {
        assert_eq!(snap_drag(400, 300, 200, 100, AREA, 15), (400, 300));
    }

    #[test]
    fn clamp_offscreen_position() {
        assert_eq!(clamp_into(5000, -300, 200, 100, AREA), (1720, 0));
        assert_eq!(clamp_into(100, 100, 200, 100, AREA), (100, 100));
    }
}
