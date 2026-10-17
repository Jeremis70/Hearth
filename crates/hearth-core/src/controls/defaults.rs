//! Shipped controls files, `data/controls/<system>.toml`: each console's controls (with labels
//! and hints for the UI) and Hearth's default mapping for it.

use serde::Deserialize;

use crate::systems::{ConsoleControl, ConsoleControls};

const SHIPPED: &[(&str, &str)] = &[
    ("gc", include_str!("../../../../data/controls/gc.toml")),
    ("n64", include_str!("../../../../data/controls/n64.toml")),
];

/// The shipped controls file of `system`, if there is one.
pub fn shipped(system: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(id, _)| *id == system)
        .map(|(_, text)| *text)
}

#[derive(Deserialize)]
struct ConsoleDoc {
    #[serde(default)]
    console: Vec<ConsoleControl>,
}

/// The `[[console]]` declarations of a shipped controls file.
pub fn console_controls(text: &str) -> Result<ConsoleControls, toml::de::Error> {
    #[derive(Deserialize)]
    struct Loose {
        #[serde(flatten)]
        doc: ConsoleDoc,
        #[serde(flatten)]
        _rest: toml::Table,
    }
    let loose: Loose = toml::from_str(text)?;
    Ok(ConsoleControls::new(loose.doc.console))
}
