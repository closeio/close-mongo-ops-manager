//! Screen geometry: the main layout, centered boxes and table columns.

use ratatui::layout::Rect;

/// Height of the filter bar: its border around 3-line inputs.
pub const FILTER_BAR_HEIGHT: u16 = 5;

/// Areas of the main screen, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainAreas {
    pub header: Rect,
    /// Zero-sized when the filter bar is hidden.
    pub filter_bar: Rect,
    pub table: Rect,
    pub status: Rect,
    pub footer: Rect,
    /// Where toasts may be stacked: between the header and the status bar,
    /// above the table's bottom border.
    pub toasts: Rect,
}

impl MainAreas {
    /// Splits the screen. On short terminals the header, footer and status
    /// bar keep their line first, then the filter bar, then the table.
    pub fn new(area: Rect, filter_bar_visible: bool) -> Self {
        let mut rest = area;
        let header = take_top(&mut rest, 1);
        let footer = take_bottom(&mut rest, 1);
        let status = take_bottom(&mut rest, 1);
        let toasts = Rect {
            height: rest.height.saturating_sub(1),
            ..rest
        };
        let filter_bar = if filter_bar_visible {
            take_top(&mut rest, FILTER_BAR_HEIGHT)
        } else {
            Rect::default()
        };
        Self {
            header,
            filter_bar,
            table: rest,
            status,
            footer,
            toasts,
        }
    }
}

/// Takes up to `height` lines from the top of `rest`.
fn take_top(rest: &mut Rect, height: u16) -> Rect {
    let height = height.min(rest.height);
    let taken = Rect { height, ..*rest };
    rest.y += height;
    rest.height -= height;
    taken
}

/// Takes up to `height` lines from the bottom of `rest`.
fn take_bottom(rest: &mut Rect, height: u16) -> Rect {
    let height = height.min(rest.height);
    rest.height -= height;
    Rect {
        y: rest.y + rest.height,
        height,
        ..*rest
    }
}

/// `percent`% of `value`, rounded down.
pub fn percent(value: u16, percent: u16) -> u16 {
    u16::try_from(u32::from(value) * u32::from(percent) / 100).unwrap_or(value)
}

/// A `width` x `height` box centered in `area`, shrunk to fit it.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The column of `area`'s right edge, between `top` and `bottom` rows of
/// `rows`: where a scrollbar is drawn over a block's right border.
pub fn right_edge(area: Rect, rows: Rect) -> Rect {
    if area.is_empty() {
        return Rect::default();
    }
    Rect {
        x: area.right() - 1,
        y: rows.y,
        width: 1,
        height: rows.height,
    }
    .intersection(area)
}

/// How a table column is sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSpec {
    /// Width of the content (header and cells), capped.
    pub natural: usize,
    /// Width of the header: columns are cut below it only when cutting the
    /// cells of every column is not enough.
    pub header: usize,
    /// Narrowest width before the column is hidden instead.
    pub min: usize,
    /// Columns shrink in increasing order; `None` never shrinks.
    pub shrink_rank: Option<u8>,
    /// Columns are hidden in increasing order; `None` is never hidden (unless
    /// nothing else fits).
    pub hide_rank: Option<u8>,
}

/// Fits columns separated by 1-cell gaps into `available` cells.
///
/// Columns get their natural width when everything fits, the last visible
/// column taking the leftover space. Otherwise columns are hidden (`None`) by
/// rank until the rest fits at their minimum widths, then shrink by rank
/// (down to their header first, then to their minimum); as a last resort the
/// rightmost columns are cut.
pub fn fit_columns(specs: &[ColumnSpec], available: usize) -> Vec<Option<usize>> {
    let narrowest = |s: &ColumnSpec| match s.shrink_rank {
        Some(_) => s.min.min(s.natural),
        None => s.natural,
    };
    let mut widths: Vec<Option<usize>> = specs.iter().map(|s| Some(narrowest(s))).collect();
    for i in by_rank(specs, |s| s.hide_rank) {
        if total_width(&widths) <= available {
            break;
        }
        widths[i] = None;
    }

    for (w, spec) in widths.iter_mut().zip(specs) {
        if let Some(w) = w {
            *w = spec.natural;
        }
    }
    let shrink_order = by_rank(specs, |s| s.shrink_rank);
    let floors: [fn(&ColumnSpec) -> usize; 2] = [|s| s.header.max(s.min), |s| s.min];
    for floor in floors {
        for &i in &shrink_order {
            let excess = total_width(&widths).saturating_sub(available);
            if let Some(w) = widths[i].as_mut() {
                *w -= excess.min(w.saturating_sub(floor(&specs[i])));
            }
        }
    }
    while total_width(&widths) > available {
        let Some(last) = widths.iter().rposition(Option::is_some) else {
            break;
        };
        let excess = total_width(&widths) - available;
        widths[last] = widths[last].filter(|&w| w > excess).map(|w| w - excess);
    }

    let spare = available.saturating_sub(total_width(&widths));
    if let Some(w) = widths.iter_mut().rev().find_map(Option::as_mut) {
        *w += spare;
    }
    widths
}

