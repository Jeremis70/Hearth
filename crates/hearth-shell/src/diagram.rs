//! The controls page's controller drawing and its per-input highlights as ready-made images.
//!
//! Slint rasterizes an SVG image lazily, whenever it is first shown at a size, and its image
//! cache can't hold the drawing plus every full-size highlight: each focus move re-rasterized
//! a highlight (35 ms frames in release, 350 ms in debug). Here everything is rasterized once
//! per scale with resvg, and each highlight is cropped to its visible pixels and placed in
//! page units (the 1920 × 1080 canvas of [`crate::callouts`]), the same units as the lines and
//! labels.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use resvg::{tiny_skia, usvg};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::callouts::Rect;

const DRAWING: &str = include_str!("../../../assets/controllers/generic.svg");

/// Hearth pad inputs on the drawing and their highlight (`scripts`-generated, see the
/// `every_input_has_a_highlight_matching_the_drawing` test).
pub(crate) const HIGHLIGHTS: [(&str, &str); 18] = [
    (
        "l2",
        include_str!("../../../assets/controllers/generic/l2.svg"),
    ),
    (
        "r2",
        include_str!("../../../assets/controllers/generic/r2.svg"),
    ),
    (
        "l1",
        include_str!("../../../assets/controllers/generic/l1.svg"),
    ),
    (
        "r1",
        include_str!("../../../assets/controllers/generic/r1.svg"),
    ),
    (
        "dpad_up",
        include_str!("../../../assets/controllers/generic/dpad_up.svg"),
    ),
    (
        "dpad_down",
        include_str!("../../../assets/controllers/generic/dpad_down.svg"),
    ),
    (
        "dpad_left",
        include_str!("../../../assets/controllers/generic/dpad_left.svg"),
    ),
    (
        "dpad_right",
        include_str!("../../../assets/controllers/generic/dpad_right.svg"),
    ),
    (
        "north",
        include_str!("../../../assets/controllers/generic/face_north.svg"),
    ),
    (
        "south",
        include_str!("../../../assets/controllers/generic/face_south.svg"),
    ),
    (
        "west",
        include_str!("../../../assets/controllers/generic/face_west.svg"),
    ),
    (
        "east",
        include_str!("../../../assets/controllers/generic/face_east.svg"),
    ),
    (
        "left_stick",
        include_str!("../../../assets/controllers/generic/ls.svg"),
    ),
    (
        "l3",
        include_str!("../../../assets/controllers/generic/l3.svg"),
    ),
    (
        "right_stick",
        include_str!("../../../assets/controllers/generic/rs.svg"),
    ),
    (
        "r3",
        include_str!("../../../assets/controllers/generic/r3.svg"),
    ),
    (
        "select",
        include_str!("../../../assets/controllers/generic/select.svg"),
    ),
    (
        "start",
        include_str!("../../../assets/controllers/generic/start.svg"),
    ),
];

/// An image and where it goes, in page units.
#[derive(Debug, Clone)]
pub struct Piece {
    pub image: Image,
    pub rect: Rect,
}

/// Pixels and where they go, in page units: what [`Piece`] is made from, on any thread.
struct Raw {
    pixels: SharedPixelBuffer<Rgba8Pixel>,
    rect: Rect,
}

impl Raw {
    fn piece(self) -> Piece {
        Piece {
            // tiny-skia pixels are premultiplied.
            image: Image::from_rgba8_premultiplied(self.pixels),
            rect: self.rect,
        }
    }
}

