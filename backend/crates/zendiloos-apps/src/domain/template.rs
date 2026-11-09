//! An app of a catalog as the template of an administrator's Compose
//! project: its Compose file filled in once with the values given, after
//! which the project is the administrator's like any other
//! (docs/architecture.md, 4.9).
//!
//! The variables (`docs/format.md` of the catalog, section 3) get what an
//! administrator would write by hand: the app's data folder is the
//! project's own folder (`./config`), a share its path on the machine, the
//! containers run as the administrator in the group `zendiloos-files`,
//! whose access every share's folder gives (docs/architecture.md, 4.4).

use std::collections::BTreeMap;

use super::{CatalogApp, InstallValues, WebScheme};

/// What `${APP_DATA}` becomes: the project's own folder, where its Compose
/// file lies and relative paths start.
pub const PROJECT_FOLDER: &str = ".";

/// What the machine says for filling in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    /// The administrator's account, which the containers run as.
    pub uid: u32,
    /// The group `zendiloos-files`, which they run in: the shares' folders
    /// let it read and write.
    pub gid: u32,
    /// `Europe/Berlin`.
    pub time_zone: String,
    /// Of the devices the app may use, those the machine has.
    pub devices: Vec<String>,
}

/// The variable a port is published with: `PORT_WEB`.
pub fn port_variable(id: &str) -> String {
    format!("PORT_{}", id.to_ascii_uppercase().replace('-', "_"))
}

/// The variable a share is mounted with: `SHARE_MEDIA`.
pub fn share_variable(id: &str) -> String {
    format!("SHARE_{}", id.to_ascii_uppercase().replace('-', "_"))
}

/// The values of the variables `app`'s Compose file uses. `shares` has the
/// path on the machine of each share picked, by the app's share id
/// (`/volume1/media/Films`).
pub fn template_variables(
    app: &CatalogApp,
    values: &InstallValues,
    shares: &BTreeMap<String, String>,
    facts: &HostFacts,
) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::from([
        ("APP_DATA".to_owned(), PROJECT_FOLDER.to_owned()),
        ("PUID".to_owned(), facts.uid.to_string()),
        ("PGID".to_owned(), facts.gid.to_string()),
        ("TZ".to_owned(), facts.time_zone.clone()),
    ]);
    for share in &app.shares {
        if let Some(path) = shares.get(&share.id) {
            variables.insert(share_variable(&share.id), path.clone());
        }
    }
    for port in &app.ports {
        if let Some(number) = values.ports.get(&port.id) {
            variables.insert(port_variable(&port.id), number.to_string());
        }
    }
    for setting in &app.settings {
        let value = values.settings.get(&setting.id).cloned().unwrap_or_default();
        variables.insert(setting.id.clone(), value);
    }
    variables
}

/// Why a text cannot be filled in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// `${NAME:?…}` or `${NAME?…}` of a variable without a value.
    #[error("{name}: {message}")]
    Required { name: String, message: String },
    #[error("a `${{` without its `}}`")]
    Unclosed,
}

