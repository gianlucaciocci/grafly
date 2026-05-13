//! grafly-query — interactive querying of a [`DependencyMap`].
//!
//! - [`find_path`] — weighted shortest path between two artifacts. Edge weights
//!   prefer runtime call chains (`Calls`=1) over file-level co-occurrence
//!   (`Imports`=5) so paths in message-bus-style architectures route through
//!   the actual mediation chain rather than taking import shortcuts.
//! - [`neighbors`] — depth-limited BFS subgraph centered on an artifact, with
//!   edge-kind filtering and a degree cap to avoid supernode blowup.
//! - [`ancestors`] / [`descendants`] — convenience wrappers around [`neighbors`].
//!
//! Design rationale for the weights and BFS filters: see the
//! `future_improvements.md` memory under "### 3. Querying API → Design constraints".

use grafly_core::{Artifact, Confidence, Dependency, DependencyKind, DependencyMap};
use petgraph::algo::astar;
use petgraph::graph::{EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use thiserror::Error;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("no artifact matches `{0}`")]
    NotFound(String),
    #[error("`{query}` is ambiguous — {candidates} candidates (try the full artifact id)")]
    Ambiguous { query: String, candidates: usize },
}

// ── Public refs (serializable, light-weight projections) ──────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRef {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub source_file: String,
    pub source_line: usize,
}

