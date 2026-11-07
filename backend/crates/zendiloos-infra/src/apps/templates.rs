//! Filling in an app's Compose file: [`ComposeFiller`] on the YAML parser's
//! events, which know where each value is in the text.
//!
//! Compose interpolates the values of a file after reading it as YAML, so
//! that is where the variables are filled in: each value with a variable
//! is interpolated as Compose would (`zendiloos_apps::domain::interpolate`)
//! and put back in its place, quoted so that it reads back as exactly that
//! value, `$` doubled so that Compose leaves it alone. Everything else, the
//! comments and the layout too, stays as written. The result is read again
//! and must give the same document with the values filled in, or it is
//! refused.

use std::collections::BTreeMap;

use granit_parser::{Event, Parser, ScalarStyle, Span};
use zendiloos_apps::application::ports::ComposeFiller;
use zendiloos_apps::domain::{escape, interpolate};

/// [`ComposeFiller`] that keeps the file as written around the values.
#[derive(Debug, Clone, Copy, Default)]
pub struct YamlFiller;

impl ComposeFiller for YamlFiller {
    fn fill(
        &self,
        compose: &str,
        variables: &BTreeMap<String, String>,
        absent: &[String],
    ) -> Result<String, String> {
        let without = leave_out_devices(compose, absent)?;
        fill_values(&without, variables)
    }
}

/// A node of the document, with what matters here.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Scalar {
        value: String,
        style: ScalarStyle,
        /// In bytes.
        start: usize,
        end: usize,
        /// A key of a mapping, which Compose does not interpolate.
        key: bool,
        /// The keys from the root down to the mapping or sequence it is in.
        path: Vec<String>,
        /// Where the block sequence it is an item of starts, if it is one.
        sequence: Option<usize>,
    },
    /// Anything else, by kind: the document's shape.
    Other(&'static str),
}

/// Where a mapping or sequence is while walking.
enum Frame {
    Mapping { key: Option<String>, expecting_key: bool },
    Sequence { block: bool, start: usize },
}

fn bytes(span: &Span) -> Result<(usize, usize), String> {
    match (span.start.byte_offset(), span.end.byte_offset()) {
        (Some(start), Some(end)) if start <= end => Ok((start, end)),
        _ => Err("the parser gave no place in the text".into()),
    }
}

/// The nodes of `text` in their order.
fn nodes(text: &str) -> Result<Vec<Node>, String> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut nodes = Vec::new();
    // The keys down to a node: a key itself is not inside the key its
    // mapping had before it.
    let path = |frames: &[Frame], key: bool| -> Vec<String> {
        let above = if key { &frames[..frames.len() - 1] } else { frames };
        above
            .iter()
            .filter_map(|frame| match frame {
                Frame::Mapping { key, .. } => Some(key.clone().unwrap_or_default()),
                Frame::Sequence { .. } => None,
            })
            .collect()
    };
    // A node is done: in a mapping, a key or a value is.
    let done = |frames: &mut Vec<Frame>| {
        if let Some(Frame::Mapping { expecting_key, .. }) = frames.last_mut() {
            *expecting_key = !*expecting_key;
        }
    };
    for item in Parser::new_from_str(text) {
        let (event, span) = item.map_err(|error| error.to_string())?;
        match event {
            Event::Scalar(value, style, _, _) => {
                let (start, end) = bytes(&span)?;
                let key = matches!(frames.last(), Some(Frame::Mapping { expecting_key: true, .. }));
                let sequence = match frames.last() {
                    Some(Frame::Sequence { block: true, start }) => Some(*start),
                    _ => None,
                };
                let value = value.into_owned();
                nodes.push(Node::Scalar {
                    value: value.clone(),
                    style,
                    start,
                    end,
                    key,
                    path: path(&frames, key),
                    sequence,
                });
                if key && let Some(Frame::Mapping { key: current, .. }) = frames.last_mut() {
                    *current = Some(value);
                }
                done(&mut frames);
            }
            Event::Alias(_) => {
                nodes.push(Node::Other("alias"));
                done(&mut frames);
            }
            Event::SequenceStart(..) => {
                let (start, end) = bytes(&span)?;
                // A flow sequence starts with its `[`; a block one has no text.
                let block = start == end;
                nodes.push(Node::Other("sequence"));
                frames.push(Frame::Sequence { block, start });
            }
            Event::MappingStart(..) => {
                nodes.push(Node::Other("mapping"));
                frames.push(Frame::Mapping { key: None, expecting_key: true });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                nodes.push(Node::Other("end"));
                frames.pop();
                done(&mut frames);
            }
            Event::DocumentStart(..) => nodes.push(Node::Other("document")),
            _ => {}
        }
    }
    Ok(nodes)
}

