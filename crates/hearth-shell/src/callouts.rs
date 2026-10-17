//! Where the controls page puts each callout and its leader line, in page pixels at 1920×1080
//! (the page multiplies everything by `Theme.scale`).
//!
//! Each side column lists its inputs in the order of their anchors' height, and each leader
//! line runs horizontally from its label to an elbow, then straight to the input, like the
//! PS5's controller screen. A small search then swaps rows or moves elbows until no two lines
//! cross and no line crosses a label (`no_leader_lines_cross` checks the result).

use hearth_core::controls::PadInput;

use crate::controller_layout::{Layout, Side};

/// Page size the geometry is computed for.
pub const PAGE_W: f32 = 1920.0;
/// Title-safe margin left and right (`Theme.safe-x`).
const SAFE_X: f32 = 96.0;
/// Drawing's top edge and height.
const DRAW_Y: f32 = 236.0;
const DRAW_H: f32 = 520.0;
/// Side columns.
const COL_W: f32 = 400.0;
const ROW_H: f32 = 52.0;
const ROW_STEP: f32 = 58.0;
/// Where side rows may go: under the header, above the bottom row.
const ROWS_TOP: f32 = 200.0;
const ROWS_BOTTOM: f32 = 750.0;
/// Bottom row (Select, Start, extras).
const BOTTOM_Y: f32 = DRAW_Y + DRAW_H + 12.0;
const BOTTOM_W: f32 = 260.0;
const BOTTOM_W_NARROW: f32 = 180.0;
const BOTTOM_GAP: f32 = 16.0;
/// Distance from a bottom callout's edge to its glyph's center: padding + half the 40 px glyph.
const BOTTOM_INSET: f32 = 8.0 + 20.0;
/// Shortest horizontal part of a side line.
const MIN_RUN: f32 = 16.0;
/// Elbows' distance outside the outermost shape of their side.
const ELBOW_GAP: f32 = 28.0;

/// A point on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A rectangle on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Glyph position inside a callout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Label, then the glyph (left column, and bottom callouts left of the middle).
    GlyphLast,
    /// Glyph, then the label.
    GlyphFirst,
}

/// One placed callout.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub input: PadInput,
    pub side: Side,
    pub label: Rect,
    pub align: Align,
    /// Label edge, elbow, anchor; none for inputs that aren't on the drawing.
    pub leader: Option<[Point; 3]>,
    /// The input's shape on the page, for the focus highlight.
    pub shape: Option<Rect>,
}

/// The drawing's rectangle on the page.
pub fn drawing(layout: &Layout) -> Rect {
    let k = DRAW_H / layout.height;
    let w = layout.width * k;
    Rect {
        x: (PAGE_W - w) / 2.0,
        y: DRAW_Y,
        w,
        h: DRAW_H,
    }
}

/// Every callout: the left column top to bottom, then the right column, then the bottom row
/// left to right. `extras` (touchpad, share, paddles) have no shape and go on the bottom row.
pub fn place(layout: &Layout, extras: &[PadInput]) -> Vec<Placed> {
    let draw = drawing(layout);
    let k = draw.h / layout.height;
    let on_page = |x: f32, y: f32| Point {
        x: draw.x + x * k,
        y: draw.y + y * k,
    };
    let mut out = Vec::new();
    for side in [Side::Left, Side::Right] {
        let anchors: Vec<_> = layout
            .column(side)
            .into_iter()
            .map(|a| {
                let p = on_page(a.x, a.y);
                let shape = Rect {
                    x: p.x - a.w * k / 2.0,
                    y: p.y - a.h * k / 2.0,
                    w: a.w * k,
                    h: a.h * k,
                };
                (a.input, p, shape)
            })
            .collect();
        out.extend(column(side, anchors));
    }
    out.extend(bottom_row(layout, draw, extras));
    out
}

