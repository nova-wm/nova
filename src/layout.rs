use smithay::{
    desktop::Window,
    reexports::wayland_server::{backend::ObjectId, Resource},
    utils::{Logical, Point, Rectangle},
};

/// Niri-style scroll layout.
///

/// Shared by layout and viewport positioning.
pub fn scroll_column_width(screen_rect: Rectangle<i32, Logical>, gaps: i32) -> i32 {
    let maximum = screen_rect.size.w.max(1);
    let minimum = 200.min(maximum);

    let target = ((screen_rect.size.w - gaps) / 2).max(minimum);
    target.min(maximum)
}

/// Per-window custom scroll column widths (from live resize). Always merged
/// into the layout here so a dragged column can grow beyond the default
type ScrollWidths = std::collections::HashMap<ObjectId, i32>;

/// Width in logical px of each column: full output width for the maximized
/// window, a persisted custom width for a window the user resized, otherwise
pub fn scroll_column_widths(
    screen_rect: Rectangle<i32, Logical>,
    gaps: i32,
    maximized: Option<&ObjectId>,
    windows: &[Window],
    custom: &ScrollWidths,
) -> Vec<i32> {
    windows
        .iter()
        .map(|w| {
            let id = w.toplevel().map(|t| t.wl_surface().id());
            let is_max = id.as_ref() == maximized;
            if is_max {
                screen_rect.size.w.max(1)
            } else if let Some(id) = id {
                if let Some(&width) = custom.get(&id) {
                    let max = screen_rect.size.w.max(1);
                    width.clamp(scroll_column_width(screen_rect, gaps).min(200), max)
                } else {
                    scroll_column_width(screen_rect, gaps)
                }
            } else {
                scroll_column_width(screen_rect, gaps)
            }
        })
        .collect()
}

/// Absolute left x (in layout space) of each column, laid out left to right
/// from the workspace origin, one gap apart.
fn scroll_column_abs(widths: &[i32], gaps: i32) -> Vec<i32> {
    let mut abs = Vec::with_capacity(widths.len());
    let mut cur = 0i32;
    for &w in widths {
        abs.push(cur);
        cur += w + gaps;
    }
    abs
}

/// Where the view should sit so column `active_idx` is on screen.
///
pub fn scroll_view_origin(
    screen_rect: Rectangle<i32, Logical>,
    gaps: i32,
    active_idx: usize,
    maximized: Option<&ObjectId>,
    windows: &[Window],
    custom: &ScrollWidths,
) -> Point<i32, Logical> {
    if windows.is_empty() {
        return screen_rect.loc;
    }

    let active_idx = active_idx.min(windows.len() - 1);

    let any_custom = custom.iter().any(|(id, _)| {
        windows
            .iter()
            .any(|w| w.toplevel().is_some_and(|t| t.wl_surface().id() == *id))
    });

    if maximized.is_none() && !any_custom {
        let width = scroll_column_width(screen_rect, gaps);
        let x = screen_rect.loc.x
            + (active_idx as i32 * (width + gaps))
                .min((screen_rect.size.w - width).max(0));
        return Point::from((x, screen_rect.loc.y));
    }

    let widths = scroll_column_widths(screen_rect, gaps, maximized, windows, custom);
    let w = widths[active_idx];
    let x = screen_rect.loc.x + (screen_rect.size.w - w) / 2;
    Point::from((x, screen_rect.loc.y))
}

pub fn layout_scroll(
    windows: &[Window],
    screen_rect: Rectangle<i32, Logical>,
    gaps: i32,
    active_idx: usize,
    view_origin: Point<i32, Logical>,
    maximized: Option<&ObjectId>,
    custom: &ScrollWidths,
) -> Vec<(Window, Rectangle<i32, Logical>)> {
    let mut layouts = Vec::new();
    if windows.is_empty() {
        return layouts;
    }

    let active_idx = active_idx.min(windows.len() - 1);

    let widths = scroll_column_widths(screen_rect, gaps, maximized, windows, custom);
    let abs = scroll_column_abs(&widths, gaps);
    let col_h = screen_rect.size.h;

    for (i, window) in windows.iter().enumerate() {
        let x = view_origin.x + abs[i] - abs[active_idx];
        let y = view_origin.y;
        let rect = Rectangle::new(Point::from((x, y)), (widths[i], col_h).into());
        layouts.push((window.clone(), rect));
    }

    layouts
}