/// `text` as Compose interpolates it with `variables`: `$NAME`, `${NAME}`,
/// `${NAME:-default}`, `${NAME-default}`, `${NAME:+other}`,
/// `${NAME+other}`, `${NAME:?error}`, `${NAME?error}`, with defaults
/// interpolated in turn; `$$` is a `$`. A variable not given is unset; a
/// lone `$` stays. The result is the value itself, `$` and all: written
/// into a Compose file again, it needs [`escape`].
pub fn interpolate(text: &str, variables: &BTreeMap<String, String>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(next) = after.strip_prefix('$') {
            out.push('$');
            rest = next;
        } else if let Some(braced) = after.strip_prefix('{') {
            let end = closing(braced).ok_or(TemplateError::Unclosed)?;
            out.push_str(&expand(&braced[..end], variables)?);
            rest = &braced[end + 1..];
        } else {
            let length = name_length(after);
            if length == 0 {
                out.push('$');
            } else {
                out.push_str(variables.get(&after[..length]).map_or("", String::as_str));
            }
            rest = &after[length..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// A value written into a Compose file: `$` doubled, so that Compose,
/// interpolating the file again, leaves it as it is.
pub fn escape(value: &str) -> String {
    value.replace('$', "$$")
}

/// The length of the variable name `text` starts with.
fn name_length(text: &str) -> usize {
    let bytes = text.as_bytes();
    if !bytes.first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
        return 0;
    }
    bytes.iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'_').count()
}

/// Where the `}` that closes a `${` is in `text` (which follows the `${`),
/// past the ones of `${…}` inside.
fn closing(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if bytes.get(i + 1) == Some(&b'$') => i += 1,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 1;
            }
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// What is between `${` and `}`: a name, perhaps with a modifier.
fn expand(inner: &str, variables: &BTreeMap<String, String>) -> Result<String, TemplateError> {
    let length = name_length(inner);
    let (name, modifier) = inner.split_at(length);
    let value = variables.get(name);
    let set = value.is_some();
    let filled = value.is_some_and(|v| !v.is_empty());
    let own = || value.cloned().unwrap_or_default();
    let (colon, modifier) = match modifier.strip_prefix(':') {
        Some(rest) => (true, rest),
        None => (false, modifier),
    };
    let present = if colon { filled } else { set };
    let Some(operator) = modifier.chars().next() else { return Ok(own()) };
    let argument = &modifier[1..];
    match operator {
        '-' if present => Ok(own()),
        '-' => interpolate(argument, variables),
        '+' if present => interpolate(argument, variables),
        '+' => Ok(String::new()),
        '?' if present => Ok(own()),
        '?' => Err(TemplateError::Required { name: name.to_owned(), message: argument.to_owned() }),
        // Not a form Compose knows: left as it was written.
        _ => Ok(format!("${{{inner}}}")),
    }
}

/// Where a port the app publishes lands in its containers, from its
/// Compose file as the catalog wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortTarget {
    /// `${PORT_WEB}:8096`: the container's port 8096.
    Container(u16),
    /// `${PORT_WEB}:${PORT_WEB}`: the same port inside as outside.
    Same,
}

/// How `compose` publishes the port `id`, if it says in the short form.
pub fn port_target(compose: &str, id: &str) -> Option<PortTarget> {
    let variable = format!("${{{}}}:", port_variable(id));
    compose.match_indices(&variable).find_map(|(at, found)| {
        let after = &compose[at + found.len()..];
        if after.starts_with(&variable[..variable.len() - 1]) {
            return Some(PortTarget::Same);
        }
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok().map(PortTarget::Container)
    })
}

/// Where an app made from a template answers, from what its containers
/// publish now (container port, host port): the catalog's web port, found
/// where the template put it, so a port changed in the file since is
/// followed. A fixed port is where it always is.
pub fn web_address(app: &CatalogApp, published: &[(u16, u16)]) -> Option<super::WebAddress> {
    let web = app.web.as_ref()?;
    let port = app.ports.iter().find(|port| port.id == web.port)?;
    let host = if port.fixed {
        port.default
    } else {
        match port_target(&app.compose, &port.id)? {
            PortTarget::Container(inside) => published.iter().find(|(target, _)| *target == inside)?.1,
            PortTarget::Same => published.iter().find(|(target, host)| target == host)?.1,
        }
    };
    Some(super::WebAddress { scheme: web.scheme, port: host, path: web.path.clone() })
}

impl WebScheme {
    pub fn is_https(self) -> bool {
        self == WebScheme::Https
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Access, Port, Share, Web};

