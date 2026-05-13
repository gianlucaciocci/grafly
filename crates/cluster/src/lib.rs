//! grafly-cluster — detect modules in a dependency map using the Leiden algorithm.

use grafly_core::{ArtifactKind, DependencyMap};
use leiden_rs::{Leiden, LeidenConfig, QualityType};
use petgraph::graph::{Graph, NodeIndex};
use petgraph::Undirected;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ModuleDetectionError {
    #[error("Graph conversion failed: {0}")]
    Conversion(String),
    #[error("Leiden algorithm failed: {0}")]
    Leiden(String),
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

/// Detect modules in a dependency map using the Leiden algorithm.
pub fn detect_modules(
    map: &mut DependencyMap,
    resolution: f64,
    seed: Option<u64>,
) -> Result<Modules, ModuleDetectionError> {
    if map.node_count() == 0 {
        return Ok(Modules {
            members: vec![],
            names: vec![],
            quality: 0.0,
        });
    }

    // ── 1. Project to undirected unit-weight shadow graph ────────────────────
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
        .quality(QualityType::Modularity);

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

/// Pick a representative name for a module: the label of its highest-priority
/// artifact. Priority favours Classes/Structs/Interfaces/Traits over Functions/
/// Methods over everything else; degree is the tiebreaker.
fn pick_module_name(map: &DependencyMap, members: &[NodeIndex]) -> String {
    if members.is_empty() {
        return String::from("empty");
    }

    members
        .iter()
        .map(|&n| {
            let a = &map[n];
            let degree = map.edges_directed(n, petgraph::Direction::Outgoing).count()
                + map.edges_directed(n, petgraph::Direction::Incoming).count();
            (n, kind_priority(&a.kind), degree)
        })
        .max_by_key(|&(_, prio, deg)| (prio, deg))
        .map(|(n, _, _)| map[n].label.clone())
        .unwrap_or_else(|| String::from("unnamed"))
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
        ArtifactKind::Import => 0,
    }
}
