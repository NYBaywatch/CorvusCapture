//! Shift+F9 region-selection overlay (REG-01..06).
//!
//! This module owns the pure, HWND-free selection-geometry core: types and
//! functions here do no windowing, no GDI, and no `unsafe` -- they are
//! proven correct by unit tests before any window exists. The window,
//! paint routine, and input handling that consume this contract arrive in
//! Phase 3 Plans 02-04.

// Every type/fn below is currently exercised only by this module's own
// unit tests -- no window/paint/input code calls them yet. Plans 02-04
// wire real call sites (window creation, wnd_proc, paint) and remove the
// need for this allowance.
#![allow(dead_code)]

use windows::Win32::Foundation::{POINT, RECT};

use crate::constants;

// ---------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------

/// One of the 8 resize handles arranged around a selection's corners and
/// edge midpoints (REG-03).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Handle {
    NW,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
}

/// Result of hit-testing a point against a selection (REG-03). Handles are
/// tested before the interior, so a handle near the border always wins.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Handle(Handle),
}

/// Overlay interaction state (RESEARCH Pattern 3). Drives both drag
/// semantics and the `WM_SETCURSOR` cursor table in later plans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayState {
    /// No selection exists yet; crosshair cursor.
    Idle,
    /// Rubber-banding a brand-new selection from `anchor`.
    DraggingNew { anchor: POINT },
    /// A selection exists; handles + readout are visible.
    Selected,
    /// Dragging the selection interior to a new position.
    Moving { grab_offset: POINT },
    /// Dragging one resize handle; the opposite corner/edge is the anchor.
    Resizing { handle: Handle, anchor: POINT },
}

// ---------------------------------------------------------------------
// Pure geometry helpers
// ---------------------------------------------------------------------

/// Normalizes two arbitrary drag endpoints into a valid (non-negative
/// width/height) RECT. REG-03: the user can drag in any of the four
/// directions, so the anchor is not necessarily the top-left corner.
pub fn normalize_rect(a: POINT, b: POINT) -> RECT {
    RECT {
        left: a.x.min(b.x),
        top: a.y.min(b.y),
        right: a.x.max(b.x),
        bottom: a.y.max(b.y),
    }
}

/// Returns the 8 resize-handle rects for `sel`, each `size` pixels square,
/// centered on the 4 corners and 4 edge midpoints (D-25).
pub fn handle_rects(sel: RECT, size: i32) -> [(Handle, RECT); 8] {
    let half = size / 2;
    let cx = (sel.left + sel.right) / 2;
    let cy = (sel.top + sel.bottom) / 2;

    let square = |cx: i32, cy: i32| RECT {
        left: cx - half,
        top: cy - half,
        right: cx - half + size,
        bottom: cy - half + size,
    };

    [
        (Handle::NW, square(sel.left, sel.top)),
        (Handle::N, square(cx, sel.top)),
        (Handle::NE, square(sel.right, sel.top)),
        (Handle::E, square(sel.right, cy)),
        (Handle::SE, square(sel.right, sel.bottom)),
        (Handle::S, square(cx, sel.bottom)),
        (Handle::SW, square(sel.left, sel.bottom)),
        (Handle::W, square(sel.left, cy)),
    ]
}

