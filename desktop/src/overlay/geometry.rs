//! The launcher has one width and a height that follows its content. It is
//! not resizable or draggable, so there is no geometry to persist: it opens
//! over the browser window in use, else on the display the user is working on.
pub const WIDTH: f64 = 680.0;
/// Room around natively drawn shapes for their shadow. Liquid Glass casts a
/// wide, soft one; any less and the window's edge cuts it off in a visible
/// rectangle. The window is transparent there.
pub const SHAPE_INSET: f64 = 40.0;
/// The field alone.
pub const RESTING_HEIGHT: f64 = 56.0;
// The ordinary three-row home fits at first reveal; subsequent layouts use content height.
pub const INITIAL_HEIGHT: f64 = if cfg!(target_os = "windows") {
    250.0
} else {
    RESTING_HEIGHT
};
pub const MAX_HEIGHT: f64 = 600.0;
const MARGIN: f64 = 24.0;
/// Where the top edge sits, as a share of the work area's height. High
/// enough that results grow into open space, low enough to be near the eye.
const TOP: f64 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    max_height: f64,
}

impl Placement {
    /// `work_width`/`work_height` are the display's usable area in logical
    /// points; the returned origin is relative to that area.
    #[cfg(test)]
    pub fn new(work_width: f64, work_height: f64, inset: f64) -> Self {
        Self::over(
            work_width,
            work_height,
            inset,
            [0.0, 0.0, work_width, work_height],
        )
    }

    /// Centred over `window` (`[x, y, width, height]` relative to the work
    /// area, y downward) instead of the whole area, and kept inside the area.
    pub fn over(work_width: f64, work_height: f64, inset: f64, window: [f64; 4]) -> Self {
        let [left, top, window_width, window_height] = window;
        let width = (WIDTH + 2.0 * inset)
            .min(work_width - 2.0 * MARGIN)
            .max(1.0);
        let max_height = MAX_HEIGHT.min(work_height - 2.0 * MARGIN).max(1.0);
        // Keep the top edge fixed for every height the launcher can reach, so
        // it never moves upward on a short display.
        let y = (top + window_height * TOP)
            .min(work_height - max_height - MARGIN)
            .max(MARGIN.min(work_height - max_height).max(0.0));
        Self {
            x: (left + (window_width - width) / 2.0)
                .min(work_width - width)
                .max(0.0),
            y,
            width,
            max_height,
        }
    }

    pub fn height(&self, content: f64) -> f64 {
        content.clamp(RESTING_HEIGHT.min(self.max_height), self.max_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_centred_near_the_top_of_an_ordinary_display() {
        let placement = Placement::new(1440.0, 875.0, 0.0);
        assert_eq!(placement.width, WIDTH);
        assert_eq!(placement.x, (1440.0 - WIDTH) / 2.0);
        assert_eq!(placement.y, 875.0 * TOP);
        assert_eq!(placement.height(20.0), RESTING_HEIGHT);
        assert_eq!(placement.height(9000.0), MAX_HEIGHT);
    }

    #[test]
    fn the_tallest_launcher_still_fits_a_short_display() {
        let placement = Placement::new(1024.0, 600.0, SHAPE_INSET);
        assert!(placement.y >= MARGIN);
        assert!(placement.y + placement.height(9000.0) <= 600.0 - MARGIN);
    }

    #[test]
    fn opens_centred_over_a_window_and_stays_on_its_display() {
        let window = [700.0, 100.0, 600.0, 500.0];
        let placement = Placement::over(1440.0, 875.0, 0.0, window);
        assert_eq!(placement.x, 700.0 + (600.0 - WIDTH) / 2.0);
        assert_eq!(placement.y, 100.0 + 500.0 * TOP);
        // A window hanging off the display's right edge keeps the launcher on it.
        let placement = Placement::over(1440.0, 875.0, 0.0, [1200.0, 0.0, 800.0, 600.0]);
        assert_eq!(placement.x, 1440.0 - WIDTH);
    }

    #[test]
    fn a_tiny_display_bounds_width_and_height() {
        let placement = Placement::new(300.0, 120.0, SHAPE_INSET);
        assert!(placement.x >= 0.0 && placement.x + placement.width <= 300.0);
        assert!(placement.y >= 0.0 && placement.y + placement.height(9000.0) <= 120.0);
    }
}
