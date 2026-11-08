//! The rules of docs/architecture.md (2.1), checked against the workspace. A
//! crate that breaks them fails here, naming the crate and the dependency or
//! the file and the path, instead of in a review.
//!
//! - which internal crate may depend on which: the table of 2.1, one row per
//!   crate, so a new crate needs its row first
//! - the core (contexts, kernel, jobs, notify) uses no outside crates but
//!   errors, async ports, tokio and tracing, and does no I/O
//! - domain layers know neither their application layer nor tokio
//! - the agent side knows no database and no HTTP

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use syn::visit::{self, Visit};

/// The bounded contexts.
const CONTEXTS: &[&str] = &[
    "zendiloos-identity",
    "zendiloos-system",
    "zendiloos-storage",
    "zendiloos-sharing",
    "zendiloos-apps",
    "zendiloos-containers",
    "zendiloos-updates",
    "zendiloos-network",
    "zendiloos-certificates",
];

/// What the contexts share: the kernel, jobs and notifications.
const SHARED: &[&str] = &["zendiloos-kernel", "zendiloos-jobs", "zendiloos-notify"];

/// Root and what runs beside it: it knows nothing of contexts, the database
/// or HTTP, so a bug in a parser, a repository or a handler never runs as root.
const AGENT_SIDE: &[&str] = &[
    "zendiloos-agent",
    "zendiloos-agent-proto",
    "zendiloos-exec",
    "zendiloos-fileops",
    "zendiloos-sim",
    "zendiloos-logging",
];

/// The only outside crates the core uses: errors, async ports, tokio's locks,
/// tasks and timers, and logging.
const CORE_OUTSIDE: &[&str] = &["async-trait", "thiserror", "tokio", "tracing"];

/// Never on the agent side, not even in its tests.
const NOT_ON_AGENT_SIDE: &[&str] = &[
    "sqlx",
    "axum",
    "axum-extra",
    "hyper",
    "hyper-util",
    "tower-http",
    "utoipa",
    "utoipa-axum",
    "sysinfo",
    "lettre",
];

/// Paths that mean I/O; the core names none of them.
const IO: &[&str] = &[
    "std::fs",
    "std::process",
    "std::env",
    "std::os",
    "std::net",
    "tokio::fs",
    "tokio::process",
    "tokio::net",
    "tokio::signal",
];

/// Values rather than I/O: addresses, and the handle of a file the agent
/// opened, which the file manager's port hands through.
const NOT_IO: &[&str] = &[
    "std::net::IpAddr",
    "std::net::Ipv4Addr",
    "std::net::Ipv6Addr",
    "std::net::SocketAddr",
    "tokio::fs::File",
];

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn core() -> BTreeSet<String> {
    let mut core = set(SHARED);
    core.extend(set(CONTEXTS));
    core
}

/// The internal crates a crate may depend on: the table of 2.1.
fn allowed(name: &str, members: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    Some(match name {
        "zendiloos-kernel"
        | "zendiloos-agent-proto"
        | "zendiloos-exec"
        | "zendiloos-logging"
        | "zendiloos-architecture" => BTreeSet::new(),
        "zendiloos-jobs" | "zendiloos-notify" | "zendiloos-identity" | "zendiloos-system"
        | "zendiloos-network" => set(&["zendiloos-kernel"]),
        "zendiloos-storage" | "zendiloos-sharing" => {
            set(&["zendiloos-kernel", "zendiloos-jobs", "zendiloos-notify"])
        }
        "zendiloos-apps" | "zendiloos-containers" => set(&["zendiloos-kernel", "zendiloos-jobs"]),
        "zendiloos-updates" => set(&["zendiloos-kernel", "zendiloos-notify"]),
        "zendiloos-certificates" => set(&["zendiloos-kernel", "zendiloos-jobs", "zendiloos-notify"]),
        "zendiloos-api" => core(),
        "zendiloos-infra" => {
            let mut infra = core();
            infra.extend(set(&["zendiloos-agent-proto", "zendiloos-exec"]));
            infra
        }
        "zendiloosd" => members
            .iter()
            .filter(|m| !matches!(m.as_str(), "zendiloosd" | "zendiloos-agent" | "zendiloos-architecture"))
            .cloned()
            .collect(),
        "zendiloos-fileops" => set(&["zendiloos-agent-proto"]),
        "zendiloos-agent" => {
            set(&["zendiloos-agent-proto", "zendiloos-exec", "zendiloos-fileops", "zendiloos-logging"])
        }
        "zendiloos-sim" => set(&["zendiloos-agent-proto", "zendiloos-fileops"]),
        _ => return None,
    })
}

/// Allowed in a crate's tests beyond its own row.
fn allowed_in_tests(name: &str) -> BTreeSet<String> {
    match name {
        // Infra's agent gateways, parsers included, run against the simulated agent.
        "zendiloos-infra" => set(&["zendiloos-sim"]),
        _ => BTreeSet::new(),
    }
}

