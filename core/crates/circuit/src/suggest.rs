//! Parts for the simulator from a project's wiring table, as the site writes it:
//! "Green LED anode, through 220 Ω" to "GPIO 16", "OLED SDA" to "GPIO 21". Only
//! parts a built-in model simulates, and only when the wiring says clearly which pin
//! they are on; everything else is named in `unmatched`, so the app can say what the
//! simulator leaves out. The wiring is free text, so this matches conservatively: a
//! part it cannot place for certain is left out, not guessed.

use std::collections::BTreeMap;

use crate::{Circuit, I2cPlace, Part, SCHEMA_VERSION};

/// One row of a project's wiring table.
#[derive(Debug, Clone)]
pub struct Wire<'a> {
    pub from: &'a str,
    pub to: &'a str,
}

/// What the wiring gives: parts to place, and the other parts it names that have no
/// model (in the order they first appear).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub circuit: Circuit,
    pub unmatched: Vec<String>,
}

/// I2C models, by the names the site uses for them, with the address they usually
/// have (the model's first).
const I2C: &[(&[&str], &str, u8)] = &[
    (&["oled", "ssd1306"], "ssd1306", 0x3c),
    (&["bme280"], "bme280", 0x76),
    (&["mpu-6050", "mpu6050"], "mpu6050", 0x68),
    (&["ds3231"], "ds3231", 0x68),
];

/// Words that start a part's name but are not the part: "Optional: LED anode".
const PREFIXES: &[&str] = &["optional:", "optional"];

const COLOURS: &[&str] = &["red", "green", "yellow", "blue", "white", "orange"];

/// `hints` are the project's parts list, for what the wiring leaves vague: an "RTC" is
/// placed as a DS3231 only when the parts list names one.
pub fn from_wiring(wiring: &[Wire], hints: &[&str]) -> Suggestion {
    let ds3231_hint = hints.iter().any(|h| h.to_lowercase().contains("ds3231"));
    let mut parts: Vec<Part> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    // rgb-led legs by the LED's name, to make one part of three rows.
    let mut rgb: BTreeMap<String, BTreeMap<String, u8>> = BTreeMap::new();

    for w in wiring {
        let from = strip_prefix(w.from.trim());
        let lower = from.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !(c.is_alphanumeric() || c == '-'))
            .filter(|s| !s.is_empty())
            .collect();
        let has = |w: &str| words.contains(&w);
        let gpio = gpio(w.to);
        let name = part_name(from);

        // I2C parts: placed once, from their SDA row.
        let i2c = I2C
            .iter()
            .find(|(names, _, _)| names.iter().any(|n| has(n)))
            .or_else(|| (has("rtc") && ds3231_hint).then(|| &I2C[3]));
        if let Some((_, kind, address)) = i2c {
            if has("sda") || has("sdi") {
                add_i2c(&mut parts, kind, *address, &name);
            }
            continue;
        }
        let Some(gpio) = gpio else {
            // Power and ground rows say nothing about parts we can place, but name a
            // part the simulator may not have.
            note_unmatched(&mut unmatched, &parts, &name, &words);
            continue;
        };
        if has("rgb") && !has("strip") && !has("matrix") {
            if let Some(leg) = ["red", "green", "blue"].iter().find(|c| has(c)) {
                rgb.entry(name.clone())
                    .or_default()
                    .insert((*leg).to_string(), gpio);
            }
            continue;
        }
        if has("led")
            && !has("strip")
            && !has("matrix")
            && (has("anode") || has("onboard") || words.len() <= 2)
        {
            let colour = COLOURS.iter().find(|c| has(c));
            let label = if has("onboard") {
                "Onboard LED".to_string()
            } else if let Some(c) = colour {
                format!("{} LED", capitalise(c))
            } else {
                "LED".to_string()
            };
            add_pin(&mut parts, "led", gpio, &label, None);
            continue;
        }
        // A push switch springs back: a button, not a switch that stays.
        if has("button") || has("pushbutton") || (has("push") && has("switch")) {
            // Wired to GND (as BOOT is), the pin reads low when pressed; wired to 3V3,
            // high. The other side's row says which.
            let active_low = !other_side_to(wiring, w, "3v3");
            add_pin(&mut parts, "button", gpio, &name, Some(active_low));
            continue;
        }
        if has("switch") {
            let active_low = !other_side_to(wiring, w, "3v3");
            add_pin(&mut parts, "switch", gpio, &name, Some(active_low));
            continue;
        }
        if (has("strip") || has("neopixel") || has("ws2812") || has("ws2812b"))
            && (has("din") || has("data"))
        {
            add_pin(&mut parts, "ws2812", gpio, &name, None);
            continue;
        }
        if has("potentiometer") || has("pot") || has("knob") {
            if has("wiper") || has("middle") || has("signal") {
                add_pin(&mut parts, "potentiometer", gpio, &name, None);
            }
            continue;
        }
        note_unmatched(&mut unmatched, &parts, &name, &words);
    }
    for (name, legs) in rgb {
        if legs.len() == 3 {
            let id = next_id(&parts, "rgb-led");
            // A common leg to 3V3 makes it common-anode: lit when a pin is low.
            let common_anode = wiring.iter().any(|w| {
                let f = w.from.to_lowercase();
                part_name(strip_prefix(w.from.trim())) == name
                    && f.contains("common")
                    && w.to.trim().to_lowercase().replace(' ', "") == "3v3"
            });
            let mut props = serde_json::Map::new();
            if common_anode {
                props.insert("commonAnode".into(), true.into());
            }
            parts.push(Part {
                id,
                kind: "rgb-led".into(),
                pin: None,
                pins: legs,
                i2c: None,
                props,
                active_low: None,
                label: label(&if name.to_lowercase().contains("led") {
                    name.clone()
                } else {
                    format!("{name} LED")
                }),
                controls: Default::default(),
                at: None,
            });
        }
    }
    // Names of parts that were placed after all (an I2C part's power row comes first).
    unmatched.retain(|n| !parts.iter().any(|p| p.label == *n));
    Suggestion {
        circuit: Circuit {
            schema_version: SCHEMA_VERSION.to_owned(),
            parts,
        },
        unmatched,
    }
}

