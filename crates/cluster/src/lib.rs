//! grafly-cluster — detect modules in a dependency map using the Leiden algorithm.

use grafly_core::{ArtifactKind, DependencyKind, DependencyMap};
use leiden_rs::{Leiden, LeidenConfig, QualityType};
use petgraph::graph::{Graph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Undirected;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, thiserror::Error)]
pub enum ModuleDetectionError {
    #[error("Graph conversion failed: {0}")]
    Conversion(String),
    #[error("Leiden algorithm failed: {0}")]
    Leiden(String),
}

/// Tunables for the Leiden run. The defaults trade a sub-percent quality drop
/// for a meaningful speedup vs leiden-rs's stock defaults.
///
/// Note on epsilon: it must stay **tight enough** that the first iteration's
/// modularity delta doesn't fall below the threshold and trigger premature
/// convergence on large graphs (the algorithm would exit with every node in
/// its own community). `min_iterations >= 2` guards against this regardless.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Hard cap on Leiden iterations. Default 30 (stock leiden-rs: 100).
    pub max_iterations: usize,
    /// Convergence threshold. Default 1e-8 (stock leiden-rs: 1e-10).
    /// Looser values risk premature exit on large graphs — see struct docs.
    pub epsilon: f64,
    /// Minimum iterations before convergence check. Default 3 (stock: 1).
    /// Floor that forces Leiden to do real work before believing "converged".
    pub min_iterations: usize,
    /// When true, skip Leiden's refinement phase (Louvain-equivalent behavior).
    /// Faster, slight quality drop, loses Leiden's "well-connected modules"
    /// guarantee. Default false.
    pub skip_refinement: bool,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            max_iterations: 30,
            epsilon: 1e-8,
            min_iterations: 3,
            skip_refinement: false,
        }
    }
}

impl DetectionConfig {
    /// Restores leiden-rs's stock defaults — slowest, highest quality.
    pub fn thorough() -> Self {
        Self {
            max_iterations: 100,
            epsilon: 1e-10,
            min_iterations: 1,
            skip_refinement: false,
        }
    }
}

/// The result of running module detection on a dependency map.
pub struct Modules {
    /// `members[i]` is the list of artifact NodeIndexes in module i.
    pub members: Vec<Vec<NodeIndex>>,
    /// `names[i]` is a human-readable name derived from the dominant artifact in module i.
    pub names: Vec<String>,
    /// Leiden modularity score of the partition.
    pub quality: f64,
}

impl Modules {
    pub fn count(&self) -> usize {
        self.members.len()
    }

    pub fn name_of(&self, module_id: usize) -> &str {
        self.names
            .get(module_id)
            .map(String::as_str)
            .unwrap_or("unnamed")
    }
}

/// Detect modules in a dependency map using the Leiden algorithm with fast defaults.
/// See [`DetectionConfig`] for the tunables; [`detect_modules_with_config`] takes
/// an explicit config.
pub fn detect_modules(
    map: &mut DependencyMap,
    resolution: f64,
    seed: Option<u64>,
) -> Result<Modules, ModuleDetectionError> {
    detect_modules_with_config(map, resolution, seed, &DetectionConfig::default())
}

/// Detect modules with an explicit [`DetectionConfig`]. Use this to override the
/// fast defaults — for example, [`DetectionConfig::thorough`] for maximum quality.
pub fn detect_modules_with_config(
    map: &mut DependencyMap,
    resolution: f64,
    seed: Option<u64>,
    config: &DetectionConfig,
) -> Result<Modules, ModuleDetectionError> {
    if map.node_count() == 0 {
        return Ok(Modules {
            members: vec![],
            names: vec![],
            quality: 0.0,
        });
    }

    // ── 1. Project to an undirected unit-weight shadow graph ─────────────────
    let mut shadow: Graph<(), f64, Undirected> = Graph::new_undirected();
    let orig_to_shadow: HashMap<NodeIndex, NodeIndex> = map
        .node_indices()
        .map(|n| (n, shadow.add_node(())))
        .collect();

    for edge in map.edge_indices() {
        let (src, dst) = map.edge_endpoints(edge).unwrap();
        let ws = orig_to_shadow[&src];
        let wd = orig_to_shadow[&dst];
        shadow.update_edge(ws, wd, 1.0);
    }

    // ── 2. Convert to leiden-rs GraphData ────────────────────────────────────
    let leiden_graph = leiden_rs::convert::petgraph::from_petgraph(&shadow)
        .map_err(|e| ModuleDetectionError::Conversion(e.to_string()))?;

    // ── 3. Build config and run ──────────────────────────────────────────────
    let mut builder = LeidenConfig::builder()
        .resolution(resolution)
        .quality(QualityType::Modularity)
        .max_iterations(config.max_iterations)
        .epsilon(config.epsilon)
        .min_iterations(config.min_iterations)
        .skip_refinement(config.skip_refinement);

    if let Some(s) = seed {
        builder = builder.seed(s);
    }

    let output = Leiden::new(builder.build())
        .run(&leiden_graph)
        .map_err(|e| ModuleDetectionError::Leiden(e.to_string()))?;

    // ── 4. Map modules back to original artifact indices ─────────────────────
    let num_modules = output.partition.num_communities();
    let mut members: Vec<Vec<NodeIndex>> = vec![Vec::new(); num_modules];

    let shadow_to_orig: HashMap<NodeIndex, NodeIndex> =
        orig_to_shadow.iter().map(|(&k, &v)| (v, k)).collect();

    for (shadow_id, module_id) in output.partition.iter() {
        let shadow_idx = NodeIndex::new(shadow_id);
        if let Some(&orig_idx) = shadow_to_orig.get(&shadow_idx) {
            members[module_id].push(orig_idx);
            if let Some(artifact) = map.node_weight_mut(orig_idx) {
                artifact.module_id = Some(module_id);
            }
        }
    }

    // ── 5. Derive human-readable module names ────────────────────────────────
    let names: Vec<String> = members
        .iter()
        .map(|nodes| pick_module_name(map, nodes))
        .collect();

    Ok(Modules {
        members,
        names,
        quality: output.quality,
    })
}