fn column(side: Side, mut anchors: Vec<(PadInput, Point, Rect)>) -> Vec<Placed> {
    let n = anchors.len();
    if n == 0 {
        return Vec::new();
    }
    // Top to bottom; on a tie the outer anchor first.
    let outward = if side == Side::Left { 1.0 } else { -1.0 };
    anchors.sort_by(|a, b| {
        a.1.y
            .total_cmp(&b.1.y)
            .then((a.1.x * outward).total_cmp(&(b.1.x * outward)))
    });
    let span = (n - 1) as f32 * ROW_STEP;
    let middle = (anchors[0].1.y + anchors[n - 1].1.y) / 2.0;
    let top = (middle - span / 2.0)
        .min(ROWS_BOTTOM - ROW_H / 2.0 - span)
        .max(ROWS_TOP + ROW_H / 2.0);
    let rows: Vec<f32> = (0..n).map(|row| top + row as f32 * ROW_STEP).collect();
    let (edge, label_x) = if side == Side::Left {
        (SAFE_X + COL_W, SAFE_X)
    } else {
        (PAGE_W - SAFE_X - COL_W, PAGE_W - SAFE_X - COL_W)
    };
    // Every elbow sits on one vertical line just outside the outermost shape: the horizontal
    // parts can't cross anything, and the straight parts all start on that line.
    let outermost = anchors
        .iter()
        .map(|(_, _, shape)| {
            if side == Side::Left {
                shape.x
            } else {
                -(shape.x + shape.w)
            }
        })
        .fold(f32::INFINITY, f32::min);
    let elbow_x = if side == Side::Left {
        (outermost - ELBOW_GAP).max(edge + MIN_RUN)
    } else {
        (-outermost + ELBOW_GAP).min(edge - MIN_RUN)
    };
    let order = shortest_matching(&rows, elbow_x, &anchors);
    (0..n)
        .map(|row| {
            let (input, anchor, shape) = anchors[order[row]];
            let y = rows[row];
            Placed {
                input,
                side,
                label: Rect {
                    x: label_x,
                    y: y - ROW_H / 2.0,
                    w: COL_W,
                    h: ROW_H,
                },
                align: if side == Side::Left {
                    Align::GlyphLast
                } else {
                    Align::GlyphFirst
                },
                leader: Some([
                    Point { x: edge, y },
                    Point { x: elbow_x, y },
                    at_edge(anchor, shape, Point { x: elbow_x, y }),
                ]),
                shape: Some(shape),
            }
        })
        .collect()
}

/// Which anchor each row points at: the assignment with the shortest straight parts in total.
/// Two crossing straight segments can always be swapped for a shorter pair, so the shortest
/// assignment has no crossings. Ties keep the top-to-bottom order. Columns have at most a dozen
/// inputs; this tries every assignment, pruned by the best total so far.
fn shortest_matching(
    rows: &[f32],
    elbow_x: f32,
    anchors: &[(PadInput, Point, Rect)],
) -> Vec<usize> {
    let n = rows.len();
    let cost: Vec<Vec<f32>> = rows
        .iter()
        .map(|&y| {
            anchors
                .iter()
                .map(|(_, a, _)| (a.x - elbow_x).hypot(a.y - y))
                .collect()
        })
        .collect();
    let mut best: Vec<usize> = (0..n).collect();
    let mut best_cost: f32 = (0..n).map(|row| cost[row][row]).sum();
    let mut current = Vec::with_capacity(n);
    let mut used = vec![false; n];
    search(
        &cost,
        &mut current,
        &mut used,
        0.0,
        &mut best,
        &mut best_cost,
    );
    best
}

fn search(
    cost: &[Vec<f32>],
    current: &mut Vec<usize>,
    used: &mut [bool],
    total: f32,
    best: &mut Vec<usize>,
    best_cost: &mut f32,
) {
    // A strictly shorter total is needed to beat the earlier (more top-to-bottom) assignment.
    if total >= *best_cost - 0.01 {
        return;
    }
    let row = current.len();
    if row == cost.len() {
        *best_cost = total;
        best.clone_from(current);
        return;
    }
    for anchor in 0..cost.len() {
        if !used[anchor] {
            used[anchor] = true;
            current.push(anchor);
            search(
                cost,
                current,
                used,
                total + cost[row][anchor],
                best,
                best_cost,
            );
            current.pop();
            used[anchor] = false;
        }
    }
}