/// Indexes of the columns with a rank, in rank order.
fn by_rank(specs: &[ColumnSpec], rank: impl Fn(&ColumnSpec) -> Option<u8>) -> Vec<usize> {
    let mut ranked: Vec<(u8, usize)> = specs
        .iter()
        .enumerate()
        .filter_map(|(i, s)| rank(s).map(|r| (r, i)))
        .collect();
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// Width of the visible columns and the gaps between them.
fn total_width(widths: &[Option<usize>]) -> usize {
    let visible = widths.iter().flatten().count();
    widths.iter().flatten().sum::<usize>() + visible.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_areas_without_filter_bar() {
        let a = MainAreas::new(Rect::new(0, 0, 80, 24), false);
        assert_eq!(a.header, Rect::new(0, 0, 80, 1));
        assert_eq!(a.filter_bar, Rect::default());
        assert_eq!(a.table, Rect::new(0, 1, 80, 21));
        assert_eq!(a.status, Rect::new(0, 22, 80, 1));
        assert_eq!(a.footer, Rect::new(0, 23, 80, 1));
        assert_eq!(a.toasts, Rect::new(0, 1, 80, 20));
    }

    #[test]
    fn main_areas_with_filter_bar() {
        let a = MainAreas::new(Rect::new(0, 0, 80, 24), true);
        assert_eq!(a.filter_bar, Rect::new(0, 1, 80, 5));
        assert_eq!(a.table, Rect::new(0, 6, 80, 16));
    }

    #[test]
    fn main_areas_on_tiny_screens() {
        let a = MainAreas::new(Rect::new(0, 0, 10, 2), true);
        assert_eq!(a.header.height, 1);
        assert_eq!(a.footer, Rect::new(0, 1, 10, 1));
        assert_eq!(a.status.height, 0);
        assert_eq!(a.filter_bar.height, 0);
        assert_eq!(a.table.height, 0);
        let a = MainAreas::new(Rect::default(), true);
        assert_eq!(a.table, Rect::default());
    }

    #[test]
    fn centered_box_is_clamped() {
        let area = Rect::new(10, 5, 100, 40);
        assert_eq!(centered(area, 50, 10), Rect::new(35, 20, 50, 10));
        assert_eq!(centered(area, 500, 100), area);
        assert_eq!(percent(80, 80), 64);
        assert_eq!(percent(u16::MAX, 100), u16::MAX);
    }

    #[test]
    fn right_edge_of_area() {
        let area = Rect::new(2, 2, 10, 10);
        let rows = Rect::new(3, 4, 8, 5);
        assert_eq!(right_edge(area, rows), Rect::new(11, 4, 1, 5));
        assert_eq!(right_edge(Rect::default(), rows), Rect::default());
    }

    fn spec(natural: usize, min: usize, shrink: Option<u8>, hide: Option<u8>) -> ColumnSpec {
        ColumnSpec {
            natural,
            header: 0,
            min,
            shrink_rank: shrink,
            hide_rank: hide,
        }
    }

    #[test]
    fn columns_get_leftover_space_in_the_last_column() {
        let specs = [spec(5, 5, None, None), spec(10, 3, Some(0), Some(0))];
        assert_eq!(fit_columns(&specs, 30), [Some(5), Some(24)]);
        assert_eq!(fit_columns(&specs, 16), [Some(5), Some(10)]);
    }

    #[test]
    fn columns_shrink_then_hide_by_rank() {
        let specs = [
            spec(6, 6, None, Some(1)),
            spec(10, 4, Some(1), None),
            spec(20, 5, Some(0), Some(0)),
        ];
        // 6 + 10 + 20 + 2 = 38: shrink the last column first.
        assert_eq!(fit_columns(&specs, 30), [Some(6), Some(10), Some(12)]);
        // Then the middle one.
        assert_eq!(fit_columns(&specs, 20), [Some(6), Some(7), Some(5)]);
        // Hide by rank when the minimum widths do not fit; the others get
        // their width back.
        assert_eq!(fit_columns(&specs, 15), [Some(6), Some(8), None]);
        assert_eq!(fit_columns(&specs, 16), [Some(6), Some(9), None]);
        assert_eq!(fit_columns(&specs, 17), [Some(6), Some(4), Some(5)]);
        assert_eq!(fit_columns(&specs, 10), [None, Some(10), None]);
    }

    #[test]
    fn cells_are_cut_before_headers() {
        let users = ColumnSpec {
            header: 15,
            ..spec(15, 6, Some(0), Some(0))
        };
        let description = ColumnSpec {
            header: 11,
            ..spec(40, 8, Some(1), Some(1))
        };
        // Cut the description's cells rather than the users' header.
        assert_eq!(fit_columns(&[description, users], 40), [Some(24), Some(15)]);
        // Then headers, by rank.
        assert_eq!(fit_columns(&[description, users], 20), [Some(11), Some(8)]);
    }

    #[test]
    fn columns_are_cut_when_nothing_else_fits() {
        let specs = [spec(8, 8, None, None), spec(6, 6, None, None)];
        assert_eq!(fit_columns(&specs, 12), [Some(8), Some(3)]);
        assert_eq!(fit_columns(&specs, 9), [Some(9), None]);
        assert_eq!(fit_columns(&specs, 0), [None, None]);
        assert!(fit_columns(&[], 10).is_empty());
    }
}