/// Minimum descendant count below which intra-package clustering is skipped.
/// Leiden on a < 5-node graph degenerates to one-node-per-module and adds noise.
pub const MIN_INTRA_PACKAGE_ARTIFACTS: usize = 5;

/// For each `Package` artifact in the map, run Leiden on the subgraph induced
/// by its transitive `Contains` descendants. Returns a map from Package
/// NodeIndex → `Modules` for that package.
///
/// This is *additive* to [`detect_modules`] — it doesn't mutate `Artifact.module_id`,
/// which keeps the global cross-package partition intact. Use both together to
/// answer (a) "where are the cross-cuts in this codebase?" (global modules) and
/// (b) "what subsystems exist inside each package?" (intra-package modules).
///
/// Packages with fewer than [`MIN_INTRA_PACKAGE_ARTIFACTS`] descendants get
/// an empty `Modules` result and aren't clustered.
pub fn detect_modules_within_packages(
    map: &DependencyMap,
    config: &DetectionConfig,
    seed: Option<u64>,
) -> Result<HashMap<NodeIndex, Modules>, ModuleDetectionError> {
    let mut result: HashMap<NodeIndex, Modules> = HashMap::new();

    for pkg_idx in map.node_indices() {
        if map[pkg_idx].kind != ArtifactKind::Package {
            continue;
        }

        let descendants = contains_descendants(map, pkg_idx);

        if descendants.len() < MIN_INTRA_PACKAGE_ARTIFACTS {
            result.insert(pkg_idx, Modules {
                members: vec![],
                names: vec![],
                quality: 0.0,
            });
            continue;
        }

        let modules = leiden_on_induced_subgraph(map, &descendants, config, seed)?;
        result.insert(pkg_idx, modules);
    }

    Ok(result)
}

/// Transitive descendants of `root` via `Contains` edges only. Excludes `root`.
fn contains_descendants(map: &DependencyMap, root: NodeIndex) -> Vec<NodeIndex> {
    let mut out = Vec::new();
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    visited.insert(root);
    queue.push_back(root);

    while let Some(n) = queue.pop_front() {
        for e in map.edges_directed(n, petgraph::Direction::Outgoing) {
            if e.weight().kind != DependencyKind::Contains {
                continue;
            }
            let target = e.target();
            if visited.insert(target) {
                queue.push_back(target);
                out.push(target);
            }
        }
    }
    out
}

/// Run Leiden on the subgraph of `map` induced on `nodes`. Returned `Modules`
/// reference the *original* NodeIndexes, not subgraph indices.
fn leiden_on_induced_subgraph(
    map: &DependencyMap,
    nodes: &[NodeIndex],
    config: &DetectionConfig,
    seed: Option<u64>,
) -> Result<Modules, ModuleDetectionError> {
    let mut shadow: Graph<(), f64, Undirected> = Graph::new_undirected();
    let mut orig_to_shadow: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    for &orig in nodes {
        orig_to_shadow.insert(orig, shadow.add_node(()));
    }
    for edge in map.edge_indices() {
        let (src, dst) = map.edge_endpoints(edge).unwrap();
        if let (Some(&ws), Some(&wd)) = (orig_to_shadow.get(&src), orig_to_shadow.get(&dst)) {
            shadow.update_edge(ws, wd, 1.0);
        }
    }

    let leiden_graph = leiden_rs::convert::petgraph::from_petgraph(&shadow)
        .map_err(|e| ModuleDetectionError::Conversion(e.to_string()))?;

    let mut builder = LeidenConfig::builder()
        .resolution(1.0)
        .quality(QualityType::Modularity)
        .max_iterations(config.max_iterations)
        .epsilon(config.epsilon)
        .min_iterations(config.min_iterations)
        .skip_refinement(config.skip_refinement);
    if let Some(s) = seed {
        builder = builder.seed(s);
    }

    let output = Leiden::new(builder.build())
        .run(&leiden_graph)
        .map_err(|e| ModuleDetectionError::Leiden(e.to_string()))?;

    let num_modules = output.partition.num_communities();
    let mut members: Vec<Vec<NodeIndex>> = vec![Vec::new(); num_modules];
    let shadow_to_orig: HashMap<NodeIndex, NodeIndex> =
        orig_to_shadow.iter().map(|(&k, &v)| (v, k)).collect();

    for (shadow_id, module_id) in output.partition.iter() {
        let shadow_idx = NodeIndex::new(shadow_id);
        if let Some(&orig_idx) = shadow_to_orig.get(&shadow_idx) {
            members[module_id].push(orig_idx);
        }
    }

    let names: Vec<String> = members
        .iter()
        .map(|nodes| pick_module_name(map, nodes))
        .collect();

    Ok(Modules {
        members,
        names,
        quality: output.quality,
    })
}