/// `value` as a YAML double-quoted scalar, which reads back as exactly it.
fn double_quoted(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for c in value.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                quoted.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Whether `value` may stand as a plain scalar and still read as that
/// text: a path such as `./config:/config` or `Europe/Berlin`. A `/` keeps
/// it from reading as a number, a boolean or null.
fn plain_is_safe(value: &str) -> bool {
    let starts =
        value.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '_'));
    starts
        && value.contains('/')
        && !value.ends_with(':')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '=' | '+' | '-'))
}

/// The value a scalar of the template gets, in Compose's notation.
fn filled(value: &str, variables: &BTreeMap<String, String>) -> Result<String, String> {
    interpolate(value, variables).map(|value| escape(&value)).map_err(|error| error.to_string())
}

/// The line `at` is on, as the byte range from its start to its end.
fn line_around(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    (start, end)
}

/// The value scalars with a variable filled in; the rest as written.
fn fill_values(text: &str, variables: &BTreeMap<String, String>) -> Result<String, String> {
    let parsed = nodes(text)?;
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut expected = Vec::with_capacity(parsed.len());
    for node in &parsed {
        let Node::Scalar { value, style, start, end, key: false, .. } = node else {
            expected.push(node.clone());
            continue;
        };
        let new = if value.contains('$') { filled(value, variables)? } else { value.clone() };
        if new != *value {
            let raw = &text[*start..*end];
            let replacement = match style {
                ScalarStyle::Plain if plain_is_safe(&new) => new.clone(),
                ScalarStyle::Literal | ScalarStyle::Folded => {
                    // A block's lines are its text as written: each filled in
                    // where it is, the indentation and the indicator kept.
                    raw.split('\n')
                        .map(|line| filled(line, variables))
                        .collect::<Result<Vec<_>, _>>()?
                        .join("\n")
                }
                _ => double_quoted(&new),
            };
            replacements.push((*start, *end, replacement));
        }
        let mut node = node.clone();
        if let Node::Scalar { value, .. } = &mut node {
            *value = new;
        }
        expected.push(node);
    }
    let mut out = text.to_owned();
    for (start, end, replacement) in replacements.into_iter().rev() {
        out.replace_range(start..end, &replacement);
    }
    let differently = || "the file would read differently once filled in".to_owned();
    let read = nodes(&out).map_err(|_| differently())?;
    if nodes_shape(&read) != nodes_shape(&expected) {
        return Err(differently());
    }
    Ok(out)
}

/// The document as Compose sees it: kinds and values, not where or how
/// they are written.
fn nodes_shape(nodes: &[Node]) -> Vec<(bool, String)> {
    nodes
        .iter()
        .map(|node| match node {
            Node::Scalar { value, key, .. } => (*key, value.clone()),
            Node::Other(kind) => (false, format!("\0{kind}")),
        })
        .collect()
}

/// A service's block sequence of devices: the keys down to it, and each
/// item's line with whether it goes.
type DeviceList = (Vec<String>, Vec<(usize, bool)>);

