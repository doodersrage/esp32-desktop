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

/// Display RAM in pages (each byte a column of 8 pixels, bit 0 on top, as SSD1306 and
/// most monochrome controllers store it) to one byte per pixel, row by row. Done in
/// Rust: it touches every pixel, too much for a script on every frame.
/// `seg_remap` and `com_reverse` are the controller's flags: a module is wired so that
/// with both set the picture is upright, so each one cleared mirrors it, as on hardware.
fn pages_to_pixels(
    ram: rhai::Blob,
    width: i64,
    height: i64,
    seg_remap: bool,
    com_reverse: bool,
    invert: bool,
    all_on: bool,
) -> rhai::Blob {
    let (w, h) = (width.clamp(1, 512) as usize, height.clamp(1, 512) as usize);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let sy = if com_reverse { y } else { h - 1 - y };
        for x in 0..w {
            let sx = if seg_remap { x } else { w - 1 - x };
            let byte = ram.get((sy / 8) * w + sx).copied().unwrap_or(0);
            let lit = all_on || (byte >> (sy % 8)) & 1 == 1;
            out[y * w + x] = u8::from(lit != invert);
        }
    }
    out
}

/// What running the parts asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Drive a GPIO as an input.
    Drive { gpio: u8, level: bool },
    /// Put a voltage on a GPIO's analog input, in volts (0 to 3.3). The host turns it
    /// into the 12-bit reading the chip's ADC would give.
    Analog { id: String, gpio: u8, volts: f64 },
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
    /// Saved slider values, applied after `init`.
    controls: Vec<(String, f64)>,
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

