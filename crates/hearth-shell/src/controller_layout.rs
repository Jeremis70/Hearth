//! Where each Hearth pad input sits on the controller drawing (`assets/controllers/`), for the
//! controls page's callouts. The anchors file is generated from the drawing's shapes (see the
//! `anchors_match_the_drawing` test), so another outline can be dropped in.

use std::collections::BTreeMap;

use hearth_core::controls::PadInput;
use serde::Deserialize;

const ANCHORS: &str = include_str!("../../../assets/controllers/generic.toml");

/// Which callout column an input is listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    /// Left of the drawing.
    Left,
    /// Right of the drawing.
    Right,
    /// Below it.
    Bottom,
}

/// One input's shape on the drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// The input.
    pub input: PadInput,
    /// Shape center, in drawing units.
    pub x: f32,
    /// Shape center, in drawing units.
    pub y: f32,
    /// Shape width, in drawing units.
    pub w: f32,
    /// Shape height, in drawing units.
    pub h: f32,
}

/// The drawing's size and anchors.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Drawing width in its units.
    pub width: f32,
    /// Drawing height in its units.
    pub height: f32,
    /// Anchors, by input.
    pub anchors: Vec<Anchor>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    width: f32,
    height: f32,
    anchors: BTreeMap<String, AnchorDoc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnchorDoc {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Inputs near the middle are listed below the drawing (fraction of its width from the center).
const BOTTOM_ZONE: f32 = 0.09;

impl Layout {
    /// The generic controller.
    pub fn generic() -> Self {
        Self::parse(ANCHORS).expect("assets/controllers/generic.toml is valid (tested)")
    }

    fn parse(text: &str) -> Result<Self, String> {
        let doc: Doc = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut anchors = Vec::new();
        for (name, a) in doc.anchors {
            let input = PadInput::from_name(&name).ok_or(format!("unknown input {name}"))?;
            anchors.push(Anchor {
                input,
                x: a.x,
                y: a.y,
                w: a.w,
                h: a.h,
            });
        }
        Ok(Self {
            width: doc.width,
            height: doc.height,
            anchors,
        })
    }

    /// The column an anchor's callout goes in: by which half of the drawing it is on, with the
    /// buttons near the middle (Select, Start) below it.
    pub fn side(&self, a: &Anchor) -> Side {
        let from_center = a.x - self.width / 2.0;
        if from_center.abs() < self.width * BOTTOM_ZONE {
            Side::Bottom
        } else if from_center < 0.0 {
            Side::Left
        } else {
            Side::Right
        }
    }

    /// The anchors of one column, top to bottom (left to right below the drawing).
    pub fn column(&self, side: Side) -> Vec<&Anchor> {
        let mut column: Vec<_> = self
            .anchors
            .iter()
            .filter(|a| self.side(a) == side)
            .collect();
        if side == Side::Bottom {
            column.sort_by(|a, b| a.x.total_cmp(&b.x));
        } else {
            column.sort_by(|a, b| a.y.total_cmp(&b.y));
        }
        column
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = include_str!("../../../assets/controllers/generic.svg");

    /// Drawing ids to Hearth inputs: face buttons by position; each stick is a ring (stick
    /// movement) and a cap (stick click).
    const IDS: [(&str, &[&str]); 18] = [
        ("l2", &["l2"]),
        ("r2", &["r2"]),
        ("l1", &["l1"]),
        ("r1", &["r1"]),
        ("dpad_up", &["dpad_up"]),
        ("dpad_down", &["dpad_down"]),
        ("dpad_left", &["dpad_left"]),
        ("dpad_right", &["dpad_right"]),
        ("face_north", &["north"]),
        ("face_south", &["south"]),
        ("face_west", &["west"]),
        ("face_east", &["east"]),
        ("ls", &["left_stick"]),
        ("l3", &["l3"]),
        ("rs", &["right_stick"]),
        ("r3", &["r3"]),
        ("select", &["select"]),
        ("start", &["start"]),
    ];

    /// The anchors file as derived from the drawing's shapes.
    fn derived_anchors() -> String {
        let tree =
            usvg::Tree::from_str(SVG, &usvg::Options::default()).expect("generic.svg parses");
        let size = tree.size();
        let mut out = format!(
            "# Generated from generic.svg by the `anchors_match_the_drawing` test \
             (HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell anchors); don't edit.\n\
             # Each input's shape: center (x, y) and size (w, h), in the drawing's units.\n\n\
             width = {}\nheight = {}\n\n[anchors]\n",
            round(size.width()),
            round(size.height())
        );
        let mut rows = Vec::new();
        for (id, inputs) in IDS {
            let node = tree
                .node_by_id(id)
                .unwrap_or_else(|| panic!("generic.svg has no shape with id {id}"));
            let b = node.abs_stroke_bounding_box();
            for input in inputs {
                rows.push(format!(
                    "{input} = {{ x = {}, y = {}, w = {}, h = {} }}\n",
                    round(b.x() + b.width() / 2.0),
                    round(b.y() + b.height() / 2.0),
                    round(b.width()),
                    round(b.height())
                ));
            }
        }
        rows.sort();
        out.extend(rows);
        out
    }

    fn round(v: f32) -> f32 {
        (v * 100.0).round() / 100.0
    }

    /// Folder of the per-input highlight images.
    const HIGHLIGHTS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/controllers/generic"
    );

    /// Highlight style, baked into the files so `colorize` keeps it: a see-through fill (the
    /// drawing stays visible under it), a full outline a bit thicker than the drawing's
    /// (0.85), and a soft glow (a blurred, wider outline underneath).
    const HIGHLIGHT_FILL: f32 = 0.38;
    const HIGHLIGHT_STROKE: f32 = 1.3;
    const GLOW_STROKE: f32 = 2.6;
    const GLOW_BLUR: f32 = 1.4;
    const GLOW_OPACITY: f32 = 0.55;

    /// One image per drawing id, the size of the whole drawing, with only that input's shapes
    /// (transforms flattened by usvg): the controls page tints it with the accent and stacks
    /// it over the drawing to highlight the focused input.
    fn derived_highlights() -> Vec<(&'static str, String)> {
        let tree =
            usvg::Tree::from_str(SVG, &usvg::Options::default()).expect("generic.svg parses");
        let size = tree.size();
        let (w, h) = (round(size.width()), round(size.height()));
        IDS.iter()
            .map(|(id, _)| {
                let node = tree
                    .node_by_id(id)
                    .unwrap_or_else(|| panic!("generic.svg has no shape with id {id}"));
                let mut shapes = Vec::new();
                collect_paths(node, &mut shapes);
                let glow: String = shapes
                    .iter()
                    .map(|(d, rule)| {
                        format!(
                            "<path d=\"{d}\"{rule} fill=\"none\" stroke=\"#fff\" \
                             stroke-width=\"{GLOW_STROKE}\" stroke-linejoin=\"round\"/>\n"
                        )
                    })
                    .collect();
                let shape: String = shapes
                    .iter()
                    .map(|(d, rule)| {
                        format!(
                            "<path d=\"{d}\"{rule} fill=\"#fff\" fill-opacity=\"{HIGHLIGHT_FILL}\" \
                             stroke=\"#fff\" stroke-width=\"{HIGHLIGHT_STROKE}\" \
                             stroke-linejoin=\"round\"/>\n"
                        )
                    })
                    .collect();
                let svg = format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
                     viewBox=\"0 0 {w} {h}\">\n<!-- Generated from generic.svg ({id}) by the \
                     `every_input_has_a_highlight_matching_the_drawing` test; don't edit. -->\n\
                     <defs><filter id=\"glow\" filterUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" \
                     width=\"{w}\" height=\"{h}\"><feGaussianBlur stdDeviation=\"{GLOW_BLUR}\"/>\
                     </filter></defs>\n<g filter=\"url(#glow)\" opacity=\"{GLOW_OPACITY}\">\n{glow}</g>\n\
                     {shape}</svg>\n"
                );
                (*id, svg)
            })
            .collect()
    }

    /// Every path under `node`, flattened: its data and ` fill-rule="evenodd"` if it has that
    /// rule (the sticks' rings).
    fn collect_paths(node: &usvg::Node, out: &mut Vec<(String, &'static str)>) {
        use std::fmt::Write as _;
        use usvg::tiny_skia_path::PathSegment;
        match node {
            usvg::Node::Group(group) => {
                for child in group.children() {
                    collect_paths(child, out);
                }
            }
            usvg::Node::Path(path) => {
                let Some(data) = path.data().clone().transform(path.abs_transform()) else {
                    return;
                };
                let mut d = String::new();
                let p = |p: usvg::tiny_skia_path::Point| format!("{} {}", round(p.x), round(p.y));
                for segment in data.segments() {
                    let _ = match segment {
                        PathSegment::MoveTo(a) => write!(d, "M{}", p(a)),
                        PathSegment::LineTo(a) => write!(d, "L{}", p(a)),
                        PathSegment::QuadTo(a, b) => write!(d, "Q{} {}", p(a), p(b)),
                        PathSegment::CubicTo(a, b, c) => write!(d, "C{} {} {}", p(a), p(b), p(c)),
                        PathSegment::Close => write!(d, "Z"),
                    };
                }
                let even_odd = path
                    .fill()
                    .is_some_and(|f| f.rule() == usvg::FillRule::EvenOdd);
                out.push((
                    d,
                    if even_odd {
                        " fill-rule=\"evenodd\""
                    } else {
                        ""
                    },
                ));
            }
            _ => {}
        }
    }

    #[test]
    fn every_input_has_a_highlight_matching_the_drawing() {
        let slint = include_str!("../ui/controls/highlights.slint");
        for (id, svg) in derived_highlights() {
            let path = format!("{HIGHLIGHTS}/{id}.svg");
            if std::env::var_os("HEARTH_WRITE_ANCHORS").is_some() {
                std::fs::create_dir_all(HIGHLIGHTS).expect("create highlights folder");
                std::fs::write(&path, &svg).expect("write highlight");
                continue;
            }
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("no highlight for {id}: HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell anchors"));
            assert_eq!(
                on_disk, svg,
                "{id}.svg is out of date: HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell highlight"
            );
            assert!(
                slint.contains(&format!("generic/{id}.svg")),
                "highlights.slint doesn't use {id}.svg"
            );
        }
        // Every Hearth input on the drawing has one.
        for (_, inputs) in IDS {
            for input in inputs {
                assert!(
                    slint.contains(&format!("\"{input}\"")),
                    "highlights.slint misses {input}"
                );
            }
        }
    }

    #[test]
    fn anchors_match_the_drawing() {
        let derived = derived_anchors();
        if std::env::var_os("HEARTH_WRITE_ANCHORS").is_some() {
            let path = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/controllers/generic.toml"
            );
            std::fs::write(path, &derived).expect("write generic.toml");
            return;
        }
        assert_eq!(
            ANCHORS, derived,
            "generic.toml is out of date: HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell anchors"
        );
    }

    #[test]
    fn every_standard_input_has_one_anchor_inside_the_drawing() {
        let layout = Layout::generic();
        for input in PadInput::ALL {
            if input.is_extra() || input == PadInput::Dpad {
                continue;
            }
            let found: Vec<_> = layout.anchors.iter().filter(|a| a.input == input).collect();
            assert_eq!(found.len(), 1, "{input}");
            let a = found[0];
            assert!((0.0..=layout.width).contains(&a.x) && (0.0..=layout.height).contains(&a.y));
            assert!(a.w > 0.0 && a.h > 0.0, "{input}");
        }
        assert_eq!(layout.column(Side::Left).len(), 8);
        assert_eq!(layout.column(Side::Right).len(), 8);
        let bottom: Vec<_> = layout
            .column(Side::Bottom)
            .iter()
            .map(|a| a.input)
            .collect();
        assert_eq!(bottom, [PadInput::Select, PadInput::Start]);
    }
}
