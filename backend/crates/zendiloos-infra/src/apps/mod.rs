//! App catalogs over https, read with the catalog's own checks
//! (`zendiloos-catalog`, pinned): an app that fails the format or the
//! checks of its Compose file as written is left out, not repaired. The
//! policy proper is checked on the normalized file, which needs Docker: by
//! the agent, when installing.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zendiloos_apps::application::ports::{CatalogError, CatalogReader, ReadIndex, Skipped};
use zendiloos_apps::domain::{
    Access, CatalogApp, Permissions, Port, Setting, SettingKind, Share, Translation, Web, WebScheme,
};
use zendiloos_catalog as catalog;

/// The most an index may be; the community catalog's is a few dozen KiB.
const MAX_INDEX_BYTES: usize = 8 * 1024 * 1024;

/// [`CatalogReader`] over https.
#[derive(Debug, Clone)]
pub struct HttpsCatalogs {
    tls: Arc<rustls::ClientConfig>,
}

impl HttpsCatalogs {
    pub fn new() -> Self {
        Self { tls: crate::https::client_config() }
    }
}

impl Default for HttpsCatalogs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CatalogReader for HttpsCatalogs {
    async fn fetch(&self, address: &str) -> Result<String, CatalogError> {
        let bytes = crate::https::get(&self.tls, address, MAX_INDEX_BYTES)
            .await
            .map_err(CatalogError::Unreachable)?;
        String::from_utf8(bytes).map_err(|_| CatalogError::NotAnIndex("not text".into()))
    }

    fn read(&self, text: &str) -> Result<ReadIndex, CatalogError> {
        read_index(text)
    }

    async fn fetch_icon(&self, address: &str, icon: &str) -> Result<Vec<u8>, CatalogError> {
        let base = address.rsplit_once('/').map_or(address, |(base, _)| base);
        let bytes = crate::https::get(&self.tls, &format!("{base}/{icon}"), catalog::MAX_ICON_BYTES)
            .await
            .map_err(CatalogError::Unreachable)?;
        match catalog::check_icon(&bytes).first() {
            Some(problem) => Err(CatalogError::NotAnIndex(problem.to_string())),
            None => Ok(bytes),
        }
    }
}

/// The apps of an index that pass the catalog's checks.
pub fn read_index(text: &str) -> Result<ReadIndex, CatalogError> {
    let not_an_index = |reason: String| CatalogError::NotAnIndex(reason);
    let document: Value = serde_json::from_str(text).map_err(|e| not_an_index(format!("not JSON: {e}")))?;
    let format = document.get("format").and_then(Value::as_u64);
    if format != Some(u64::from(catalog::index::INDEX_FORMAT)) {
        return Err(not_an_index(format!(
            "format {} is not the one this version reads ({})",
            format.map_or("none".to_owned(), |f| f.to_string()),
            catalog::index::INDEX_FORMAT
        )));
    }
    let entries =
        document.get("apps").and_then(Value::as_array).ok_or_else(|| not_an_index("no apps".into()))?;
    let mut read = ReadIndex { apps: Vec::new(), skipped: Vec::new() };
    for (position, entry) in entries.iter().enumerate() {
        let name =
            entry.get("id").and_then(Value::as_str).map_or_else(|| format!("#{position}"), str::to_owned);
        match app(entry.clone()) {
            Ok(app) if read.apps.iter().any(|a| a.id == app.id) => {
                read.skipped.push(Skipped { app: name, reason: "listed twice".into() });
            }
            Ok(app) => read.apps.push(app),
            Err(reason) => read.skipped.push(Skipped { app: name, reason }),
        }
    }
    Ok(read)
}

fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One index entry: the manifest plus `compose`, `icon` and `digest`.
fn app(mut entry: Value) -> Result<CatalogApp, String> {
    let object = entry.as_object_mut().ok_or("not an object")?;
    let mut take = |key: &str| {
        object.remove(key).and_then(|v| v.as_str().map(str::to_owned)).ok_or(format!("no `{key}`"))
    };
    let (compose, icon, digest) = (take("compose")?, take("icon")?, take("digest")?);
    let manifest: catalog::Manifest = serde_json::from_value(entry).map_err(|e| format!("manifest: {e}"))?;
    if let Some(problem) = manifest.check(&manifest.id).first() {
        return Err(problem.to_string());
    }
    if let Some(problem) = catalog::compose::check_source(&compose, Some(&manifest)).first() {
        return Err(problem.to_string());
    }
    if icon != format!("icons/{}.png", manifest.id) {
        return Err(format!("the icon must be icons/{}.png", manifest.id));
    }
    if !is_digest(&digest) {
        return Err("not a sha256 digest".into());
    }
    Ok(domain_app(manifest, compose, icon, digest))
}