/// Select and Start below the drawing, glyphs pulled toward the middle and lines rising
/// between the sticks; extras alternate right, then left, outside them.
fn bottom_row(layout: &Layout, draw: Rect, extras: &[PadInput]) -> Vec<Placed> {
    let k = draw.h / layout.height;
    let middle = PAGE_W / 2.0;
    let mut placed = Vec::new();
    for a in layout.column(Side::Bottom) {
        let anchor = Point {
            x: draw.x + a.x * k,
            y: draw.y + a.y * k,
        };
        let left = anchor.x < middle;
        let glyph_x = anchor.x + (middle - anchor.x) / 3.0;
        let elbow_y = anchor.y + (BOTTOM_Y - anchor.y) / 2.0;
        let shape = Rect {
            x: anchor.x - a.w * k / 2.0,
            y: anchor.y - a.h * k / 2.0,
            w: a.w * k,
            h: a.h * k,
        };
        let label = Rect {
            x: if left {
                glyph_x + BOTTOM_INSET - BOTTOM_W
            } else {
                glyph_x - BOTTOM_INSET
            },
            y: BOTTOM_Y,
            w: BOTTOM_W,
            h: ROW_H,
        };
        placed.push(Placed {
            input: a.input,
            side: Side::Bottom,
            label,
            align: if left {
                Align::GlyphLast
            } else {
                Align::GlyphFirst
            },
            leader: Some([
                Point {
                    x: glyph_x,
                    y: BOTTOM_Y,
                },
                Point {
                    x: glyph_x,
                    y: elbow_y,
                },
                at_edge(
                    anchor,
                    shape,
                    Point {
                        x: glyph_x,
                        y: elbow_y,
                    },
                ),
            ]),
            shape: Some(shape),
        });
    }
    let w = if extras.len() > 4 {
        BOTTOM_W_NARROW
    } else {
        BOTTOM_W
    };
    let mut right = placed
        .iter()
        .map(|p| p.label.x + p.label.w)
        .fold(middle, f32::max);
    let mut left = placed.iter().map(|p| p.label.x).fold(middle, f32::min);
    for (i, &input) in extras.iter().enumerate() {
        let on_right = i % 2 == 0;
        let x = if on_right {
            right + BOTTOM_GAP
        } else {
            left - BOTTOM_GAP - w
        };
        if x < SAFE_X || x + w > PAGE_W - SAFE_X {
            tracing::warn!(%input, "no room for this input's callout");
            continue;
        }
        if on_right {
            right = x + w;
        } else {
            left = x;
        }
        placed.push(Placed {
            input,
            side: Side::Bottom,
            label: Rect {
                x,
                y: BOTTOM_Y,
                w,
                h: ROW_H,
            },
            align: if on_right {
                Align::GlyphFirst
            } else {
                Align::GlyphLast
            },
            leader: None,
            shape: None,
        });
    }
    placed.sort_by(|a, b| a.label.x.total_cmp(&b.label.x));
    placed
}

/// Where a line from `from` to the shape's center meets the shape's edge (approximated by its
/// inscribed circle): lines stop at the button, not in its middle.
fn at_edge(center: Point, shape: Rect, from: Point) -> Point {
    let (dx, dy) = (from.x - center.x, from.y - center.y);
    let len = dx.hypot(dy);
    let r = shape.w.min(shape.h) / 2.0;
    if len <= r {
        return center;
    }
    Point {
        x: center.x + dx / len * r,
        y: center.y + dy / len * r,
    }
}

/// Geometry checks for the tests.
#[cfg(test)]
mod check {
    use super::{Point, Rect};

    /// Whether two leader lines cross. Lines meeting only at a shared anchor don't.
    pub(super) fn lines_cross(a: &[Point; 3], b: &[Point; 3]) -> bool {
        let (a, b) = if near(a[2], b[2]) {
            (trimmed(a), trimmed(b))
        } else {
            (*a, *b)
        };
        a.windows(2).any(|sa| {
            b.windows(2)
                .any(|sb| segments_cross(sa[0], sa[1], sb[0], sb[1]))
        })
    }