/// Minimum height of a stacked row in logical px. A stack whose even split
/// would make rows shorter than this overflows the column instead, and the
pub const MIN_STACK_ROW_H: i32 = 300;

/// Split a full-height scroll column into vertical stack rows (niri-style).
///
pub fn layout_stack_rows(
    column: Rectangle<i32, Logical>,
    gaps: i32,
    rows: usize,
    focused_row: usize,
) -> Vec<Rectangle<i32, Logical>> {
    if rows == 0 {
        return Vec::new();
    }

    if rows == 1 {
        return vec![column];
    }

    let gaps = gaps.max(0);
    let focused_row = focused_row.min(rows - 1);

    let even_h = (column.size.h - gaps * (rows as i32 - 1)) / rows as i32;

    let (row_h, offset) = if even_h >= MIN_STACK_ROW_H {
        (even_h.max(1), 0)
    } else {
        let total = rows as i32 * (MIN_STACK_ROW_H + gaps) - gaps;
        let center =
            focused_row as i32 * (MIN_STACK_ROW_H + gaps) + MIN_STACK_ROW_H / 2;
        let max_offset = (total - column.size.h).max(0);
        (
            MIN_STACK_ROW_H,
            (center - column.size.h / 2).clamp(0, max_offset),
        )
    };

    (0..rows)
        .map(|i| {
            let y = column.loc.y + i as i32 * (row_h + gaps) - offset;
            Rectangle::new((column.loc.x, y).into(), (column.size.w, row_h).into())
        })
        .collect()
}

/// A persistent node in the mouse-location-based dwindle tree.
///
#[derive(Clone, Debug)]
pub enum DwindleNode {
    Leaf {
        window: ObjectId,
        rect: Rectangle<i32, Logical>,
    },
    Split {
        /// `true` => children are laid out left/right, `false` => top/bottom.
        horizontal: bool,
        /// Fraction of the split's inner (content) space given to the
        /// left/top child, in `(0, 1)`. Default 0.5; live resize scales these
        ratio: f32,
        rect: Rectangle<i32, Logical>,
        left_or_top: Box<DwindleNode>,
        right_or_bottom: Box<DwindleNode>,
    },
}

impl DwindleNode {
    /// Does this node (or its subtree) hold the given window?
    pub fn contains(&self, window: &ObjectId) -> bool {
        match self {
            DwindleNode::Leaf { window: w, .. } => w == window,
            DwindleNode::Split {
                left_or_top,
                right_or_bottom,
                ..
            } => left_or_top.contains(window) || right_or_bottom.contains(window),
        }
    }

    /// Iterate all leaves (window, rect) in tree order.
    pub fn leaves(&self) -> Vec<(ObjectId, Rectangle<i32, Logical>)> {
        let mut out = Vec::new();
        self.for_each_leaf(&mut out);
        out
    }

    fn for_each_leaf(&self, out: &mut Vec<(ObjectId, Rectangle<i32, Logical>)>) {
        match self {
            DwindleNode::Leaf { window, rect } => out.push((window.clone(), *rect)),
            DwindleNode::Split {
                left_or_top,
                right_or_bottom,
                ..
            } => {
                left_or_top.for_each_leaf(out);
                right_or_bottom.for_each_leaf(out);
            }
        }
    }

    /// Assign a new rect (leaf base or split base).
    pub fn set_rect(&mut self, rect: Rectangle<i32, Logical>) {
        match self {
            DwindleNode::Leaf { rect: r, .. } => *r = rect,
            DwindleNode::Split { rect: r, .. } => *r = rect,
        }
    }
}

/// Create the root dwindle leaf for the first window.
pub fn dwindle_new(window: ObjectId, rect: Rectangle<i32, Logical>) -> DwindleNode {
    DwindleNode::Leaf { window, rect }
}

/// Insert `new_window` into the tree, splitting the leaf under the mouse
/// pointer (or the closest one), following Hyprland's `dwindle` logic:
pub fn dwindle_insert(
    root: &mut DwindleNode,
    new_window: ObjectId,
    mouse: Point<i32, Logical>,
    gaps: i32,
    split_ratio: f32,
    split_direction: &str,
) {
    let target_idx = find_leaf_under_or_closest(root, mouse);
    insert_into(
        root,
        &target_idx,
        new_window,
        mouse,
        gaps,
        split_ratio,
        split_direction,
    );
}

