//! Numbers to plot from serial or simulator output, in the Arduino Serial Plotter's
//! line format: values separated by spaces, commas or tabs, each a number or a named
//! number (`temp:21.5`, `temp=21.5`, `temp = 21.5`). A line with anything else in it
//! is text, not a plot row. Lines are assembled across chunks.

use serde::{Deserialize, Serialize};

/// One value of a row: its name (`1`, `2`, ... for unnamed values, by position) and
/// the number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Value {
    pub name: String,
    pub value: f64,
}

/// Collects output into lines and returns the rows the plottable ones make.
#[derive(Default)]
pub struct Lines {
    line: String,
}

/// A line longer than this is not a plot row; it is dropped while it lasts.
const MAX_LINE: usize = 512;

/// The most values a row may have; the plotter would not show more usefully.
const MAX_VALUES: usize = 16;

impl Lines {
    pub fn new() -> Lines {
        Lines::default()
    }

    /// Feed output bytes; the rows completed by them, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<Value>> {
        let mut rows = Vec::new();
        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\n' => {
                    if let Some(row) = parse(&self.line) {
                        rows.push(row);
                    }
                    self.line.clear();
                }
                '\r' => {}
                _ if self.line.len() < MAX_LINE => self.line.push(ch),
                _ => {}
            }
        }
        rows
    }
}

/// A line as a plot row, or None if it is text.
pub fn parse(line: &str) -> Option<Vec<Value>> {
    // "temp = 21.5" reads as "temp=21.5".
    let mut joined = line.trim().to_owned();
    for sep in [" = ", " =", "= ", " : ", " :", ": "] {
        joined = joined.replace(sep, sep.trim());
    }
    let mut row = Vec::new();
    for (i, token) in joined
        .split([',', '\t', ' '])
        .filter(|t| !t.is_empty())
        .enumerate()
    {
        let (name, number) = match token.split_once([':', '=']) {
            Some((n, v))
                if !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c)) =>
            {
                (n.to_owned(), v)
            }
            Some(_) => return None,
            None => ((i + 1).to_string(), token),
        };
        let value: f64 = number.parse().ok().filter(|v: &f64| v.is_finite())?;
        row.push(Value { name, value });
        if row.len() > MAX_VALUES {
            return None;
        }
    }
    (!row.is_empty()).then_some(row)
}

/// Rows as CSV: a `row` column, then one column per series in the order they first
/// appear; a series a row does not have is left empty. Names are quoted when needed.
pub fn csv(rows: &[Vec<Value>]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for row in rows {
        for v in row {
            if !names.contains(&v.name.as_str()) {
                names.push(&v.name);
            }
        }
    }
    let quote = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out = String::from("row");
    for n in &names {
        out.push(',');
        out.push_str(&quote(n));
    }
    out.push('\n');
    for (i, row) in rows.iter().enumerate() {
        out.push_str(&(i + 1).to_string());
        for n in &names {
            out.push(',');
            if let Some(v) = row.iter().find(|v| v.name == *n) {
                out.push_str(&v.value.to_string());
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(row: &[Value]) -> Vec<(String, f64)> {
        row.iter().map(|v| (v.name.clone(), v.value)).collect()
    }

    #[test]
    fn reads_the_plotter_formats() {
        assert_eq!(
            names(&parse("21.5 45").unwrap()),
            [("1".into(), 21.5), ("2".into(), 45.0)]
        );
        assert_eq!(names(&parse("1,2,3").unwrap()).len(), 3);
        assert_eq!(
            names(&parse("temp:21.5\thum:45").unwrap()),
            [("temp".into(), 21.5), ("hum".into(), 45.0)]
        );
        // From esp32bench.dev's projects.
        assert_eq!(
            names(&parse("temp_c = 21.50").unwrap()),
            [("temp_c".into(), 21.5)]
        );
        assert_eq!(
            names(&parse("distance_cm=12.3, freq=440").unwrap()),
            [("distance_cm".into(), 12.3), ("freq".into(), 440.0)]
        );
        assert_eq!(names(&parse("-3.5e2").unwrap()), [("1".into(), -350.0)]);
    }

    #[test]
    fn text_is_not_plotted() {
        for line in [
            "",
            "tick 1000 ms",
            "Display ready",
            "BME280 not found at 0x76",
            "ets Jul 29 2019",
            "x:NaN",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
    }

    #[test]
    fn rows_make_csv() {
        let rows = vec![
            parse("temp:21.5 hum:45").unwrap(),
            parse("temp:22").unwrap(),
            // Not a name a line can give, but a name to quote all the same.
            vec![
                Value {
                    name: "temp".into(),
                    value: 22.5,
                },
                Value {
                    name: "odd,name".into(),
                    value: 1.0,
                },
            ],
        ];
        assert_eq!(
            csv(&rows),
            "row,temp,hum,\"odd,name\"\n1,21.5,45,\n2,22,,\n3,22.5,,1\n"
        );
        assert_eq!(csv(&[]), "row\n");
    }

    #[test]
    fn rows_are_assembled_across_chunks() {
        let mut l = Lines::new();
        assert!(l.feed(b"t:1").is_empty());
        let rows = l.feed(b"2\r\nhello\nt:13\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].value, 12.0);
        assert_eq!(rows[1][0].value, 13.0);
    }
}
