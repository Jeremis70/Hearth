//! Shipped controls files, `data/controls/<system>.toml`: each console's controls (with labels
//! and hints for the UI) and Hearth's default mapping for it. `data/controls/pads/<type>.toml`
//! adjust those defaults for a controller type (PlayStation Create is Capture, …).

use serde::Deserialize;

use super::device::ControllerKind;
use super::model::{HearthAction, PadInput};
use crate::systems::{ConsoleControl, ConsoleControls};

const SHIPPED: &[(&str, &str)] = &[
    ("gc", include_str!("../../../../data/controls/gc.toml")),
    ("n64", include_str!("../../../../data/controls/n64.toml")),
];

const PADS: &[(&str, &str)] = &[
    (
        "playstation",
        include_str!("../../../../data/controls/pads/playstation.toml"),
    ),
    (
        "xbox",
        include_str!("../../../../data/controls/pads/xbox.toml"),
    ),
    (
        "switch",
        include_str!("../../../../data/controls/pads/switch.toml"),
    ),
];

/// The defaults file for a controller type (`data/controls/pads/<name>.toml`): its name and
/// text, adapted to `controls`.
pub fn pad_defaults(
    kind: ControllerKind,
    controls: &ConsoleControls,
) -> Option<(&'static str, String)> {
    let name = match kind {
        ControllerKind::DualSense | ControllerKind::DualShock4 => "playstation",
        ControllerKind::Xbox => "xbox",
        ControllerKind::SwitchPro => "switch",
        ControllerKind::Generic => return None,
    };
    let (_, text) = PADS.iter().find(|(n, _)| *n == name)?;
    Some((name, for_console(text, controls)))
}

/// The Hearth actions a controller type's defaults put on its buttons (PlayStation Create is
/// Capture): what those buttons do in Hearth's own menus, where no console mapping applies.
pub fn pad_hearth_binds(kind: ControllerKind) -> Vec<(PadInput, HearthAction)> {
    let Some((_, text)) = pad_defaults(kind, &ConsoleControls::default()) else {
        return Vec::new();
    };
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(bind) = doc
        .get("player1")
        .and_then(|p| p.get("bind"))
        .and_then(toml::Value::as_table)
    else {
        return Vec::new();
    };
    bind.iter()
        .filter_map(|(input, value)| {
            let action = value.as_str()?.strip_prefix('@')?;
            Some((
                PadInput::from_name(input)?,
                HearthAction::from_name(action)?,
            ))
        })
        .collect()
}

/// A pad defaults file without the binds to controls this console doesn't have (`touchpad =
/// "Select"` on a console without Select): those are meant for other consoles, not errors.
fn for_console(text: &str, controls: &ConsoleControls) -> String {
    let Ok(mut doc) = text.parse::<toml::Table>() else {
        // Invalid shipped file: let the layer parser report it.
        return text.to_owned();
    };
    if let Some(bind) = doc
        .get_mut("player1")
        .and_then(|p| p.get_mut("bind"))
        .and_then(toml::Value::as_table_mut)
    {
        bind.retain(|_, value| match value.as_str() {
            Some(v) => v == "none" || v.starts_with('@') || controls.find(v).is_some(),
            None => true,
        });
    }
    toml::to_string(&doc).unwrap_or_else(|_| text.to_owned())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::ControlKind;

    fn control(name: &str) -> ConsoleControl {
        ConsoleControl {
            name: name.into(),
            kind: ControlKind::Button,
            label: name.into(),
            hint: None,
        }
    }

    #[test]
    fn pad_defaults_keep_only_this_consoles_controls() {
        let with_select = ConsoleControls::new(vec![control("A"), control("Select")]);
        let (name, text) = pad_defaults(ControllerKind::DualSense, &with_select).unwrap();
        assert_eq!(name, "playstation");
        assert!(text.contains("touchpad = \"Select\""), "{text}");
        assert!(text.contains("select = \"@capture\""), "{text}");
        let without = ConsoleControls::new(vec![control("A")]);
        let (_, text) = pad_defaults(ControllerKind::DualShock4, &without).unwrap();
        assert!(!text.contains("touchpad"), "{text}");
        assert!(text.contains("select = \"@capture\""), "{text}");
        assert!(pad_defaults(ControllerKind::Generic, &without).is_none());
        assert_eq!(
            pad_hearth_binds(ControllerKind::DualSense),
            [(PadInput::Select, HearthAction::Capture)]
        );
        assert_eq!(
            pad_hearth_binds(ControllerKind::Xbox),
            [(PadInput::Share, HearthAction::Capture)]
        );
        assert!(pad_hearth_binds(ControllerKind::Generic).is_empty());
        let (name, text) = pad_defaults(ControllerKind::SwitchPro, &without).unwrap();
        assert_eq!(name, "switch");
        assert!(text.contains("share = \"@capture\""), "{text}");
    }
}
