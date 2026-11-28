//! Parties: a shared queue that a host's device plays and others add to.
//! The plan is `docs/party.md`. So far this holds the parts that need no
//! server: a Party's link, its QR code, and reading a code back out of a
//! scan or a pasted link.

use crate::account::Qr;

/// Where Party links point. The site opens the app (an Android App Link)
/// or, without the app, shows a page with the Party and a download link.
pub const LINK_BASE: &str = "https://tonearm.eli-zac.dev/party/";

/// The link for a Party's code, `K7Q-M4P`.
pub fn link(code: &str) -> String {
    format!("{LINK_BASE}{}", code.trim())
}

/// The QR code for a Party: its link, so a phone's own camera app can open it.
pub fn qr(code: &str) -> Option<Qr> {
    let made = qrcode::QrCode::new(link(code).as_bytes()).ok()?;
    Some(Qr {
        size: made.width(),
        dark: made.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect(),
    })
}

/// The Party code in a scan, a pasted link or a typed code, as `K7Q-M4P`.
/// None for anything else.
pub fn code_from(text: &str) -> Option<String> {
    let text = text.trim();
    let code = if let Some(rest) = text.strip_prefix(LINK_BASE).or_else(|| text.strip_prefix("tonearm://party/")) {
        rest.split(['?', '#', '/']).next()?
    } else if text.contains("://") {
        return None;
    } else {
        text
    };
    let clean: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    (clean.len() == 6).then(|| format!("{}-{}", &clean[..3], &clean[3..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_and_its_code_go_both_ways() {
        assert_eq!(link("K7Q-M4P"), "https://tonearm.eli-zac.dev/party/K7Q-M4P");
        assert_eq!(code_from(&link("K7Q-M4P")).as_deref(), Some("K7Q-M4P"));
        assert_eq!(code_from("tonearm://party/k7qm4p").as_deref(), Some("K7Q-M4P"));
        assert_eq!(code_from(" k7q m4p ").as_deref(), Some("K7Q-M4P"));
    }

    #[test]
    fn other_links_and_short_codes_are_not_parties() {
        assert_eq!(code_from("https://example.com/party/K7Q-M4P"), None);
        assert_eq!(code_from("tonearm://pair?code=TG2G-P6SE"), None);
        assert_eq!(code_from("K7Q"), None);
    }

    #[test]
    fn a_party_qr_is_square() {
        let qr = qr("K7Q-M4P").unwrap();
        assert_eq!(qr.dark.len(), qr.size * qr.size);
    }
}
