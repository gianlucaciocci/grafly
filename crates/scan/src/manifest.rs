//! Manifest discovery for the Package artifact layer.
//!
//! Supports four manifest formats, all returning the same `Manifest` shape:
//! - `Cargo.toml` — Rust (workspaces and standalone)
//! - `pyproject.toml` — Python (PEP 621 only; Poetry-style `[tool.poetry]` deferred)
//! - `package.json` — JavaScript / TypeScript
//! - `go.mod` — Go modules (entry-point detection delegated to the Go scanner)
//!
//! Each manifest with a declared package name becomes a [`Manifest`] record;
//! virtual workspaces (no `[package]` section in Cargo) are skipped.

use serde::Deserialize;
use std::path::Path;

/// A discovered package manifest with the information needed to emit a
/// `Package` artifact, link source files via `Contains`, and surface the
/// package's description / entry-point status in reports.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Declared package name (e.g. `grafly-scan`, `requests`, `react`).
    pub name: String,
    /// Path to the manifest file itself, normalised to forward slashes.
    pub manifest_path: String,
    /// Directory containing the manifest, normalised to forward slashes and
    /// terminated with `/` so prefix-matching source files is unambiguous.
    pub root_dir: String,
    /// Short description from the manifest. `None` if the manifest omits it.
    pub description: Option<String>,
    /// True when the manifest declares a buildable binary / executable.
    /// - Cargo: `[[bin]]` array present (we don't detect `src/main.rs` heuristically here)
    /// - pyproject: `[project.scripts]` non-empty
    /// - package.json: `bin` field present
    /// - go.mod: deferred — set by a downstream pass that knows about `package main`
    pub is_binary: bool,
}

// ── Cargo.toml ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CargoToml {
    package: Option<CargoPackage>,
    #[serde(default)]
    bin: Vec<toml::Value>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    description: Option<String>,
}

/// Parse a `Cargo.toml` file. Returns `Some` when the manifest declares a
/// `[package]` with a name, `None` for virtual workspaces or unparseable files.
pub fn parse_cargo_toml(path: &Path) -> Option<Manifest> {
    let source = std::fs::read_to_string(path).ok()?;
    parse_cargo_toml_str(&source, path)
}

/// Same as [`parse_cargo_toml`] but takes the TOML content directly.
pub fn parse_cargo_toml_str(source: &str, path: &Path) -> Option<Manifest> {
    let parsed: CargoToml = toml::from_str(source).ok()?;
    let pkg = parsed.package?;
    let (manifest_path, root_dir) = paths(path)?;
    Some(Manifest {
        name: pkg.name,
        manifest_path,
        root_dir,
        description: pkg.description,
        is_binary: !parsed.bin.is_empty(),
    })
}

// ── pyproject.toml (PEP 621) ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Pyproject {
    project: Option<PyProject621>,
}

#[derive(Debug, Deserialize)]
struct PyProject621 {
    name: String,
    description: Option<String>,
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

/// Parse a `pyproject.toml` file (PEP 621 `[project]` table only).
pub fn parse_pyproject_toml(path: &Path) -> Option<Manifest> {
    let source = std::fs::read_to_string(path).ok()?;
    parse_pyproject_toml_str(&source, path)
}

pub fn parse_pyproject_toml_str(source: &str, path: &Path) -> Option<Manifest> {
    let parsed: Pyproject = toml::from_str(source).ok()?;
    let proj = parsed.project?;
    let (manifest_path, root_dir) = paths(path)?;
    Some(Manifest {
        name: proj.name,
        manifest_path,
        root_dir,
        description: proj.description,
        is_binary: !proj.scripts.is_empty(),
    })
}

// ── package.json ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PackageJson {
    name: Option<String>,
    description: Option<String>,
    bin: Option<serde_json::Value>,
}

/// Parse a `package.json` file.
pub fn parse_package_json(path: &Path) -> Option<Manifest> {
    let source = std::fs::read_to_string(path).ok()?;
    parse_package_json_str(&source, path)
}

pub fn parse_package_json_str(source: &str, path: &Path) -> Option<Manifest> {
    let parsed: PackageJson = serde_json::from_str(source).ok()?;
    let name = parsed.name?;
    let (manifest_path, root_dir) = paths(path)?;
    Some(Manifest {
        name,
        manifest_path,
        root_dir,
        description: parsed.description,
        // `bin` may be a string (single binary) or an object (named binaries).
        // Either way its presence indicates an executable package.
        is_binary: parsed.bin.is_some(),
    })
}