/// "GPIO 16" (or "GPIO16", "IO16") to 16; anything else (3V3, GND, 5V) to None.
/// What each GPIO is wired to, as the wiring says it ("Green LED anode",
/// "HC-SR04 TRIG"), for naming pins in the app; rows that name no GPIO are left out,
/// and a pin with more than one row lists them joined by " / ".
pub fn pin_labels(wiring: &[Wire]) -> BTreeMap<u8, String> {
    let mut out: BTreeMap<u8, String> = BTreeMap::new();
    for w in wiring {
        let Some(g) = gpio(w.to) else { continue };
        let head = strip_prefix(w.from.trim())
            .split([',', '('])
            .next()
            .unwrap_or_default()
            .trim();
        let label: String = head.chars().take(32).collect();
        if label.is_empty() {
            continue;
        }
        out.entry(g)
            .and_modify(|l| {
                if !l.split(" / ").any(|x| x == label) {
                    l.push_str(" / ");
                    l.push_str(&label);
                }
            })
            .or_insert(label);
    }
    out
}

fn gpio(to: &str) -> Option<u8> {
    let t = to.trim().to_lowercase();
    let n = t
        .strip_prefix("gpio")
        .or_else(|| t.strip_prefix("io"))?
        .trim();
    let digits: String = n.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() != n.len() {
        return None;
    }
    digits.parse::<u8>().ok().filter(|&g| g <= 48)
}

fn strip_prefix(s: &str) -> &str {
    let lower = s.to_lowercase();
    for p in PREFIXES {
        if lower.starts_with(p) {
            return s[p.len()..].trim_start_matches([':', ' ']);
        }
    }
    s
}

/// The part a row is about: its text before the first comma or slash, up to the first
/// word naming a leg, signal or pin ("Green LED anode" -> "Green LED", "OLED SDA" ->
/// "OLED", "HC-SR04 TRIG" -> "HC-SR04", "Keypad R1" -> "Keypad").
fn part_name(from: &str) -> String {
    let head = from.split([',', '(', '/']).next().unwrap_or(from).trim();
    const LEGS: &[&str] = &[
        "anode",
        "cathode",
        "sda",
        "sdi",
        "scl",
        "sck",
        "vcc",
        "vin",
        "vdd",
        "gnd",
        "din",
        "data",
        "signal",
        "wiper",
        "side",
        "a",
        "b",
        "+",
        "-",
        "5v",
        "+5v",
        "3v3",
        "vdc",
        "power",
        "wire",
        "leg",
        "pin",
        "middle",
        "out",
        "in",
        "or",
        "and",
        "trig",
        "echo",
        "clk",
        "mosi",
        "miso",
        "cs",
        "csn",
        "ce",
        "dc",
        "rst",
        "busy",
        "so",
        "do",
        "rx",
        "tx",
        "vrx",
        "vry",
        "sw",
        "sig",
        "aout",
        "dout",
        "en",
        "int",
        "sqw",
        "k",
        "plug",
        "collector",
        "emitter",
        "base",
        "common",
        "output",
        "input",
        "ground",
        "through",
        "red",
        "black",
        "orange",
        "yellow",
        "white",
        "brown",
        "green",
        "blue",
        "ser",
        "srclk",
        "rclk",
        "oe",
        "mr",
        "blk",
        "vo",
        "rs",
        "rw",
        "e",
        "from",
    ];
    let words: Vec<&str> = head.split_whitespace().collect();
    let leg = |w: &str| {
        let l = w.to_lowercase();
        LEGS.contains(&l.as_str())
            || l.chars().all(|c| c.is_ascii_digit())
            // Numbered pins: Q0, R1, C4, D4, IN1.
            || (l.len() >= 2
                && ["q", "r", "c", "d", "in"].iter().any(|p| {
                    l.strip_prefix(p)
                        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                }))
    };
    let end = words
        .iter()
        .skip(1)
        .position(|w| leg(w))
        .map_or(words.len(), |i| i + 1);
    words[..end].join(" ")
}