    /// The line stopped 1 px short of its anchor.
    pub(super) fn trimmed(line: &[Point; 3]) -> [Point; 3] {
        let [start, elbow, end] = *line;
        let (dx, dy) = (elbow.x - end.x, elbow.y - end.y);
        let len = dx.hypot(dy).max(1e-3);
        let end = Point {
            x: end.x + dx / len,
            y: end.y + dy / len,
        };
        [start, elbow, end]
    }

    /// Whether a leader line passes through a rectangle's inside.
    pub(super) fn line_hits_rect(line: &[Point; 3], r: &Rect) -> bool {
        let inset = Rect {
            x: r.x + 0.5,
            y: r.y + 0.5,
            w: r.w - 1.0,
            h: r.h - 1.0,
        };
        let corners = [
            Point {
                x: inset.x,
                y: inset.y,
            },
            Point {
                x: inset.x + inset.w,
                y: inset.y,
            },
            Point {
                x: inset.x + inset.w,
                y: inset.y + inset.h,
            },
            Point {
                x: inset.x,
                y: inset.y + inset.h,
            },
        ];
        line.windows(2).any(|s| {
            let inside = |p: Point| {
                p.x > inset.x && p.x < inset.x + inset.w && p.y > inset.y && p.y < inset.y + inset.h
            };
            inside(s[0])
                || inside(s[1])
                || (0..4).any(|i| segments_cross(s[0], s[1], corners[i], corners[(i + 1) % 4]))
        })
    }

    pub(super) fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01
    }

    pub(super) fn cross(o: Point, a: Point, b: Point) -> f32 {
        (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
    }

    /// Whether segments p1p2 and q1q2 intersect (touching or overlapping counts).
    pub(super) fn segments_cross(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
        let d1 = cross(q1, q2, p1);
        let d2 = cross(q1, q2, p2);
        let d3 = cross(p1, p2, q1);
        let d4 = cross(p1, p2, q2);
        let eps = 1e-3;
        if ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps))
            && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
        {
            return true;
        }
        let on = |a: Point, b: Point, p: Point, d: f32| {
            d.abs() <= eps
                && p.x >= a.x.min(b.x) - eps
                && p.x <= a.x.max(b.x) + eps
                && p.y >= a.y.min(b.y) - eps
                && p.y <= a.y.max(b.y) + eps
        };
        on(q1, q2, p1, d1) || on(q1, q2, p2, d2) || on(p1, p2, q1, d3) || on(p1, p2, q2, d4)
    }
}

#[cfg(test)]
mod tests {
    use super::check::{line_hits_rect, lines_cross};
    use super::*;

    fn scaled(placed: &[Placed], s: f32) -> (Vec<[Point; 3]>, Vec<Rect>) {
        let p = |p: Point| Point {
            x: (p.x * s).round(),
            y: (p.y * s).round(),
        };
        let lines = placed
            .iter()
            .filter_map(|c| c.leader.map(|l| [p(l[0]), p(l[1]), p(l[2])]))
            .collect();
        let labels = placed
            .iter()
            .map(|c| Rect {
                x: (c.label.x * s).round(),
                y: (c.label.y * s).round(),
                w: (c.label.w * s).round(),
                h: (c.label.h * s).round(),
            })
            .collect();
        (lines, labels)
    }