/// Recursive descent: replace the found leaf with a new split rooted at that
/// leaf's position.
fn insert_into(
    node: &mut DwindleNode,
    target: &ObjectId,
    new_window: ObjectId,
    mouse: Point<i32, Logical>,
    gaps: i32,
    split_ratio: f32,
    split_direction: &str,
) {
    match node {
        DwindleNode::Leaf { window, rect } if window == target => {
            let replacement = build_split(
                window.clone(),
                rect.clone(),
                new_window,
                mouse,
                gaps,
                split_ratio,
                split_direction,
            );
            *node = replacement;
        }
        DwindleNode::Split {
            left_or_top,
            right_or_bottom,
            ..
        } => {
            if left_or_top.contains(target) {
                insert_into(
                    left_or_top,
                    target,
                    new_window,
                    mouse,
                    gaps,
                    split_ratio,
                    split_direction,
                );
            } else {
                insert_into(
                    right_or_bottom,
                    target,
                    new_window,
                    mouse,
                    gaps,
                    split_ratio,
                    split_direction,
                );
            }
        }
        _ => {}
    }
}

/// Build a split node replacing a leaf: old window + new window, oriented and
/// ordered by the mouse position relative to the leaf's box.
fn build_split(
    old_window: ObjectId,
    leaf_rect: Rectangle<i32, Logical>,
    new_window: ObjectId,
    mouse: Point<i32, Logical>,
    gaps: i32,
    split_ratio: f32,
    split_direction: &str,
) -> DwindleNode {
    let horizontal = match split_direction {
        "horizontal" => true,
        "vertical" => false,
        _ => leaf_rect.size.w >= leaf_rect.size.h,
    };
    let center_x = leaf_rect.loc.x + leaf_rect.size.w / 2;
    let center_y = leaf_rect.loc.y + leaf_rect.size.h / 2;

    let new_on_side = if horizontal {
        mouse.x < center_x
    } else {
        mouse.y < center_y
    };

    let ratio = split_ratio.clamp(0.05, 0.95);
    let (rect_old, rect_new) = if horizontal {
        let half_w_plus = ((leaf_rect.size.w - gaps) as f32 * ratio).round() as i32;
        let old = Rectangle::new(leaf_rect.loc, (half_w_plus, leaf_rect.size.h).into());
        let new = Rectangle::new(
            (leaf_rect.loc.x + half_w_plus + gaps, leaf_rect.loc.y).into(),
            (leaf_rect.size.w - half_w_plus - gaps, leaf_rect.size.h).into(),
        );
        (old, new)
    } else {
        let half_h_plus = ((leaf_rect.size.h - gaps) as f32 * ratio).round() as i32;
        let old = Rectangle::new(leaf_rect.loc, (leaf_rect.size.w, half_h_plus).into());
        let new = Rectangle::new(
            (leaf_rect.loc.x, leaf_rect.loc.y + half_h_plus + gaps).into(),
            (leaf_rect.size.w, leaf_rect.size.h - half_h_plus - gaps).into(),
        );
        (old, new)
    };

    let (left_or_top, right_or_bottom) =
        rect_pair(rect_old, rect_new, old_window, new_window, new_on_side);
    DwindleNode::Split {
        horizontal,
        ratio,
        rect: leaf_rect,
        left_or_top: Box::new(left_or_top),
        right_or_bottom: Box::new(right_or_bottom),
    }
}

/// Returns tile boxes for both halves, with the *new* window occupying the
/// mouse-side half.
fn rect_pair(
    rect_old: Rectangle<i32, Logical>,
    rect_new: Rectangle<i32, Logical>,
    old_window: ObjectId,
    new_window: ObjectId,
    new_on_side: bool,
) -> (DwindleNode, DwindleNode) {
    let old_leaf = DwindleNode::Leaf {
        window: old_window,
        rect: rect_old,
    };
    let new_leaf = DwindleNode::Leaf {
        window: new_window,
        rect: rect_new,
    };
    if new_on_side {
        (new_leaf, old_leaf)
    } else {
        (old_leaf, new_leaf)
    }
}

