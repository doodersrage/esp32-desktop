//! Virtual parts on a simulated board.
//!
//! - [`Circuit`]: the parts placed on one project's board, saved on this computer
//!   (`schema/circuit/v1.schema.json`), versioned so the shared project manifest can
//!   carry the same data later.
//! - [`Model`]: what a kind of part is, as data (`schema/part-model/`); the models that
//!   ship with the program are in `models/`, each a JSON description and a Rhai script.
//! - [`Runtime`]: runs a circuit's parts during a simulation (in
//!   `esp32bench-qemu-host`): pin changes, controls and I2C go in, pin drives, states
//!   and pictures come out. See docs/parts.md.
//!
//! Parts are firmware-level: an LED is lit when its pin drives it, with no current,
//! voltage or timing modelled.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod runtime;
pub use runtime::{Effect, I2cEvent, Runtime};

/// The circuit format this program writes. It reads every 1.x.
pub const SCHEMA_VERSION: &str = "1.2.0";
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

// ---------------------------------------------------------------- models

/// A property a person sets when placing a part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prop {
    #[serde(rename = "type")]
    pub kind: PropKind,
    pub default: serde_json::Value,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropKind {
    Bool,
    Int,
    Number,
}

/// Something a person works while the simulation runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ControlKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlKind {
    /// On while held (a push button): value true, then false.
    Momentary,
    /// Stays where it is put: value true or false.
    Toggle,
    /// A number between min and max (a sensor reading).
    Slider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct I2cSpec {
    /// 7-bit addresses the part can be set to; the first is the default.
    pub addresses: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplaySpec {
    pub width: u32,
    pub height: u32,
}

/// A kind of part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    /// How the app draws it: led, button, switch, rgb-led, display, or box.
    pub look: String,
    /// Named connections to GPIOs.
    #[serde(default)]
    pub pins: Vec<String>,
    /// Pins that may be left unconnected (e.g. an RTC's alarm output).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_pins: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2c: Option<I2cSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplaySpec>,
    #[serde(default)]
    pub props: BTreeMap<String, Prop>,
    #[serde(default)]
    pub controls: Vec<Control>,
    /// The state the app shows, with its starting values.
    #[serde(default)]
    pub state: serde_json::Map<String, serde_json::Value>,
    pub script: String,
}

/// The models built into the program: (description, script).
const BUILT_IN: [(&str, &str); 8] = [
    (
        include_str!("../models/led.json"),
        include_str!("../models/led.rhai"),
    ),
    (
        include_str!("../models/button.json"),
        include_str!("../models/button.rhai"),
    ),
    (
        include_str!("../models/switch.json"),
        include_str!("../models/switch.rhai"),
    ),
    (
        include_str!("../models/rgb-led.json"),
        include_str!("../models/rgb-led.rhai"),
    ),
    (
        include_str!("../models/ssd1306.json"),
        include_str!("../models/ssd1306.rhai"),
    ),
    (
        include_str!("../models/bme280.json"),
        include_str!("../models/bme280.rhai"),
    ),
    (
        include_str!("../models/mpu6050.json"),
        include_str!("../models/mpu6050.rhai"),
    ),
    (
        include_str!("../models/ds3231.json"),
        include_str!("../models/ds3231.rhai"),
    ),
];

/// A model with its script.
#[derive(Debug, Clone)]
pub struct ModelSource {
    pub model: Model,
    pub script: &'static str,
}

/// The built-in models. They are checked by the tests, so this cannot fail at run time.
pub fn models() -> Vec<ModelSource> {
    BUILT_IN
        .iter()
        .filter_map(|(json, script)| {
            serde_json::from_str::<Model>(json)
                .ok()
                .map(|model| ModelSource { model, script })
        })
        .collect()
}

pub fn model(id: &str) -> Option<ModelSource> {
    models().into_iter().find(|m| m.model.id == id)
}

