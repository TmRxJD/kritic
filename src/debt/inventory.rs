//! Bounded manifest-only inventory. Requirements never masquerade as installed versions.
use super::{Dependency, Ecosystem, Inventory};
use std::path::Path;

mod cargo;
mod npm;
mod pnpm;
mod python;

#[derive(Clone)]
struct Locked {
    name: String,
    version: String,
    manifest: String,
    public: bool,
    consumer: Option<String>,
}

// Keep one overflow sentinel so the caller errors instead of returning a truncated pass.
fn push(inv: &mut Inventory, dependency: Dependency) {
    if inv.dependencies.len() <= 1000 {
        inv.dependencies.push(dependency);
    }
}
fn push_locked(locked: &mut Vec<Locked>, package: Locked) {
    if locked.len() <= 1000 {
        locked.push(package);
    }
}

pub fn collect(base: &Path) -> Result<Inventory, String> {
    let root = base.canonicalize().map_err(|e| e.to_string())?;
    let names = corpus(&root)?;
    if names.len() > 200 {
        return Err("Dependency corpus exceeds 200 manifests/configs".into());
    }
    let mut inventory = Inventory {
        dependencies: Vec::new(),
        manifests: Vec::new(),
        coverage_gaps: Vec::new(),
    };
    let mut files = std::collections::BTreeMap::new();
    let mut total = 0usize;
    for name in names {
        let path = root.join(&name);
        let resolved = match path.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                inventory
                    .coverage_gaps
                    .push(format!("Unavailable tracked manifest {name:?}"));
                continue;
            }
        };
        if !resolved.starts_with(&root)
            || std::fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err(format!("Manifest escapes corpus or is a symlink: {name:?}"));
        }
        let bytes = read_bounded(&path);
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                inventory
                    .coverage_gaps
                    .push(format!("Unreadable manifest {name:?}: {error}"));
                continue;
            }
        };
        total += bytes.len();
        if total > 16_777_216 {
            return Err("Dependency corpus exceeds 16 MiB".into());
        }
        inventory.manifests.push(name.clone());
        match String::from_utf8(bytes) {
            Ok(text) => {
                files.insert(name, text);
            }
            Err(_) => inventory
                .coverage_gaps
                .push(format!("Manifest {name:?} is not UTF-8")),
        }
    }
    let mut cargo_locks = Vec::new();
    let mut npm_locks = Vec::new();
    let mut python_locks = Vec::new();
    for (name, text) in &files {
        match leaf(name) {
            "Cargo.lock" => cargo::lock(name, text, &mut cargo_locks, &mut inventory),
            "package-lock.json" => npm::lock(name, text, &files, &mut npm_locks, &mut inventory),
            "pnpm-lock.yaml" => pnpm::lock(name, text, &files, &mut npm_locks, &mut inventory),
            "uv.lock" | "poetry.lock" => {
                python::lock(name, text, &mut python_locks, &mut inventory)
            }
            _ => (),
        }
    }
    for (name, text) in &files {
        match leaf(name) {
            "Cargo.toml" => cargo::manifest(name, text, &cargo_locks, &mut inventory),
            "package.json" => npm::manifest(name, text, &files, &npm_locks, &mut inventory),
            "pyproject.toml" => python::manifest(name, text, &python_locks, &mut inventory),
            "debt.toml" if name == ".kritic/debt.toml" => custom(name, text, &mut inventory),
            "yarn.lock" | "go.mod" | "go.sum" | "pom.xml" | "packages.config" | "Gemfile" | "Gemfile.lock" => inventory.coverage_gaps.push(format!("Unsupported dependency format {name:?}; declare exact libraries in .kritic/debt.toml")),
            value if value.starts_with("requirements") && value.ends_with(".txt") => python::requirements(name, text, &python_locks, &mut inventory),
            value if value.ends_with(".csproj") || value.ends_with(".fsproj") => inventory.coverage_gaps.push(format!("Unsupported NuGet manifest {name:?}; declare libraries in .kritic/debt.toml")),
            _ => (),
        }
    }
    if inventory.dependencies.len() > 1000 {
        return Err(
            "Dependency inventory exceeds 1000 entries; narrow the corpus explicitly".into(),
        );
    }
    if inventory.manifests.is_empty() {
        inventory
            .coverage_gaps
            .push("No supported dependency manifests found in the defined corpus".into());
    }
    Ok(inventory)
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}
fn excluded(path: &str) -> bool {
    path.split('/').any(|part| {
        matches!(
            part,
            "node_modules"
                | "dist"
                | "build"
                | "target"
                | "vendor"
                | "coverage"
                | ".turbo"
                | ".git"
                | "cache"
                | "generated"
        )
    })
}
fn accepted(path: &str) -> bool {
    let name = leaf(path);
    !excluded(path)
        && (matches!(
            name,
            "Cargo.toml"
                | "Cargo.lock"
                | "package.json"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "pyproject.toml"
                | "uv.lock"
                | "poetry.lock"
                | ".npmrc"
                | "go.mod"
                | "go.sum"
                | "pom.xml"
                | "packages.config"
                | "Gemfile"
                | "Gemfile.lock"
        ) || path == ".kritic/debt.toml"
            || name.ends_with(".csproj")
            || name.ends_with(".fsproj")
            || (name.starts_with("requirements") && name.ends_with(".txt")))
}

