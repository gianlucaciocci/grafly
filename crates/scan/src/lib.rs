//! grafly-scan — discover artifacts and dependencies from source files.
//!
//! Public entry points:
//! - `scan_file(path)` — scan a single source file
//! - `scan_dir(root)`  — recursively scan a directory in parallel,
//!                       skipping `.gitignore`d paths, hidden directories,
//!                       and well-known dependency/build directories
//!                       (`node_modules`, `target`, `__pycache__`, ...)
//! - `scan_dir_with_options(root, opts)` — explicit control over filtering

mod go;
mod java;
mod javascript;
mod python;
mod rust_lang;
mod typescript;

pub mod common;
pub mod manifest;

use grafly_core::{
    ArtifactKind, Confidence, DependencyKind, RawArtifact, RawDependency, ScanResult,
};
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unsupported file extension: {0}")]
    Unsupported(String),
}

/// Filtering options for `scan_dir`.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Respect `.gitignore`, `.ignore`, parent gitignores and global ignore.
    pub respect_gitignore: bool,
    /// Skip hidden directories (`.git`, `.venv`, `.cache`, ...).
    pub skip_hidden: bool,
    /// Skip well-known build/dependency directories even when not gitignored.
    pub skip_common_dirs: bool,
    /// Skip test and example files / directories. These are not part of the
    /// project's runtime architecture and pollute hotspot/module detection.
    /// Detects per-language conventions (Python `test_*.py`, Go `*_test.go`,
    /// JS `*.test.ts`/`*.spec.ts`, Rust `examples/` + `tests/`, etc.).
    pub skip_tests_and_examples: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            skip_hidden: true,
            skip_common_dirs: true,
            skip_tests_and_examples: true,
        }
    }
}

impl ScanOptions {
    /// Disable all filtering — scan every file under the root.
    pub fn unrestricted() -> Self {
        Self {
            respect_gitignore: false,
            skip_hidden: false,
            skip_common_dirs: false,
            skip_tests_and_examples: false,
        }
    }
}

/// Directories grafly always skips unless `skip_common_dirs` is disabled.
/// These are language-specific build/dependency dirs that bloat the graph
/// with vendored code that isn't part of the user's project.
const ALWAYS_SKIP: &[&str] = &[
    // Python
    "__pycache__",
    "site-packages",
    "venv",
    "env",
    ".tox",
    ".pytest_cache",
    // Node / JS / TS
    "node_modules",
    // Rust
    "target",
    // Java / Kotlin
    ".gradle",
    // Go
    "vendor",
    // Generic build
    "dist",
    "build",
    "out",
    "_build",
    ".cache",
    // grafly's own output (avoid recursion if user points at workspace root)
    "grafly-out",
];

fn is_skipped_common_dir(name: &str) -> bool {
    ALWAYS_SKIP.contains(&name) || name.ends_with(".egg-info")
}

/// Directory names that signal test / example code in any language.
/// Matched case-insensitively against each path component.
const TEST_OR_EXAMPLE_DIRS: &[&str] = &[
    "tests", "test", "__tests__", "__test__",
    "spec", "specs",
    "benches", "bench",
    "e2e", "integration_tests", "testing",
    "examples", "example", "demos", "demo",
    "samples", "sample",
];

fn is_test_or_example_dir(name: &str) -> bool {
    TEST_OR_EXAMPLE_DIRS
        .iter()
        .any(|d| d.eq_ignore_ascii_case(name))
}

/// Per-language filename conventions for test files (no enclosing test dir).
/// Examples: Python `test_foo.py`, Go `foo_test.go`, JS `foo.test.ts`.
fn is_test_filename(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    match ext {
        "py" => stem.starts_with("test_") || stem.ends_with("_test") || stem == "conftest",
        "rs" => stem.ends_with("_test") || stem.ends_with("_tests"),
        "go" => stem.ends_with("_test"),
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => {
            stem.ends_with(".test") || stem.ends_with(".spec")
        }
        "java" => stem.ends_with("Test") || stem.ends_with("Tests") || stem.ends_with("Spec"),
        _ => false,
    }
}

/// True when `path` lives in a test/example directory or matches a per-language
/// test filename convention.
fn is_test_or_example_path(path: &Path) -> bool {
    for component in path.components() {
        if let Some(name) = component.as_os_str().to_str() {
            if is_test_or_example_dir(name) {
                return true;
            }
        }
    }
    is_test_filename(path)
}

pub fn scan_file(path: &Path) -> Result<ScanResult, ScanError> {
    let source = std::fs::read_to_string(path)?;
    let result = match path.extension().and_then(|e| e.to_str()) {
        Some("py") => python::scan(path, &source),
        Some("rs") => rust_lang::scan(path, &source),
        Some("js" | "mjs" | "cjs") => javascript::scan(path, &source),
        Some("ts") => typescript::scan(path, &source),
        Some("tsx") => typescript::scan_tsx(path, &source),
        Some("go") => go::scan(path, &source),
        Some("java") => java::scan(path, &source),
        ext => {
            return Err(ScanError::Unsupported(
                ext.unwrap_or("none").to_string(),
            ))
        }
    };
    Ok(result)
}

pub fn scan_dir(root: &Path) -> Result<ScanResult, ScanError> {
    scan_dir_with_options(root, &ScanOptions::default())
}

