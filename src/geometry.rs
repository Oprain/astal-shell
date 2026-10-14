use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputSize {
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Placement {
    pub horizontal: i32,
    pub vertical: i32,
    pub bottom_vertical: i32,
    pub left: i32,
    pub left_width: i32,
    pub card_width: i32,
    pub top_visible: bool,
    pub bottom_visible: bool,
    pub left_visible: bool,
}

// Match Niri's physical-pixel rounding of workspace_size, workspace_gap and
// static_offset. Layer surfaces use logical coordinates and are not zoomed.
pub fn place(
    output: OutputSize,
    zoom: f64,
    top_height: i32,
    bottom_height: i32,
    left_width: i32,
    clearance: i32,
) -> Placement {
    let scale = output.scale.max(0.01);
    let zoom = if zoom.is_finite() {
        zoom.clamp(0.0001, 0.75)
    } else {
        0.5
    };
    let rounded = |v: f64| (v * scale).round() / scale;
    let card_w = (output.width * zoom * scale).ceil() / scale;
    let card_h = (output.height * zoom * scale).ceil() / scale;
    let x = rounded((output.width - card_w) / 2.);
    let y = rounded((output.height - card_h) / 2.);
    let gap = (rounded(output.height * 0.1 * zoom)).max(1. / scale);
    let gap_start = y - gap;
    let gap_end = y;
    let edge = f64::from(clearance.max(0));
    // GTK layer margins are integer logical pixels. Round inward, leaving a
    // further pixel of room for fractional-scale compositor rounding.
    let margin = |height: i32| {
        let height = f64::from(height.max(0));
        let start = (gap_start + (gap - height) / 2.).ceil().max(0.);
        let fits = start >= edge && start >= gap_start + edge && start + height <= gap_end - edge;
        (start as i32, fits)
    };
    let (top, top_visible) = margin(top_height);
    let (bottom, bottom_visible) = margin(bottom_height);
    Placement {
        horizontal: x.ceil() as i32,
        vertical: top,
        bottom_vertical: bottom,
        left: edge.ceil() as i32,
        left_width,
        card_width: (output.width - 2. * x.ceil()).floor().max(0.) as i32,
        top_visible,
        bottom_visible,
        left_visible: f64::from(left_width) + 2. * edge + 1. <= x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_never_intersect_workspace_cards_at_mixed_scales() {
        for (w, h, scale) in [
            (1920., 1080., 1.),
            (1536., 864., 1.25),
            (2560., 1440., 1.5),
            (1080., 1920., 2.),
            (800., 600., 1.),
        ] {
            for zoom in [0.1, 0.35, 0.5, 0.6, 0.75, 1.] {
                let p = place(
                    OutputSize {
                        width: w,
                        height: h,
                        scale,
                    },
                    zoom,
                    48,
                    52,
                    20,
                    3,
                );
                let z = zoom.clamp(0.0001, 0.75);
                let card_h = (h * z * scale).ceil() / scale;
                let y = (((h - card_h) / 2.) * scale).round() / scale;
                let gap = ((h * 0.1 * z * scale).round() / scale).max(1. / scale);
                if p.top_visible {
                    assert!(f64::from(p.vertical) >= y - gap + 3. - 1e-6);
                    assert!(f64::from(p.vertical + 48) <= y - 3. + 1e-6);
                }
                if p.bottom_visible {
                    assert!(f64::from(p.bottom_vertical) >= y - gap + 3. - 1e-6);
                    assert!(f64::from(p.bottom_vertical + 52) <= y - 3. + 1e-6);
                }
                if p.left_visible {
                    assert!(f64::from(p.left + p.left_width) < f64::from(p.horizontal));
                }
            }
        }
    }

    #[test]
    fn small_gap_hides_bars_instead_of_overlapping() {
        let p = place(
            OutputSize {
                width: 1280.,
                height: 720.,
                scale: 1.,
            },
            0.5,
            48,
            52,
            20,
            3,
        );
        assert!(!p.top_visible);
        assert!(!p.bottom_visible);
        assert!(p.left_visible);
    }

    #[test]
    fn oversized_content_only_hides_its_own_bar() {
        let output = OutputSize {
            width: 1920.,
            height: 1080.,
            scale: 1.,
        };
        let tall_media = place(output, 0.5, 512, 32, 20, 3);
        assert!(!tall_media.top_visible);
        assert!(tall_media.bottom_visible);
        assert!(tall_media.left_visible);

        let tall_dock = place(output, 0.5, 32, 512, 20, 3);
        assert!(tall_dock.top_visible);
        assert!(!tall_dock.bottom_visible);
    }

    #[test]
    fn uses_niris_zoom_limit_and_handles_nan() {
        let o = OutputSize {
            width: 1920.,
            height: 1080.,
            scale: 1.,
        };
        assert_eq!(place(o, 1., 40, 40, 20, 3), place(o, 0.75, 40, 40, 20, 3));
        assert_eq!(
            place(o, f64::NAN, 40, 40, 20, 3),
            place(o, 0.5, 40, 40, 20, 3)
        );
    }
}
