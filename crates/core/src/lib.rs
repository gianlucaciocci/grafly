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
}

impl ScanResult {
    pub fn merge(&mut self, other: ScanResult) {
        self.artifacts.extend(other.artifacts);
        self.dependencies.extend(other.dependencies);
        self.unresolved.extend(other.unresolved);
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
    pub fn build_with_stats(mut self) -> (DependencyMap, ResolutionStats) {
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

        for u in self.unresolved {
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