impl ArtifactRef {
    fn from_artifact(a: &Artifact) -> Self {
        Self {
            id: a.id.clone(),
            label: a.display_label(),
            kind: format!("{:?}", a.kind),
            source_file: a.source_file.clone(),
            source_line: a.source_line,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyRef {
    pub source_id: String,
    pub target_id: String,
    pub kind: String,
    pub confidence: String,
    pub source_line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupernodeSkip {
    pub id: String,
    pub label: String,
    pub degree: usize,
}

// ── Resolution: id or label → NodeIndex ───────────────────────────────────────

pub fn resolve(map: &DependencyMap, query: &str) -> Result<NodeIndex, QueryError> {
    if let Some(n) = map.node_indices().find(|&n| map[n].id == query) {
        return Ok(n);
    }
    let matches: Vec<NodeIndex> = map
        .node_indices()
        .filter(|&n| map[n].label == query)
        .collect();
    match matches.len() {
        0 => Err(QueryError::NotFound(query.to_string())),
        1 => Ok(matches[0]),
        n => Err(QueryError::Ambiguous {
            query: query.to_string(),
            candidates: n,
        }),
    }
}

// ── Edge weighting ────────────────────────────────────────────────────────────

fn kind_weight(kind: &DependencyKind) -> f64 {
    match kind {
        DependencyKind::Calls => 1.0,
        DependencyKind::Extends | DependencyKind::Implements | DependencyKind::Contains => 2.0,
        DependencyKind::Imports => 5.0,
        DependencyKind::References | DependencyKind::Uses => 10.0,
    }
}

fn confidence_multiplier(c: &Confidence) -> f64 {
    match c {
        Confidence::Extracted => 1.0,
        Confidence::Inferred => 1.5,
        Confidence::Ambiguous => 3.0,
    }
}

fn weight_of(d: &Dependency) -> f64 {
    kind_weight(&d.kind) * confidence_multiplier(&d.confidence)
}

fn confidence_rank(c: &Confidence) -> u8 {
    match c {
        Confidence::Extracted => 0,
        Confidence::Inferred => 1,
        Confidence::Ambiguous => 2,
    }
}

fn edge_allowed(
    edge: &Dependency,
    allowed_kinds: &Option<Vec<DependencyKind>>,
    min_confidence: &Confidence,
) -> bool {
    if confidence_rank(&edge.confidence) > confidence_rank(min_confidence) {
        return false;
    }
    if let Some(ks) = allowed_kinds {
        if !ks.contains(&edge.kind) {
            return false;
        }
    }
    true
}

// ── Path finding ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PathOptions {
    /// When true, edge weight is `kind_weight × confidence_multiplier` so paths
    /// prefer high-confidence runtime call chains over import shortcuts.
    /// When false, every edge has weight 1 (pure structural shortest path).
    pub weighted: bool,
    /// Safety cap on hop count. `None` = unbounded.
    pub max_hops: Option<usize>,
    /// Only traverse these dependency kinds. `None` = all kinds allowed.
    pub allowed_kinds: Option<Vec<DependencyKind>>,
    /// Edges with confidence *strictly worse* than this are excluded.
    /// E.g. `Inferred` excludes `Ambiguous`. Default `Ambiguous` = include all.
    pub min_confidence: Confidence,
}

impl Default for PathOptions {
    fn default() -> Self {
        Self {
            weighted: true,
            max_hops: Some(20),
            allowed_kinds: None,
            min_confidence: Confidence::Ambiguous,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Path {
    pub from: ArtifactRef,
    pub to: ArtifactRef,
    pub hops: Vec<Hop>,
    pub total_weight: f64,
    pub total_hops: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hop {
    pub from: ArtifactRef,
    pub to: ArtifactRef,
    pub kind: String,
    pub confidence: String,
    pub source_line: usize,
}

pub fn find_path(
    map: &DependencyMap,
    from: NodeIndex,
    to: NodeIndex,
    opts: &PathOptions,
) -> Option<Path> {
    if from == to {
        let a = ArtifactRef::from_artifact(&map[from]);
        return Some(Path {
            from: a.clone(),
            to: a,
            hops: vec![],
            total_weight: 0.0,
            total_hops: 0,
        });
    }

    // Disallowed edges get INFINITY cost so astar effectively ignores them;
    // a finite total at the end proves the returned path uses only allowed edges.
    let edge_cost = |e: petgraph::graph::EdgeReference<'_, Dependency>| -> f64 {
        let d = e.weight();
        if !edge_allowed(d, &opts.allowed_kinds, &opts.min_confidence) {
            return f64::INFINITY;
        }
        if opts.weighted {
            weight_of(d)
        } else {
            1.0
        }
    };

    let (total, nodes) = astar(map, from, |n| n == to, edge_cost, |_| 0.0)?;

    if !total.is_finite() {
        return None;
    }
    if let Some(max) = opts.max_hops {
        if nodes.len().saturating_sub(1) > max {
            return None;
        }
    }

    // For each consecutive pair, recover the actual min-cost allowed edge
    // (handles parallel edges between the same two nodes — pick the one astar used).
    let mut hops = Vec::with_capacity(nodes.len().saturating_sub(1));
    for win in nodes.windows(2) {
        let (a, b) = (win[0], win[1]);
        let edge = map
            .edges_connecting(a, b)
            .filter(|e| edge_allowed(e.weight(), &opts.allowed_kinds, &opts.min_confidence))
            .min_by(|x, y| {
                let wx = if opts.weighted {
                    weight_of(x.weight())
                } else {
                    1.0
                };
                let wy = if opts.weighted {
                    weight_of(y.weight())
                } else {
                    1.0
                };
                wx.partial_cmp(&wy).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|e| e.weight().clone())?;

        hops.push(Hop {
            from: ArtifactRef::from_artifact(&map[a]),
            to: ArtifactRef::from_artifact(&map[b]),
            kind: format!("{:?}", edge.kind),
            confidence: format!("{:?}", edge.confidence),
            source_line: edge.source_line,
        });
    }

    let total_hops = hops.len();
    Some(Path {
        from: ArtifactRef::from_artifact(&map[from]),
        to: ArtifactRef::from_artifact(&map[to]),
        hops,
        total_weight: total,
        total_hops,
    })
}

// ── Subgraph (depth-limited BFS) ──────────────────────────────────────────────

/// Direction of BFS traversal. Distinct from [`petgraph::Direction`] because
/// we also need a `Both` (undirected) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Traversal {
    /// Follow outgoing edges — "what does this artifact reach?"
    Outgoing,
    /// Follow incoming edges — "what reaches this artifact?"
    Incoming,
    /// Follow both — undirected neighborhood.
    Both,
}

#[derive(Debug, Clone)]
pub struct SubgraphOptions {
    pub depth: usize,
    pub direction: Traversal,
    /// Default: `Some([Calls, Extends, Implements, Contains])` — exclude
    /// `Imports`/`References`/`Uses` to avoid file-level co-occurrence noise.
    /// Pass `None` to allow every kind.
    pub allowed_kinds: Option<Vec<DependencyKind>>,
    /// Skip *expanding through* any node whose total degree exceeds this.
    /// The supernode itself is still included as a boundary node and reported
    /// in [`Subgraph::supernodes_skipped`]. `None` = no cap.
    pub max_degree: Option<usize>,
}

impl Default for SubgraphOptions {
    fn default() -> Self {
        Self {
            depth: 2,
            direction: Traversal::Outgoing,
            allowed_kinds: Some(vec![
                DependencyKind::Calls,
                DependencyKind::Extends,
                DependencyKind::Implements,
                DependencyKind::Contains,
            ]),
            max_degree: Some(200),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Subgraph {
    pub center: ArtifactRef,
    pub depth: usize,
    pub artifacts: Vec<ArtifactRef>,
    pub dependencies: Vec<DependencyRef>,
    pub supernodes_skipped: Vec<SupernodeSkip>,
}

fn total_degree(map: &DependencyMap, n: NodeIndex) -> usize {
    map.edges_directed(n, Direction::Outgoing).count()
        + map.edges_directed(n, Direction::Incoming).count()
}

pub fn neighbors(map: &DependencyMap, center: NodeIndex, opts: &SubgraphOptions) -> Subgraph {
    let mut visited_nodes: HashSet<NodeIndex> = HashSet::new();
    let mut visited_edges: HashSet<EdgeIndex> = HashSet::new();
    let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();
    let mut dependencies: Vec<DependencyRef> = Vec::new();
    let mut supernodes: Vec<SupernodeSkip> = Vec::new();
    let mut supernodes_seen: HashSet<NodeIndex> = HashSet::new();

    visited_nodes.insert(center);
    queue.push_back((center, 0));

    let dirs: &[Direction] = match opts.direction {
        Traversal::Outgoing => &[Direction::Outgoing],
        Traversal::Incoming => &[Direction::Incoming],
        Traversal::Both => &[Direction::Outgoing, Direction::Incoming],
    };

    while let Some((node, d)) = queue.pop_front() {
        if d >= opts.depth {
            continue;
        }

        for dir in dirs {
            for e in map.edges_directed(node, *dir) {
                if let Some(ref ks) = opts.allowed_kinds {
                    if !ks.contains(&e.weight().kind) {
                        continue;
                    }
                }

                if !visited_edges.insert(e.id()) {
                    continue;
                }

                let (src, dst) = (e.source(), e.target());
                dependencies.push(DependencyRef {
                    source_id: map[src].id.clone(),
                    target_id: map[dst].id.clone(),
                    kind: format!("{:?}", e.weight().kind),
                    confidence: format!("{:?}", e.weight().confidence),
                    source_line: e.weight().source_line,
                });

                let other = if *dir == Direction::Outgoing {
                    dst
                } else {
                    src
                };

                if visited_nodes.insert(other) {
                    if let Some(cap) = opts.max_degree {
                        let deg = total_degree(map, other);
                        if deg > cap {
                            if supernodes_seen.insert(other) {
                                supernodes.push(SupernodeSkip {
                                    id: map[other].id.clone(),
                                    label: map[other].label.clone(),
                                    degree: deg,
                                });
                            }
                            continue; // include as boundary, don't expand through
                        }
                    }
                    queue.push_back((other, d + 1));
                }
            }
        }
    }

    let mut artifacts: Vec<ArtifactRef> = visited_nodes
        .iter()
        .map(|&n| ArtifactRef::from_artifact(&map[n]))
        .collect();
    artifacts.sort_by(|a, b| a.id.cmp(&b.id));

    Subgraph {
        center: ArtifactRef::from_artifact(&map[center]),
        depth: opts.depth,
        artifacts,
        dependencies,
        supernodes_skipped: supernodes,
    }
}

pub fn ancestors(map: &DependencyMap, target: NodeIndex, depth: usize) -> Subgraph {
    let opts = SubgraphOptions {
        depth,
        direction: Traversal::Incoming,
        ..Default::default()
    };
    neighbors(map, target, &opts)
}

pub fn descendants(map: &DependencyMap, source: NodeIndex, depth: usize) -> Subgraph {
    let opts = SubgraphOptions {
        depth,
        direction: Traversal::Outgoing,
        ..Default::default()
    };
    neighbors(map, source, &opts)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use grafly_core::{
        ArtifactKind, MapBuilder, RawArtifact, RawDependency, ScanResult, Visibility,
    };

    fn raw_artifact(id: &str, label: &str) -> RawArtifact {
        RawArtifact {
            id: id.to_string(),
            label: label.to_string(),
            kind: ArtifactKind::Function,
            source_file: format!("{}.rs", id),
            source_line: 1,
            description: None,
            is_entry_point: false,
            visibility: Visibility::Unknown,
        }
    }

    fn raw_dep(src: &str, dst: &str, kind: DependencyKind) -> RawDependency {
        RawDependency {
            source_id: src.to_string(),
            target_id: dst.to_string(),
            kind,
            confidence: Confidence::Extracted,
            source_line: 1,
        }
    }

    fn build_map(scan: ScanResult) -> DependencyMap {
        let mut b = MapBuilder::new();
        b.add_scan(scan);
        b.build()
    }

    /// Calls path (A→B→C) is preferred over the import shortcut (A→C via Imports).
    /// This is the "DataActorCore→ExecutionEngine" regression in miniature.
    #[test]
    fn weighted_path_prefers_calls_chain_over_import_shortcut() {
        let scan = ScanResult {
            artifacts: vec![
                raw_artifact("A", "A"),
                raw_artifact("B", "B"),
                raw_artifact("C", "C"),
            ],
            dependencies: vec![
                raw_dep("A", "B", DependencyKind::Calls),
                raw_dep("B", "C", DependencyKind::Calls),
                raw_dep("A", "C", DependencyKind::Imports),
            ],
            unresolved: vec![],
            main_package_dirs: vec![],
        };
        let map = build_map(scan);
        let a = resolve(&map, "A").unwrap();
        let c = resolve(&map, "C").unwrap();

        let path = find_path(&map, a, c, &PathOptions::default()).expect("path exists");
        assert_eq!(
            path.total_hops, 2,
            "should route through B (Calls=1+1=2), not direct Imports (=5)"
        );
        assert_eq!(path.hops[0].to.label, "B");
        assert_eq!(path.hops[1].to.label, "C");
    }

    /// Unweighted: shortest by hop count wins — direct Imports edge is preferred.
    #[test]
    fn unweighted_path_takes_direct_edge() {
        let scan = ScanResult {
            artifacts: vec![
                raw_artifact("A", "A"),
                raw_artifact("B", "B"),
                raw_artifact("C", "C"),
            ],
            dependencies: vec![
                raw_dep("A", "B", DependencyKind::Calls),
                raw_dep("B", "C", DependencyKind::Calls),
                raw_dep("A", "C", DependencyKind::Imports),
            ],
            unresolved: vec![],
            main_package_dirs: vec![],
        };
        let map = build_map(scan);
        let a = resolve(&map, "A").unwrap();
        let c = resolve(&map, "C").unwrap();

        let opts = PathOptions {
            weighted: false,
            ..Default::default()
        };
        let path = find_path(&map, a, c, &opts).expect("path exists");
        assert_eq!(path.total_hops, 1);
    }

    /// allowed_kinds excluding Imports forces the longer Calls path.
    #[test]
    fn allowed_kinds_filter_forces_calls_route() {
        let scan = ScanResult {
            artifacts: vec![
                raw_artifact("A", "A"),
                raw_artifact("B", "B"),
                raw_artifact("C", "C"),
            ],
            dependencies: vec![
                raw_dep("A", "B", DependencyKind::Calls),
                raw_dep("B", "C", DependencyKind::Calls),
                raw_dep("A", "C", DependencyKind::Imports),
            ],
            unresolved: vec![],
            main_package_dirs: vec![],
        };
        let map = build_map(scan);
        let a = resolve(&map, "A").unwrap();
        let c = resolve(&map, "C").unwrap();

        let opts = PathOptions {
            weighted: false,
            allowed_kinds: Some(vec![DependencyKind::Calls]),
            ..Default::default()
        };
        let path = find_path(&map, a, c, &opts).expect("path exists");
        assert_eq!(path.total_hops, 2);
    }

    /// Supernode (degree > max_degree) is included as boundary node but BFS
    /// doesn't expand through it — its neighbors are NOT pulled into the subgraph.
    #[test]
    fn supernode_cap_blocks_expansion_through_high_degree_node() {
        // Center A connects to Hub. Hub has 10 other neighbors. With max_degree=5,
        // Hub is reported as a supernode and its 10 neighbors are NOT included.
        let mut artifacts = vec![raw_artifact("A", "A"), raw_artifact("Hub", "Hub")];
        let mut deps = vec![raw_dep("A", "Hub", DependencyKind::Calls)];
        for i in 0..10 {
            let id = format!("X{}", i);
            artifacts.push(raw_artifact(&id, &id));
            deps.push(raw_dep("Hub", &id, DependencyKind::Calls));
        }
        let map = build_map(ScanResult {
            artifacts,
            dependencies: deps,
            unresolved: vec![],
            main_package_dirs: vec![],
        });
        let a = resolve(&map, "A").unwrap();

        let opts = SubgraphOptions {
            depth: 5,
            max_degree: Some(5),
            allowed_kinds: None,
            ..Default::default()
        };
        let sub = neighbors(&map, a, &opts);

        assert!(
            sub.artifacts.iter().any(|x| x.label == "Hub"),
            "Hub must be in the subgraph as a boundary node"
        );
        assert_eq!(
            sub.artifacts.len(),
            2,
            "only A and Hub — Hub's 10 neighbors are blocked by the supernode cap"
        );
        assert_eq!(sub.supernodes_skipped.len(), 1);
        assert_eq!(sub.supernodes_skipped[0].label, "Hub");
    }

    #[test]
    fn ancestors_returns_incoming_subgraph() {
        let scan = ScanResult {
            artifacts: vec![
                raw_artifact("A", "A"),
                raw_artifact("B", "B"),
                raw_artifact("C", "C"),
            ],
            dependencies: vec![
                raw_dep("A", "C", DependencyKind::Calls),
                raw_dep("B", "C", DependencyKind::Calls),
            ],
            unresolved: vec![],
            main_package_dirs: vec![],
        };
        let map = build_map(scan);
        let c = resolve(&map, "C").unwrap();

        let sub = ancestors(&map, c, 1);
        // C plus its two incoming callers A and B
        assert_eq!(sub.artifacts.len(), 3);
        let labels: HashSet<String> = sub.artifacts.iter().map(|a| a.label.clone()).collect();
        assert!(labels.contains("A") && labels.contains("B") && labels.contains("C"));
    }

    #[test]
    fn resolve_ambiguous_label_errors() {
        let scan = ScanResult {
            artifacts: vec![
                raw_artifact("a.rs::fn::new", "new"),
                raw_artifact("b.rs::fn::new", "new"),
            ],
            dependencies: vec![],
            unresolved: vec![],
            main_package_dirs: vec![],
        };
        let map = build_map(scan);
        assert!(matches!(
            resolve(&map, "new"),
            Err(QueryError::Ambiguous { .. })
        ));
        // But the exact ID still works.
        assert!(resolve(&map, "a.rs::fn::new").is_ok());
    }
}
