//! Virtual parts on a simulated board.
//!
//! A circuit is the parts placed on one project's board, saved on this computer
//! (`schema/circuit/v1.schema.json`). The format is versioned so the shared project
//! manifest can carry the same data later. [`Parts`] is what the parts do while a
//! simulation runs: it turns pin changes into part states and part actions into pin
//! levels. It is firmware-level: an LED is lit when its pin drives it, with no
//! current, voltage or timing modelled.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "1.0.0";
pub const MAX_PARTS: usize = 32;
pub const MAX_GPIO: u8 = 48;
const MAX_LABEL: usize = 40;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("reading or writing the circuit: {0}")]
    Io(#[from] std::io::Error),
    #[error("the circuit file is not valid: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Lit while its pin is an output at its active level.
    Led,
    /// Drives its pin to its active level while pressed, and to the other level
    /// otherwise, as a pull-up or pull-down resistor would.
    Button,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Part {
    /// Unique within the circuit: lowercase letters, digits and dashes.
    pub id: String,
    pub kind: Kind,
    pub pin: u8,
    /// Active when the pin is low: an LED wired to 3V3, or a button to GND with a
    /// pull-up (the usual `INPUT_PULLUP` wiring).
    #[serde(default)]
    pub active_low: bool,
    /// What a person calls it, e.g. "Status LED".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Circuit {
    pub schema_version: String,
    pub parts: Vec<Part>,
}

impl Default for Circuit {
    fn default() -> Self {
        Circuit {
            schema_version: SCHEMA_VERSION.to_owned(),
            parts: Vec::new(),
        }
    }
}

impl Circuit {
    /// Check ids, pins, labels and limits. Parts may share a pin (an LED and a
    /// button on one GPIO is a real, if odd, circuit), but not two buttons.
    pub fn validate(&self) -> Result<(), Error> {
        let major = self.schema_version.split('.').next().unwrap_or("");
        if major != "1" {
            return Err(Error::Invalid(format!(
                "circuit schema {} is not supported (this program reads 1.x)",
                self.schema_version
            )));
        }
        if self.parts.len() > MAX_PARTS {
            return Err(Error::Invalid(format!("at most {MAX_PARTS} parts")));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut button_pins = std::collections::BTreeSet::new();
        for p in &self.parts {
            if !valid_id(&p.id) {
                return Err(Error::Invalid(format!(
                    "`{}` is not a part id (lowercase letters, digits and dashes, up to 32)",
                    p.id
                )));
            }
            if !ids.insert(p.id.as_str()) {
                return Err(Error::Invalid(format!("two parts are called `{}`", p.id)));
            }
            if p.pin > MAX_GPIO {
                return Err(Error::Invalid(format!("GPIO {} does not exist", p.pin)));
            }
            if p.label.chars().count() > MAX_LABEL || p.label.chars().any(char::is_control) {
                return Err(Error::Invalid(format!(
                    "the label of `{}` is too long or has control characters",
                    p.id
                )));
            }
            if p.kind == Kind::Button && !button_pins.insert(p.pin) {
                return Err(Error::Invalid(format!(
                    "two buttons on GPIO {}: they would fight over the pin",
                    p.pin
                )));
            }
        }
        Ok(())
    }

    /// GPIOs the parts use, sorted.
    pub fn pins(&self) -> Vec<u8> {
        let mut pins: Vec<u8> = self.parts.iter().map(|p| p.pin).collect();
        pins.sort_unstable();
        pins.dedup();
        pins
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Where circuits live: `<root>/<project>/<board>.json`. `project` and `board` must
/// already be checked as safe ids by the caller.
pub fn path(root: &Path, project: &str, board: &str) -> PathBuf {
    root.join(project).join(format!("{board}.json"))
}

/// The saved circuit, or an empty one if there is none.
pub fn load(root: &Path, project: &str, board: &str) -> Result<Circuit, Error> {
    match std::fs::read(path(root, project, board)) {
        Ok(data) => {
            let c: Circuit = serde_json::from_slice(&data)?;
            c.validate()?;
            Ok(c)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Circuit::default()),
        Err(e) => Err(e.into()),
    }
}

/// Save a circuit (validated first), replacing the file in one step.
pub fn save(root: &Path, project: &str, board: &str, circuit: &Circuit) -> Result<(), Error> {
    circuit.validate()?;
    let file = path(root, project, board);
    let dir = file
        .parent()
        .ok_or_else(|| Error::Invalid("bad path".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{board}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(circuit)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// A part's state while a simulation runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartState {
    pub id: String,
    /// LED lit, or button pressed.
    pub on: bool,
}

/// The parts of one running simulation.
pub struct Parts {
    parts: Vec<Part>,
    /// gpio -> (level, output) as the firmware last set them.
    pins: BTreeMap<u8, (Option<bool>, Option<bool>)>,
    pressed: BTreeMap<String, bool>,
}

impl Parts {
    pub fn new(circuit: &Circuit) -> Self {
        Parts {
            parts: circuit.parts.clone(),
            pins: BTreeMap::new(),
            pressed: BTreeMap::new(),
        }
    }

    /// Levels to drive when the simulation starts: every button released.
    pub fn idle_levels(&self) -> Vec<(u8, bool)> {
        self.parts
            .iter()
            .filter(|p| p.kind == Kind::Button)
            .map(|p| (p.pin, p.active_low))
            .collect()
    }

    fn state(&self, p: &Part) -> PartState {
        let on = match p.kind {
            Kind::Led => {
                let (level, output) = self.pins.get(&p.pin).copied().unwrap_or_default();
                output == Some(true) && level == Some(!p.active_low)
            }
            Kind::Button => self.pressed.get(&p.id).copied().unwrap_or(false),
        };
        PartState {
            id: p.id.clone(),
            on,
        }
    }

    /// Every part's state.
    pub fn states(&self) -> Vec<PartState> {
        self.parts.iter().map(|p| self.state(p)).collect()
    }

    /// The firmware changed a pin: the parts on it whose state changed.
    pub fn on_pin(
        &mut self,
        gpio: u8,
        level: Option<bool>,
        output: Option<bool>,
    ) -> Vec<PartState> {
        let before: Vec<PartState> = self.on_gpio(gpio).map(|p| self.state(p)).collect();
        let e = self.pins.entry(gpio).or_default();
        if level.is_some() {
            e.0 = level;
        }
        if output.is_some() {
            e.1 = output;
        }
        self.on_gpio(gpio)
            .map(|p| self.state(p))
            .filter(|s| !before.contains(s))
            .collect()
    }

    fn on_gpio(&self, gpio: u8) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(move |p| p.pin == gpio)
    }

    /// Press or release button `id`: the pin level to drive, and its new state.
    pub fn press(&mut self, id: &str, pressed: bool) -> Result<((u8, bool), PartState), Error> {
        let part = self
            .parts
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::Invalid(format!("no part `{id}`")))?;
        if part.kind != Kind::Button {
            return Err(Error::Invalid(format!("`{id}` is not a button")));
        }
        let level = pressed != part.active_low;
        let pin = part.pin;
        self.pressed.insert(id.to_owned(), pressed);
        let state = PartState {
            id: id.to_owned(),
            on: pressed,
        };
        Ok(((pin, level), state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(id: &str, kind: Kind, pin: u8, active_low: bool) -> Part {
        Part {
            id: id.into(),
            kind,
            pin,
            active_low,
            label: String::new(),
        }
    }

    fn circuit(parts: Vec<Part>) -> Circuit {
        Circuit {
            parts,
            ..Circuit::default()
        }
    }

    #[test]
    fn led_is_lit_only_when_driven_at_its_active_level() {
        let c = circuit(vec![
            part("led", Kind::Led, 2, false),
            part("led-low", Kind::Led, 2, true),
        ]);
        let mut p = Parts::new(&c);
        assert!(p.states().iter().all(|s| !s.on));
        // High, but still an input: not driven, so not lit.
        assert!(p.on_pin(2, Some(true), None).is_empty());
        let changed = p.on_pin(2, None, Some(true));
        assert_eq!(
            changed,
            [PartState {
                id: "led".into(),
                on: true
            }]
        );
        let changed = p.on_pin(2, Some(false), None);
        assert_eq!(
            changed,
            [
                PartState {
                    id: "led".into(),
                    on: false
                },
                PartState {
                    id: "led-low".into(),
                    on: true
                }
            ]
        );
        // Other pins do not touch it.
        assert!(p.on_pin(4, Some(true), Some(true)).is_empty());
    }

    #[test]
    fn button_drives_its_pin_like_a_pull_resistor() {
        let c = circuit(vec![
            part("boot", Kind::Button, 0, true),
            part("go", Kind::Button, 4, false),
            part("led", Kind::Led, 2, false),
        ]);
        let mut p = Parts::new(&c);
        // Released: active-low idles high (pull-up), active-high idles low.
        assert_eq!(p.idle_levels(), [(0, true), (4, false)]);
        let ((pin, level), s) = p.press("boot", true).unwrap();
        assert_eq!((pin, level, s.on), (0, false, true));
        let ((_, level), _) = p.press("boot", false).unwrap();
        assert!(level);
        let ((pin, level), _) = p.press("go", true).unwrap();
        assert_eq!((pin, level), (4, true));
        assert!(p.press("led", true).is_err());
        assert!(p.press("nope", true).is_err());
    }

    #[test]
    fn validation_catches_bad_circuits() {
        let ok = circuit(vec![
            part("led1", Kind::Led, 2, false),
            part("btn", Kind::Button, 2, true),
        ]);
        ok.validate().unwrap();
        for bad in [
            circuit(vec![part("Led", Kind::Led, 2, false)]),
            circuit(vec![part("", Kind::Led, 2, false)]),
            circuit(vec![
                part("a", Kind::Led, 2, false),
                part("a", Kind::Led, 3, false),
            ]),
            circuit(vec![part("a", Kind::Led, 49, false)]),
            circuit(vec![
                part("a", Kind::Button, 4, false),
                part("b", Kind::Button, 4, true),
            ]),
            circuit(
                (0..33)
                    .map(|i| part(&format!("p{i}"), Kind::Led, 2, false))
                    .collect(),
            ),
            Circuit {
                schema_version: "2.0.0".into(),
                parts: vec![],
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let mut label = part("a", Kind::Led, 2, false);
        label.label = "two\nlines".into();
        assert!(circuit(vec![label]).validate().is_err());
    }

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(dir.path(), "blink", "esp32").unwrap(),
            Circuit::default()
        );
        let mut c = circuit(vec![part("led", Kind::Led, 2, false)]);
        c.parts[0].label = "Status LED".into();
        save(dir.path(), "blink", "esp32", &c).unwrap();
        assert_eq!(load(dir.path(), "blink", "esp32").unwrap(), c);
        assert_eq!(c.pins(), [2]);
        // Not saved when invalid.
        let bad = circuit(vec![part("x", Kind::Led, 99, false)]);
        assert!(save(dir.path(), "blink", "esp32", &bad).is_err());
        assert_eq!(load(dir.path(), "blink", "esp32").unwrap(), c);
    }

    #[test]
    fn example_matches_the_shared_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schema/circuit");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("v1.schema.json")).unwrap())
                .unwrap();
        let example: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("example.json")).unwrap())
                .unwrap();
        let v = jsonschema::validator_for(&schema).unwrap();
        assert!(v.is_valid(&example));
        // And this program reads it.
        let c: Circuit = serde_json::from_value(example).unwrap();
        c.validate().unwrap();
        // What we write passes the schema too.
        let written = serde_json::to_value(&c).unwrap();
        assert!(v.is_valid(&written));
        // The schema rejects what validate rejects at the format level.
        let bad = serde_json::json!({ "schemaVersion": "1.0.0", "parts": [ { "id": "a", "kind": "motor", "pin": 2 } ] });
        assert!(!v.is_valid(&bad));
    }
}
