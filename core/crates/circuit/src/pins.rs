//! Pins to be careful with: per board from the site's data when the manifest has it
//! ([`from_site`]), else per chip from Espressif's datasheets ([`notes`]). What the app
//! says when a part is placed on one; the kinds are the core's, the app words them.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Read at reset to choose the boot mode or flash voltage: a part pulling it the
    /// wrong way at power-on can stop the board starting.
    Strapping,
    /// Has no output driver and no internal pull-up or pull-down.
    InputOnly,
    /// Wired to the module's flash (or PSRAM): using it crashes the chip.
    Flash,
    /// The chip's own USB (D- and D+): a board that talks over USB uses them.
    Usb,
    /// UART0, the console and the port flashing uses.
    Console,
    /// Has another job on this board (a camera line, an SD card, a part soldered on):
    /// the site says which, in English.
    Caution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    pub gpio: u8,
    pub kind: Kind,
    /// Why, in English, when the note comes from the site's board data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A board's notes from the site's data: `(gpio, kind, why)` for each pad that needs
/// care (site kinds `strap`, `input-only`, `flash`, `usb`, `uart`, `caution`; supply,
/// ground and reset pads are not GPIOs and have none), sorted by GPIO.
pub fn from_site<'a>(rules: impl IntoIterator<Item = (u8, &'a str, &'a str)>) -> Vec<Note> {
    let mut notes: Vec<Note> = rules
        .into_iter()
        .filter_map(|(gpio, kind, why)| {
            let kind = match kind {
                "strap" => Kind::Strapping,
                "input-only" => Kind::InputOnly,
                "flash" => Kind::Flash,
                "usb" => Kind::Usb,
                "uart" => Kind::Console,
                "caution" => Kind::Caution,
                _ => return None,
            };
            Some(Note {
                gpio,
                kind,
                why: (!why.is_empty()).then(|| why.to_owned()),
            })
        })
        .collect();
    notes.sort_by_key(|n| n.gpio);
    notes.dedup_by_key(|n| (n.gpio, n.kind));
    notes
}

/// The pins of `chip` ("esp32", "esp32s3", "esp32c3") to be careful with; empty for
/// chips this table does not cover.
pub fn notes(chip: &str) -> Vec<Note> {
    let groups: &[(Kind, &[u8])] = match chip {
        "esp32" => &[
            (Kind::Strapping, &[0, 2, 5, 12, 15]),
            (Kind::InputOnly, &[34, 35, 36, 39]),
            (Kind::Flash, &[6, 7, 8, 9, 10, 11]),
            (Kind::Console, &[1, 3]),
        ],
        "esp32s3" => &[
            (Kind::Strapping, &[0, 3, 45, 46]),
            (Kind::Flash, &[26, 27, 28, 29, 30, 31, 32]),
            (Kind::Usb, &[19, 20]),
            (Kind::Console, &[43, 44]),
        ],
        "esp32c3" => &[
            (Kind::Strapping, &[2, 8, 9]),
            (Kind::Flash, &[12, 13, 14, 15, 16, 17]),
            (Kind::Usb, &[18, 19]),
            (Kind::Console, &[20, 21]),
        ],
        _ => &[],
    };
    let mut out: Vec<Note> = groups
        .iter()
        .flat_map(|(kind, gpios)| {
            gpios.iter().map(|&gpio| Note {
                gpio,
                kind: *kind,
                why: None,
            })
        })
        .collect();
    out.sort_by_key(|n| n.gpio);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn notes_from_the_sites_board_data() {
        let n = from_site([
            (12, "strap", "a strapping pin."),
            (34, "input-only", "can only read."),
            (1, "uart", "carries the console."),
            (4, "caution", "the camera's PCLK."),
            (0, "gnd", ""),
            (12, "strap", "listed twice."),
        ]);
        let gpios: Vec<u8> = n.iter().map(|n| n.gpio).collect();
        assert_eq!(gpios, [1, 4, 12, 34]);
        assert_eq!(n[1].kind, Kind::Caution);
        assert_eq!(n[1].why.as_deref(), Some("the camera's PCLK."));
        assert_eq!(n[0].kind, Kind::Console);
    }

    use super::*;

    #[test]
    fn knows_the_pins_to_be_careful_with() {
        let esp32 = notes("esp32");
        assert!(esp32.contains(&Note {
            gpio: 12,
            kind: Kind::Strapping,
            why: None
        }));
        assert!(esp32.contains(&Note {
            gpio: 34,
            kind: Kind::InputOnly,
            why: None
        }));
        assert!(esp32.contains(&Note {
            gpio: 6,
            kind: Kind::Flash,
            why: None
        }));
        assert!(!esp32.iter().any(|n| n.gpio == 4)); // a plain pin
        assert!(notes("esp32c3").contains(&Note {
            gpio: 9,
            kind: Kind::Strapping,
            why: None
        }));
        assert!(notes("esp32s3").contains(&Note {
            gpio: 19,
            kind: Kind::Usb,
            why: None
        }));
        assert!(notes("esp32c6").is_empty());
        // One note per pin, in order.
        for chip in ["esp32", "esp32s3", "esp32c3"] {
            let n = notes(chip);
            assert!(n.windows(2).all(|w| w[0].gpio < w[1].gpio), "{chip}");
        }
        assert_eq!(
            serde_json::to_value(&notes("esp32")[0]).unwrap()["kind"],
            "strapping"
        );
    }
}
