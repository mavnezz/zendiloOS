//! What Docker Compose prints for a file (`docker compose config --format
//! json`), as far as the simulation needs it: the file read as YAML, the
//! short forms written out as Compose writes them, relative paths from the
//! project's folder and the project's own names. It is no Compose, but
//! what zendiloosd reads of it (what a file lets containers do, the
//! catalog's policy on an app's template) comes out in development as it
//! would on a NAS.

use serde_json::{Map, Value, json};

/// `text` as Compose reads it for the project `project` in `directory`.
pub(crate) fn normalized(project: &str, directory: &str, text: &str) -> Result<Value, String> {
    let mut file: Value = serde_saphyr::from_str(text).map_err(|error| format!("yaml: {error}"))?;
    let root = file.as_object_mut().ok_or("the file is no mapping")?;
    let services = root.get_mut("services").and_then(Value::as_object_mut).ok_or("no services")?;
    let mut on_default = false;
    for service in services.values_mut().filter_map(Value::as_object_mut) {
        if let Some(Value::Array(volumes)) = service.get_mut("volumes") {
            for volume in volumes.iter_mut() {
                *volume = mount(volume, directory);
            }
        }
        if let Some(Value::Array(ports)) = service.get_mut("ports") {
            for port in ports.iter_mut() {
                *port = published(port);
            }
        }
        if let Some(Value::Array(devices)) = service.get_mut("devices") {
            for device in devices.iter_mut() {
                *device = device_mapping(device);
            }
        }
        if let Some(Value::Array(pairs)) = service.get("environment") {
            let environment: Map<String, Value> = pairs
                .iter()
                .filter_map(Value::as_str)
                .map(|pair| match pair.split_once('=') {
                    Some((name, value)) => (name.to_owned(), Value::String(value.to_owned())),
                    None => (pair.to_owned(), Value::Null),
                })
                .collect();
            service.insert("environment".into(), Value::Object(environment));
        }
        if let Some(Value::String(test)) = service.get("healthcheck").and_then(|h| h.get("test")).cloned() {
            service["healthcheck"]["test"] = json!(["CMD-SHELL", test]);
        }
        match service.get("networks") {
            Some(Value::Array(names)) => {
                let joined: Map<String, Value> = names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|name| (name.to_owned(), Value::Null))
                    .collect();
                on_default |= joined.contains_key("default");
                service.insert("networks".into(), Value::Object(joined));
            }
            Some(Value::Object(joined)) => on_default |= joined.contains_key("default"),
            Some(_) => {}
            None if !service.contains_key("network_mode") => {
                service.insert("networks".into(), json!({ "default": null }));
                on_default = true;
            }
            None => {}
        }
    }
    if on_default {
        let networks = root.entry("networks").or_insert_with(|| json!({}));
        if let Some(networks) = networks.as_object_mut() {
            networks.entry("default").or_insert_with(|| json!({}));
        }
    }
    for kind in ["networks", "volumes"] {
        for (key, entry) in root.get_mut(kind).and_then(Value::as_object_mut).into_iter().flatten() {
            if entry.is_null() {
                *entry = json!({});
            }
            let Some(entry) = entry.as_object_mut() else { continue };
            let external = entry.get("external").is_some_and(|external| external != &Value::Bool(false));
            if !external && !entry.contains_key("name") {
                entry.insert("name".into(), Value::String(format!("{project}_{key}")));
            }
            if kind == "networks" && !external {
                entry.entry("ipam").or_insert_with(|| json!({}));
            }
        }
    }
    root.insert("name".into(), Value::String(project.to_owned()));
    Ok(file)
}

/// `path` from `directory` if it is relative, `.` and `..` taken out as
/// Compose does; an absolute one as written.
fn resolved(path: &str, directory: &str) -> String {
    if path.starts_with('/') {
        return path.to_owned();
    }
    let mut names: Vec<&str> = directory.split('/').filter(|name| !name.is_empty()).collect();
    for name in path.split('/') {
        match name {
            "" | "." => {}
            ".." => {
                names.pop();
            }
            name => names.push(name),
        }
    }
    format!("/{}", names.join("/"))
}

