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

    /// Drawing ids to Hearth inputs: face buttons by position, sticks for both the stick and
    /// its click.
    const IDS: [(&str, &[&str]); 16] = [
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
        ("ls", &["left_stick", "l3"]),
        ("rs", &["right_stick", "r3"]),
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
