//! Running a circuit's parts: the Rhai side of docs/parts.md.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rhai::{AST, CallFnOptions, Dynamic, Engine, Scope};

use crate::{Circuit, Error, model};

/// Per hook call: enough for a sensor's arithmetic or a display's byte, not a loop
/// that never ends.
const MAX_OPERATIONS: u64 = 200_000;
/// A display picture is the largest value a script builds.
const MAX_BLOB: usize = 128 * 1024;

/// What running the parts asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Drive a GPIO as an input.
    Drive { gpio: u8, level: bool },
    /// A part's state changed; the whole state.
    State {
        id: String,
        state: serde_json::Value,
    },
    /// A display part's picture: one byte per pixel (0 off, 1 on), row by row.
    Picture {
        id: String,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// A part's script failed. The part keeps its last state.
    Error { id: String, message: String },
}

/// One I2C bus event, as the host sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cEvent {
    Start { read: bool },
    Write(u8),
    Read,
    Stop,
}

struct Instance {
    id: String,
    ast: Arc<AST>,
    hooks: BTreeSet<String>,
    /// `this` in the script: `#{ props: #{..}, state: #{..} }`.
    this: Dynamic,
    /// pin name -> GPIO.
    pins: BTreeMap<String, u8>,
    i2c: Option<(u8, u8)>,
    display: Option<(u32, u32)>,
    last_state: serde_json::Value,
}

/// What hook calls write to: the part being called, and the effects so far.
#[derive(Default)]
struct Context {
    pins: BTreeMap<String, u8>,
    display: Option<(u32, u32)>,
    id: String,
    effects: Vec<Effect>,
}

/// A circuit's parts while a simulation runs.
pub struct Runtime {
    engine: Engine,
    parts: Vec<Instance>,
    ctx: Arc<Mutex<Context>>,
    /// gpio -> (level, output), as the firmware last set them.
    pins: BTreeMap<u8, (bool, bool)>,
}