// ── go.mod ────────────────────────────────────────────────────────────────────

/// Parse a `go.mod` file. The `module` directive on the first non-comment line
/// gives the module path; the last path segment is the package name. The Go
/// `is_binary` flag is set later, when the Go scanner reports which files
/// declare `package main`.
pub fn parse_go_mod(path: &Path) -> Option<Manifest> {
    let source = std::fs::read_to_string(path).ok()?;
    parse_go_mod_str(&source, path)
}

pub fn parse_go_mod_str(source: &str, path: &Path) -> Option<Manifest> {
    let module_path = source
        .lines()
        .map(str::trim_start)
        .find_map(|line| line.strip_prefix("module "))?
        .trim()
        .trim_matches('"');
    if module_path.is_empty() {
        return None;
    }
    // Use the last path segment as the human-readable name
    // (e.g. `github.com/foo/bar` → `bar`).
    let name = module_path
        .rsplit('/')
        .next()
        .unwrap_or(module_path)
        .to_string();
    let (manifest_path, root_dir) = paths(path)?;
    Some(Manifest {
        name,
        manifest_path,
        root_dir,
        description: None,
        is_binary: false,
    })
}

// ── Discovery dispatch ────────────────────────────────────────────────────────

/// File names this module knows how to parse.
pub const KNOWN_MANIFEST_FILENAMES: &[&str] =
    &["Cargo.toml", "pyproject.toml", "package.json", "go.mod"];