fn label(name: &str) -> String {
    name.chars().take(40).collect()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Whether the row for the same part's other side goes to `rail` (3v3, gnd).
fn other_side_to(wiring: &[Wire], this: &Wire, rail: &str) -> bool {
    let name = part_name(strip_prefix(this.from.trim())).to_lowercase();
    wiring.iter().any(|w| {
        !std::ptr::eq(w, this)
            && part_name(strip_prefix(w.from.trim())).to_lowercase() == name
            && w.to.trim().to_lowercase().replace(' ', "") == rail
    })
}

fn next_id(parts: &[Part], kind: &str) -> String {
    (1..)
        .map(|n| format!("{kind}-{n}"))
        .find(|id| !parts.iter().any(|p| &p.id == id))
        .unwrap_or_else(|| kind.to_owned())
}

fn add_pin(parts: &mut Vec<Part>, kind: &str, gpio: u8, name: &str, active_low: Option<bool>) {
    // One part per pin and kind: "Onboard LED" and "Optional: LED" on GPIO 2 are one.
    if parts.iter().any(|p| p.kind == kind && p.pin == Some(gpio)) {
        return;
    }
    let mut props = serde_json::Map::new();
    if let Some(a) = active_low {
        props.insert("activeLow".into(), a.into());
    }
    parts.push(Part {
        id: next_id(parts, kind),
        kind: kind.into(),
        pin: Some(gpio),
        pins: Default::default(),
        i2c: None,
        props,
        active_low: None,
        label: label(name),
        controls: Default::default(),
        at: None,
    });
}

fn add_i2c(parts: &mut Vec<Part>, kind: &str, address: u8, name: &str) {
    if parts.iter().any(|p| p.kind == kind) {
        return;
    }
    // Two parts at one address (an MPU-6050 and a DS3231 both answer at 0x68): the
    // second takes the model's other address if it has one, else it is left out.
    let taken = |a: u8| {
        parts
            .iter()
            .any(|p| p.i2c.as_ref().is_some_and(|i| i.address == a))
    };
    let address = if !taken(address) {
        address
    } else if kind == "mpu6050" && !taken(0x69) {
        0x69
    } else if kind == "ssd1306" && !taken(0x3d) {
        0x3d
    } else if kind == "bme280" && !taken(0x77) {
        0x77
    } else {
        return;
    };
    parts.push(Part {
        id: next_id(parts, kind),
        kind: kind.into(),
        pin: None,
        pins: Default::default(),
        i2c: Some(I2cPlace { bus: 0, address }),
        props: Default::default(),
        active_low: None,
        label: label(name),
        controls: Default::default(),
        at: None,
    });
}

fn note_unmatched(unmatched: &mut Vec<String>, parts: &[Part], name: &str, words: &[&str]) {
    // The board itself and its own pins are not parts.
    const NOT_PARTS: &[&str] = &[
        "esp32", "board", "gnd", "3v3", "5v", "usb", "vin", "boot", "en", "reset",
    ];
    // A power or ground row of a part with a model (its signal row places it).
    const MODELLED: &[&str] = &[
        "led",
        "button",
        "pushbutton",
        "switch",
        "strip",
        "neopixel",
        "ws2812",
        "ws2812b",
        "potentiometer",
        "pot",
        "knob",
        "rgb",
        "oled",
        "ssd1306",
        "bme280",
        "mpu-6050",
        "mpu6050",
        "ds3231",
    ];
    // Resistors, capacitors and supplies are not what anyone wants simulated.
    const PASSIVE: &[&str] = &[
        "resistor",
        "capacitor",
        "pull-up",
        "pull-down",
        "supply",
        "diode",
        "1n4001",
        "transistor",
        "jumper",
        "trimmer",
    ];
    let lower = name.to_lowercase();
    if name.is_empty()
        || words.first().is_some_and(|w| NOT_PARTS.contains(w))
        || words.iter().any(|w| MODELLED.contains(w))
        || PASSIVE.iter().any(|p| lower.contains(p))
        || lower.contains(['Ω', 'ω'])
        || lower.contains("µf")
        || gpio(name).is_some()
    {
        return;
    }
    if !unmatched.iter().any(|n| n == name) && !parts.iter().any(|p| p.label == name) {
        unmatched.push(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wires(rows: &[(&'static str, &'static str)]) -> Vec<Wire<'static>> {
        rows.iter().map(|(from, to)| Wire { from, to }).collect()
    }

    fn from_wiring_t(w: &[Wire]) -> Suggestion {
        from_wiring(w, &[])
    }

    fn summary(s: &Suggestion) -> Vec<String> {
        s.circuit
            .parts
            .iter()
            .map(|p| {
                let place = match (&p.pin, &p.i2c) {
                    (Some(g), _) => format!("gpio{g}"),
                    (_, Some(i)) => format!("i2c{}@{:#x}", i.bus, i.address),
                    _ => format!("{:?}", p.pins),
                };
                let low = p
                    .props
                    .get("activeLow")
                    .map(|v| format!(" low={v}"))
                    .unwrap_or_default();
                format!("{} {} {place} \"{}\"{low}", p.id, p.kind, p.label)
            })
            .collect()
    }

    // The rows below are copied from esp32bench.dev's library (desktop-manifest.json).

    #[test]
    fn leds_with_their_colours_and_the_onboard_one_once() {
        let s = from_wiring_t(&wires(&[
            ("Green LED anode, through 220 Ω", "GPIO 16"),
            ("Green LED cathode", "GND"),
            ("Red LED anode, through 220 Ω", "GPIO 17"),
            ("Red LED cathode", "GND"),
            ("Onboard LED", "GPIO 2"),
            ("Optional: LED anode, through 220 Ω", "GPIO 2"),
            ("Optional: LED cathode", "GND"),
        ]));
        assert_eq!(
            summary(&s),
            [
                "led-1 led gpio16 \"Green LED\"",
                "led-2 led gpio17 \"Red LED\"",
                "led-3 led gpio2 \"Onboard LED\""
            ]
        );
        assert!(s.unmatched.is_empty(), "{:?}", s.unmatched);
        s.circuit.validate().unwrap();
    }

    #[test]
    fn buttons_read_low_or_high_by_where_their_other_side_goes() {
        let s = from_wiring_t(&wires(&[
            ("Button, side A", "3V3"),
            ("Button, side B", "GPIO 33"),
            ("BOOT button", "GPIO 0"),
            ("Optional: door reed switch, wire A", "GPIO 26"),
            ("Optional: door reed switch, wire B", "GND"),
            ("Encoder push switch / SW", "GPIO 23"),
        ]));
        assert_eq!(
            summary(&s),
            [
                "button-1 button gpio33 \"Button\" low=false",
                "button-2 button gpio0 \"BOOT button\" low=true",
                "switch-1 switch gpio26 \"door reed switch\" low=true",
                "button-3 button gpio23 \"Encoder push switch\" low=true"
            ]
        );
        s.circuit.validate().unwrap();
    }

    #[test]
    fn i2c_parts_once_each_and_a_strip_on_its_data_pin() {
        let s = from_wiring_t(&wires(&[
            ("OLED VCC", "3V3"),
            ("OLED GND", "GND"),
            ("OLED SDA", "GPIO 21"),
            ("OLED SCL", "GPIO 22"),
            ("BME280 VIN or VCC", "3V3"),
            ("BME280 SDA or SDI", "GPIO 21"),
            ("BME280 SCL or SCK", "GPIO 22"),
            ("Strip 5V or VDC", "5V"),
            ("Strip DIN, through 330 Ω", "GPIO 16"),
        ]));
        assert_eq!(
            summary(&s),
            [
                "ssd1306-1 ssd1306 i2c0@0x3c \"OLED\"",
                "bme280-1 bme280 i2c0@0x76 \"BME280\"",
                "ws2812-1 ws2812 gpio16 \"Strip\""
            ]
        );
        assert!(s.unmatched.is_empty(), "{:?}", s.unmatched);
        s.circuit.validate().unwrap();
    }

    #[test]
    fn parts_without_a_model_are_named_not_guessed() {
        let s = from_wiring_t(&wires(&[
            ("DS18B20 VDD (red)", "3V3"),
            ("DS18B20 DATA (yellow)", "GPIO 4"),
            ("Servo red (power)", "5V"),
            ("Servo orange or yellow (signal)", "GPIO 18"),
            ("ESP32 GND", "GND"),
        ]));
        assert!(s.circuit.parts.is_empty(), "{:?}", summary(&s));
        assert_eq!(s.unmatched, ["DS18B20", "Servo"]);
    }

    #[test]
    fn an_mpu_6050_and_a_clock_share_the_bus_at_different_addresses() {
        let s = from_wiring_t(&wires(&[
            ("DS3231 SDA", "GPIO 21"),
            ("MPU-6050 SDA", "GPIO 21"),
        ]));
        assert_eq!(
            summary(&s),
            [
                "ds3231-1 ds3231 i2c0@0x68 \"DS3231\"",
                "mpu6050-1 mpu6050 i2c0@0x69 \"MPU-6050\""
            ]
        );
        s.circuit.validate().unwrap();
    }

    #[test]
    fn knobs_an_rgb_led_without_the_word_led_and_a_named_rtc() {
        let s = from_wiring(
            &wires(&[
                ("Left knob, middle pin", "GPIO 34"),
                ("Left knob, outer pin", "3V3"),
                ("RGB red", "GPIO 25"),
                ("RGB green", "GPIO 26"),
                ("RGB blue", "GPIO 33"),
                ("RGB common leg (the longest)", "GND"),
                ("RTC SDA", "GPIO 21"),
            ]),
            &["DS3231 real-time clock module"],
        );
        assert_eq!(
            summary(&s),
            [
                "potentiometer-1 potentiometer gpio34 \"Left knob\"",
                "ds3231-1 ds3231 i2c0@0x68 \"RTC\"",
                "rgb-led-1 rgb-led {\"blue\": 33, \"green\": 26, \"red\": 25} \"RGB LED\""
            ]
        );
        assert!(s.unmatched.is_empty(), "{:?}", s.unmatched);
        s.circuit.validate().unwrap();
        // Without a parts list naming the chip, an RTC is not guessed.
        let s = from_wiring(&wires(&[("RTC SDA", "GPIO 21")]), &[]);
        assert!(s.circuit.parts.is_empty());
    }

    #[test]
    fn names_unmodelled_parts_without_their_pins_or_passives() {
        let s = from_wiring_t(&wires(&[
            ("HC-SR04 TRIG", "GPIO 5"),
            ("HC-SR04 ECHO, through a 1 kΩ / 2 kΩ divider", "GPIO 18"),
            ("2 kΩ resistor", "GND"),
            ("Keypad R1", "GPIO 13"),
            ("Keypad C1", "GPIO 26"),
            ("74HC595 Q0 to Q6", "Segments"),
            ("10 kΩ pull-down", "GND"),
            ("Joystick VRx", "GPIO 34"),
            ("74HC595 SRCLK", "GPIO 18"),
            ("100 kΩ from GPIO 35", "GND"),
            ("IO0", "GND"),
        ]));
        assert_eq!(s.unmatched, ["HC-SR04", "Keypad", "74HC595", "Joystick"]);
    }

    #[test]
    fn reads_gpio_names() {
        assert_eq!(gpio("GPIO 16"), Some(16));
        assert_eq!(gpio("GPIO16"), Some(16));
        assert_eq!(gpio("IO4"), Some(4));
        assert_eq!(gpio("3V3"), None);
        assert_eq!(gpio("GPIO 99"), None);
        assert_eq!(gpio("GPIO 21 (SDA)"), None);
    }

    #[test]
    fn names_pins_from_the_wiring() {
        let w = |from, to| Wire { from, to };
        let labels = pin_labels(&[
            w("Green LED anode, through 220 Ω", "GPIO 16"),
            w("Optional: Button", "GPIO 4"),
            w("OLED SDA", "GPIO 21"),
            w("Buzzer +", "GPIO 21"),
            w("OLED VCC", "3V3"),
        ]);
        assert_eq!(labels.get(&16).map(String::as_str), Some("Green LED anode"));
        assert_eq!(labels.get(&4).map(String::as_str), Some("Button"));
        assert_eq!(
            labels.get(&21).map(String::as_str),
            Some("OLED SDA / Buzzer +")
        );
        assert_eq!(labels.len(), 3);
    }
}
