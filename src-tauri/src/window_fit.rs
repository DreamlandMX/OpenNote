//! Keeps the startup window inside the monitor's work area. The configured
//! size is fixed, so on a smaller screen the bottom of the window would
//! otherwise sit behind the taskbar.

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Shrinks `window` to fit `work`, then moves it just far enough to be inside.
fn fit_within(window: Rect, work: Rect) -> Rect {
    let width = window.width.min(work.width);
    let height = window.height.min(work.height);
    let max_x = work.x + (work.width - width) as i32;
    let max_y = work.y + (work.height - height) as i32;
    Rect {
        x: window.x.clamp(work.x, max_x),
        y: window.y.clamp(work.y, max_y),
        width,
        height,
    }
}

/// Best effort: if the window or monitor cannot be read, the window keeps its
/// configured placement rather than blocking startup.
pub(crate) fn fit_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let (Ok(Some(monitor)), Ok(position), Ok(outer), Ok(inner)) = (
        window.current_monitor(),
        window.outer_position(),
        window.outer_size(),
        window.inner_size(),
    ) else {
        return;
    };
    let area = monitor.work_area();
    let current = Rect {
        x: position.x,
        y: position.y,
        width: outer.width,
        height: outer.height,
    };
    let fitted = fit_within(
        current,
        Rect {
            x: area.position.x,
            y: area.position.y,
            width: area.size.width,
            height: area.size.height,
        },
    );
    if fitted == current {
        return;
    }
    // `set_size` takes the inner size, so remove what the frame adds.
    let _ = window.set_size(PhysicalSize::new(
        inner.width - (outer.width - fitted.width),
        inner.height - (outer.height - fitted.height),
    ));
    let _ = window.set_position(PhysicalPosition::new(fitted.x, fitted.y));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_taller_than_the_work_area_is_shrunk_and_moved_inside() {
        // 2560x1600 at 150%: a 1024-logical-pixel content area plus its frame
        // is taller than the 1529 pixels left above the taskbar.
        let work = rect(0, 0, 2560, 1529);
        let fitted = fit_within(rect(150, 10, 2176, 1583), work);
        assert_eq!(fitted, rect(150, 0, 2176, 1529));
    }

    #[test]
    fn a_window_that_already_fits_is_left_alone() {
        let work = rect(0, 0, 2560, 1529);
        let window = rect(100, 100, 1200, 800);
        assert_eq!(fit_within(window, work), window);
    }

    #[test]
    fn a_window_running_off_the_bottom_right_moves_back_without_shrinking() {
        let work = rect(0, 0, 1920, 1040);
        let fitted = fit_within(rect(1000, 500, 1200, 800), work);
        assert_eq!(fitted, rect(720, 240, 1200, 800));
    }

    #[test]
    fn a_secondary_monitor_with_negative_coordinates_is_respected() {
        let work = rect(-1920, -200, 1920, 1040);
        let fitted = fit_within(rect(-2000, -300, 1200, 2000), work);
        assert_eq!(fitted, rect(-1920, -200, 1200, 1040));
    }
}
