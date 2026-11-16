//! Time synced lyrics.
//!
//! We ask the same public services the BitChord client asks, in the same
//! order, and stop at the first one that has the track:
//!
//! 1. lrc.red, syllable timed TTML filed by ISRC
//! 2. BetterLyrics, the same kind of TTML matched on title and artist
//! 3. LRCLIB, whole line timings
//! 4. YouTube Music's own Lyrics tab, plain text
//!
//! BitChord also asks LyricsPlus. Its community mirrors were down when we
//! built this, so it is not wired in yet.

use crate::innertube::{agent, Client};
use serde_json::Value;
use std::time::Duration;

const AGENT: &str = concat!("tonearm/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq)]
pub struct Syllable {
    pub start: u32,
    pub end: u32,
    /// Ends with a space when the syllable closes a word.
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub start: u32,
    pub end: u32,
    pub text: String,
    /// Empty when the source only times whole lines.
    pub syllables: Vec<Syllable>,
    /// Where the singer's lines sit: 0 left, 1 right, 2 centred for a group.
    pub align: u8,
    /// Backing vocals sung along with the line, without their brackets.
    pub background: Vec<Syllable>,
    pub background_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    /// False for plain text. Then every start and end is zero.
    pub synced: bool,
    pub source: &'static str,
}

pub struct Query<'a> {
    pub video_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub seconds: f32,
}

pub fn fetch(client: &Client, q: &Query) -> Option<Lyrics> {
    let http = agent(Duration::from_secs(6));
    let wrap = |lines: Vec<Line>, source| Lyrics {
        lines,
        synced: true,
        source,
    };
    if let Some(lines) = lrc_red(&http, q) {
        return Some(wrap(lines, "lrc.red"));
    }
    if let Some(lines) = better_lyrics(&http, q) {
        return Some(wrap(lines, "BetterLyrics"));
    }
    if let Some(lines) = lrclib(&http, q) {
        return Some(wrap(lines, "LRCLIB"));
    }
    let text = client.plain_lyrics(q.video_id).ok()??;
    let lines = text
        .lines()
        .map(|l| Line {
            text: l.trim().to_string(),
            ..Line::default()
        })
        .collect();
    Some(Lyrics {
        lines,
        synced: false,
        source: "YouTube Music",
    })
}

fn get(http: &ureq::Agent, url: &str, query: &[(&str, &str)]) -> Option<String> {
    let mut request = http.get(url).set("User-Agent", AGENT);
    for (key, value) in query {
        request = request.query(key, value);
    }
    request.call().ok()?.into_string().ok()
}

// ---------------------------------------------------------------- lrc.red

fn lrc_red(http: &ureq::Agent, q: &Query) -> Option<Vec<Line>> {
    let body = get(
        http,
        "https://lrc.red/search.json",
        &[("q", &format!("{} {}", q.title, q.artist))],
    )?;
    let json: Value = serde_json::from_str(&body).ok()?;
    let hits = json.get("hits")?.as_array()?;
    let isrc = best_hit(hits, q)?;
    let ttml = get(http, &format!("https://lrc.red/s/{isrc}.ttml"), &[])?;
    Some(parse_ttml(&ttml)).filter(|l| !l.is_empty())
}

/// Lower case letters and digits only, single spaces.
fn normalized(s: &str) -> String {
    let spaced: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Splits "Song (Live) - 2011 Remaster" into "song" and its extras.
fn split_title(title: &str) -> (String, String) {
    let mut core = String::new();
    let mut extras = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth = (depth - 1).max(0);
                extras.push(' ');
            }
            _ if depth > 0 => extras.push(c),
            _ => core.push(c),
        }
    }
    if let Some((head, tail)) = core.split_once(" - ") {
        extras.push(' ');
        extras.push_str(tail);
        core = head.to_string();
    }
    (normalized(&core), normalized(&extras))
}

const VERSION_WORDS: [&str; 20] = [
    "live",
    "remix",
    "remixed",
    "mix",
    "acoustic",
    "unplugged",
    "instrumental",
    "karaoke",
    "cappella",
    "acapella",
    "demo",
    "edit",
    "version",
    "cover",
    "sped",
    "slowed",
    "reverb",
    "nightcore",
    "orchestral",
    "extended",
];