struct Crate {
    name: String,
    dir: PathBuf,
    deps: Vec<Dependency>,
}

struct Dependency {
    name: String,
    dev: bool,
}

fn workspace() -> Vec<Crate> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .expect("cargo metadata runs");
    assert!(output.status.success(), "cargo metadata: {}", String::from_utf8_lossy(&output.stderr));
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata answers JSON");
    let text = |v: &Value| v.as_str().expect("a string in cargo metadata").to_owned();
    metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| Crate {
            name: text(&package["name"]),
            dir: PathBuf::from(text(&package["manifest_path"]))
                .parent()
                .expect("a manifest in a directory")
                .to_owned(),
            deps: package["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .map(|dep| Dependency { name: text(&dep["name"]), dev: dep["kind"].as_str() == Some("dev") })
                .collect(),
        })
        .collect()
}

fn members(crates: &[Crate]) -> BTreeSet<String> {
    crates.iter().map(|c| c.name.clone()).collect()
}

fn verdict(problems: &[String], what: &str) {
    assert!(problems.is_empty(), "{what} (docs/architecture.md, 2.1):\n  {}", problems.join("\n  "));
}

#[test]
fn every_crate_has_a_row_in_the_table() {
    let crates = workspace();
    let members = members(&crates);
    let problems: Vec<String> = crates
        .iter()
        .filter(|c| allowed(&c.name, &members).is_none())
        .map(|c| format!("{} has no row: add it to the table and to this test", c.name))
        .collect();
    verdict(&problems, "crates without a place in the architecture");
}

#[test]
fn crates_depend_only_on_what_the_table_allows() {
    let crates = workspace();
    let members = members(&crates);
    let mut problems = Vec::new();
    for krate in &crates {
        let Some(allowed) = allowed(&krate.name, &members) else { continue };
        let in_tests = allowed_in_tests(&krate.name);
        for dep in krate.deps.iter().filter(|d| members.contains(&d.name)) {
            let ok = allowed.contains(&dep.name) || (dep.dev && in_tests.contains(&dep.name));
            if !ok {
                let kind = if dep.dev { " (in its tests)" } else { "" };
                problems.push(format!("{} depends on {}{kind}", krate.name, dep.name));
            }
        }
    }
    verdict(&problems, "dependencies the table does not allow");
}

#[test]
fn the_core_uses_no_outside_crates_but_a_few() {
    let crates = workspace();
    let members = members(&crates);
    let core = core();
    let outside = set(CORE_OUTSIDE);
    let problems: Vec<String> = crates
        .iter()
        .filter(|c| core.contains(&c.name))
        .flat_map(|c| {
            c.deps
                .iter()
                .filter(|d| !d.dev && !members.contains(&d.name) && !outside.contains(&d.name))
                .map(move |d| format!("{} depends on {}", c.name, d.name))
        })
        .collect();
    verdict(&problems, "outside crates in the core");
}

#[test]
fn the_agent_side_knows_no_database_or_http() {
    let crates = workspace();
    let agent_side = set(AGENT_SIDE);
    let forbidden = set(NOT_ON_AGENT_SIDE);
    let problems: Vec<String> = crates
        .iter()
        .filter(|c| agent_side.contains(&c.name))
        .flat_map(|c| {
            c.deps
                .iter()
                .filter(|d| forbidden.contains(&d.name))
                .map(move |d| format!("{} depends on {}", c.name, d.name))
        })
        .collect();
    verdict(&problems, "database or HTTP on the agent side");
}

#[test]
fn the_core_does_no_io() {
    let mut problems = Vec::new();
    for krate in core_crates() {
        for (file, path) in names(&krate) {
            let io = IO.iter().any(|p| within(&path, p));
            let value = NOT_IO.iter().any(|p| within(&path, p));
            if io && !value {
                problems.push(format!("{} names {path}", shown(&krate, &file)));
            }
        }
    }
    verdict(&problems, "I/O in the core");
}

#[test]
fn domain_layers_stand_on_their_own() {
    let mut problems = Vec::new();
    for krate in core_crates() {
        for (file, path) in names(&krate).into_iter().filter(|(file, _)| in_domain(&krate, file)) {
            let first = path.split("::").next().unwrap_or_default();
            let local = matches!(first, "crate" | "super" | "self");
            let application = local && path.split("::").any(|s| s == "application");
            if application || matches!(first, "tokio" | "async_trait") {
                problems.push(format!("{} names {path}", shown(&krate, &file)));
            }
        }
    }
    verdict(&problems, "domain layers that reach out");
}