    fn variables(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn variables_are_filled_in_as_compose_does() {
        let given = variables(&[("A", "x"), ("EMPTY", ""), ("PATH_", "/p")]);
        let cases = [
            ("${A}/config", "x/config"),
            ("$A-$PATH_", "x-/p"),
            ("${UNSET}", ""),
            ("${EMPTY:-fallback} ${EMPTY-kept}", "fallback "),
            ("${UNSET:-${A}y} ${UNSET-$A}", "xy x"),
            ("${A:+yes} ${EMPTY:+no} ${EMPTY+set} ${UNSET+no}", "yes  set "),
            ("${A:?needed}", "x"),
            ("price: 5$ and $$A and $", "price: 5$ and $A and $"),
            ("$1 ${A}", "$1 x"),
            ("${A:-a}b}", "xb}"),
            ("${A%%x}", "${A%%x}"),
        ];
        for (text, expected) in cases {
            assert_eq!(interpolate(text, &given).unwrap(), expected, "{text}");
        }
        assert_eq!(
            interpolate("${EMPTY:?is needed}", &given),
            Err(TemplateError::Required { name: "EMPTY".into(), message: "is needed".into() })
        );
        assert!(interpolate("${UNSET?x}", &given).is_err());
        assert_eq!(interpolate("${EMPTY?x}", &given).unwrap(), "");
        assert_eq!(interpolate("${A", &given), Err(TemplateError::Unclosed));
        assert_eq!(interpolate("${A:-${B}", &given), Err(TemplateError::Unclosed));
    }

    #[test]
    fn values_with_dollars_stay_as_they_are() {
        let given = variables(&[("P", "a$b$$c${D}")]);
        let written = escape(&interpolate("pw=${P}", &given).unwrap());
        assert_eq!(written, "pw=a$$b$$$$c$${D}");
        // Compose reads what is written back as the value.
        assert_eq!(interpolate(&written, &BTreeMap::new()).unwrap(), "pw=a$b$$c${D}");
    }

    fn jellyfin() -> CatalogApp {
        let mut app = crate::testing::app("jellyfin", "Jellyfin");
        app.compose = "services:\n  jellyfin:\n    ports:\n      - ${PORT_WEB}:8096\n      - ${PORT_PEER}:${PORT_PEER}/udp\n".into();
        app.web = Some(Web { port: "web".into(), path: "/web".into(), scheme: WebScheme::Http });
        app.ports.push(Port { id: "peer".into(), label: "Peer".into(), default: 6881, fixed: false });
        app.shares = vec![Share {
            id: "media-files".into(),
            label: "Media".into(),
            description: None,
            access: Access::ReadOnly,
        }];
        app
    }

    #[test]
    fn a_template_gets_the_projects_folder_the_shares_paths_and_the_administrator() {
        let app = jellyfin();
        let mut values = InstallValues::default();
        values.ports.insert("web".into(), 8097);
        values.ports.insert("peer".into(), 6881);
        let shares = BTreeMap::from([("media-files".to_owned(), "/volume1/media/Films".to_owned())]);
        let facts = HostFacts { uid: 2001, gid: 1500, time_zone: "Europe/Berlin".into(), devices: vec![] };

        let filled = template_variables(&app, &values, &shares, &facts);

        assert_eq!(
            filled,
            variables(&[
                ("APP_DATA", "."),
                ("PGID", "1500"),
                ("PORT_PEER", "6881"),
                ("PORT_WEB", "8097"),
                ("PUID", "2001"),
                ("SHARE_MEDIA_FILES", "/volume1/media/Films"),
                ("TZ", "Europe/Berlin"),
            ])
        );
        assert_eq!(interpolate("${APP_DATA}/config", &filled).unwrap(), "./config");
    }

    #[test]
    fn the_web_interface_is_found_where_the_template_published_it() {
        let mut app = jellyfin();
        assert_eq!(port_target(&app.compose, "web"), Some(PortTarget::Container(8096)));
        assert_eq!(port_target(&app.compose, "peer"), Some(PortTarget::Same));
        assert_eq!(port_target(&app.compose, "api"), None);

        let web = web_address(&app, &[(6881, 6881), (8096, 9000)]).unwrap();
        assert_eq!((web.port, web.path.as_str(), web.scheme.is_https()), (9000, "/web", false));
        assert_eq!(web_address(&app, &[]), None, "not published: not running");

        app.web.as_mut().unwrap().port = "peer".into();
        assert_eq!(web_address(&app, &[(8096, 9000), (6881, 6882), (6883, 6883)]).unwrap().port, 6883);
        app.ports[1].fixed = true;
        assert_eq!(web_address(&app, &[]).unwrap().port, 6881, "a fixed port is always there");
        app.web = None;
        assert_eq!(web_address(&app, &[(8096, 9000)]), None);
    }
}