/// Parse any supported manifest by inspecting its filename. Returns `None`
/// for unsupported filenames or unparseable / virtual manifests.
pub fn parse_any(path: &Path) -> Option<Manifest> {
    match path.file_name().and_then(|s| s.to_str())? {
        "Cargo.toml" => parse_cargo_toml(path),
        "pyproject.toml" => parse_pyproject_toml(path),
        "package.json" => parse_package_json(path),
        "go.mod" => parse_go_mod(path),
        _ => None,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn paths(manifest_path: &Path) -> Option<(String, String)> {
    let parent = manifest_path.parent()?;
    let mp = normalise(manifest_path);
    let mut rd = normalise(parent);
    if !rd.ends_with('/') {
        rd.push('/');
    }
    Some((mp, rd))
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

    // ── Cargo.toml ────────────────────────────────────────────────────────────

    #[test]
    fn cargo_parses_minimal_package_manifest() {
        let m = parse_cargo_toml_str(
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
            Path::new("./crate-dir/Cargo.toml"),
        )
        .expect("should parse");
        assert_eq!(m.name, "my-crate");
        assert_eq!(m.description, None);
        assert!(!m.is_binary);
        assert!(m.root_dir.ends_with('/'));
    }

    #[test]
    fn cargo_picks_up_description_and_binary_flag() {
        let m = parse_cargo_toml_str(
            r#"
                [package]
                name = "my-cli"
                version = "0.1.0"
                description = "Command-line tool for X"

                [[bin]]
                name = "my-cli"
                path = "src/main.rs"
            "#,
            Path::new("./Cargo.toml"),
        )
        .expect("should parse");
        assert_eq!(m.description.as_deref(), Some("Command-line tool for X"));
        assert!(m.is_binary);
    }

    #[test]
    fn cargo_skips_virtual_workspace_manifest() {
        assert!(parse_cargo_toml_str(
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
            Path::new("./Cargo.toml")
        )
        .is_none());
    }

    #[test]
    fn cargo_parses_manifest_with_both_workspace_and_package_sections() {
        let m = parse_cargo_toml_str(
            "[workspace]\nmembers = [\"a\"]\n\n[package]\nname = \"root-pkg\"\n",
            Path::new("./Cargo.toml"),
        )
        .expect("should parse");
        assert_eq!(m.name, "root-pkg");
    }

    // ── pyproject.toml ────────────────────────────────────────────────────────

    #[test]
    fn pyproject_parses_pep621_with_description() {
        let m = parse_pyproject_toml_str(
            r#"
                [project]
                name = "mypkg"
                version = "0.1.0"
                description = "A Python package"
            "#,
            Path::new("./pyproject.toml"),
        )
        .expect("should parse");
        assert_eq!(m.name, "mypkg");
        assert_eq!(m.description.as_deref(), Some("A Python package"));
        assert!(!m.is_binary);
    }

    #[test]
    fn pyproject_scripts_section_marks_binary() {
        let m = parse_pyproject_toml_str(
            r#"
                [project]
                name = "mycli"

                [project.scripts]
                mycli = "mycli.__main__:main"
            "#,
            Path::new("./pyproject.toml"),
        )
        .expect("should parse");
        assert!(m.is_binary);
    }

    #[test]
    fn pyproject_without_project_table_is_skipped() {
        // Poetry-style pyproject without PEP 621 [project]
        assert!(parse_pyproject_toml_str(
            "[tool.poetry]\nname = \"x\"\n",
            Path::new("./pyproject.toml")
        )
        .is_none());
    }

    // ── package.json ──────────────────────────────────────────────────────────

    #[test]
    fn package_json_parses_basic() {
        let m = parse_package_json_str(
            r#"{ "name": "my-lib", "version": "1.0.0", "description": "A JS lib" }"#,
            Path::new("./package.json"),
        )
        .expect("should parse");
        assert_eq!(m.name, "my-lib");
        assert_eq!(m.description.as_deref(), Some("A JS lib"));
        assert!(!m.is_binary);
    }

    #[test]
    fn package_json_bin_string_marks_binary() {
        let m = parse_package_json_str(
            r#"{ "name": "tool", "bin": "./bin/tool.js" }"#,
            Path::new("./package.json"),
        )
        .expect("should parse");
        assert!(m.is_binary);
    }

    #[test]
    fn package_json_bin_object_marks_binary() {
        let m = parse_package_json_str(
            r#"{ "name": "tool", "bin": { "tool": "./bin/tool.js" } }"#,
            Path::new("./package.json"),
        )
        .expect("should parse");
        assert!(m.is_binary);
    }

    #[test]
    fn package_json_without_name_is_skipped() {
        assert!(
            parse_package_json_str(r#"{ "version": "1.0.0" }"#, Path::new("./package.json"))
                .is_none()
        );
    }

    // ── go.mod ────────────────────────────────────────────────────────────────

    #[test]
    fn go_mod_picks_last_path_segment_as_name() {
        let m = parse_go_mod_str(
            "module github.com/foo/bar\n\ngo 1.21\n",
            Path::new("./go.mod"),
        )
        .expect("should parse");
        assert_eq!(m.name, "bar");
        assert!(!m.is_binary);
    }

    #[test]
    fn go_mod_handles_local_module_path() {
        let m = parse_go_mod_str("module example\n", Path::new("./go.mod")).expect("should parse");
        assert_eq!(m.name, "example");
    }

    #[test]
    fn go_mod_without_module_directive_is_skipped() {
        assert!(parse_go_mod_str("go 1.21\n", Path::new("./go.mod")).is_none());
    }

    // ── Dispatch ──────────────────────────────────────────────────────────────

    #[test]
    fn parse_any_unsupported_filename_returns_none() {
        // parse_any reads from disk; for missing files it returns None too,
        // which doubles as the "unsupported filename" answer.
        assert!(parse_any(Path::new("./random.txt")).is_none());
    }

    // ── owning_manifest ───────────────────────────────────────────────────────

    #[test]
    fn owning_manifest_picks_deepest_prefix() {
        let workspace = Manifest {
            name: "root".into(),
            manifest_path: "./Cargo.toml".into(),
            root_dir: "./".into(),
            description: None,
            is_binary: false,
        };
        let scan = Manifest {
            name: "scan".into(),
            manifest_path: "./crates/scan/Cargo.toml".into(),
            root_dir: "./crates/scan/".into(),
            description: None,
            is_binary: false,
        };
        let m = vec![workspace.clone(), scan.clone()];

        assert_eq!(
            owning_manifest(&m, "./crates/scan/src/lib.rs").map(|m| &m.name),
            Some(&scan.name)
        );
        assert_eq!(
            owning_manifest(&m, "./src/lib.rs").map(|m| &m.name),
            Some(&workspace.name)
        );
    }
}
