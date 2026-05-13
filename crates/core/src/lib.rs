//! Core types for grafly — the ubiquitous language of codebase intelligence.
//!
//! ## Two-phase scan → build pipeline
//!
//! Scanners produce `ScanResult`s containing artifacts, fully-resolved
//! dependencies (e.g. file Contains function), AND *unresolved* references —
//! places where one artifact calls/extends/implements something by **name**
//! but we don't know which artifact node that name refers to yet.
//!
//! `MapBuilder::build()` runs a resolution pass that uses two indexes:
//! - `name_index`:   simple-name → [NodeIndex]   — for bare function calls,
//!                                                 unqualified type references
//! - `method_index`: (type_name, method) → [NodeIndex] — for receiver-typed
//!                                                       method calls like
//!                                                       `self.foo()` inside
//!                                                       `impl Bar` or
//!                                                       `Foo::new()` qualified.
//!
//! Receiver-typed lookups are the antidote to the "supernode" problem (every
//! `Foo::new()` resolving to a single global `new` node). When a scanner can
//! identify the receiver, resolution picks the right `Foo::new` artifact.

use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// A buildable unit declared in a manifest (`Cargo.toml`, `pyproject.toml`,
    /// `package.json`, `go.mod`). Sits above File in the containment hierarchy.
    Package,
    File,
    Namespace,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Function,
    Method,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyKind {
    Contains,
    Imports,
    Calls,
    Extends,
    Implements,
    References,
    Uses,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    Extracted,
    Inferred,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub label: String,
    pub kind: ArtifactKind,
    pub source_file: String,
    pub source_line: usize,
    pub module_id: Option<usize>,
    /// Human-readable description from the artifact's source. Populated for
    /// `Package` artifacts from manifest fields (`Cargo.toml` `description`,
    /// `pyproject.toml` `[project].description`, `package.json` `description`).
    /// `None` for all other artifact kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// True when this artifact represents a buildable binary / executable
    /// entry point. Set for `Package` artifacts whose manifest declares a
    /// binary target (`[[bin]]`, `[project.scripts]`, `bin` field, `package main`).
    /// Always `false` for other artifact kinds.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_entry_point: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

impl Artifact {
    /// Human-readable label suitable for paths, reports, HTML, and hotspot
    /// listings. For methods this is `Type::method_name` (derived from the
    /// artifact ID's `<...>::method::<name>` suffix), disambiguating common
    /// method names like `fmt`, `new`, `default` across different parent types.
    /// For everything else, this is just `label`.
    ///
    /// **Important:** `label` itself stays bare because `MapBuilder`'s name
    /// index uses it for symbol resolution — a method called `fmt` must match
    /// any unresolved `fmt` reference regardless of its parent type.
    pub fn display_label(&self) -> String {
        if self.kind == ArtifactKind::Method {
            if let Some(method_pos) = self.id.rfind("::method::") {
                let parent_id = &self.id[..method_pos];
                if let Some(type_name) = parent_id.rsplit("::").next() {
                    if !type_name.is_empty() {
                        return format!("{}::{}", type_name, self.label);
                    }
                }
            }
        }
        self.label.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub kind: DependencyKind,
    pub confidence: Confidence,
    /// Line in the source artifact where this dependency originates (1-based).
    /// For `Calls`, this is the call site. For `Imports`, the import statement.
    /// For `Contains`, the line of the contained child.
    pub source_line: usize,
}

pub type DependencyMap = DiGraph<Artifact, Dependency>;

// ── Raw scan output ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawArtifact {
    pub id: String,
    pub label: String,
    pub kind: ArtifactKind,
    pub source_file: String,
    pub source_line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_entry_point: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDependency {
    pub source_id: String,
    pub target_id: String,
    pub kind: DependencyKind,
    pub confidence: Confidence,
    pub source_line: usize,
}

/// A dependency where the *target* is given by simple name (not artifact ID).
///
/// `receiver` is the enclosing/qualifying type for method calls:
/// - For `self.foo()` inside `impl Bar { ... }` → `Some("Bar")`
/// - For `Foo::new()` qualified call → `Some("Foo")`
/// - For bare `foo()` → `None`
///
/// When `receiver` is set, resolution uses `method_index[(receiver, target_name)]`
/// for precise per-type method lookup — the antidote to the supernode problem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedReference {
    pub source_id: String,
    pub target_name: String,
    pub receiver: Option<String>,
    pub kind: DependencyKind,
    pub source_line: usize,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub artifacts: Vec<RawArtifact>,
    pub dependencies: Vec<RawDependency>,
    pub unresolved: Vec<UnresolvedReference>,
    /// Directory paths (forward-slash normalised) of Go files that declare
    /// `package main`. Used by manifest discovery to flag the owning `go.mod`
    /// as a binary package. Empty for all non-Go scans.
    pub main_package_dirs: Vec<String>,
}

impl ScanResult {
    pub fn merge(&mut self, other: ScanResult) {
        self.artifacts.extend(other.artifacts);
        self.dependencies.extend(other.dependencies);
        self.unresolved.extend(other.unresolved);
        self.main_package_dirs.extend(other.main_package_dirs);
    }
}

// ── MapBuilder with name + method resolution ─────────────────────────────────

#[derive(Default)]
pub struct MapBuilder {
    id_to_index: HashMap<String, NodeIndex>,
    map: DependencyMap,
    unresolved: Vec<UnresolvedReference>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ResolutionStats {
    pub attempted: usize,
    pub resolved_unique: usize,
    pub resolved_ambiguous: usize,
    pub unresolved: usize,
}

impl MapBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_scan(&mut self, scan: ScanResult) {
        for raw in scan.artifacts {
            if self.id_to_index.contains_key(&raw.id) {
                continue;
            }
            let idx = self.map.add_node(Artifact {
                id: raw.id.clone(),
                label: raw.label,
                kind: raw.kind,
                source_file: raw.source_file,
                source_line: raw.source_line,
                module_id: None,
                description: raw.description,
                is_entry_point: raw.is_entry_point,
            });
            self.id_to_index.insert(raw.id, idx);
        }

        for raw in scan.dependencies {
            if let (Some(&src), Some(&dst)) = (
                self.id_to_index.get(&raw.source_id),
                self.id_to_index.get(&raw.target_id),
            ) {
                self.map.add_edge(
                    src,
                    dst,
                    Dependency {
                        kind: raw.kind,
                        confidence: raw.confidence,
                        source_line: raw.source_line,
                    },
                );
            }
        }

        self.unresolved.extend(scan.unresolved);
    }

    /// Number of unresolved references queued for the resolution pass.
    /// Useful for sizing a progress indicator before calling
    /// [`build_with_progress`](Self::build_with_progress).
    pub fn unresolved_len(&self) -> usize {
        self.unresolved.len()
    }

    /// Build the final `DependencyMap` and run the resolution pass.
    ///
    /// Resolution rules:
    /// - **With receiver** (e.g. `self.new()` inside `impl Foo`): looks up
    ///   `method_index[(receiver, target_name)]`. Single match → `Inferred`.
    ///   Multiple matches → pick the one in a different file from caller →
    ///   `Inferred`. None → drop.
    /// - **Without receiver, `Calls` kind**: requires *unique* match in
    ///   `name_index`. Multiple matches → drop (no Ambiguous Calls — they
    ///   create supernode shortcuts that explain nothing).
    /// - **Without receiver, other kinds** (`Extends` / `Implements` /
    ///   `References` / `Uses`): single match → `Inferred`. Multiple → pick
    ///   non-self-file match → `Ambiguous` (these are rarer and more useful
    ///   even when uncertain).
    pub fn build_with_stats(self) -> (DependencyMap, ResolutionStats) {
        self.build_with_progress(|_, _| {})
    }

    /// Same as [`build_with_stats`](Self::build_with_stats) but invokes
    /// `on_progress(done, total)` periodically (~every 1%) during the
    /// resolution loop, so callers can render a progress indicator.
    /// `total` is the number of unresolved references; `done` is how many
    /// have been processed.
    pub fn build_with_progress<F>(mut self, mut on_progress: F) -> (DependencyMap, ResolutionStats)
    where
        F: FnMut(usize, usize),
    {
        let mut stats = ResolutionStats::default();

        // ── name_index: label → [NodeIndex] ────────────────────────────────
        let mut name_index: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for idx in self.map.node_indices() {
            let a = &self.map[idx];
            if a.label.is_empty() || a.label.contains(|c: char| c.is_whitespace()) {
                continue;
            }
            // Skip Import artifacts — they're raw text records, not symbols.
            if a.kind == ArtifactKind::Import {
                continue;
            }
            name_index.entry(a.label.clone()).or_default().push(idx);
        }

        // ── method_index: (type_name, method_name) → [NodeIndex] ───────────
        // Built from Method artifacts by parsing their IDs.
        // Artifact ID layout: <file>::<parent_kind>::<TypeName>::method::<method>
        let mut method_index: HashMap<(String, String), Vec<NodeIndex>> = HashMap::new();
        for idx in self.map.node_indices() {
            let a = &self.map[idx];
            if a.kind != ArtifactKind::Method {
                continue;
            }
            let Some(method_pos) = a.id.rfind("::method::") else {
                continue;
            };
            let parent_id = &a.id[..method_pos];
            let type_name = parent_id.rsplit("::").next().unwrap_or("");
            if !type_name.is_empty() {
                method_index
                    .entry((type_name.to_string(), a.label.clone()))
                    .or_default()
                    .push(idx);
            }
        }

        let total_unresolved = self.unresolved.len();
        let report_step = (total_unresolved / 100).max(1);
        on_progress(0, total_unresolved);

        for (i, u) in self.unresolved.into_iter().enumerate() {
            if i > 0 && i % report_step == 0 {
                on_progress(i, total_unresolved);
            }
            stats.attempted += 1;
            let Some(&src_idx) = self.id_to_index.get(&u.source_id) else {
                stats.unresolved += 1;
                continue;
            };

            let src_file = self.map[src_idx].source_file.clone();

            let resolved: Option<(NodeIndex, Confidence)> = match (&u.receiver, &u.kind) {
                // ── Receiver-typed lookup ─────────────────────────────────
                (Some(recv), _) => method_index
                    .get(&(recv.clone(), u.target_name.clone()))
                    .and_then(|cands| pick_best(cands, src_idx, &src_file, &self.map))
                    .map(|n| (n, Confidence::Inferred)),

                // ── Calls without receiver: strict (unique only) ──────────
                (None, DependencyKind::Calls) => name_index.get(&u.target_name).and_then(|cands| {
                    if cands.len() == 1 && cands[0] != src_idx {
                        Some((cands[0], Confidence::Inferred))
                    } else {
                        None
                    }
                }),

                // ── Extends / Implements / References / Uses: looser ──────
                (None, _) => name_index.get(&u.target_name).and_then(|cands| {
                    pick_best(cands, src_idx, &src_file, &self.map).map(|n| {
                        let conf = if cands.len() == 1 {
                            Confidence::Inferred
                        } else {
                            Confidence::Ambiguous
                        };
                        (n, conf)
                    })
                }),
            };

            match resolved {
                Some((target, conf)) if target != src_idx => {
                    match conf {
                        Confidence::Inferred => stats.resolved_unique += 1,
                        Confidence::Ambiguous => stats.resolved_ambiguous += 1,
                        Confidence::Extracted => {}
                    }
                    self.map.add_edge(
                        src_idx,
                        target,
                        Dependency {
                            kind: u.kind,
                            confidence: conf,
                            source_line: u.source_line,
                        },
                    );
                }
                _ => {
                    stats.unresolved += 1;
                }
            }
        }

        on_progress(total_unresolved, total_unresolved);
        (self.map, stats)
    }

    pub fn build(self) -> DependencyMap {
        self.build_with_stats().0
    }
}

/// Pick the best candidate from a set of NodeIndexes — prefers a node in a
/// different file from the source (since same-file calls are usually already
/// captured as Contains and most cross-file resolution is what we want here).
/// Returns None if the only candidate is the source itself.
fn pick_best(
    cands: &[NodeIndex],
    src_idx: NodeIndex,
    src_file: &str,
    map: &DependencyMap,
) -> Option<NodeIndex> {
    if cands.is_empty() {
        return None;
    }
    // Prefer cross-file match
    if let Some(&n) = cands
        .iter()
        .find(|&&n| n != src_idx && map[n].source_file != src_file)
    {
        return Some(n);
    }
    // Fall back to any non-self match
    cands.iter().copied().find(|&n| n != src_idx)
}