fn domain_app(m: catalog::Manifest, compose: String, icon: String, digest: String) -> CatalogApp {
    let text = |value: &Value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    CatalogApp {
        categories: m
            .categories
            .iter()
            .map(|c| {
                serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
            })
            .collect(),
        architectures: m.architectures.iter().map(|a| a.as_str().to_owned()).collect(),
        web: m.web.map(|w| Web {
            port: w.port,
            path: w.path,
            scheme: match w.scheme {
                catalog::WebScheme::Http => WebScheme::Http,
                catalog::WebScheme::Https => WebScheme::Https,
            },
        }),
        ports: m
            .ports
            .into_iter()
            .map(|p| Port { id: p.id, label: p.label, default: p.default, fixed: p.fixed })
            .collect(),
        shares: m
            .shares
            .into_iter()
            .map(|s| Share {
                id: s.id,
                label: s.label,
                description: s.description,
                access: match s.access {
                    catalog::Access::ReadOnly => Access::ReadOnly,
                    catalog::Access::ReadWrite => Access::ReadWrite,
                },
            })
            .collect(),
        settings: m
            .settings
            .into_iter()
            .map(|s| Setting {
                id: s.id,
                label: s.label,
                description: s.description,
                kind: match s.kind {
                    catalog::SettingKind::Text => SettingKind::Text,
                    catalog::SettingKind::Password => SettingKind::Password,
                    catalog::SettingKind::Number => SettingKind::Number,
                    catalog::SettingKind::Boolean => SettingKind::Boolean,
                },
                default: s.default.as_ref().map(text),
                required: s.required,
                generate: s.generate,
            })
            .collect(),
        permissions: Permissions {
            devices: m.permissions.devices,
            capabilities: m.permissions.capabilities,
            host_network: m.permissions.host_network,
        },
        translations: m
            .translations
            .into_iter()
            .map(|(language, t)| {
                (
                    language,
                    Translation {
                        name: t.name,
                        tagline: t.tagline,
                        description: t.description,
                        notes: t.notes,
                    },
                )
            })
            .collect(),
        id: m.id,
        name: m.name,
        version: m.version,
        tagline: m.tagline,
        description: m.description,
        developer: m.developer,
        website: m.website,
        source_code: m.source,
        license: m.license,
        notes: m.notes,
        compose,
        icon,
        digest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The community catalog's index as published on 2026-11-06.
    const COMMUNITY: &str = include_str!("fixtures/community-index.json");

    #[test]
    fn reads_the_community_catalog() {
        let read = read_index(COMMUNITY).unwrap();
        assert_eq!(read.skipped, [], "every app passes");
        assert_eq!(read.apps.len(), 13);
        let jellyfin = read.apps.iter().find(|a| a.id == "jellyfin").unwrap();
        assert_eq!(jellyfin.permissions.devices, ["/dev/dri"]);
        assert_eq!(jellyfin.icon, "icons/jellyfin.png");
        assert!(jellyfin.compose.contains("${SHARE_MEDIA}"));
        let immich = read.apps.iter().find(|a| a.id == "immich").unwrap();
        assert!(immich.settings.iter().any(|s| s.id == "DB_PASSWORD" && s.generate));
        assert!(immich.translations["de"].tagline.is_some());
        let home = read.apps.iter().find(|a| a.id == "home-assistant").unwrap();
        assert!(home.permissions.host_network && home.ports[0].fixed);
    }

    #[test]
    fn apps_that_break_the_policy_are_left_out() {
        let mut index: Value = serde_json::from_str(COMMUNITY).unwrap();
        let apps = index["apps"].as_array_mut().unwrap();
        // A file Docker would read on its own, a path out of the catalog,
        // a field the format does not know.
        let first = apps[0]["compose"]
            .as_str()
            .unwrap()
            .replace("restart: unless-stopped", "restart: unless-stopped\n    env_file: /etc/shadow");
        apps[0]["compose"] = first.into();
        apps[1]["icon"] = "../../etc/passwd".into();
        apps[2]["unexpected"] = true.into();
        let id = |i: usize| apps[i]["id"].as_str().unwrap().to_owned();
        let (a, b, c) = (id(0), id(1), id(2));

        let read = read_index(&index.to_string()).unwrap();

        assert_eq!(read.apps.len(), 10);
        let skipped: Vec<&str> = read.skipped.iter().map(|s| s.app.as_str()).collect();
        assert_eq!(skipped, [a.as_str(), b.as_str(), c.as_str()], "{:?}", read.skipped);
    }

    /// Over the network: `cargo test -p zendiloos-infra -- --ignored community`.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn fetches_the_community_catalog_and_an_icon() {
        let catalogs = HttpsCatalogs::new();
        let address = zendiloos_apps::domain::OFFICIAL_INDEX;
        let read = catalogs.read(&catalogs.fetch(address).await.unwrap()).unwrap();
        assert!(read.apps.len() >= 13 && read.skipped.is_empty(), "{:?}", read.skipped);
        let icon = catalogs.fetch_icon(address, &read.apps[0].icon).await.unwrap();
        assert!(icon.starts_with(b"\x89PNG"));
    }

    #[test]
    fn other_formats_are_no_index() {
        for text in ["[]", r#"{"format":2,"apps":[]}"#, "<html>", r#"{"format":1}"#] {
            assert!(matches!(read_index(text), Err(CatalogError::NotAnIndex(_))), "{text}");
        }
    }
}
