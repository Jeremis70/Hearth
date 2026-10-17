//! Where each Hearth pad input sits on the controller drawing (`assets/controllers/`), for the
//! controls page's callouts. Loaded from data so another outline can be dropped in.

use std::collections::BTreeMap;

use hearth_core::controls::PadInput;
use serde::Deserialize;

const ANCHORS: &str = include_str!("../../../assets/controllers/generic.toml");

/// Which callout column an input is listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Left of the drawing.
    Left,
    /// Right of the drawing.
    Right,
    /// Below it.
    Bottom,
}

/// One input's anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// The input.
    pub input: PadInput,
    /// Point in drawing units.
    pub x: f32,
    /// Point in drawing units.
    pub y: f32,
    /// Column.
    pub side: Side,
    /// Row in its column.
    pub order: u32,
}

/// The drawing's size and anchors.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Drawing width in its units.
    pub width: f32,
    /// Drawing height in its units.
    pub height: f32,
    /// Anchors sorted by side, then order.
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
    side: Side,
    order: u32,
}

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
                side: a.side,
                order: a.order,
            });
        }
        anchors.sort_by_key(|a| (a.side, a.order));
        Ok(Self {
            width: doc.width,
            height: doc.height,
            anchors,
        })
    }

    /// The anchors of one column, top to bottom.
    pub fn column(&self, side: Side) -> impl Iterator<Item = &Anchor> {
        self.anchors.iter().filter(move |a| a.side == side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
        let mut slots: Vec<_> = layout.anchors.iter().map(|a| (a.side, a.order)).collect();
        slots.dedup();
        assert_eq!(slots.len(), layout.anchors.len(), "two inputs share a row");
        assert_eq!(layout.column(Side::Left).count(), 8);
        assert_eq!(layout.column(Side::Right).count(), 8);
    }

    #[test]
    fn svg_size_matches_anchor_units() {
        let svg = include_str!("../../../assets/controllers/generic.svg");
        let layout = Layout::generic();
        assert!(svg.contains(&format!(
            "viewBox=\"0 0 {} {}\"",
            layout.width, layout.height
        )));
    }
}