/// The scanner itself: it sees paths in code and in `use`, and skips what
/// only tests compile.
#[test]
fn the_scanner_sees_code_and_skips_tests() {
    let source = r#"
        use std::{fs, net::{IpAddr, TcpStream as Stream}};
        fn read() { let _ = ::std::process::id(); tokio::fs::File::open("x"); }
        #[cfg(test)]
        mod tests { fn helper() { std::env::var("X"); } }
        #[cfg(any(test, feature = "testing"))]
        pub mod testing;
    "#;
    let mut found = Vec::new();
    let mut modules = Vec::new();
    let file = syn::parse_file(source).expect("the sample parses");
    let mut collector = Collector { dir: PathBuf::new(), modules: &mut modules, paths: &mut found };
    collector.visit_file(&file);
    for expected in
        ["std::fs", "std::net::IpAddr", "std::net::TcpStream", "std::process::id", "tokio::fs::File::open"]
    {
        assert!(found.iter().any(|p| p == expected), "{expected} not found in {found:?}");
    }
    assert!(!found.iter().any(|p| p.starts_with("std::env")), "test code was scanned: {found:?}");
    assert!(modules.is_empty(), "a test-only module was followed: {modules:?}");
}

fn core_crates() -> Vec<Crate> {
    let core = core();
    let crates: Vec<Crate> = workspace().into_iter().filter(|c| core.contains(&c.name)).collect();
    assert_eq!(crates.len(), core.len(), "every core crate is in the workspace");
    crates
}

fn within(path: &str, prefix: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with("::"))
}

fn in_domain(krate: &Crate, file: &Path) -> bool {
    let src = krate.dir.join("src");
    file == src.join("domain.rs") || file.starts_with(src.join("domain"))
}

fn shown(krate: &Crate, file: &Path) -> String {
    let relative = file.strip_prefix(&krate.dir).unwrap_or(file);
    format!("{}/{}", krate.name, relative.display())
}

/// Every path a crate's code names outside its tests, with the file it is in,
/// following its modules from `src/lib.rs`.
fn names(krate: &Crate) -> Vec<(PathBuf, String)> {
    let mut queue = vec![krate.dir.join("src/lib.rs")];
    let mut found = Vec::new();
    while let Some(file) = queue.pop() {
        let source = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let parsed = syn::parse_file(&source).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        // Modules declared in lib.rs and mod.rs live beside it, those of
        // foo.rs in foo/.
        let parent = file.parent().expect("a file in a directory").to_owned();
        let dir = match file.file_name().and_then(|n| n.to_str()) {
            Some("lib.rs" | "main.rs" | "mod.rs") => parent,
            _ => parent.join(file.file_stem().expect("a file name")),
        };
        let mut paths = Vec::new();
        Collector { dir, modules: &mut queue, paths: &mut paths }.visit_file(&parsed);
        found.extend(paths.into_iter().map(|p| (file.clone(), p)));
    }
    assert!(!found.is_empty(), "{}: no code found", krate.name);
    found
}

struct Collector<'a> {
    /// Where the module files declared here live.
    dir: PathBuf,
    modules: &'a mut Vec<PathBuf>,
    paths: &'a mut Vec<String>,
}

/// `#[cfg(test)]`, `#[cfg(any(test, feature = "testing"))]`: compiled for tests only.
fn for_tests(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr.meta.require_list().is_ok_and(|list| {
                let tokens = list.tokens.to_string();
                let words: Vec<&str> = tokens.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
                words.contains(&"test") && !words.contains(&"not")
            })
    })
}

fn attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !for_tests(attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        if let syn::ImplItem::Fn(f) = item
            && for_tests(&f.attrs)
        {
            return;
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        let name = module.ident.to_string();
        match &module.content {
            None => {
                let file = self.dir.join(format!("{name}.rs"));
                let file = if file.exists() { file } else { self.dir.join(&name).join("mod.rs") };
                self.modules.push(file);
            }
            Some((_, items)) => {
                let inner = self.dir.join(&name);
                let outer = std::mem::replace(&mut self.dir, inner);
                for item in items {
                    self.visit_item(item);
                }
                self.dir = outer;
            }
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        flatten(&item.tree, "", self.paths);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() > 1 {
            let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            self.paths.push(names.join("::"));
        }
        visit::visit_path(self, path);
    }
}

/// `use a::{b, c::d as e}` → `a::b`, `a::c::d`.
fn flatten(tree: &syn::UseTree, prefix: &str, out: &mut Vec<String>) {
    let join = |name: &str| {
        if prefix.is_empty() {
            name.to_owned()
        } else if name == "self" {
            prefix.to_owned()
        } else {
            format!("{prefix}::{name}")
        }
    };
    match tree {
        syn::UseTree::Path(p) => flatten(&p.tree, &join(&p.ident.to_string()), out),
        syn::UseTree::Name(n) => out.push(join(&n.ident.to_string())),
        syn::UseTree::Rename(r) => out.push(join(&r.ident.to_string())),
        syn::UseTree::Glob(_) => out.push(join("*")),
        syn::UseTree::Group(g) => {
            for tree in &g.items {
                flatten(tree, prefix, out);
            }
        }
    }
}