/// When the runtime started, for `millis()`.
fn started() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
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
        engine.register_fn("analog", move |name: &str, volts: f64| {
            let mut cx = lock(&c);
            if let Some(&gpio) = cx.pins.get(name) {
                // Nothing on the board goes outside its 3.3 V supply.
                let volts = if volts.is_finite() {
                    volts.clamp(0.0, 3.3)
                } else {
                    0.0
                };
                let id = cx.id.clone();
                cx.effects.push(Effect::Analog { id, gpio, volts });
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

        engine.register_fn("pages_to_pixels", pages_to_pixels);
        // Clocks: milliseconds of host time since the simulation started (QEMU's
        // virtual time follows the host's), and the computer's clock in Unix seconds,
        // UTC (an RTC that was set and kept its battery).
        let start = started();
        engine.register_fn("millis", move || -> i64 {
            i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
        });
        engine.register_fn("wall_time", || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
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
                controls: p.controls.iter().map(|(k, v)| (k.clone(), *v)).collect(),
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

    /// Run every part's `tick`, for parts that keep time (an RTC's alarm). The host
    /// calls this about every 100 ms.
    pub fn tick(&mut self) -> Vec<Effect> {
        for i in 0..self.parts.len() {
            self.call(i, "tick", ());
        }
        self.take()
    }

    /// Whether any part has a `tick` hook.
    pub fn has_tick(&self) -> bool {
        self.parts.iter().any(|p| p.hooks.contains("tick"))
    }

    /// Run every part's `init`.
    pub fn init(&mut self) -> Vec<Effect> {
        for i in 0..self.parts.len() {
            self.call(i, "init", ());
            // Then where its sliders were left, as if a person had set them.
            for (name, v) in self.parts[i].controls.clone() {
                self.call(i, "control", (name, Dynamic::from(v)));
            }
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

    /// The firmware sent a WS2812 frame on a GPIO (RMT, decoded by the host): the
    /// bytes in the order they went out. Runs the `rmt(name, bytes)` hook of parts on
    /// that pin; `bytes` is a blob.
    pub fn rmt(&mut self, gpio: u8, bytes: Vec<u8>) -> Vec<Effect> {
        for i in 0..self.parts.len() {
            let names: Vec<String> = self.parts[i]
                .pins
                .iter()
                .filter(|(_, g)| **g == gpio)
                .map(|(n, _)| n.clone())
                .collect();
            for name in names {
                self.call(i, "rmt", (name, rhai::Blob::from(bytes.clone())));
            }
        }
        self.take()
    }

    /// Whether any part has an `rmt` hook.
    pub fn has_rmt(&self) -> bool {
        self.parts.iter().any(|p| p.hooks.contains("rmt"))
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
    fn ws2812_strip_takes_its_colours_from_rmt_frames() {
        let mut r = runtime(json!([
            { "id": "strip", "kind": "ws2812", "pin": 16, "props": { "count": 3 } },
            { "id": "rgb", "kind": "ws2812", "pin": 17, "props": { "count": 1, "rgbOrder": true } }
        ]));
        let e = r.init();
        assert_eq!(
            state(&e, "strip"),
            Some(json!({ "leds": [[0, 0, 0], [0, 0, 0], [0, 0, 0]] }))
        );
        assert!(r.has_rmt());
        // Green first: LED 0 red, LED 1 blue; the frame stops short of LED 2.
        let e = r.rmt(16, vec![0, 255, 0, 0, 0, 40]);
        assert_eq!(
            state(&e, "strip"),
            Some(json!({ "leds": [[255, 0, 0], [0, 0, 40], [0, 0, 0]] }))
        );
        // Not this strip's pin; and a strip wired red first.
        assert!(state(&r.rmt(4, vec![1, 2, 3]), "strip").is_none());
        let e = r.rmt(17, vec![10, 20, 30]);
        assert_eq!(state(&e, "rgb"), Some(json!({ "leds": [[10, 20, 30]] })));
    }

    /// Feed an I2C write transaction to the part at (0, 0x3c).
    fn write(r: &mut Runtime, bytes: &[u8]) -> Vec<Effect> {
        let mut e = r.i2c(0, 0x3c, I2cEvent::Start { read: false }).unwrap().1;
        for &b in bytes {
            let (ack, more) = r.i2c(0, 0x3c, I2cEvent::Write(b)).unwrap();
            assert_eq!(ack, 0);
            e.extend(more);
        }
        e.extend(r.i2c(0, 0x3c, I2cEvent::Stop).unwrap().1);
        e
    }

    fn picture(effects: &[Effect]) -> Option<Vec<u8>> {
        effects.iter().rev().find_map(|e| match e {
            Effect::Picture { pixels, .. } => Some(pixels.clone()),
            _ => None,
        })
    }

    #[test]
    fn ssd1306_draws_what_adafruit_sends() {
        let mut r = runtime(json!([
            { "id": "oled", "kind": "ssd1306", "i2c": { "bus": 0, "address": 60 } }
        ]));
        assert!(r.has_i2c(0, 0x3c));
        assert!(r.i2c(0, 0x3d, I2cEvent::Start { read: false }).is_none());
        r.init();
        // Adafruit_SSD1306::begin, abridged: off, horizontal mode, segment remap, COM
        // scan reversed, on.
        let e = write(&mut r, &[0x00, 0xAE, 0x20, 0x00, 0xA1, 0xC8, 0xAF]);
        assert_eq!(state(&e, "oled"), Some(json!({ "on": true })));
        // display(): window = whole screen, then data. First page: column 0 all lit,
        // column 1 only its top pixel.
        write(&mut r, &[0x00, 0x22, 0x00, 0x07, 0x21, 0x00, 0x7F]);
        let mut data = vec![0x40, 0xFF, 0x01];
        data.extend(std::iter::repeat_n(0u8, 1022));
        let e = write(&mut r, &data);
        let px = picture(&e).expect("no picture");
        assert_eq!(px.len(), 128 * 64);
        let at = |x: usize, y: usize| px[y * 128 + x];
        assert_eq!((0..8).map(|y| at(0, y)).collect::<Vec<_>>(), [1; 8]);
        assert_eq!((at(1, 0), at(1, 1)), (1, 0));
        assert_eq!(at(0, 8), 0);
        assert_eq!(px.iter().filter(|p| **p == 1).count(), 9);
        // Invert, then display off: blank.
        let e = write(&mut r, &[0x00, 0xA7]);
        assert_eq!(
            picture(&e).unwrap().iter().filter(|p| **p == 1).count(),
            128 * 64 - 9
        );
        let e = write(&mut r, &[0x00, 0xAE]);
        assert!(picture(&e).unwrap().iter().all(|p| *p == 0));
    }

    #[test]
    fn ssd1306_page_mode_and_mirroring() {
        let mut r = runtime(json!([
            { "id": "oled", "kind": "ssd1306", "i2c": { "bus": 0, "address": 60 } }
        ]));
        r.init();
        // Reset defaults (page mode, no remap): u8g2-style page 1, column 3.
        write(&mut r, &[0x00, 0xAF, 0xB1, 0x03, 0x10]);
        let e = write(&mut r, &[0x40, 0x01]);
        let px = picture(&e).unwrap();
        // Without A1/C8 the picture is mirrored both ways, as on a real module.
        assert_eq!(px[(63 - 8) * 128 + (127 - 3)], 1);
        assert_eq!(px.iter().filter(|p| **p == 1).count(), 1);
    }

    /// Read `n` registers from `reg` of the part at (0, addr), as Wire does: a write of
    /// the register pointer, then a read.
    fn read_regs(r: &mut Runtime, addr: u8, reg: u8, n: usize) -> Vec<u8> {
        r.i2c(0, addr, I2cEvent::Start { read: false }).unwrap();
        r.i2c(0, addr, I2cEvent::Write(reg)).unwrap();
        r.i2c(0, addr, I2cEvent::Stop).unwrap();
        r.i2c(0, addr, I2cEvent::Start { read: true }).unwrap();
        let out = (0..n)
            .map(|_| r.i2c(0, addr, I2cEvent::Read).unwrap().0 as u8)
            .collect();
        r.i2c(0, addr, I2cEvent::Stop).unwrap();
        out
    }

    fn write_reg(r: &mut Runtime, addr: u8, reg: u8, v: u8) {
        r.i2c(0, addr, I2cEvent::Start { read: false }).unwrap();
        r.i2c(0, addr, I2cEvent::Write(reg)).unwrap();
        r.i2c(0, addr, I2cEvent::Write(v)).unwrap();
        r.i2c(0, addr, I2cEvent::Stop).unwrap();
    }

    /// What firmware computes from the registers: Bosch's compensation (datasheet
    /// 4.2.3), written here independently of the model's script.
    fn bme280_readings(r: &mut Runtime, addr: u8) -> (f64, f64, f64) {
        let c = read_regs(r, addr, 0x88, 24);
        let u = |i: usize| i64::from(u16::from_le_bytes([c[i], c[i + 1]]));
        let s = |i: usize| i64::from(i16::from_le_bytes([c[i], c[i + 1]]));
        let (t1, t2, t3) = (u(0), s(2), s(4));
        let (p1, p2, p3, p4, p5, p6, p7, p8, p9) =
            (u(6), s(8), s(10), s(12), s(14), s(16), s(18), s(20), s(22));
        let h1 = i64::from(read_regs(r, addr, 0xA1, 1)[0]);
        let e = read_regs(r, addr, 0xE1, 7);
        let h2 = i64::from(i16::from_le_bytes([e[0], e[1]]));
        let h3 = i64::from(e[2]);
        let h4 = (i64::from(e[3] as i8) << 4) | i64::from(e[4] & 0x0F);
        let h5 = (i64::from(e[5] as i8) << 4) | i64::from(e[4] >> 4);
        let h6 = i64::from(e[6] as i8);
        let d = read_regs(r, addr, 0xF7, 8);
        let adc_p = (i64::from(d[0]) << 12) | (i64::from(d[1]) << 4) | i64::from(d[2] >> 4);
        let adc_t = (i64::from(d[3]) << 12) | (i64::from(d[4]) << 4) | i64::from(d[5] >> 4);
        let adc_h = (i64::from(d[6]) << 8) | i64::from(d[7]);
        let var1 = (((adc_t >> 3) - (t1 << 1)) * t2) >> 11;
        let var2 = (((((adc_t >> 4) - t1) * ((adc_t >> 4) - t1)) >> 12) * t3) >> 14;
        let t_fine = var1 + var2;
        let t = ((t_fine * 5 + 128) >> 8) as f64 / 100.0;
        let mut v1 = t_fine - 128000;
        let mut v2 = v1 * v1 * p6;
        v2 += (v1 * p5) << 17;
        v2 += p4 << 35;
        v1 = ((v1 * v1 * p3) >> 8) + ((v1 * p2) << 12);
        v1 = (((1i64 << 47) + v1) * p1) >> 33;
        let mut p = 1048576 - adc_p;
        p = (((p << 31) - v2) * 3125) / v1;
        let w1 = (p9 * (p >> 13) * (p >> 13)) >> 25;
        let w2 = (p8 * p) >> 19;
        let p = (((p + w1 + w2) >> 8) + (p7 << 4)) as f64 / 256.0 / 100.0;
        let mut h = t_fine - 76800;
        h = ((((adc_h << 14) - (h4 << 20) - (h5 * h)) + 16384) >> 15)
            * (((((((h * h6) >> 10) * (((h * h3) >> 11) + 32768)) >> 10) + 2097152) * h2 + 8192)
                >> 14);
        h -= ((((h >> 15) * (h >> 15)) >> 7) * h1) >> 4;
        let h = (h.clamp(0, 419430400) >> 12) as f64 / 1024.0;
        (t, h, p)
    }

    #[test]
    fn bme280_reads_back_what_the_sliders_say() {
        let mut r = runtime(json!([
            { "id": "env", "kind": "bme280", "i2c": { "bus": 0, "address": 118 } }
        ]));
        r.init();
        assert_eq!(read_regs(&mut r, 0x76, 0xD0, 1), [0x60], "chip id");
        // Adafruit's begin(): soft reset, then setSampling (x16 everything, normal mode).
        write_reg(&mut r, 0x76, 0xE0, 0xB6);
        write_reg(&mut r, 0x76, 0xF2, 0x05);
        write_reg(&mut r, 0x76, 0xF5, 0x00);
        write_reg(&mut r, 0x76, 0xF4, 0xB7);
        let (t, h, p) = bme280_readings(&mut r, 0x76);
        assert!((t - 21.0).abs() <= 0.01, "{t}");
        assert!((h - 45.0).abs() <= 0.01, "{h}");
        assert!((p - 1013.25).abs() <= 0.01, "{p}");
        // Move the sliders.
        r.control("env", "temperature", Dynamic::from(-12.5_f64))
            .unwrap();
        r.control("env", "humidity", Dynamic::from(80.0_f64))
            .unwrap();
        let e = r
            .control("env", "pressure", Dynamic::from(950.0_f64))
            .unwrap();
        assert_eq!(
            state(&e, "env"),
            Some(json!({ "temperature": -12.5, "humidity": 80.0, "pressure": 950.0 }))
        );
        let (t, h, p) = bme280_readings(&mut r, 0x76);
        assert!((t + 12.5).abs() <= 0.01, "{t}");
        assert!((h - 80.0).abs() <= 0.01, "{h}");
        assert!((p - 950.0).abs() <= 0.01, "{p}");
        // Oversampling 0 skips a measurement: 0x80000, as the datasheet says.
        write_reg(&mut r, 0x76, 0xF4, 0x03);
        let d = read_regs(&mut r, 0x76, 0xF7, 6);
        assert_eq!(d, [0x80, 0x00, 0x00, 0x80, 0x00, 0x00]);
        // Nothing at 0x77, and a control it does not have is an error.
        assert!(!r.has_i2c(0, 0x77));
        let e = r.control("env", "light", Dynamic::from(1.0_f64)).unwrap();
        assert!(e.iter().any(|x| matches!(x, Effect::Error { .. })));
    }

    #[test]
    fn mpu6050_reads_tilt_as_gravity() {
        let mut r = runtime(json!([
            { "id": "imu", "kind": "mpu6050", "i2c": { "bus": 0, "address": 104 } }
        ]));
        r.init();
        assert_eq!(read_regs(&mut r, 0x68, 0x75, 1), [0x68], "WHO_AM_I");
        // Asleep after power-up: outputs read zero.
        assert_eq!(read_regs(&mut r, 0x68, 0x6B, 1), [0x40]);
        assert!(read_regs(&mut r, 0x68, 0x3B, 14).iter().all(|b| *b == 0));
        // Wake it, as the projects do, and read the 14-byte burst.
        write_reg(&mut r, 0x68, 0x6B, 0x00);
        let burst = |r: &mut Runtime| {
            let d = read_regs(r, 0x68, 0x3B, 14);
            (0..7)
                .map(|i| i16::from_be_bytes([d[2 * i], d[2 * i + 1]]))
                .collect::<Vec<i16>>()
        };
        let v = burst(&mut r);
        assert_eq!(&v[..3], [0, 0, 16384], "flat: +1 g on z");
        let temp = f64::from(v[3]) / 340.0 + 36.53;
        assert!((temp - 25.0).abs() < 0.01, "{temp}");
        assert_eq!(&v[4..], [0, 0, 0], "held still");
        // Tilt it, and the usual atan2 formulas give the angles back.
        r.control("imu", "pitch", Dynamic::from(20.0_f64)).unwrap();
        r.control("imu", "roll", Dynamic::from(-35.0_f64)).unwrap();
        let v = burst(&mut r);
        let (ax, ay, az) = (f64::from(v[0]), f64::from(v[1]), f64::from(v[2]));
        let roll = ay.atan2(az).to_degrees();
        let pitch = (-ax).atan2((ay * ay + az * az).sqrt()).to_degrees();
        assert!((roll + 35.0).abs() < 0.05, "{roll}");
        assert!((pitch - 20.0).abs() < 0.05, "{pitch}");
        // The +-8 g range: 4096 counts per g.
        r.control("imu", "pitch", Dynamic::from(0.0_f64)).unwrap();
        r.control("imu", "roll", Dynamic::from(0.0_f64)).unwrap();
        write_reg(&mut r, 0x68, 0x1C, 0x10);
        assert_eq!(burst(&mut r)[2], 4096);
        // Outputs are read-only; DEVICE_RESET puts it back to sleep.
        write_reg(&mut r, 0x68, 0x3F, 0x00);
        assert_eq!(burst(&mut r)[2], 4096);
        write_reg(&mut r, 0x68, 0x6B, 0x80);
        assert_eq!(read_regs(&mut r, 0x68, 0x6B, 1), [0x40]);
        assert!(!r.has_i2c(0, 0x69));
    }

    fn write_regs(r: &mut Runtime, addr: u8, reg: u8, data: &[u8]) -> Vec<Effect> {
        r.i2c(0, addr, I2cEvent::Start { read: false }).unwrap();
        r.i2c(0, addr, I2cEvent::Write(reg)).unwrap();
        for b in data {
            r.i2c(0, addr, I2cEvent::Write(*b)).unwrap();
        }
        r.i2c(0, addr, I2cEvent::Stop).unwrap().1
    }

    #[test]
    fn ds3231_keeps_time_and_fires_alarms() {
        let mut r = runtime(json!([
            { "id": "rtc", "kind": "ds3231", "pins": { "sqw": 4 }, "i2c": { "bus": 0, "address": 104 } }
        ]));
        r.init();
        // Kept its time: the oscillator-stop flag is clear (RTClib's lostPower()).
        assert_eq!(read_regs(&mut r, 0x68, 0x0F, 1)[0] & 0x80, 0);
        // A year that RTClib can show: the computer's clock, so 20xx.
        let y = read_regs(&mut r, 0x68, 0x06, 1)[0];
        assert!(y >> 4 <= 9 && y & 0x0F <= 9, "BCD year {y:#x}");

        // Set 2026-02-28 23:59:59, a Saturday (RTClib writes 6), as RTClib's adjust().
        write_regs(&mut r, 0x68, 0x00, &[0x59, 0x59, 0x23, 6, 0x28, 0x02, 0x26]);
        assert_eq!(
            read_regs(&mut r, 0x68, 0x00, 7),
            [0x59, 0x59, 0x23, 6, 0x28, 0x02, 0x26]
        );
        // A second later it is March (2026 is not a leap year), and Sunday in the
        // firmware's numbering.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let t = read_regs(&mut r, 0x68, 0x00, 7);
        assert!(t[0] <= 0x01, "{t:x?}");
        assert_eq!(t[1..], [0x00, 0x00, 7, 0x01, 0x03, 0x26]);
        // Leap days count.
        write_regs(&mut r, 0x68, 0x00, &[0x00, 0x00, 0x12, 4, 0x29, 0x02, 0x24]);
        assert_eq!(read_regs(&mut r, 0x68, 0x04, 3), [0x29, 0x02, 0x24]);

        // Alarm 1 once a minute at :00 (A1M2..A1M4 set), with its interrupt on.
        write_regs(&mut r, 0x68, 0x00, &[0x59, 0x14, 0x07, 1, 0x16, 0x11, 0x26]);
        write_regs(&mut r, 0x68, 0x07, &[0x00, 0x80, 0x80, 0x80]);
        let fx = write_regs(&mut r, 0x68, 0x0E, &[0x1D]);
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::Drive { .. })),
            "{fx:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let fx = r.tick();
        assert_eq!(read_regs(&mut r, 0x68, 0x0F, 1)[0] & 1, 1, "A1F");
        assert!(
            fx.iter().any(|e| matches!(
                e,
                Effect::Drive {
                    gpio: 4,
                    level: false
                }
            )),
            "INT pulled low: {fx:?}"
        );
        // Writing 1 does not clear the flag; writing 0 does, and releases INT.
        write_regs(&mut r, 0x68, 0x0F, &[0x09]);
        assert_eq!(read_regs(&mut r, 0x68, 0x0F, 1)[0] & 1, 1);
        let fx = write_regs(&mut r, 0x68, 0x0F, &[0x08]);
        assert_eq!(read_regs(&mut r, 0x68, 0x0F, 1)[0] & 1, 0);
        assert!(
            fx.iter().any(|e| matches!(
                e,
                Effect::Drive {
                    gpio: 4,
                    level: true
                }
            )),
            "{fx:?}"
        );

        // Temperature in quarter degrees, from the slider.
        r.control("rtc", "temperature", Dynamic::from(30.25_f64))
            .unwrap();
        assert_eq!(read_regs(&mut r, 0x68, 0x11, 2), [30, 0x40]);
        r.control("rtc", "temperature", Dynamic::from(-5.5_f64))
            .unwrap();
        let t = read_regs(&mut r, 0x68, 0x11, 2);
        assert_eq!(f64::from(t[0] as i8) + f64::from(t[1] >> 6) * 0.25, -5.5);
    }

    #[test]
    fn ds3231_alarm_pin_is_optional() {
        let mut r = runtime(json!([
            { "id": "rtc", "kind": "ds3231", "i2c": { "bus": 0, "address": 104 } }
        ]));
        assert!(r.init().iter().all(|e| !matches!(e, Effect::Error { .. })));
    }

    #[test]
    fn saved_slider_values_apply_at_start() {
        let mut r = runtime(json!([
            { "id": "air", "kind": "bme280", "i2c": { "bus": 0, "address": 118 },
              "controls": { "temperature": 30.5, "pressure": 990.0 } }
        ]));
        let fx = r.init();
        let s = state(&fx, "air").unwrap();
        assert_eq!(s["temperature"], 30.5);
        assert_eq!(s["pressure"], 990.0);
        assert_eq!(s["humidity"], 45.0, "unsaved controls keep their defaults");
        // As firmware sees it, after Adafruit's begin() (see the test above).
        write_reg(&mut r, 0x76, 0xF2, 0x05);
        write_reg(&mut r, 0x76, 0xF4, 0xB7);
        let (t, _, p) = bme280_readings(&mut r, 0x76);
        assert!((t - 30.5).abs() <= 0.01, "{t}");
        assert!((p - 990.0).abs() <= 0.01, "{p}");
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

    #[test]
    fn potentiometer_sets_its_pins_analog_input() {
        let mut r = runtime(json!([
            { "id": "pot", "kind": "potentiometer", "pins": { "wiper": 34 } }
        ]));
        let fx = r.init();
        assert!(fx.contains(&Effect::Analog {
            id: "pot".into(),
            gpio: 34,
            volts: 1.65
        }));
        let fx = r
            .control("pot", "position", Dynamic::from(25.0_f64))
            .unwrap();
        assert!(fx.contains(&Effect::Analog {
            id: "pot".into(),
            gpio: 34,
            volts: 0.825
        }));
        let s = state(&fx, "pot").unwrap();
        assert_eq!(s["position"], 25.0);
        assert_eq!(s["volts"], 0.83);
        // Integers from the CLI work too, and out-of-range values are clamped.
        let fx = r
            .control("pot", "position", Dynamic::from(150_i64))
            .unwrap();
        assert!(fx.contains(&Effect::Analog {
            id: "pot".into(),
            gpio: 34,
            volts: 3.3
        }));
        assert_eq!(state(&fx, "pot").unwrap()["position"], 100.0);
    }
}
