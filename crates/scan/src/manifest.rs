//! Manifest discovery for the Package artifact layer.
//!
//! Currently parses `Cargo.toml` only. Each manifest with a `[package]` section
//! becomes a [`Manifest`] record; pure workspace manifests (no `[package]`) are
//! skipped. Other languages (`pyproject.toml`, `package.json`, `go.mod`) will
//! follow the same shape in subsequent PRs.

use serde::Deserialize;
use std::path::Path;

/// A discovered package manifest with the minimum information needed to emit
/// a `Package` artifact and `Contains` edges to the source files it owns.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Declared package name (e.g. `grafly-scan`).
    pub name: String,
    /// Path to the manifest file itself, normalised to forward slashes.
    pub manifest_path: String,
    /// Directory containing the manifest, normalised to forward slashes and
    /// terminated with `/` so prefix-matching source files is unambiguous.
    pub root_dir: String,
}

#[derive(Debug, Deserialize)]
struct CargoToml {
    package: Option<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
}

/// Parse a `Cargo.toml` file. Returns `Some` when the manifest declares a
/// `[package]` with a name, `None` for virtual workspaces or unparseable files.
pub fn parse_cargo_toml(path: &Path) -> Option<Manifest> {
    let source = std::fs::read_to_string(path).ok()?;
    parse_cargo_toml_str(&source, path)
}

/// Same as [`parse_cargo_toml`] but takes the TOML content directly. Useful
/// for unit tests and for cases where the caller already has the bytes.
pub fn parse_cargo_toml_str(source: &str, path: &Path) -> Option<Manifest> {
    let parsed: CargoToml = toml::from_str(source).ok()?;
    let pkg = parsed.package?;
    let parent = path.parent()?;

    let manifest_path = normalise(path);
    let mut root_dir = normalise(parent);
    if !root_dir.ends_with('/') {
        root_dir.push('/');
    }

    Some(Manifest {
        name: pkg.name,
        manifest_path,
        root_dir,
    })
}

fn normalise(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Given a set of manifests and a source-file path, return the manifest whose
/// `root_dir` is the longest prefix of the file path. None if no manifest owns
/// the file.
pub fn owning_manifest<'a>(manifests: &'a [Manifest], file_path: &str) -> Option<&'a Manifest> {
    manifests
        .iter()
        .filter(|m| file_path.starts_with(&m.root_dir))
        .max_by_key(|m| m.root_dir.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_package_manifest() {
        let m = parse_cargo_toml_str(
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
            Path::new("./crate-dir/Cargo.toml"),
        )
        .expect("should parse");
        assert_eq!(m.name, "my-crate");
        assert!(m.root_dir.ends_with('/'));
    }

    #[test]
    fn skips_virtual_workspace_manifest() {
        assert!(parse_cargo_toml_str(
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
            Path::new("./Cargo.toml")
        )
        .is_none());
    }

    #[test]
    fn parses_manifest_with_both_workspace_and_package_sections() {
        // grafly's own root Cargo.toml is this shape.
        let m = parse_cargo_toml_str(
            "[workspace]\nmembers = [\"a\"]\n\n[package]\nname = \"root-pkg\"\n",
            Path::new("./Cargo.toml"),
        )
        .expect("should parse");
        assert_eq!(m.name, "root-pkg");
    }

    #[test]
    fn owning_manifest_picks_deepest_prefix() {
        let workspace = Manifest {
            name: "root".into(),
            manifest_path: "./Cargo.toml".into(),
            root_dir: "./".into(),
        };
        let scan = Manifest {
            name: "scan".into(),
            manifest_path: "./crates/scan/Cargo.toml".into(),
            root_dir: "./crates/scan/".into(),
        };
        let m = vec![workspace.clone(), scan.clone()];

        assert_eq!(
            owning_manifest(&m, "./crates/scan/src/lib.rs").map(|m| &m.name),
            Some(&scan.name)
        );
        // File outside any member belongs to the workspace root.
        assert_eq!(
            owning_manifest(&m, "./src/lib.rs").map(|m| &m.name),
            Some(&workspace.name)
        );
    }
}
