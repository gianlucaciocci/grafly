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

use grafly_core::ScanResult;
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
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            skip_hidden: true,
            skip_common_dirs: true,
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

    if opts.skip_common_dirs {
        builder.filter_entry(|entry| {
            let Some(name) = entry.file_name().to_str() else {
                return true;
            };
            !is_skipped_common_dir(name)
        });
    }

    let files: Vec<PathBuf> = builder
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false) && is_supported(e.path())
        })
        .map(|e| e.path().to_path_buf())
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
    Ok(combined)
}

fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("py" | "rs" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "go" | "java")
    )
}