fn point_in_rect(p: POINT, r: RECT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// Hit-tests `p` against `sel` (REG-03): handles first (inflated to
/// `hit_size`, not the smaller visual size), then the interior, then
/// `Outside`. Handle-first ordering lets a handle near the border win over
/// the interior.
pub fn hit_test(sel: RECT, p: POINT, hit_size: i32) -> Hit {
    for (handle, rect) in handle_rects(sel, hit_size) {
        if point_in_rect(p, rect) {
            return Hit::Handle(handle);
        }
    }
    if point_in_rect(p, sel) {
        return Hit::Inside;
    }
    Hit::Outside
}

/// Resizes `original` by dragging `handle` from `anchor` (the fixed
/// opposite corner/edge) to `floating` (the current mouse point). Edge
/// handles (N/S/E/W) only move their perpendicular axis; the other axis
/// stays at `original`'s bounds. Always returns a normalized rect, so
/// dragging a corner handle past its anchor re-normalizes cleanly.
pub fn resize_rect(anchor: POINT, floating: POINT, handle: Handle, original: RECT) -> RECT {
    let (p1, p2) = match handle {
        Handle::NW | Handle::NE | Handle::SE | Handle::SW => (anchor, floating),
        Handle::N | Handle::S => (
            POINT {
                x: original.left,
                y: anchor.y,
            },
            POINT {
                x: original.right,
                y: floating.y,
            },
        ),
        Handle::E | Handle::W => (
            POINT {
                x: anchor.x,
                y: original.top,
            },
            POINT {
                x: floating.x,
                y: original.bottom,
            },
        ),
    };
    normalize_rect(p1, p2)
}

/// Pushes `sel` fully inside `bounds`, preserving width/height when they
/// already fit. Never returns a rect smaller than 1x1 (a degenerate
/// `bounds` still yields a valid, if minimal, rect).
pub fn clamp_rect(sel: RECT, bounds: RECT) -> RECT {
    let bw = (bounds.right - bounds.left).max(1);
    let bh = (bounds.bottom - bounds.top).max(1);
    let w = (sel.right - sel.left).max(1).min(bw);
    let h = (sel.bottom - sel.top).max(1).min(bh);

    let mut left = sel.left;
    let mut top = sel.top;
    if left < bounds.left {
        left = bounds.left;
    }
    if left + w > bounds.right {
        left = bounds.right - w;
    }
    if top < bounds.top {
        top = bounds.top;
    }
    if top + h > bounds.bottom {
        top = bounds.bottom - h;
    }

    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Moves `sel` by `(dx, dy)` (D-30 arrow keys), stopping at the edge of
/// `bounds` instead of walking the selection off-screen.
pub fn nudge_rect(sel: RECT, dx: i32, dy: i32, bounds: RECT) -> RECT {
    let w = sel.right - sel.left;
    let h = sel.bottom - sel.top;
    let max_left = (bounds.right - w).max(bounds.left);
    let max_top = (bounds.bottom - h).max(bounds.top);
    let left = (sel.left + dx).clamp(bounds.left, max_left);
    let top = (sel.top + dy).clamp(bounds.top, max_top);
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Grows/shrinks `sel` by moving only the bottom-right edge (D-30
/// Shift+arrows), floored at 1x1 (D-31 -- no minimum-size gate above 1) and
/// clamped so the edge never leaves `bounds`.
pub fn grow_rect(sel: RECT, dx: i32, dy: i32, bounds: RECT) -> RECT {
    let right = (sel.right + dx).clamp(sel.left + 1, bounds.right.max(sel.left + 1));
    let bottom = (sel.bottom + dy).clamp(sel.top + 1, bounds.bottom.max(sel.top + 1));
    RECT {
        left: sel.left,
        top: sel.top,
        right,
        bottom,
    }
}

/// Places the dimension-readout chip (D-27/D-28): `OVERLAY_READOUT_GAP`
/// pixels outside the selection's bottom-right corner by default; flips to
/// hug the inside of that corner when the default placement would cross
/// `bounds`' right/bottom edge within `OVERLAY_READOUT_EDGE_THRESHOLD`.
pub fn readout_rect(sel: RECT, chip_w: i32, chip_h: i32, bounds: RECT) -> RECT {
    let gap = constants::OVERLAY_READOUT_GAP;
    let threshold = constants::OVERLAY_READOUT_EDGE_THRESHOLD;

    let mut left = sel.right + gap;
    let mut top = sel.bottom + gap;

    if left + chip_w + threshold > bounds.right || top + chip_h + threshold > bounds.bottom {
        left = sel.right - gap - chip_w;
        top = sel.bottom - gap - chip_h;
    }

    RECT {
        left,
        top,
        right: left + chip_w,
        bottom: top + chip_h,
    }
}

/// REG-06: distinguishes a click from a drag by total movement against the
/// caller-supplied system drag thresholds (`SM_CXDRAG`/`SM_CYDRAG` --
/// never hardcoded here, per UI-SPEC).
pub fn is_click(start: POINT, end: POINT, cx_drag: i32, cy_drag: i32) -> bool {
    (end.x - start.x).abs() <= cx_drag && (end.y - start.y).abs() <= cy_drag
}

/// Returns the union of the old and new selection + readout-chip rects,
/// inflated by `pad` (handle radius + border), for `InvalidateRect`
/// (RESEARCH Pattern 6 -- never full-screen invalidate per mouse move).
pub fn dirty_union(old_sel: RECT, old_chip: RECT, new_sel: RECT, new_chip: RECT, pad: i32) -> RECT {
    let left = [old_sel.left, old_chip.left, new_sel.left, new_chip.left]
        .into_iter()
        .min()
        .unwrap()
        - pad;
    let top = [old_sel.top, old_chip.top, new_sel.top, new_chip.top]
        .into_iter()
        .min()
        .unwrap()
        - pad;
    let right = [old_sel.right, old_chip.right, new_sel.right, new_chip.right]
        .into_iter()
        .max()
        .unwrap()
        + pad;
    let bottom = [
        old_sel.bottom,
        old_chip.bottom,
        new_sel.bottom,
        new_chip.bottom,
    ]
    .into_iter()
    .max()
    .unwrap()
        + pad;

    RECT {
        left,
        top,
        right,
        bottom,
    }
}

/// Formats the live dimension readout (D-29 / UI-SPEC copywriting
/// contract): `{w} × {h}` using U+00D7, single space each side, no "px"
/// suffix.
pub fn readout_text(sel: RECT) -> String {
    let w = sel.right - sel.left;
    let h = sel.bottom - sel.top;
    format!("{w} \u{00D7} {h}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    fn pt(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    #[test]
    fn normalize_rect_handles_reverse_drag() {
        let r = normalize_rect(pt(100, 100), pt(40, 30));
        assert_eq!(r, rect(40, 30, 100, 100));
    }

    #[test]
    fn normalize_rect_handles_forward_drag() {
        let r = normalize_rect(pt(10, 20), pt(50, 60));
        assert_eq!(r, rect(10, 20, 50, 60));
    }

    #[test]
    fn handle_rects_returns_eight_centered_squares() {
        let sel = rect(100, 100, 200, 200);
        let handles = handle_rects(sel, 8);
        assert_eq!(handles.len(), 8);
        for (_, r) in handles {
            assert_eq!(r.right - r.left, 8);
            assert_eq!(r.bottom - r.top, 8);
        }
        // SE handle centered on the selection's bottom-right corner.
        let (_, se) = handles.iter().find(|(h, _)| *h == Handle::SE).unwrap();
        assert_eq!(*se, rect(196, 196, 204, 204));
    }

    #[test]
    fn hit_test_prefers_handle_over_interior() {
        let sel = rect(100, 100, 200, 200);
        // Point inside the inflated SE handle hit box, but also inside the
        // selection interior -- handle must win.
        let hit = hit_test(sel, pt(198, 198), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Handle(Handle::SE));
    }

    #[test]
    fn hit_test_returns_inside_away_from_handles() {
        let sel = rect(100, 100, 200, 200);
        let hit = hit_test(sel, pt(150, 150), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Inside);
    }

    #[test]
    fn hit_test_returns_outside() {
        let sel = rect(100, 100, 200, 200);
        let hit = hit_test(sel, pt(0, 0), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Outside);
    }

    #[test]
    fn clamp_rect_preserves_size_when_it_fits() {
        let sel = rect(10, 10, 50, 40);
        let bounds = rect(0, 0, 1000, 1000);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out, sel);
    }

    #[test]
    fn clamp_rect_pushes_fully_inside_bounds() {
        let sel = rect(-10, -10, 30, 30);
        let bounds = rect(0, 0, 100, 100);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out.left, 0);
        assert_eq!(out.top, 0);
        assert_eq!(out.right - out.left, 40);
        assert_eq!(out.bottom - out.top, 40);
    }

    #[test]
    fn clamp_rect_never_smaller_than_1x1() {
        let sel = rect(0, 0, 10, 10);
        let bounds = rect(0, 0, 0, 0);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out.right - out.left, 1);
        assert_eq!(out.bottom - out.top, 1);
    }

    #[test]
    fn nudge_rect_moves_and_stops_at_bounds() {
        let sel = rect(0, 0, 10, 10);
        let bounds = rect(0, 0, 100, 100);
        let moved = nudge_rect(sel, 5, 5, bounds);
        assert_eq!(moved, rect(5, 5, 15, 15));

        // Walking far past the right/bottom edge stops flush with bounds.
        let stopped = nudge_rect(sel, 1000, 1000, bounds);
        assert_eq!(stopped, rect(90, 90, 100, 100));
    }

    #[test]
    fn grow_rect_moves_only_bottom_right_edge() {
        let sel = rect(10, 10, 20, 20);
        let bounds = rect(0, 0, 100, 100);
        let grown = grow_rect(sel, 5, 3, bounds);
        assert_eq!(grown, rect(10, 10, 25, 23));
    }

    #[test]
    fn grow_rect_floors_at_1x1() {
        let sel = rect(10, 10, 20, 20);
        let bounds = rect(0, 0, 100, 100);
        let shrunk = grow_rect(sel, -1000, -1000, bounds);
        assert_eq!(shrunk.right - shrunk.left, 1);
        assert_eq!(shrunk.bottom - shrunk.top, 1);
    }

    #[test]
    fn resize_rect_corner_handle_renormalizes_past_anchor() {
        // SE handle anchored at the NW corner (100, 100); dragging past
        // the anchor (to the upper-left of it) must still yield a valid,
        // non-negative rect.
        let original = rect(100, 100, 200, 200);
        let anchor = pt(100, 100);
        let floating = pt(50, 60);
        let out = resize_rect(anchor, floating, Handle::SE, original);
        assert_eq!(out, rect(50, 60, 100, 100));
    }

    #[test]
    fn resize_rect_edge_handle_only_moves_perpendicular_axis() {
        let original = rect(100, 100, 200, 200);
        // E handle: anchor is the left edge; only the right edge (x) moves.
        let anchor = pt(100, 100);
        let floating = pt(250, 9999);
        let out = resize_rect(anchor, floating, Handle::E, original);
        assert_eq!(out, rect(100, 100, 250, 200));
    }

    #[test]
    fn readout_rect_places_outside_bottom_right_by_default() {
        let sel = rect(100, 100, 200, 200);
        let bounds = rect(0, 0, 1000, 1000);
        let chip = readout_rect(sel, 60, 24, bounds);
        assert_eq!(chip.left, 200 + constants::OVERLAY_READOUT_GAP);
        assert_eq!(chip.top, 200 + constants::OVERLAY_READOUT_GAP);
    }

    #[test]
    fn readout_rect_flips_inside_near_edge() {
        // Selection's bottom-right corner is close to the bounds edge, so
        // the default outside placement would clip -- must flip inside.
        let sel = rect(900, 900, 990, 990);
        let bounds = rect(0, 0, 1000, 1000);
        let chip = readout_rect(sel, 60, 24, bounds);
        assert!(chip.right <= sel.right);
        assert!(chip.bottom <= sel.bottom);
    }

    #[test]
    fn is_click_within_threshold() {
        assert!(is_click(pt(100, 100), pt(102, 101), 4, 4));
        assert!(!is_click(pt(100, 100), pt(110, 100), 4, 4));
    }

    #[test]
    fn dirty_union_contains_old_and_new_rects() {
        let old_sel = rect(0, 0, 10, 10);
        let old_chip = rect(12, 12, 30, 20);
        let new_sel = rect(5, 5, 20, 20);
        let new_chip = rect(22, 22, 40, 30);
        let union = dirty_union(old_sel, old_chip, new_sel, new_chip, 2);
        assert_eq!(union, rect(-2, -2, 42, 32));
    }

    #[test]
    fn readout_text_formats_multiplication_sign() {
        let sel = rect(0, 0, 1280, 720);
        let text = readout_text(sel);
        assert_eq!(text, "1280 \u{00D7} 720");
        assert!(text.contains('\u{00D7}'));
        assert!(!text.contains('x'));
    }

    #[test]
    fn overlay_state_variants_are_distinguishable() {
        assert_ne!(OverlayState::Idle, OverlayState::Selected);
        let a = OverlayState::DraggingNew { anchor: pt(0, 0) };
        let b = OverlayState::DraggingNew { anchor: pt(0, 0) };
        assert_eq!(a, b);
        let moving = OverlayState::Moving {
            grab_offset: pt(1, 1),
        };
        let resizing = OverlayState::Resizing {
            handle: Handle::NW,
            anchor: pt(0, 0),
        };
        assert_ne!(moving, resizing);
    }
}