pub fn scan_dir_with_options(
    root: &Path,
    opts: &ScanOptions,
) -> Result<ScanResult, ScanError> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(opts.skip_hidden)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .ignore(opts.respect_gitignore)
        .parents(opts.respect_gitignore);

    if opts.skip_common_dirs || opts.skip_tests_and_examples {
        let skip_common = opts.skip_common_dirs;
        let skip_tests = opts.skip_tests_and_examples;
        builder.filter_entry(move |entry| {
            let Some(name) = entry.file_name().to_str() else {
                return true;
            };
            if skip_common && is_skipped_common_dir(name) {
                return false;
            }
            if skip_tests {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir && is_test_or_example_dir(name) {
                    return false;
                }
            }
            true
        });
    }

    // One walk, partitioned into manifest paths and source-file paths.
    let entries: Vec<PathBuf> = builder
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path().to_path_buf())
        .collect();

    let manifest_paths: Vec<&PathBuf> = entries
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| manifest::KNOWN_MANIFEST_FILENAMES.contains(&n))
                .unwrap_or(false)
        })
        .collect();

    let files: Vec<PathBuf> = entries
        .iter()
        .filter(|p| is_supported(p))
        .filter(|p| !opts.skip_tests_and_examples || !is_test_or_example_path(p))
        .cloned()
        .collect();

    let results: Vec<Result<ScanResult, ScanError>> =
        files.par_iter().map(|p| scan_file(p)).collect();

    let mut combined = ScanResult::default();
    for r in results {
        match r {
            Ok(res) => combined.merge(res),
            Err(ScanError::Unsupported(_)) => {}
            Err(e) => return Err(e),
        }
    }

    // Manifest discovery → Package artifacts + Contains edges to owned files.
    let mut manifests: Vec<manifest::Manifest> = manifest_paths
        .iter()
        .filter_map(|p| manifest::parse_any(p))
        .collect();

    // For go.mod manifests, flip is_binary if any owned .go file declared
    // `package main` during the scan. This is the post-scan step described in
    // the Go entry-point detection design — the scanner doesn't know about
    // manifests, manifests don't see source content.
    for m in &mut manifests {
        if !m.manifest_path.ends_with("go.mod") {
            continue;
        }
        if combined.main_package_dirs.iter().any(|d| {
            // d is a directory; m.root_dir ends with '/'. Treat the trailing
            // '/' as inclusive so `./` matches `.` and `./cmd/foo/` matches
            // `./cmd/foo`.
            let root = m.root_dir.trim_end_matches('/');
            d == root || d.starts_with(&m.root_dir)
        }) {
            m.is_binary = true;
        }
    }

    for m in &manifests {
        combined.artifacts.push(RawArtifact {
            id: format!("{}::package::{}", m.manifest_path, m.name),
            label: m.name.clone(),
            kind: ArtifactKind::Package,
            source_file: m.manifest_path.clone(),
            source_line: 0,
            description: m.description.clone(),
            is_entry_point: m.is_binary,
        });
    }

    for file_path in &files {
        let file_id = file_path.to_string_lossy().replace('\\', "/");
        if let Some(m) = manifest::owning_manifest(&manifests, &file_id) {
            combined.dependencies.push(RawDependency {
                source_id: format!("{}::package::{}", m.manifest_path, m.name),
                target_id: file_id,
                kind: DependencyKind::Contains,
                confidence: Confidence::Extracted,
                source_line: 0,
            });
        }
    }

    Ok(combined)
}

fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("py" | "rs" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "go" | "java")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn test_directory_at_any_depth_is_skipped() {
        assert!(is_test_or_example_path(&p("tests/foo.py")));
        assert!(is_test_or_example_path(&p("src/tests/foo.py")));
        assert!(is_test_or_example_path(&p("project/test/bar.go")));
        assert!(is_test_or_example_path(&p("project/__tests__/foo.ts")));
        assert!(is_test_or_example_path(&p("project/spec/foo.rs")));
        assert!(is_test_or_example_path(&p("project/benches/bench1.rs")));
    }

    #[test]
    fn examples_and_samples_are_skipped() {
        assert!(is_test_or_example_path(&p("examples/hello.rs")));
        assert!(is_test_or_example_path(&p("crates/x/examples/demo.py")));
        assert!(is_test_or_example_path(&p("samples/foo.ts")));
        assert!(is_test_or_example_path(&p("demos/bar.go")));
    }

    #[test]
    fn per_language_test_filename_patterns() {
        assert!(is_test_or_example_path(&p("src/test_foo.py")));
        assert!(is_test_or_example_path(&p("src/foo_test.py")));
        assert!(is_test_or_example_path(&p("conftest.py")));
        assert!(is_test_or_example_path(&p("src/foo_test.go")));
        assert!(is_test_or_example_path(&p("src/foo_test.rs")));
        assert!(is_test_or_example_path(&p("src/foo.test.ts")));
        assert!(is_test_or_example_path(&p("src/foo.spec.js")));
        assert!(is_test_or_example_path(&p("src/FooTest.java")));
        assert!(is_test_or_example_path(&p("src/FooSpec.java")));
    }

    #[test]
    fn non_test_files_are_not_skipped() {
        assert!(!is_test_or_example_path(&p("src/main.rs")));
        assert!(!is_test_or_example_path(&p("src/lib.py")));
        // "latest" contains "test" as substring but isn't a test
        assert!(!is_test_or_example_path(&p("src/latest.py")));
        // "testing.rs" doesn't match _test/_tests suffix
        assert!(!is_test_or_example_path(&p("src/testing_utils.rs")));
        // a directory named "testdata" isn't in our skip list (it's fixtures, not tests)
        assert!(!is_test_or_example_path(&p("src/testdata/golden.json")));
    }

    #[test]
    fn dir_match_is_case_insensitive() {
        assert!(is_test_or_example_path(&p("Tests/foo.cs")));
        assert!(is_test_or_example_path(&p("Examples/foo.py")));
    }
}