/// A service's mount in Compose's long form: `./data:/data:ro` is a bind,
/// `cache:/cache` a named volume, `/cache` an anonymous one.
fn mount(entry: &Value, directory: &str) -> Value {
    let Some(text) = entry.as_str() else {
        let mut entry = entry.clone();
        let bind = entry.get("type").and_then(Value::as_str) == Some("bind");
        if let (true, Some(source)) = (bind, entry.get("source").and_then(Value::as_str)) {
            entry["source"] = Value::String(resolved(source, directory));
        }
        return entry;
    };
    let parts: Vec<&str> = text.split(':').collect();
    let (source, target, mode) = match parts.as_slice() {
        [target] => return json!({ "type": "volume", "target": target, "volume": {} }),
        [source, target] => (*source, *target, ""),
        [source, target, mode, ..] => (*source, *target, *mode),
        [] => return entry.clone(),
    };
    let mut mount = if source.starts_with(['/', '.', '~']) {
        json!({ "type": "bind", "source": resolved(source, directory), "target": target,
                "bind": { "create_host_path": true } })
    } else {
        json!({ "type": "volume", "source": source, "target": target, "volume": {} })
    };
    if mode.split(',').any(|option| option == "ro") {
        mount["read_only"] = Value::Bool(true);
    }
    mount
}

/// A published port in Compose's long form: `8080:80/udp`, `127.0.0.1:8080:80`.
fn published(entry: &Value) -> Value {
    let text = match entry {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => return other.clone(),
    };
    let (ports, protocol) = text.split_once('/').unwrap_or((&text, "tcp"));
    let parts: Vec<&str> = ports.split(':').collect();
    let number =
        |text: &str| text.parse::<u16>().map_or_else(|_| Value::String(text.to_owned()), Value::from);
    let (host_ip, host, target) = match parts.as_slice() {
        [target] => (None, None, *target),
        [host, target] => (None, Some(*host), *target),
        [ip, host, target, ..] => (Some(*ip), Some(*host), *target),
        [] => return entry.clone(),
    };
    let mut port = json!({ "mode": "ingress", "target": number(target), "protocol": protocol });
    if let Some(host) = host {
        port["published"] = Value::String(host.to_owned());
    }
    if let Some(ip) = host_ip {
        port["host_ip"] = Value::String(ip.to_owned());
    }
    port
}

/// A device in Compose's long form: `/dev/ttyUSB0:/dev/ttyUSB0:rwm`.
fn device_mapping(entry: &Value) -> Value {
    let Some(text) = entry.as_str() else { return entry.clone() };
    let mut parts = text.splitn(3, ':');
    let source = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or(source);
    let permissions = parts.next().unwrap_or("rwm");
    json!({ "source": source, "target": target, "permissions": permissions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_forms_come_out_as_compose_writes_them() {
        let text = "services:\n  app:\n    image: nginx\n    restart: unless-stopped\n    environment:\n      - TZ=UTC\n    \
                    ports:\n      - \"8080:80\"\n      - 9000:9000/udp\n    volumes:\n      - ./data:/data\n      \
                    - ../shared:/shared:ro\n      - cache:/cache\n    devices:\n      - /dev/dri:/dev/dri\n    \
                    healthcheck:\n      test: curl -f localhost\n  db:\n    image: postgres\n    network_mode: host\n\
                    volumes:\n  cache:\n";

        let read = normalized("web", "/volume1/docker/web", text).unwrap();

        assert_eq!(read["name"], "web");
        let app = &read["services"]["app"];
        assert_eq!(app["environment"], json!({ "TZ": "UTC" }));
        assert_eq!(
            app["ports"][0],
            json!({ "mode": "ingress", "target": 80, "published": "8080", "protocol": "tcp" })
        );
        assert_eq!(app["ports"][1]["protocol"], "udp");
        assert_eq!(app["volumes"][0]["source"], "/volume1/docker/web/data");
        assert_eq!(app["volumes"][1]["source"], "/volume1/docker/shared");
        assert_eq!(app["volumes"][1]["read_only"], true);
        assert_eq!(app["volumes"][2]["type"], "volume");
        assert_eq!(
            app["devices"][0],
            json!({ "source": "/dev/dri", "target": "/dev/dri", "permissions": "rwm" })
        );
        assert_eq!(app["healthcheck"]["test"], json!(["CMD-SHELL", "curl -f localhost"]));
        assert_eq!(app["networks"], json!({ "default": null }));
        assert_eq!(read["services"]["db"].get("networks"), None, "on the host's network");
        assert_eq!(read["networks"]["default"], json!({ "name": "web_default", "ipam": {} }));
        assert_eq!(read["volumes"]["cache"], json!({ "name": "web_cache" }));
        assert!(normalized("web", "/volume1/x", "broken: [").is_err());
        assert!(normalized("web", "/volume1/x", "volumes: {}\n").is_err(), "no services");
    }
}