fn lock(ctx: &Mutex<Context>) -> std::sync::MutexGuard<'_, Context> {
    ctx.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Runtime {
    /// Compile the circuit's parts (the circuit must be valid).
    pub fn new(circuit: &Circuit) -> Result<Self, Error> {
        circuit.validate()?;
        let ctx = Arc::new(Mutex::new(Context::default()));
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(16);
        engine.set_max_expr_depths(64, 32);
        engine.set_max_string_size(4096);
        engine.set_max_array_size(MAX_BLOB);
        engine.set_max_map_size(256);
        // No `import`: scripts cannot load anything.
        engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
        let c = ctx.clone();
        engine.register_fn("drive", move |name: &str, level: bool| {
            let mut cx = lock(&c);
            if let Some(&gpio) = cx.pins.get(name) {
                cx.effects.push(Effect::Drive { gpio, level });
            }
        });
        let c = ctx.clone();
        engine.register_fn("show", move |pixels: rhai::Blob| {
            let mut cx = lock(&c);
            if let Some((width, height)) = cx.display {
                let id = cx.id.clone();
                let n = (width * height) as usize;
                let mut pixels = pixels;
                pixels.resize(n, 0);
                cx.effects.push(Effect::Picture {
                    id,
                    width,
                    height,
                    pixels,
                });
            }
        });

        let mut compiled: BTreeMap<String, Arc<AST>> = BTreeMap::new();
        let mut parts = Vec::new();
        for p in &circuit.parts {
            let src =
                model(&p.kind).ok_or_else(|| Error::Invalid(format!("no model {}", p.kind)))?;
            let ast = match compiled.get(&p.kind) {
                Some(a) => a.clone(),
                None => {
                    let a = Arc::new(engine.compile(src.script).map_err(|e| {
                        Error::Invalid(format!("the {} model's script: {e}", p.kind))
                    })?);
                    compiled.insert(p.kind.clone(), a.clone());
                    a
                }
            };
            let hooks = ast.iter_functions().map(|f| f.name.to_owned()).collect();
            let props = rhai::serde::to_dynamic(p.resolved_props(&src.model))
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let state_json = serde_json::Value::Object(src.model.state.clone());
            let state =
                rhai::serde::to_dynamic(&state_json).map_err(|e| Error::Invalid(e.to_string()))?;
            let mut this = rhai::Map::new();
            this.insert("props".into(), props);
            this.insert("state".into(), state);
            parts.push(Instance {
                id: p.id.clone(),
                ast,
                hooks,
                this: Dynamic::from_map(this),
                pins: p.connections(&src.model),
                i2c: p.i2c.as_ref().map(|i| (i.bus, i.address)),
                display: src.model.display.as_ref().map(|d| (d.width, d.height)),
                last_state: state_json,
            });
        }
        Ok(Runtime {
            engine,
            parts,
            ctx,
            pins: BTreeMap::new(),
        })
    }

    /// Call `hook` on part `i` if it has one; returns its result.
    fn call(&mut self, i: usize, hook: &str, args: impl rhai::FuncArgs) -> Option<Dynamic> {
        let part = &mut self.parts[i];
        if !part.hooks.contains(hook) {
            return None;
        }
        {
            let mut cx = lock(&self.ctx);
            cx.pins = part.pins.clone();
            cx.display = part.display;
            cx.id = part.id.clone();
        }
        let options = CallFnOptions::new()
            .eval_ast(false)
            .rewind_scope(true)
            .bind_this_ptr(&mut part.this);
        let r = self.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut Scope::new(),
            &part.ast,
            hook,
            args,
        );
        let mut cx = lock(&self.ctx);
        // A changed state goes out whole.
        let state = part
            .this
            .read_lock::<rhai::Map>()
            .and_then(|m| m.get("state").cloned());
        if let Some(state) = state
            && let Ok(json) = rhai::serde::from_dynamic::<serde_json::Value>(&state)
            && json != part.last_state
        {
            part.last_state = json.clone();
            cx.effects.push(Effect::State {
                id: part.id.clone(),
                state: json,
            });
        }
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                cx.effects.push(Effect::Error {
                    id: part.id.clone(),
                    message: e.to_string(),
                });
                None
            }
        }
    }

    fn take(&mut self) -> Vec<Effect> {
        std::mem::take(&mut lock(&self.ctx).effects)
    }

    /// Every part's state, for a client that has just connected.
    pub fn states(&self) -> Vec<Effect> {
        self.parts
            .iter()
            .map(|p| Effect::State {
                id: p.id.clone(),
                state: p.last_state.clone(),
            })
            .collect()
    }

    /// GPIOs the parts are connected to.
    pub fn pins(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self
            .parts
            .iter()
            .flat_map(|p| p.pins.values().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Run every part's `init`.
    pub fn init(&mut self) -> Vec<Effect> {
        for i in 0..self.parts.len() {
            self.call(i, "init", ());
        }
        self.take()
    }

    /// The firmware changed a GPIO's level or direction.
    pub fn pin(&mut self, gpio: u8, level: Option<bool>, output: Option<bool>) -> Vec<Effect> {
        let e = self.pins.entry(gpio).or_insert((false, false));
        if let Some(l) = level {
            e.0 = l;
        }
        if let Some(o) = output {
            e.1 = o;
        }
        let (level, output) = *e;
        for i in 0..self.parts.len() {
            let names: Vec<String> = self.parts[i]
                .pins
                .iter()
                .filter(|(_, g)| **g == gpio)
                .map(|(n, _)| n.clone())
                .collect();
            for name in names {
                self.call(i, "pin", (name, level, output));
            }
        }
        self.take()
    }

    /// A person worked a control: `value` is a bool for buttons and switches, a number
    /// for sliders.
    pub fn control(&mut self, id: &str, name: &str, value: Dynamic) -> Result<Vec<Effect>, Error> {
        let i = self
            .parts
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| Error::Invalid(format!("no part `{id}`")))?;
        if !self.parts[i].hooks.contains("control") {
            return Err(Error::Invalid(format!("`{id}` has nothing to work")));
        }
        self.call(i, "control", (name.to_owned(), value));
        Ok(self.take())
    }

    /// Whether a part sits at this I2C address.
    pub fn has_i2c(&self, bus: u8, address: u8) -> bool {
        self.parts.iter().any(|p| p.i2c == Some((bus, address)))
    }

    /// An I2C event for the part at (bus, address): the answer for QEMU (0 ACK, 1 NACK,
    /// or the byte read) and the effects. None if no part is there.
    pub fn i2c(&mut self, bus: u8, address: u8, event: I2cEvent) -> Option<(i32, Vec<Effect>)> {
        let i = self
            .parts
            .iter()
            .position(|p| p.i2c == Some((bus, address)))?;
        let answer = match event {
            I2cEvent::Start { read } => {
                let ack = self
                    .call(i, "i2c_start", (read,))
                    .map(|v| v.as_bool().unwrap_or(true));
                i32::from(!ack.unwrap_or(true))
            }
            I2cEvent::Write(b) => {
                let ack = self
                    .call(i, "i2c_write", (i64::from(b),))
                    .map(|v| v.as_bool().unwrap_or(true));
                i32::from(!ack.unwrap_or(true))
            }
            I2cEvent::Read => self
                .call(i, "i2c_read", ())
                .and_then(|v| v.as_int().ok())
                .map_or(0xff, |b| (b & 0xff) as i32),
            I2cEvent::Stop => {
                self.call(i, "i2c_stop", ());
                0
            }
        };
        Some((answer, self.take()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn runtime(parts: serde_json::Value) -> Runtime {
        let c: Circuit =
            serde_json::from_value(json!({ "schemaVersion": "1.1.0", "parts": parts })).unwrap();
        Runtime::new(&c).unwrap()
    }

    fn state(effects: &[Effect], id: &str) -> Option<serde_json::Value> {
        effects.iter().rev().find_map(|e| match e {
            Effect::State { id: i, state } if i == id => Some(state.clone()),
            _ => None,
        })
    }

    #[test]
    fn led_lights_only_when_driven_at_its_active_level() {
        let mut r = runtime(json!([
            { "id": "led", "kind": "led", "pin": 2 },
            { "id": "low", "kind": "led", "pin": 2, "props": { "activeLow": true } }
        ]));
        assert!(r.init().is_empty());
        // High but still an input: not lit.
        assert!(r.pin(2, Some(true), None).is_empty());
        let e = r.pin(2, None, Some(true));
        assert_eq!(state(&e, "led"), Some(json!({ "on": true })));
        assert_eq!(state(&e, "low"), None); // unchanged: still off
        let e = r.pin(2, Some(false), None);
        assert_eq!(state(&e, "led"), Some(json!({ "on": false })));
        assert_eq!(state(&e, "low"), Some(json!({ "on": true })));
        assert!(r.pin(4, Some(true), Some(true)).is_empty());
    }

    #[test]
    fn button_and_switch_drive_their_pins() {
        let mut r = runtime(json!([
            { "id": "boot", "kind": "button", "pin": 0, "props": { "activeLow": true } },
            { "id": "go", "kind": "button", "pin": 4, "props": { "activeLow": false } },
            { "id": "sw", "kind": "switch", "pin": 5 }
        ]));
        // Released buttons hold their idle level; the switch starts off.
        let e = r.init();
        assert!(e.contains(&Effect::Drive {
            gpio: 0,
            level: true
        }));
        assert!(e.contains(&Effect::Drive {
            gpio: 4,
            level: false
        }));
        assert!(e.contains(&Effect::Drive {
            gpio: 5,
            level: false
        }));
        let e = r.control("boot", "press", Dynamic::from(true)).unwrap();
        assert!(e.contains(&Effect::Drive {
            gpio: 0,
            level: false
        }));
        assert_eq!(state(&e, "boot"), Some(json!({ "on": true })));
        let e = r.control("sw", "toggle", Dynamic::from(true)).unwrap();
        assert!(e.contains(&Effect::Drive {
            gpio: 5,
            level: true
        }));
        assert!(r.control("nope", "press", Dynamic::from(true)).is_err());
    }

    #[test]
    fn rgb_led_lights_each_colour() {
        let mut r = runtime(json!([
            { "id": "rgb", "kind": "rgb-led", "pins": { "red": 25, "green": 26, "blue": 27 } }
        ]));
        r.pin(25, Some(true), Some(true));
        let e = r.pin(27, Some(true), Some(true));
        assert_eq!(
            state(&e, "rgb"),
            Some(json!({ "red": true, "green": false, "blue": true }))
        );
        // An LED part has nothing to work.
        assert!(r.control("rgb", "press", Dynamic::from(true)).is_err());
    }

    #[test]
    fn scripts_cannot_run_forever_or_reach_out() {
        let engine_check = |script: &str| {
            let mut e = Engine::new();
            e.set_max_operations(MAX_OPERATIONS);
            e.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
            e.run(script)
        };
        assert!(engine_check("loop {}").is_err());
        assert!(engine_check(r#"import "fs" as fs;"#).is_err());
        // No file or process functions exist to call.
        assert!(engine_check(r#"open_file("/etc/passwd")"#).is_err());
    }
}