fn corpus(root: &Path) -> Result<Vec<String>, String> {
    let git = crate::proc::hidden_command("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output();
    let mut names = std::collections::BTreeSet::new();
    if let Ok(git) = git
        && git.status.success()
    {
        for raw in git
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let name =
                std::str::from_utf8(raw).map_err(|_| "Tracked manifest paths must be UTF-8")?;
            if accepted(name) {
                names.insert(name.to_string());
            }
        }
    } else {
        walk(root, root, 0, &mut 0, &mut names)?;
    }
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "pyproject.toml",
        "uv.lock",
        "poetry.lock",
        "requirements.txt",
        "requirements-dev.txt",
        ".npmrc",
        "go.mod",
        "go.sum",
        "pom.xml",
        "packages.config",
        "Gemfile",
        "Gemfile.lock",
        ".kritic/debt.toml",
    ] {
        if root.join(name).exists() {
            names.insert(name.into());
        }
    }
    Ok(names.into_iter().collect())
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    seen: &mut usize,
    names: &mut std::collections::BTreeSet<String>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        *seen += 1;
        if *seen > 2000 {
            return Err("Non-Git manifest discovery exceeds 2000 directory entries".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("Manifest path is not UTF-8")?
            .replace('\\', "/");
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if excluded(&name) || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if depth >= 6 {
                return Err("Non-Git manifest discovery exceeds six directory levels".into());
            }
            walk(root, &path, depth + 1, seen, names)?;
        } else if accepted(&name) {
            names.insert(name);
            if names.len() > 200 {
                return Err("Dependency corpus exceeds 200 manifests/configs".into());
            }
        }
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 2_097_152 {
        return Err("Manifest exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(2_097_153)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 2_097_152 {
        return Err("Manifest exceeds 2 MiB".into());
    }
    Ok(bytes)
}
fn exact(value: &str) -> bool {
    regex::Regex::new(r"^[0-9]+(?:\.[0-9]+)+(?:[-+._a-zA-Z][0-9A-Za-z.+_-]*)?$")
        .is_ok_and(|pattern| pattern.is_match(value))
}
/// Manifest-declared identity and provenance, separate from resolved lock evidence.
struct Declaration<'a> {
    ecosystem: Ecosystem,
    name: &'a str,
    requirement: Option<String>,
    scope: &'a str,
    manifest: &'a str,
    public: bool,
}

fn add(inv: &mut Inventory, declaration: Declaration<'_>, locks: &[Locked]) {
    let Declaration {
        ecosystem,
        name,
        requirement,
        scope,
        manifest,
        public,
    } = declaration;
    let nearest = locks
        .iter()
        .filter(|item| {
            item.name == name
                && (parent(&item.manifest).is_empty()
                    || parent(manifest) == parent(&item.manifest)
                    || parent(manifest).starts_with(&format!("{}/", parent(&item.manifest))))
        })
        .map(|item| parent(&item.manifest).len())
        .max();
    let matches: Vec<_> = locks
        .iter()
        .filter(|item| {
            item.name == name
                && Some(parent(&item.manifest).len()) == nearest
                && (parent(&item.manifest).is_empty()
                    || parent(manifest) == parent(&item.manifest)
                    || parent(manifest).starts_with(&format!("{}/", parent(&item.manifest))))
        })
        .collect();
    let unique: std::collections::BTreeSet<_> =
        matches.iter().map(|item| item.version.as_str()).collect();
    let current = if unique.len() == 1 {
        unique.first().map(|value| (*value).to_string())
    } else {
        None
    };
    if unique.len() > 1 {
        inv.coverage_gaps.push(format!("Ambiguous locked versions for {name:?} in {manifest:?}; direct current version is unverified"));
    }
    let public = public && matches.iter().all(|item| item.public);
    push(
        inv,
        Dependency {
            ecosystem,
            name: name.into(),
            requirement,
            current,
            scope: scope.into(),
            manifest: manifest.into(),
            public_registry: public,
        },
    );
}

fn custom(name: &str, text: &str, inv: &mut Inventory) {
    #[derive(serde::Deserialize)]
    struct Config {
        version: u32,
        #[serde(default)]
        packages: Vec<Package>,
        #[serde(flatten)]
        extra: std::collections::BTreeMap<String, toml::Value>,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Package {
        ecosystem: Ecosystem,
        name: String,
        current: Option<String>,
        requirement: Option<String>,
        scope: String,
        public_registry: bool,
    }
    match toml::from_str::<Config>(text) {
        Ok(config) if config.version == 1 => {
            for field in config
                .extra
                .keys()
                .filter(|field| !matches!(field.as_str(), "maintenance" | "migrations"))
            {
                inv.coverage_gaps
                    .push(format!("Unknown debt policy field {field:?} in {name:?}"));
            }
            for item in config.packages {
                if item.name.trim().is_empty()
                    || item.scope != "direct" && item.scope != "transitive"
                    || item.current.as_deref().is_some_and(|value| {
                        value.is_empty()
                            || value.contains(['*', '>', '<', '=', '^', '~', '|'])
                            || value.chars().any(char::is_whitespace)
                    })
                {
                    inv.coverage_gaps.push(format!(
                        "Invalid explicit dependency declaration in {name:?}"
                    ));
                    continue;
                }
                push(
                    inv,
                    Dependency {
                        ecosystem: item.ecosystem,
                        name: item.name,
                        current: item.current,
                        requirement: item.requirement,
                        scope: item.scope,
                        manifest: name.into(),
                        public_registry: item.public_registry,
                    },
                );
            }
        }
        Ok(_) => inv
            .coverage_gaps
            .push(format!("Unsupported debt policy version in {name:?}")),
        Err(_) => inv
            .coverage_gaps
            .push(format!("Malformed explicit dependency policy {name:?}")),
    }
}