/// `text` without the devices in `absent` (by their path on the machine),
/// each line of one commented out, and the `devices:` of a service with
/// none left too. A device written otherwise than as an item of its own
/// line stays.
fn leave_out_devices(text: &str, absent: &[String]) -> Result<String, String> {
    if absent.is_empty() {
        return Ok(text.to_owned());
    }
    let parsed = nodes(text)?;
    let is_devices = |path: &[String]| path.len() == 3 && path[0] == "services" && path[2] == "devices";
    // Each block sequence of devices: its items' lines, and how many go.
    let mut sequences: BTreeMap<usize, DeviceList> = BTreeMap::new();
    let mut keys: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for node in &parsed {
        let Node::Scalar { value, start, end, key, path, sequence, .. } = node else { continue };
        if *key {
            let mut at = path.clone();
            at.push(value.clone());
            keys.insert(at, *start);
            continue;
        }
        let Some(sequence) = sequence.filter(|_| is_devices(path)) else { continue };
        let source = value.split(':').next().unwrap_or_default();
        let (line_start, line_end) = line_around(text, *start);
        let own_line = !text[*start..*end].contains('\n')
            && *end <= line_end
            && text[line_start..*start].trim() == "-"
            && text[*end..line_end].trim_start().is_empty_or_comment();
        let entry = sequences.entry(sequence).or_insert_with(|| (path.clone(), Vec::new()));
        entry.1.push((line_start, own_line && absent.iter().any(|device| device == source)));
    }
    let mut lines: Vec<usize> = Vec::new();
    for (path, items) in sequences.values() {
        let going: Vec<usize> = items.iter().filter(|(_, goes)| *goes).map(|(line, _)| *line).collect();
        if going.len() == items.len()
            && let Some(key) = keys.get(path)
        {
            let (line_start, line_end) = line_around(text, *key);
            let rest = text[*key..line_end].trim_start_matches("devices").trim_start();
            if rest.strip_prefix(':').is_some_and(|after| after.trim_start().is_empty_or_comment()) {
                lines.push(line_start);
            }
        }
        lines.extend(going);
    }
    lines.sort_unstable();
    lines.dedup();
    let mut out = text.to_owned();
    for line in lines.into_iter().rev() {
        let indent = text[line..].len() - text[line..].trim_start_matches(' ').len();
        out.insert_str(line + indent, "# ");
    }
    // What is left must still be a file to read.
    nodes(&out)?;
    Ok(out)
}

trait EmptyOrComment {
    fn is_empty_or_comment(&self) -> bool;
}