fn version_words(extras: &str) -> Vec<&str> {
    let mut words: Vec<&str> = extras
        .split(' ')
        .filter(|w| VERSION_WORDS.contains(w))
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

fn artists(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    let mut parts = vec![lower.as_str()];
    for separator in [
        ",", "&", ";", "/", " and ", " x ", " with ", " feat. ", " feat ", " ft. ", " ft ",
    ] {
        parts = parts.into_iter().flat_map(|p| p.split(separator)).collect();
    }
    parts
        .into_iter()
        .map(normalized)
        .filter(|p| !p.is_empty())
        .collect()
}

/// The ISRC of the hit that is surely this recording. Wrong words scrolling
/// in time with the right song are worse than no words, so every test must
/// pass: same title, same version words, a shared artist, close in length.
fn best_hit(hits: &[Value], q: &Query) -> Option<String> {
    let (title, extras) = split_title(q.title);
    let version = version_words(&extras);
    let wanted = artists(q.artist);
    let field = |hit: &Value, key: &str| {
        hit.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    hits.iter()
        .filter_map(|hit| {
            let (hit_title, hit_extras) = split_title(&field(hit, "title"));
            let isrc = field(hit, "isrc");
            let valid = isrc.len() == 12
                && isrc
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            let shared = wanted.is_empty()
                || artists(&field(hit, "artist"))
                    .iter()
                    .any(|a| wanted.contains(a));
            let distance = match hit.get("duration").and_then(Value::as_f64) {
                Some(d) if q.seconds > 0.0 => (d as f32 - q.seconds).abs(),
                _ => 0.0,
            };
            let same = hit_title == title && version_words(&hit_extras) == version;
            (valid && same && shared && distance <= 3.0).then_some((distance, isrc))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, isrc)| isrc)
}

// ----------------------------------------------------------- BetterLyrics

fn better_lyrics(http: &ureq::Agent, q: &Query) -> Option<Vec<Line>> {
    let seconds = (q.seconds as u32).to_string();
    let mut query = vec![("s", q.title), ("a", q.artist)];
    if q.seconds > 0.0 {
        query.push(("d", &seconds));
    }
    let body = get(http, "https://lyrics-api.boidu.dev/getLyrics", &query)?;
    let json: Value = serde_json::from_str(&body).ok()?;
    Some(parse_ttml(json.get("ttml")?.as_str()?)).filter(|l| !l.is_empty())
}

// ----------------------------------------------------------------- LRCLIB

fn lrclib(http: &ureq::Agent, q: &Query) -> Option<Vec<Line>> {
    let seconds = (q.seconds as u32).to_string();
    let mut query = vec![("track_name", q.title), ("artist_name", q.artist)];
    if q.seconds > 0.0 {
        query.push(("duration", &seconds));
    }
    let body = get(http, "https://lrclib.net/api/get", &query)?;
    let json: Value = serde_json::from_str(&body).ok()?;
    Some(parse_lrc(json.get("syncedLyrics")?.as_str()?)).filter(|l| !l.is_empty())
}

pub fn parse_lrc(lrc: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = lrc
        .lines()
        .filter_map(|raw| {
            let rest = raw.trim().strip_prefix('[')?;
            let (stamp, text) = rest.split_once(']')?;
            let start = parse_time(stamp)?;
            Some(Line {
                start,
                end: 0,
                text: text.trim().to_string(),
                ..Line::default()
            })
        })
        .collect();
    lines.sort_by_key(|l| l.start);
    close_lines(&mut lines);
    lines.retain(|l| !l.text.is_empty());
    lines
}

/// A line with no end of its own lasts until the next one starts.
fn close_lines(lines: &mut [Line]) {
    for i in 0..lines.len() {
        if lines[i].end <= lines[i].start {
            lines[i].end = lines
                .get(i + 1)
                .map(|n| n.start)
                .unwrap_or(lines[i].start + 5000);
        }
    }
}

// ------------------------------------------------------------------- TTML

/// "1:02:03.5", "1:23.456" and "12.3" all mean a time. Returns milliseconds.
pub fn parse_time(s: &str) -> Option<u32> {
    let mut seconds = 0f64;
    for part in s.trim().trim_end_matches('s').split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some((seconds * 1000.0).round() as u32)
}

fn times(node: roxmltree::Node) -> Option<(u32, u32)> {
    Some((
        parse_time(node.attribute("begin")?)?,
        parse_time(node.attribute("end")?)?,
    ))
}

fn has_timed_child(node: roxmltree::Node) -> bool {
    node.descendants()
        .skip(1)
        .any(|d| d.attribute("begin").is_some())
}

/// Gathers the timed syllables under `node` into `out`, and backing vocals
/// into `bg`.
fn collect(node: roxmltree::Node, out: &mut Vec<Syllable>, bg: &mut Vec<Syllable>) {
    for child in node.children() {
        if child.is_text() {
            let text = child.text().unwrap_or("");
            match out.last_mut() {
                // White space between two spans is the word boundary.
                Some(last) if text.trim().is_empty() => {
                    if !text.is_empty() && !last.text.ends_with(' ') {
                        last.text.push(' ');
                    }
                }
                _ => {}
            }
        } else if child.is_element() {
            let role = child
                .attributes()
                .find(|a| a.name() == "role")
                .map(|a| a.value())
                .unwrap_or("");
            if role == "x-translation" || role == "x-roman" {
                continue;
            }
            if role == "x-bg" {
                if let Some(last) = out.last_mut() {
                    if !last.text.ends_with(' ') {
                        last.text.push(' ');
                    }
                }
                if let Some(last) = bg.last_mut() {
                    if !last.text.ends_with(' ') {
                        last.text.push(' ');
                    }
                }
                let mut nested = Vec::new();
                collect(child, bg, &mut nested);
                bg.append(&mut nested);
                continue;
            }
            match times(child) {
                Some((start, end)) if !has_timed_child(child) => {
                    let text: String = child
                        .descendants()
                        .filter_map(|d| d.text().filter(|_| d.is_text()))
                        .collect();
                    if !text.is_empty() {
                        out.push(Syllable { start, end, text });
                    }
                }
                _ => {
                    if let Some(last) = out.last_mut() {
                        if !last.text.ends_with(' ') {
                            last.text.push(' ');
                        }
                    }
                    collect(child, out, bg);
                }
            }
        }
    }
}

/// Takes the brackets off backing vocals, as they show apart from the line
/// anyway, and gives their text.
fn unbracket(syllables: &mut Vec<Syllable>) -> String {
    if let Some(first) = syllables.first_mut() {
        if let Some(rest) = first.text.strip_prefix('(') {
            first.text = rest.to_string();
        }
    }
    if let Some(last) = syllables.last_mut() {
        let trimmed = last.text.trim_end();
        last.text = trimmed.strip_suffix(')').unwrap_or(trimmed).to_string();
    }
    syllables.retain(|s| !s.text.trim().is_empty());
    syllables.iter().map(|s| s.text.as_str()).collect::<String>().trim().to_string()
}

pub fn parse_ttml(ttml: &str) -> Vec<Line> {
    let Ok(doc) = roxmltree::Document::parse(ttml) else {
        return Vec::new();
    };
    let attr = |n: roxmltree::Node, name: &str| -> Option<String> {
        n.attributes().find(|a| a.name() == name).map(|a| a.value().to_string())
    };
    // Groups of singers, such as everyone at once, sing from the middle.
    let groups: Vec<String> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "agent" && attr(*n, "type").as_deref() == Some("group"))
        .filter_map(|n| attr(n, "id"))
        .collect();
    // Singers take turns left and right, in the order they first sing.
    let mut singers: Vec<String> = Vec::new();
    let mut lines = Vec::new();
    for p in doc.descendants().filter(|n| n.has_tag_name("p")) {
        let mut syllables = Vec::new();
        let mut background = Vec::new();
        collect(p, &mut syllables, &mut background);
        // A line of nothing but backing vocals is sung as the line.
        if syllables.is_empty() && !background.is_empty() {
            syllables = std::mem::take(&mut background);
        }
        let span = times(p);
        if let Some(last) = syllables.last_mut() {
            last.text = last.text.trim_end().to_string();
        }
        let background_text = unbracket(&mut background);
        let agent = p.ancestors().find_map(|n| attr(n, "agent"));
        let align = match agent {
            Some(id) if groups.contains(&id) || id == "v1000" => 2,
            Some(id) => {
                let index = singers.iter().position(|s| *s == id).unwrap_or_else(|| {
                    singers.push(id);
                    singers.len() - 1
                });
                (index % 2) as u8
            }
            None => 0,
        };
        let line = if syllables.is_empty() {
            let text: String = p
                .descendants()
                .filter_map(|d| d.text().filter(|_| d.is_text()))
                .collect();
            let Some((start, end)) = span else { continue };
            Line {
                start,
                end,
                text: text.trim().to_string(),
                syllables,
                align,
                ..Line::default()
            }
        } else {
            let start = span
                .map(|s| s.0)
                .unwrap_or(u32::MAX)
                .min(syllables[0].start);
            let end = span
                .map(|s| s.1)
                .unwrap_or(0)
                .max(syllables.iter().chain(&background).map(|s| s.end).max().unwrap_or(0));
            let text = syllables.iter().map(|s| s.text.as_str()).collect();
            Line {
                start,
                end,
                text,
                syllables,
                align,
                background,
                background_text,
            }
        };
        if !line.text.is_empty() {
            lines.push(line);
        }
    }
    lines.sort_by_key(|l| l.start);
    close_lines(&mut lines);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTML: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body>
      <div><p begin="1:03.354" end="1:04.701" ttm:agent="v1"><span begin="1:03.354" end="1:03.757">One</span> <span begin="1:03.757" end="1:04.207">more</span> <span begin="1:04.207" end="1:04.701">time</span></p>
      <p begin="1:04.710" end="1:06.819"><span begin="1:04.710" end="1:04.942">cel</span><span begin="1:04.942" end="1:05.409">e</span><span begin="1:05.409" end="1:06.819">brate</span>
        <span ttm:role="x-bg"><span begin="1:06.0" end="1:06.5">(oh)</span></span></p></div></body></tt>"#;

    #[test]
    fn ttml_keeps_word_boundaries_and_syllable_times() {
        let lines = parse_ttml(TTML);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "One more time");
        assert_eq!(
            lines[0].syllables[1],
            Syllable {
                start: 63757,
                end: 64207,
                text: "more ".into()
            }
        );
        assert_eq!(lines[1].text, "celebrate");
        assert_eq!(lines[1].syllables.len(), 3);
        assert_eq!(lines[1].background_text, "oh");
        assert_eq!(lines[1].background[0].start, 66000);
        assert_eq!(lines[1].end, 66819);
    }

    #[test]
    fn singers_take_sides_and_groups_sit_in_the_middle() {
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
          <head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><ttm:agent type="group" xml:id="v3"/></metadata></head>
          <body><div>
            <p begin="1.0" end="2.0" ttm:agent="v1">first</p>
            <p begin="2.0" end="3.0" ttm:agent="v2">second</p>
            <p begin="3.0" end="4.0" ttm:agent="v3">both</p>
            <p begin="4.0" end="5.0" ttm:agent="v1">first again</p>
          </div></body></tt>"#;
        let aligns: Vec<u8> = parse_ttml(ttml).iter().map(|l| l.align).collect();
        assert_eq!(aligns, [0, 1, 2, 0]);
    }

    #[test]
    fn lrc_lines_end_where_the_next_one_starts() {
        let lines = parse_lrc("[00:10.00] first\n[00:12.50] second\n");
        assert_eq!((lines[0].start, lines[0].end), (10_000, 12_500));
        assert_eq!(lines[1].text, "second");
    }

    #[test]
    fn search_hits_must_be_the_same_recording() {
        let hits: Vec<Value> = serde_json::from_str(
            r#"[{"isrc":"GBDUW0000054","title":"One More Time (Short Radio Edit)","artist":"Daft Punk","duration":235.4},
                {"isrc":"GBDUW0000053","title":"One More Time","artist":"Daft Punk","duration":320.3}]"#,
        )
        .unwrap();
        let q = Query {
            video_id: "",
            title: "One More Time",
            artist: "Daft Punk",
            seconds: 321.0,
        };
        assert_eq!(best_hit(&hits, &q).as_deref(), Some("GBDUW0000053"));
        let live = Query {
            title: "One More Time (Live)",
            ..q
        };
        assert_eq!(best_hit(&hits, &live), None);
    }
}