/// The drawing and the highlights for one scale, rasterized (`Send`).
pub struct Rendered {
    scale: f32,
    drawing: Option<Raw>,
    highlights: Vec<(&'static str, Raw)>,
}

impl Rendered {
    fn art(self) -> Art {
        Art {
            scale: self.scale,
            drawing: self
                .drawing
                .map(|raw| raw.piece().image)
                .unwrap_or_default(),
            highlights: self
                .highlights
                .into_iter()
                .map(|(input, raw)| (input, raw.piece()))
                .collect(),
        }
    }
}

/// The drawing and the highlights as images, for one scale (UI thread).
#[derive(Debug)]
pub struct Art {
    /// Physical pixels per page unit they were made for.
    scale: f32,
    pub drawing: Image,
    highlights: HashMap<&'static str, Piece>,
}

impl Art {
    /// The highlight of `input`, if it is on the drawing.
    pub fn highlight(&self, input: &str) -> Option<&Piece> {
        self.highlights.get(input)
    }
}

thread_local! {
    static SCALE: RefCell<f32> = const { RefCell::new(1.0) };
    static ART: RefCell<Option<Rc<Art>>> = const { RefCell::new(None) };
}

/// Physical pixels per page unit on this screen (the window's scale for a 1920 × 1080 page).
/// The images are remade at the next [`art`] if it changed.
pub fn set_scale(scale: f32) {
    SCALE.with(|s| *s.borrow_mut() = scale.max(0.1));
}

fn scale() -> f32 {
    SCALE.with(|s| *s.borrow())
}

/// The images for the drawing placed at `draw` (page units) at the current scale (UI thread).
/// Made here if [`warm_up`] hasn't made them yet.
pub fn art(draw: Rect) -> Rc<Art> {
    let scale = scale();
    if let Some(art) = ART
        .with(|cell| cell.borrow().clone())
        .filter(|a| a.scale == scale)
    {
        return art;
    }
    let art = Rc::new(render_all(draw, scale).art());
    ART.with(|cell| *cell.borrow_mut() = Some(art.clone()));
    art
}

/// Makes the images for the current scale on a worker thread, so opening the controls page
/// doesn't have to (UI thread).
pub fn warm_up(draw: Rect) {
    let scale = scale();
    if ART.with(|cell| cell.borrow().as_ref().is_some_and(|a| a.scale == scale)) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("hearth-diagram".into())
        .spawn(move || {
            let rendered = render_all(draw, scale);
            let _ = slint::invoke_from_event_loop(move || {
                if super::diagram::scale() == rendered.scale {
                    let art = Rc::new(rendered.art());
                    ART.with(|cell| *cell.borrow_mut() = Some(art));
                }
            });
        });
    if let Err(err) = spawned {
        tracing::warn!("cannot start the drawing thread: {err}");
    }
}

/// Rasterizes the drawing and every highlight (any thread).
pub fn render_all(draw: Rect, scale: f32) -> Rendered {
    let started = std::time::Instant::now();
    let rendered = Rendered {
        scale,
        drawing: render(DRAWING, draw, scale, false),
        highlights: HIGHLIGHTS
            .iter()
            .filter_map(|(input, svg)| Some((*input, render(svg, draw, scale, true)?)))
            .collect(),
    };
    tracing::debug!(
        scale,
        ms = started.elapsed().as_millis(),
        "controller drawing rasterized"
    );
    rendered
}

/// Room around a highlight's shapes for its glow (3 × the blur's deviation), drawing units.
const GLOW_MARGIN: f32 = 5.0;

/// Renders `svg` (the drawing's canvas) at `draw` × `scale`. With `crop` (a highlight) only
/// the area around its shapes is rendered, then trimmed to its visible pixels; `rect` says
/// where the result goes.
fn render(svg: &str, draw: Rect, scale: f32, crop: bool) -> Option<Raw> {
    let tree = match usvg::Tree::from_str(svg, &usvg::Options::default()) {
        Ok(tree) => tree,
        Err(err) => {
            tracing::warn!("controller drawing doesn't parse: {err}");
            return None;
        }
    };
    let size = tree.size();
    // Pixels per drawing unit.
    let k = draw.h * scale / size.height();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = |v: f32| v.max(0.0) as u32;
    let (full_w, full_h) = (
        px((size.width() * k).round()).max(1),
        px((size.height() * k).round()).max(1),
    );
    // The area to render, in pixels of the whole drawing.
    let (x0, y0, x1, y1) = if crop {
        let b = tree.root().abs_stroke_bounding_box();
        (
            px(((b.x() - GLOW_MARGIN) * k).floor()),
            px(((b.y() - GLOW_MARGIN) * k).floor()),
            px(((b.right() + GLOW_MARGIN) * k).ceil()).min(full_w),
            px(((b.bottom() + GLOW_MARGIN) * k).ceil()).min(full_h),
        )
    } else {
        (0, 0, full_w, full_h)
    };
    let mut pixmap =
        tiny_skia::Pixmap::new(x1.checked_sub(x0)?.max(1), y1.checked_sub(y0)?.max(1))?;
    #[allow(clippy::cast_precision_loss)]
    let transform = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, -(x0 as f32), -(y0 as f32));
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let (cx0, cy0, cx1, cy1) = if crop {
        visible_box(&pixmap)?
    } else {
        (0, 0, pixmap.width(), pixmap.height())
    };
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(cw, ch);
    let stride = pixmap.width() as usize * 4;
    let row_bytes = cw as usize * 4;
    let out = pixels.make_mut_bytes();
    for row in 0..ch as usize {
        let from = (cy0 as usize + row) * stride + cx0 as usize * 4;
        out[row * row_bytes..(row + 1) * row_bytes]
            .copy_from_slice(&pixmap.data()[from..from + row_bytes]);
    }
    #[allow(clippy::cast_precision_loss)]
    let unit = |pixels: u32| pixels as f32 / scale;
    Some(Raw {
        pixels,
        rect: Rect {
            x: draw.x + unit(x0 + cx0),
            y: draw.y + unit(y0 + cy0),
            w: unit(cw),
            h: unit(ch),
        },
    })
}

