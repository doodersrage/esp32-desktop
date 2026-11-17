//! `esp32bench://` links from the site's "Open in desktop app" buttons.
//!
//! A link only names a project (and maybe a board). Opening one selects it in the app;
//! building or flashing it still goes through the consent prompt, like anything else
//! from the library. Links come from web pages, so they are untrusted: anything but
//! the exact forms below is refused.

/// What a link asks the app to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub project: String,
    pub board: Option<String>,
}

/// Longest link accepted.
const MAX_LEN: usize = 200;

/// Parse `esp32bench://project/<id>` or `esp32bench://project/<id>?board=<board>`
/// (a trailing `/` after the id is allowed). Ids are lowercase letters, digits and
/// dashes, starting with a letter or digit, at most 64 characters.
pub fn parse(url: &str) -> Result<Link, String> {
    if url.len() > MAX_LEN {
        return Err("the link is too long".into());
    }
    let rest = url
        .strip_prefix("esp32bench://project/")
        .ok_or("not an esp32bench://project/ link")?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let project = path.strip_suffix('/').unwrap_or(path);
    if !valid_id(project) {
        return Err(format!("`{project}` is not a project id"));
    }
    let board = match query {
        None => None,
        Some(q) => {
            let b = q
                .strip_prefix("board=")
                .ok_or("the only option a link may carry is board=")?;
            if !valid_id(b) {
                return Err(format!("`{b}` is not a board id"));
            }
            Some(b.to_owned())
        }
    };
    Ok(Link {
        project: project.to_owned(),
        board,
    })
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !s.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sites_links() {
        assert_eq!(
            parse("esp32bench://project/blink").unwrap(),
            Link {
                project: "blink".into(),
                board: None
            }
        );
        assert_eq!(
            parse("esp32bench://project/rtc-clock/").unwrap(),
            Link {
                project: "rtc-clock".into(),
                board: None
            }
        );
        assert_eq!(
            parse("esp32bench://project/bme280-serial?board=esp32c3").unwrap(),
            Link {
                project: "bme280-serial".into(),
                board: Some("esp32c3".into())
            }
        );
    }

    #[test]
    fn refuses_anything_else() {
        for bad in [
            "",
            "https://esp32bench.dev/projects/blink",
            "esp32bench://project/",
            "esp32bench://project/Blink",
            "esp32bench://project/../../etc/passwd",
            "esp32bench://project/blink/extra",
            "esp32bench://project/-blink",
            "esp32bench://project/blink?build=yes",
            "esp32bench://project/blink?board=esp32&flash=1",
            "esp32bench://project/blink?board=",
            "esp32bench://project/bl%69nk",
            "esp32bench://flash/blink",
            "ESP32BENCH://project/blink",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&format!("esp32bench://project/{}", "a".repeat(190))).is_err());
    }
}