/// Recursively recompute every node's rect from its `rect`/`horizontal` flags.
pub fn dwindle_recompute(node: &mut DwindleNode, gaps: i32) {
    if let DwindleNode::Split {
        horizontal,
        ratio,
        rect,
        left_or_top,
        right_or_bottom,
    } = node
    {
        let (rect_a, rect_b) = split_rect_for(*rect, *horizontal, *ratio, gaps);
        set_leaf_rect(left_or_top, rect_a);
        set_leaf_rect(right_or_bottom, rect_b);
        dwindle_recompute(left_or_top, gaps);
        dwindle_recompute(right_or_bottom, gaps);
    }
}

/// Assign a rect to a leaf, or the base rect of a split (then recompute below).
fn set_leaf_rect(node: &mut DwindleNode, rect: Rectangle<i32, Logical>) {
    match node {
        DwindleNode::Leaf { rect: r, .. } => *r = rect,
        DwindleNode::Split { rect: r, .. } => *r = rect,
    }
}

/// Split a rect into two halves per orientation, respecting gaps. The
/// left/top child gets `ratio` of the content space (the remainder minus one
fn split_rect_for(
    rect: Rectangle<i32, Logical>,
    horizontal: bool,
    ratio: f32,
    gaps: i32,
) -> (Rectangle<i32, Logical>, Rectangle<i32, Logical>) {
    let ratio = ratio.clamp(0.05, 0.95) as f64;
    if horizontal {
        let content = (rect.size.w - gaps).max(0);
        let a_len = (content as f64 * ratio).round() as i32;
        let b_len = content - a_len;
        let a = Rectangle::new(rect.loc, (a_len, rect.size.h).into());
        let b = Rectangle::new(
            (rect.loc.x + a_len + gaps, rect.loc.y).into(),
            (b_len, rect.size.h).into(),
        );
        (a, b)
    } else {
        let content = (rect.size.h - gaps).max(0);
        let a_len = (content as f64 * ratio).round() as i32;
        let b_len = content - a_len;
        let a = Rectangle::new(rect.loc, (rect.size.w, a_len).into());
        let b = Rectangle::new(
            (rect.loc.x, rect.loc.y + a_len + gaps).into(),
            (rect.size.w, b_len).into(),
        );
        (a, b)
    }
}

/// Remove a window from the tree, collapsing the split that held it. Returns
/// `true` if the tree became empty.
pub fn dwindle_remove(root: &mut Option<DwindleNode>, window: &ObjectId, gaps: i32) -> bool {
    let Some(node) = root.take() else { return true };
    match remove_from(node, window, gaps) {
        None => true,
        Some(n) => {
            *root = Some(n);
            false
        }
    }
}

/// Recursively remove `window`, returning `None` when the subtree it was in
/// became empty. The surviving sibling is promoted into the removed parent's
fn remove_from(node: DwindleNode, window: &ObjectId, gaps: i32) -> Option<DwindleNode> {
    match node {
        DwindleNode::Leaf { window: w, .. } if &w == window => None,
        DwindleNode::Leaf { .. } => Some(node),
        DwindleNode::Split {
            horizontal,
            ratio,
            rect,
            left_or_top,
            right_or_bottom,
        } => {
            let left = *left_or_top;
            let right = *right_or_bottom;
            let (new_left, new_right) = if left.contains(window) {
                (remove_from(left, window, gaps), Some(right))
            } else if right.contains(window) {
                (Some(left), remove_from(right, window, gaps))
            } else {
                (Some(left), Some(right))
            };
            let mut survivor = match (new_left, new_right) {
                (Some(a), Some(b)) => Some(DwindleNode::Split {
                    horizontal,
                    ratio,
                    rect,
                    left_or_top: Box::new(a),
                    right_or_bottom: Box::new(b),
                }),
                (Some(a), None) | (None, Some(a)) => {
                    let mut a = a;
                    a.set_rect(rect);
                    Some(a)
                }
                (None, None) => None,
            };
            if let Some(n) = &mut survivor {
                dwindle_recompute(n, gaps);
            }
            survivor
        }
    }
}

/// Find the leaf id whose rect contains `mouse`, or the closest leaf.
fn find_leaf_under_or_closest(root: &DwindleNode, mouse: Point<i32, Logical>) -> ObjectId {
    let mut best: Option<(ObjectId, i64)> = None;
    for (id, rect) in root.leaves() {
        if rect.contains(mouse) {
            return id;
        }
        let dist =
            (rect.loc.x as i64 - mouse.x as i64).abs() + (rect.loc.y as i64 - mouse.y as i64).abs();
        if best.as_ref().map_or(true, |(_, b)| dist < *b) {
            best = Some((id, dist));
        }
    }
    best.map(|(id, _)| id)
        .unwrap_or_else(|| root.leaves().first().map(|(id, _)| id.clone()).unwrap())
}