/// Common method/dunder labels that win the highest-degree tiebreak by sheer
/// prevalence (`new`, `default`, `__init__`, `fmt`, ...) but say nothing about
/// what the module actually contains. Excluded from name candidates.
const NOISY_LABELS: &[&str] = &[
    // Python dunder methods
    "__init__", "__str__", "__repr__", "__eq__", "__ne__", "__hash__",
    "__lt__", "__gt__", "__le__", "__ge__", "__cmp__", "__richcmp__",
    "__getitem__", "__setitem__", "__delitem__", "__contains__",
    "__len__", "__iter__", "__next__", "__reversed__",
    "__enter__", "__exit__", "__call__",
    "__new__", "__del__", "__copy__", "__deepcopy__",
    "__bool__", "__nonzero__", "__sizeof__",
    "__add__", "__sub__", "__mul__", "__truediv__", "__floordiv__",
    "__mod__", "__pow__", "__neg__", "__pos__", "__abs__",
    "__and__", "__or__", "__xor__", "__lshift__", "__rshift__",
    "__getattr__", "__setattr__", "__delattr__",
    "__getstate__", "__setstate__", "__reduce__",
    "__format__", "__dir__",
    "setUp", "tearDown", "setUpClass", "tearDownClass",
    // Rust pervasive trait methods
    "new", "default", "clone", "drop", "fmt",
    "eq", "ne", "hash", "partial_cmp", "cmp",
    "deref", "deref_mut", "borrow", "borrow_mut",
    "as_ref", "as_mut", "as_any", "as_str", "as_bytes", "as_slice",
    "from", "from_str", "from_iter", "into", "try_from", "try_into",
    "next", "iter", "into_iter", "iter_mut",
    "len", "is_empty", "size_hint",
    // Generic getters/setters that appear everywhere
    "name", "id", "kind", "type", "type_name", "value", "ts_init",
    // PyO3 wrapper convention
    "py_new", "py_to_dict",
];

fn is_noisy_label(label: &str) -> bool {
    if NOISY_LABELS.contains(&label) {
        return true;
    }
    // PyO3 bindings: `py_*` (e.g. `py_is_cash_account`, `py_from_order_side`)
    // are pyclass shims, not architectural identities.
    label.starts_with("py_")
}

/// Pick a representative name for a module. Strategy:
/// 1. Among non-noisy artifacts, prefer Classes/Structs/Traits/Interfaces over
///    Functions/Methods, breaking ties by total degree.
/// 2. If every candidate is filtered as noisy, fall back to the file stem of
///    the source file that holds the most members of this module.
fn pick_module_name(map: &DependencyMap, members: &[NodeIndex]) -> String {
    if members.is_empty() {
        return String::from("empty");
    }

    let best = members
        .iter()
        .filter_map(|&n| {
            let a = &map[n];
            if is_noisy_label(&a.label) {
                return None;
            }
            let degree = map.edges_directed(n, petgraph::Direction::Outgoing).count()
                + map.edges_directed(n, petgraph::Direction::Incoming).count();
            Some((n, kind_priority(&a.kind), degree))
        })
        .max_by_key(|&(_, prio, deg)| (prio, deg))
        .map(|(n, _, _)| map[n].display_label());

    if let Some(name) = best {
        return name;
    }

    // Fallback: dominant source-file stem.
    let mut file_counts: HashMap<&str, usize> = HashMap::new();
    for &n in members {
        *file_counts.entry(map[n].source_file.as_str()).or_default() += 1;
    }
    if let Some((file, _)) = file_counts.into_iter().max_by_key(|&(_, c)| c) {
        return std::path::Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(String::from)
            .unwrap_or_else(|| file.to_string());
    }

    String::from("unnamed")
}

fn kind_priority(kind: &ArtifactKind) -> u8 {
    match kind {
        ArtifactKind::Class
        | ArtifactKind::Struct
        | ArtifactKind::Interface
        | ArtifactKind::Trait => 4,
        ArtifactKind::Function | ArtifactKind::Method => 3,
        ArtifactKind::Enum | ArtifactKind::Namespace => 2,
        ArtifactKind::File => 1,
        // Package and Import are containers/symbolic, not naming candidates.
        ArtifactKind::Import | ArtifactKind::Package => 0,
    }
}