// ---------------------------------------------------------------- circuits

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct I2cPlace {
    pub bus: u8,
    pub address: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Part {
    /// Unique within the circuit: lowercase letters, digits and dashes.
    pub id: String,
    /// The model's id (led, button, ...).
    pub kind: String,
    /// The GPIO for a model with one pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<u8>,
    /// GPIOs by the model's pin names, for models with several pins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2c: Option<I2cPlace>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub props: serde_json::Map<String, serde_json::Value>,
    /// Circuit 1.0's shorthand for `props.activeLow`; still read, never written.
    #[serde(default, skip_serializing)]
    pub active_low: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Where the part's sliders were left (circuit 1.2): applied when a simulation
    /// starts, so a sensor keeps its readings between runs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub controls: BTreeMap<String, f64>,
}

impl Part {
    /// The part's GPIO connections by pin name, with the 1.0 `pin` shorthand applied.
    pub fn connections(&self, model: &Model) -> BTreeMap<String, u8> {
        let mut pins = self.pins.clone();
        if let (Some(gpio), [only]) = (self.pin, model.pins.as_slice()) {
            pins.entry(only.clone()).or_insert(gpio);
        }
        pins
    }

    /// The properties to run with: the model's defaults, then what the part sets.
    pub fn resolved_props(&self, model: &Model) -> serde_json::Map<String, serde_json::Value> {
        let mut props: serde_json::Map<String, serde_json::Value> = model
            .props
            .iter()
            .map(|(k, p)| (k.clone(), p.default.clone()))
            .collect();
        if let Some(a) = self
            .active_low
            .filter(|_| model.props.contains_key("activeLow"))
        {
            props.insert("activeLow".into(), a.into());
        }
        for (k, v) in &self.props {
            props.insert(k.clone(), v.clone());
        }
        props
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Check each part against its model: unique ids, every model pin connected to a
    /// real GPIO, an I2C address the model allows (and no two parts at one), known
    /// properties of the right type, labels. Two parts may watch one pin, but only one
    /// may drive it.
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
        let invalid = |m: String| Err(Error::Invalid(m));
        let mut ids = std::collections::BTreeSet::new();
        let mut driven = BTreeMap::new();
        let mut addresses = std::collections::BTreeSet::new();
        for p in &self.parts {
            if !valid_id(&p.id) {
                return invalid(format!(
                    "`{}` is not a part id (lowercase letters, digits and dashes, up to 32)",
                    p.id
                ));
            }
            if !ids.insert(p.id.as_str()) {
                return invalid(format!("two parts are called `{}`", p.id));
            }
            let Some(src) = model(&p.kind) else {
                return invalid(format!(
                    "`{}` is not a kind of part this program has",
                    p.kind
                ));
            };
            let m = &src.model;
            let pins = p.connections(m);
            for name in &m.pins {
                match pins.get(name) {
                    None if m.optional_pins.contains(name) => {}
                    None => return invalid(format!("`{}` needs its {name} pin connected", p.id)),
                    Some(&g) if g > MAX_GPIO => return invalid(format!("GPIO {g} does not exist")),
                    Some(_) => {}
                }
            }
            if let Some(extra) = pins.keys().find(|k| !m.pins.contains(k)) {
                return invalid(format!("the {} model has no pin called {extra}", m.name));
            }
            // Parts with controls drive their pins.
            if !m.controls.is_empty() {
                for (name, &g) in &pins {
                    if let Some(other) = driven.insert(g, p.id.clone()) {
                        return invalid(format!(
                            "`{}` and `{other}` would both drive GPIO {g} ({name})",
                            p.id
                        ));
                    }
                }
            }
            for (name, &v) in &p.controls {
                let Some(c) = m
                    .controls
                    .iter()
                    .find(|c| &c.name == name && c.kind == ControlKind::Slider)
                else {
                    return invalid(format!("the {} model has no slider called {name}", m.name));
                };
                let (lo, hi) = (c.min.unwrap_or(f64::MIN), c.max.unwrap_or(f64::MAX));
                if !v.is_finite() || v < lo || v > hi {
                    return invalid(format!("`{}`: {name} must be from {lo} to {hi}", p.id));
                }
            }
            match (&m.i2c, &p.i2c) {
                (Some(spec), Some(place)) => {
                    if !spec.addresses.contains(&place.address) {
                        return invalid(format!(
                            "the {} model cannot be at address {:#04x}",
                            m.name, place.address
                        ));
                    }
                    if place.bus > 1 {
                        return invalid(format!("there is no I2C bus {}", place.bus));
                    }
                    if !addresses.insert((place.bus, place.address)) {
                        return invalid(format!(
                            "two parts at address {:#04x} on bus {}",
                            place.address, place.bus
                        ));
                    }
                }
                (Some(_), None) => return invalid(format!("`{}` needs an I2C address", p.id)),
                (None, Some(_)) => {
                    return invalid(format!("the {} model is not an I2C part", m.name));
                }
                (None, None) => {}
            }
            for (k, v) in &p.props {
                let Some(prop) = m.props.get(k) else {
                    return invalid(format!("the {} model has no property {k}", m.name));
                };
                let ok = match prop.kind {
                    PropKind::Bool => v.is_boolean(),
                    PropKind::Int => v.is_i64(),
                    PropKind::Number => v.is_number(),
                };
                let in_range = v.as_f64().is_none_or(|x| {
                    prop.min.is_none_or(|lo| x >= lo) && prop.max.is_none_or(|hi| x <= hi)
                });
                if !ok || !in_range {
                    return invalid(format!("`{}`: {k} is out of range or the wrong type", p.id));
                }
            }
            if p.label.chars().count() > MAX_LABEL || p.label.chars().any(char::is_control) {
                return invalid(format!(
                    "the label of `{}` is too long or has control characters",
                    p.id
                ));
            }
        }
        Ok(())
    }

    /// GPIOs the parts use, sorted.
    pub fn pins(&self) -> Vec<u8> {
        let mut pins: Vec<u8> = self
            .parts
            .iter()
            .filter_map(|p| model(&p.kind).map(|m| p.connections(&m.model)))
            .flat_map(|c| c.into_values())
            .collect();
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

/// Save a circuit (validated first), replacing the file in one step. Written as the
/// current format version.
pub fn save(root: &Path, project: &str, board: &str, circuit: &Circuit) -> Result<(), Error> {
    circuit.validate()?;
    let mut c = circuit.clone();
    c.schema_version = SCHEMA_VERSION.to_owned();
    for p in &mut c.parts {
        // Fold the 1.0 shorthand into props.
        if let Some(a) = p.active_low.take() {
            p.props.entry("activeLow").or_insert(a.into());
        }
    }
    let file = path(root, project, board);
    let dir = file
        .parent()
        .ok_or_else(|| Error::Invalid("bad path".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{board}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&c)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn circuit(parts: serde_json::Value) -> Circuit {
        serde_json::from_value(json!({ "schemaVersion": "1.1.0", "parts": parts })).unwrap()
    }

    #[test]
    fn built_in_models_load_and_match_their_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schema/part-model");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("v1.schema.json")).unwrap())
                .unwrap();
        let v = jsonschema::validator_for(&schema).unwrap();
        assert_eq!(
            models().len(),
            BUILT_IN.len(),
            "a built-in model does not parse"
        );
        for (json, _) in BUILT_IN {
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            assert!(v.is_valid(&value), "{json}");
        }
        for m in models() {
            assert_eq!(format!("{}.rhai", m.model.id), m.model.script);
            for (k, p) in &m.model.props {
                let ok = match p.kind {
                    PropKind::Bool => p.default.is_boolean(),
                    PropKind::Int => p.default.is_i64(),
                    PropKind::Number => p.default.is_number(),
                };
                assert!(ok, "{}: default of {k}", m.model.id);
            }
        }
    }

    #[test]
    fn reads_circuit_1_0() {
        // As milestone 2 wrote it: kind, pin and activeLow.
        let c: Circuit = serde_json::from_value(json!({ "schemaVersion": "1.0.0", "parts": [
            { "id": "led-1", "kind": "led", "pin": 2, "activeLow": false },
            { "id": "go", "kind": "button", "pin": 4, "activeLow": true, "label": "Go" }
        ]}))
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.pins(), [2, 4]);
        let b = model("button").unwrap();
        assert_eq!(c.parts[1].connections(&b.model)["pin"], 4);
        assert_eq!(c.parts[1].resolved_props(&b.model)["activeLow"], true);
        // Without the field, 1.0 meant active high; the model's default keeps that.
        let plain: Circuit = serde_json::from_value(json!({ "schemaVersion": "1.0.0", "parts": [
            { "id": "b", "kind": "button", "pin": 4 } ]}))
        .unwrap();
        assert_eq!(plain.parts[0].resolved_props(&b.model)["activeLow"], false);
    }

    #[test]
    fn validation_checks_parts_against_their_models() {
        circuit(json!([
            { "id": "led-1", "kind": "led", "pin": 2 },
            { "id": "rgb", "kind": "rgb-led", "pins": { "red": 25, "green": 26, "blue": 27 } },
            { "id": "sw", "kind": "switch", "pin": 4, "props": { "activeLow": true } }
        ]))
        .validate()
        .unwrap();
        for bad in [
            json!([{ "id": "Led", "kind": "led", "pin": 2 }]),
            json!([{ "id": "a", "kind": "motor", "pin": 2 }]),
            json!([{ "id": "a", "kind": "led" }]),
            json!([{ "id": "a", "kind": "led", "pin": 49 }]),
            json!([{ "id": "a", "kind": "rgb-led", "pins": { "red": 25, "green": 26 } }]),
            json!([{ "id": "a", "kind": "led", "pins": { "anode": 2, "cathode": 3 } }]),
            json!([{ "id": "a", "kind": "button", "pin": 4 }, { "id": "b", "kind": "switch", "pin": 4 }]),
            json!([{ "id": "a", "kind": "led", "pin": 2 }, { "id": "a", "kind": "led", "pin": 3 }]),
            json!([{ "id": "a", "kind": "led", "pin": 2, "props": { "brightness": 3 } }]),
            json!([{ "id": "a", "kind": "led", "pin": 2, "props": { "activeLow": 1 } }]),
            json!([{ "id": "a", "kind": "led", "pin": 2, "i2c": { "bus": 0, "address": 60 } }]),
            json!([{ "id": "a", "kind": "led", "pin": 2, "label": "two\nlines" }]),
        ] {
            assert!(circuit(bad.clone()).validate().is_err(), "{bad}");
        }
        // An LED may watch a button's pin.
        circuit(json!([{ "id": "b", "kind": "button", "pin": 4 }, { "id": "l", "kind": "led", "pin": 4 }]))
            .validate()
            .unwrap();
    }

    #[test]
    fn controls_must_be_sliders_in_range() {
        let with = |controls: serde_json::Value| -> Result<(), Error> {
            let c: Circuit = serde_json::from_value(json!({ "schemaVersion": "1.2.0", "parts": [
                { "id": "air", "kind": "bme280", "i2c": { "bus": 0, "address": 118 }, "controls": controls },
                { "id": "go", "kind": "button", "pin": 4 }
            ] }))
            .unwrap();
            c.validate()
        };
        with(json!({ "temperature": -12.25, "humidity": 100 })).unwrap();
        let err = |v| with(v).unwrap_err().to_string();
        assert!(err(json!({ "wind": 3 })).contains("no slider called wind"));
        assert!(err(json!({ "temperature": 200 })).contains("from -40 to 85"));
        // A button's press is momentary, not a slider: not something to remember.
        let c: Circuit = serde_json::from_value(json!({ "schemaVersion": "1.2.0", "parts": [
            { "id": "go", "kind": "button", "pin": 4, "controls": { "press": 1 } }
        ] }))
        .unwrap();
        assert!(
            c.validate()
                .unwrap_err()
                .to_string()
                .contains("no slider called press")
        );
    }

    #[test]
    fn saves_as_the_current_version_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(dir.path(), "blink", "esp32").unwrap(),
            Circuit::default()
        );
        let mut c: Circuit = serde_json::from_value(json!({ "schemaVersion": "1.0.0", "parts": [
            { "id": "led-1", "kind": "led", "pin": 2, "activeLow": true, "label": "Status LED" }
        ]}))
        .unwrap();
        save(dir.path(), "blink", "esp32", &c).unwrap();
        let back = load(dir.path(), "blink", "esp32").unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.parts[0].props["activeLow"], true);
        assert_eq!(back.parts[0].active_low, None);
        // Not saved when invalid.
        c.parts[0].pin = Some(99);
        assert!(save(dir.path(), "blink", "esp32", &c).is_err());
        assert_eq!(load(dir.path(), "blink", "esp32").unwrap(), back);
    }

    #[test]
    fn example_matches_the_shared_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schema/circuit");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("v1.schema.json")).unwrap())
                .unwrap();
        let v = jsonschema::validator_for(&schema).unwrap();
        for file in ["example.json", "example-1.0.json"] {
            let example: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(root.join(file)).unwrap()).unwrap();
            assert!(v.is_valid(&example), "{file}");
            let c: Circuit = serde_json::from_value(example).unwrap();
            c.validate().unwrap();
            assert!(v.is_valid(&serde_json::to_value(&c).unwrap()));
        }
    }
}