/// Grow/shrink the tile holding `id` toward `target` by scaling the split
/// ratios along the window's path to the root.
pub fn dwindle_resize_to(root: &mut DwindleNode, id: &ObjectId, target: Rectangle<i32, Logical>) {
    let Some(cur) = root
        .leaves()
        .iter()
        .find(|(wid, _)| wid == id)
        .map(|(_, rect)| *rect)
    else {
        return;
    };

    let fx = target.size.w as f64 / cur.size.w.max(1) as f64;
    let fy = target.size.h as f64 / cur.size.h.max(1) as f64;

    let mut nx = 0usize;
    let mut ny = 0usize;
    path_split_counts(root, id, &mut nx, &mut ny);

    let sx = if nx > 0 { fx.powf(1.0 / nx as f64) } else { 1.0 };
    let sy = if ny > 0 { fy.powf(1.0 / ny as f64) } else { 1.0 };

    scale_path_ratios(root, id, sx, sy);
}

/// Count how many horizontal (`nx`) and vertical (`ny`) splits sit between
/// `root` and the leaf `id`.
fn path_split_counts(node: &DwindleNode, id: &ObjectId, nx: &mut usize, ny: &mut usize) -> bool {
    match node {
        DwindleNode::Leaf { window, .. } => window == id,
        DwindleNode::Split {
            horizontal,
            left_or_top,
            right_or_bottom,
            ..
        } => {
            let child = if left_or_top.contains(id) {
                left_or_top
            } else if right_or_bottom.contains(id) {
                right_or_bottom
            } else {
                return false;
            };
            if !path_split_counts(child, id, nx, ny) {
                return false;
            }
            if *horizontal {
                *nx += 1;
            } else {
                *ny += 1;
            }
            true
        }
    }
}

/// Scale the path splits' ratios by `sx` (horizontal) / `sy` (vertical). For a
/// split whose path child is the left/top side, its ratio grows with the
fn scale_path_ratios(node: &mut DwindleNode, id: &ObjectId, sx: f64, sy: f64) -> bool {
    match node {
        DwindleNode::Leaf { window, .. } => window == id,
        DwindleNode::Split {
            horizontal,
            ratio,
            left_or_top,
            right_or_bottom,
            ..
        } => {
            let target_is_left = left_or_top.contains(id);
            let target_is_right = right_or_bottom.contains(id);
            if !(target_is_left || target_is_right) {
                return false;
            }
            let factor = if *horizontal { sx } else { sy };
            let child = if target_is_left {
                left_or_top.as_mut()
            } else {
                right_or_bottom.as_mut()
            };
            if !scale_path_ratios(child, id, sx, sy) {
                return false;
            }
            *ratio = if target_is_left {
                (*ratio as f64 * factor).clamp(0.05, 0.95)
            } else {
                (1.0 - (1.0 - *ratio as f64) * factor).clamp(0.05, 0.95)
            } as f32;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (960, h).into())
    }

    #[test]
    fn stack_single_row_keeps_column() {
        let col = column(1000);
        assert_eq!(layout_stack_rows(col, 8, 1, 0), vec![col]);
    }

    #[test]
    fn stack_even_split_fits() {
        let rows = layout_stack_rows(column(1000), 8, 2, 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], Rectangle::new((0, 0).into(), (960, 496).into()));
        assert_eq!(rows[1], Rectangle::new((0, 504).into(), (960, 496).into()));
    }

    #[test]
    fn stack_overflow_centers_focused() {
        let rows = layout_stack_rows(column(1000), 8, 5, 2);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.size.h == MIN_STACK_ROW_H));
        assert_eq!(rows[2].loc.y, 350);
        assert!(rows[0].loc.y < 0);
        assert!(rows[4].loc.y + rows[4].size.h > 1000);
    }

    #[test]
    fn stack_overflow_clamps_at_ends() {
        let top = layout_stack_rows(column(1000), 8, 5, 0);
        assert_eq!(top[0].loc.y, 0);
        let bottom = layout_stack_rows(column(1000), 8, 5, 4);
        assert_eq!(bottom[4].loc.y + bottom[4].size.h, 1000);
    }
}