impl EmptyOrComment for str {
    fn is_empty_or_comment(&self) -> bool {
        self.is_empty() || self.starts_with('#')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variables(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect()
    }

    /// What `text` holds once read, its values as Compose interpolates
    /// them without any variable: what runs.
    fn values(text: &str) -> Vec<(bool, String)> {
        nodes(text)
            .unwrap()
            .into_iter()
            .map(|node| match node {
                Node::Scalar { value, key: false, .. } => {
                    (false, interpolate(&value, &BTreeMap::new()).unwrap())
                }
                Node::Scalar { value, key: true, .. } => (true, value),
                Node::Other(kind) => (false, format!("\0{kind}")),
            })
            .collect()
    }

    /// What `text` holds with the variables filled in as Compose would.
    fn interpolated(text: &str, given: &BTreeMap<String, String>) -> Vec<(bool, String)> {
        nodes(text)
            .unwrap()
            .into_iter()
            .map(|node| match node {
                Node::Scalar { value, key: false, .. } => (false, interpolate(&value, given).unwrap()),
                Node::Scalar { value, key: true, .. } => (true, value),
                Node::Other(kind) => (false, format!("\0{kind}")),
            })
            .collect()
    }

    const TEMPLATE: &str = "\
# Media for everyone.
services:
  jellyfin:
    image: lscr.io/linuxserver/jellyfin:10  # pinned
    user: \"${PUID}:${PGID}\"
    command: [--port, '${PORT_WEB}', --tz=$TZ]
    environment:
      TZ: ${TZ}
      PASSWORD: ${PASSWORD}
      PRICE: 5$$ a month
      RADARR: |
        - base_url: \"${URL}\"
          api_key: \"${PASSWORD}\"
      EMPTY: ${UNSET:-fall back}
    ports:
      - ${PORT_WEB}:8096
    volumes:
      - ${APP_DATA}/config:/config
      - ${SHARE_MEDIA}:/media:ro
";

    #[test]
    fn values_are_filled_in_where_they_are_and_the_rest_stays() {
        let given = variables(&[
            ("PUID", "2001"),
            ("PGID", "1500"),
            ("PORT_WEB", "8097"),
            ("TZ", "Europe/Berlin"),
            ("PASSWORD", "abc"),
            ("URL", "http://radarr:7878"),
            ("APP_DATA", "."),
            ("SHARE_MEDIA", "/volume1/media/Films"),
        ]);
        let template = TEMPLATE;

        let filled = fill_values(template, &given).unwrap();

        assert_eq!(
            filled,
            "\
# Media for everyone.
services:
  jellyfin:
    image: lscr.io/linuxserver/jellyfin:10  # pinned
    user: \"2001:1500\"
    command: [--port, \"8097\", \"--tz=Europe/Berlin\"]
    environment:
      TZ: Europe/Berlin
      PASSWORD: \"abc\"
      PRICE: 5$$ a month
      RADARR: |
        - base_url: \"http://radarr:7878\"
          api_key: \"abc\"
      EMPTY: \"fall back\"
    ports:
      - \"8097:8096\"
    volumes:
      - ./config:/config
      - /volume1/media/Films:/media:ro
"
        );
        assert_eq!(values(&filled), interpolated(template, &given));
    }

    #[test]
    fn any_value_reads_back_as_itself() {
        let template = "services:\n  db:\n    environment:\n      A: ${V}\n      B: '${V}'\n      C: \"x ${V} y\"\n    command: [run, \"${V}\"]\n    entrypoint:\n      - ${V}\n";
        let nasty = [
            "p@ss: word # not a comment",
            "\"quoted\" and 'single'",
            "$HOME and $$ and ${X}",
            "back\\slash",
            "- dash",
            "true",
            "~",
            "123",
            "[a, b]",
            "{a: b}",
            "&anchor *alias !tag",
            "ünïcödé ✓",
            "tab\there",
            "",
            " leading and trailing ",
            "%percent",
            "`tick`",
            "a\u{85}b\u{2028}c",
        ];
        for value in nasty {
            let given = variables(&[("V", value)]);
            let filled = fill_values(template, &given).unwrap_or_else(|e| panic!("{value:?}: {e}"));
            assert_eq!(values(&filled), interpolated(template, &given), "{value:?}:\n{filled}");
        }
    }

    #[test]
    fn a_block_that_would_read_otherwise_is_refused() {
        let template = "services:\n  app:\n    environment:\n      CONFIG: |\n        ${V}\n        rest\n";
        let given = variables(&[("V", "  indented")]);
        assert_eq!(
            fill_values(template, &given),
            Err("the file would read differently once filled in".to_owned())
        );
        let plain = variables(&[("V", "first")]);
        let filled = fill_values(template, &plain).unwrap();
        assert_eq!(
            filled,
            "services:\n  app:\n    environment:\n      CONFIG: |\n        first\n        rest\n"
        );
    }

    #[test]
    fn keys_and_errors() {
        let given = variables(&[("V", "x")]);
        let keyed = "services:\n  ${V}:\n    image: ${V}\n";
        assert_eq!(fill_values(keyed, &given).unwrap(), "services:\n  ${V}:\n    image: \"x\"\n");
        assert!(fill_values("services: [", &given).is_err());
        let required = fill_values("services:\n  a:\n    image: ${NONE:?is needed}\n", &given);
        assert_eq!(required, Err("NONE: is needed".to_owned()));
        assert!(plain_is_safe("./x:/y"));
        for unsafe_plain in ["8096:80", "true", "-/x", "a /b", "/x:", "#/x", "[/x]", ""] {
            assert!(!plain_is_safe(unsafe_plain), "{unsafe_plain}");
        }
    }

    #[test]
    fn devices_the_machine_lacks_are_left_out() {
        let template = "\
services:
  ha:
    devices:
      - /dev/ttyUSB0:/dev/ttyUSB0
      - /dev/ttyACM0:/dev/ttyACM0  # a stick
    volumes:
      - /dev/ttyUSB0:/x
  transcode:
    devices:
      - /dev/dri:/dev/dri
      - /dev/ttyUSB0
  flow:
    devices: [/dev/ttyUSB0]
";
        let absent = ["/dev/ttyUSB0".to_owned(), "/dev/ttyACM0".to_owned()];

        let left = leave_out_devices(template, &absent).unwrap();

        assert_eq!(
            left,
            "\
services:
  ha:
    # devices:
      # - /dev/ttyUSB0:/dev/ttyUSB0
      # - /dev/ttyACM0:/dev/ttyACM0  # a stick
    volumes:
      - /dev/ttyUSB0:/x
  transcode:
    devices:
      - /dev/dri:/dev/dri
      # - /dev/ttyUSB0
  flow:
    devices: [/dev/ttyUSB0]
"
        );
        assert_eq!(leave_out_devices(template, &[]).unwrap(), template);
        let filled = YamlFiller.fill(template, &BTreeMap::new(), &absent).unwrap();
        assert_eq!(filled, left);
    }
}