    fn check(extras: &[PadInput]) {
        let layout = Layout::generic();
        let placed = place(&layout, extras);
        for (size, s) in [("1920×1080", 1.0), ("1280×720", 1280.0 / 1920.0)] {
            let (lines, labels) = scaled(&placed, s);
            let with_line: Vec<_> = placed.iter().filter(|c| c.leader.is_some()).collect();
            for (i, a) in lines.iter().enumerate() {
                for (j, b) in lines.iter().enumerate().skip(i + 1) {
                    assert!(
                        !lines_cross(a, b),
                        "{size}: {} and {} cross",
                        with_line[i].input,
                        with_line[j].input
                    );
                }
                for (j, label) in labels.iter().enumerate() {
                    let own = placed[j].input == with_line[i].input;
                    assert!(
                        own || !line_hits_rect(a, label),
                        "{size}: {}'s line crosses {}'s label",
                        with_line[i].input,
                        placed[j].input
                    );
                }
            }
            for (i, a) in labels.iter().enumerate() {
                for b in &labels[i + 1..] {
                    let overlap =
                        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                    assert!(!overlap, "{size}: labels overlap");
                }
            }
        }
    }

    #[test]
    fn no_leader_lines_cross() {
        check(&[]);
        check(&[PadInput::Touchpad]);
        check(&[
            PadInput::Touchpad,
            PadInput::Paddle1,
            PadInput::Paddle2,
            PadInput::Paddle3,
            PadInput::Paddle4,
        ]);
    }

    #[test]
    fn crossing_detection() {
        let line = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
            [
                Point { x: a.0, y: a.1 },
                Point { x: b.0, y: b.1 },
                Point { x: c.0, y: c.1 },
            ]
        };
        let a = line((0.0, 0.0), (10.0, 0.0), (20.0, 10.0));
        let b = line((0.0, 10.0), (10.0, 10.0), (20.0, 0.0));
        assert!(lines_cross(&a, &b));
        let c = line((0.0, 20.0), (10.0, 20.0), (20.0, 10.0));
        assert!(!lines_cross(&a, &c), "meeting at a shared anchor");
        let d = line((0.0, 5.0), (30.0, 5.0), (40.0, 5.0));
        assert!(line_hits_rect(
            &d,
            &Rect {
                x: 5.0,
                y: 0.0,
                w: 10.0,
                h: 10.0
            }
        ));
    }

    #[test]
    fn columns_are_in_anchor_order() {
        let placed = place(&Layout::generic(), &[]);
        let left: Vec<_> = placed.iter().filter(|c| c.side == Side::Left).collect();
        assert_eq!(left.len(), 8);
        assert_eq!(left[0].input, PadInput::L2);
        let bottom: Vec<_> = placed
            .iter()
            .filter(|c| c.side == Side::Bottom)
            .map(|c| c.input)
            .collect();
        assert_eq!(bottom, [PadInput::Select, PadInput::Start]);
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// `HEARTH_CALLOUT_PREVIEW=out.svg cargo test -p hearth-shell callout_preview` draws the
    /// labels and lines over the drawing, to look at the layout without running Hearth.
    #[test]
    fn callout_preview() {
        let Some(path) = std::env::var_os("HEARTH_CALLOUT_PREVIEW") else {
            return;
        };
        let layout = Layout::generic();
        let draw = drawing(&layout);
        let mut svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{PAGE_W}' height='1080' \
             ><rect width='100%' height='100%' fill='#15171c'/>\
             <image href='file://{}/../../assets/controllers/generic.svg' \
             x='{}' y='{}' width='{}' height='{}' opacity='0.5'/>",
            env!("CARGO_MANIFEST_DIR"),
            draw.x,
            draw.y,
            draw.w,
            draw.h
        );
        for c in place(&layout, &[PadInput::Touchpad]) {
            let r = c.label;
            svg += &format!(
                "<rect x='{}' y='{}' width='{}' height='{}' fill='none' stroke='#555'/>\
                 <text x='{}' y='{}' fill='#ddd' font-size='20'>{}</text>",
                r.x,
                r.y,
                r.w,
                r.h,
                r.x + 8.0,
                r.y + 32.0,
                c.input
            );
            if let Some([a, b, e]) = c.leader {
                svg += &format!(
                    "<polyline points='{},{} {},{} {},{}' fill='none' stroke='#e8833a' stroke-width='2'/>",
                    a.x, a.y, b.x, b.y, e.x, e.y
                );
            }
        }
        svg += "</svg>";
        std::fs::write(path, svg).expect("write preview");
    }
}