/// The smallest box (x0, y0, x1, y1; ends exclusive) holding every pixel that isn't fully
/// transparent, or `None` if there is none.
fn visible_box(pixmap: &tiny_skia::Pixmap) -> Option<(u32, u32, u32, u32)> {
    let w = pixmap.width();
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for (i, px) in pixmap.pixels().iter().enumerate() {
        if px.alpha() == 0 {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let (x, y) = (i as u32 % w, i as u32 / w);
        found = Some(match found {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_layout::Layout;

    #[test]
    fn highlights_are_cropped_and_sit_on_their_input() {
        let layout = Layout::generic();
        let draw = crate::callouts::drawing(&layout);
        let art = render_all(draw, 1.0).art();
        let k = draw.h / layout.height;
        for (input, _) in HIGHLIGHTS {
            let piece = art.highlight(input).unwrap_or_else(|| panic!("{input}"));
            let r = piece.rect;
            assert!(
                r.w < draw.w / 2.0 && r.h < draw.h / 2.0,
                "{input} not cropped: {r:?}"
            );
            // The input's anchor (its shape's center) is inside its highlight.
            let a = layout
                .column(crate::controller_layout::Side::Left)
                .into_iter()
                .chain(layout.column(crate::controller_layout::Side::Right))
                .chain(layout.column(crate::controller_layout::Side::Bottom))
                .find(|a| a.input.name() == input)
                .unwrap_or_else(|| panic!("no anchor for {input}"));
            let (ax, ay) = (draw.x + a.x * k, draw.y + a.y * k);
            assert!(
                ax > r.x && ax < r.x + r.w && ay > r.y && ay < r.y + r.h,
                "{input}: anchor ({ax}, {ay}) outside {r:?}"
            );
        }
    }

    #[test]
    fn rendering_around_the_shapes_loses_no_glow() {
        let draw = crate::callouts::drawing(&Layout::generic());
        for (input, svg) in HIGHLIGHTS {
            let around = render(svg, draw, 1.5, true).expect("rendered");
            // The whole canvas, trimmed afterwards: the reference.
            let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap();
            let k = draw.h * 1.5 / tree.size().height();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let mut full = tiny_skia::Pixmap::new(
                (tree.size().width() * k).round() as u32,
                (tree.size().height() * k).round() as u32,
            )
            .unwrap();
            resvg::render(
                &tree,
                tiny_skia::Transform::from_scale(k, k),
                &mut full.as_mut(),
            );
            let (x0, y0, x1, y1) = visible_box(&full).unwrap();
            #[allow(clippy::cast_precision_loss)]
            let expected = (
                x0 as f32 / 1.5,
                y0 as f32 / 1.5,
                (x1 - x0) as f32 / 1.5,
                (y1 - y0) as f32 / 1.5,
            );
            let r = around.rect;
            let got = (r.x - draw.x, r.y - draw.y, r.w, r.h);
            assert!(
                (got.0 - expected.0).abs() < 1.0
                    && (got.1 - expected.1).abs() < 1.0
                    && (got.2 - expected.2).abs() < 1.5
                    && (got.3 - expected.3).abs() < 1.5,
                "{input}: {got:?} vs {expected:?}"
            );
        }
    }
}
