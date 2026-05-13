//! grafly-analyze — derive insights from a dependency map.
//!
//! - **Hotspots**: artifacts whose degree is more than 2σ above the mean.
//! - **Couplings**: dependencies that cross module boundaries.
//! - **Insights**: suggested questions about the codebase architecture.

use grafly_core::{DependencyMap, Visibility};
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub hotspots: Vec<Hotspot>,
    pub couplings: Vec<Coupling>,
    pub insights: Vec<String>,
}

/// Knobs for [`analyze_with_options`]. Defaults exclude Private symbols from
/// hotspots and couplings so the report stays focused on the public surface
/// area — flip `include_private` to surface internal helpers as well.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    pub include_private: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            include_private: false,
        }
    }
}

/// An artifact whose degree is more than 2σ above the mean — likely an
/// architectural bottleneck or central dependency.
#[derive(Debug, Serialize)]
pub struct Hotspot {
    #[serde(skip)]
    pub index: NodeIndex,
    pub label: String,
    pub degree: usize,
    pub source_file: String,
}

/// A dependency that crosses module boundaries — a coupling between modules.
#[derive(Debug, Serialize)]
pub struct Coupling {
    pub from_label: String,
    pub to_label: String,
    pub kind: String,
    pub from_module: usize,
    pub to_module: usize,
    pub source_file: String,
    pub source_line: usize,
}

pub fn analyze(map: &DependencyMap) -> Analysis {
    analyze_with_options(map, AnalysisOptions::default())
}

/// Like [`analyze`] but applies [`AnalysisOptions`]. With default options,
/// Private artifacts are excluded from hotspots and couplings — they're
/// implementation details that shouldn't drive architectural recommendations.
pub fn analyze_with_options(map: &DependencyMap, opts: AnalysisOptions) -> Analysis {
    let hotspots = find_hotspots(map, opts.include_private);
    let couplings = find_couplings(map, opts.include_private);
    let insights = generate_insights(map, &hotspots, &couplings);

    Analysis {
        hotspots,
        couplings,
        insights,
    }
}

/// True when this artifact should be excluded from the public surface of the
/// architecture report. Only fires for explicitly Private symbols — Unknown
/// (the default for files, packages, namespaces) is *not* filtered.
fn is_private(map: &DependencyMap, idx: NodeIndex) -> bool {
    matches!(map[idx].visibility, Visibility::Private)
}

fn find_hotspots(map: &DependencyMap, include_private: bool) -> Vec<Hotspot> {
    if map.node_count() < 3 {
        return vec![];
    }

    let degrees: Vec<(NodeIndex, usize)> = map
        .node_indices()
        .map(|n| {
            let degree = map
                .edges_directed(n, petgraph::Direction::Outgoing)
                .count()
                + map
                    .edges_directed(n, petgraph::Direction::Incoming)
                    .count();
            (n, degree)
        })
        .collect();

    let n = degrees.len() as f64;
    let mean = degrees.iter().map(|(_, d)| *d as f64).sum::<f64>() / n;
    let variance = degrees
        .iter()
        .map(|(_, d)| (*d as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    let std_dev = variance.sqrt();
    let threshold = mean + 2.0 * std_dev;

    let mut hotspots: Vec<Hotspot> = degrees
        .into_iter()
        .filter(|(_, d)| *d as f64 > threshold)
        .filter(|(idx, _)| include_private || !is_private(map, *idx))
        .map(|(idx, degree)| Hotspot {
            index: idx,
            label: map[idx].display_label(),
            degree,
            source_file: map[idx].source_file.clone(),
        })
        .collect();

    hotspots.sort_by(|a, b| b.degree.cmp(&a.degree));
    hotspots
}

fn find_couplings(map: &DependencyMap, include_private: bool) -> Vec<Coupling> {
    let mut couplings: Vec<Coupling> = map
        .edge_references()
        .filter_map(|e| {
            let src = &map[e.source()];
            let dst = &map[e.target()];
            let edge = e.weight();
            if !include_private && (is_private(map, e.source()) || is_private(map, e.target())) {
                return None;
            }
            match (src.module_id, dst.module_id) {
                (Some(c1), Some(c2)) if c1 != c2 => Some(Coupling {
                    from_label: src.display_label(),
                    to_label: dst.display_label(),
                    kind: format!("{:?}", edge.kind),
                    from_module: c1,
                    to_module: c2,
                    source_file: src.source_file.clone(),
                    source_line: edge.source_line,
                }),
                _ => None,
            }
        })
        .collect();

    couplings.sort_by_key(|c| {
        (c.from_module as isize - c.to_module as isize).unsigned_abs()
    });
    couplings.reverse();
    couplings
}

fn generate_insights(
    _map: &DependencyMap,
    hotspots: &[Hotspot],
    couplings: &[Coupling],
) -> Vec<String> {
    let mut insights: Vec<String> = Vec::new();

    for h in hotspots.iter().take(3) {
        insights.push(format!(
            "`{}` ({}) is a hotspot with {} connections — consider splitting it.",
            h.label,
            h.source_file,
            h.degree
        ));
    }

    for c in couplings.iter().take(5) {
        insights.push(format!(
            "`{}` (module {}) couples to `{}` (module {}) via `{}` — is this intentional?",
            c.from_label, c.from_module, c.to_label, c.to_module, c.kind
        ));
    }

    if insights.is_empty() {
        insights.push("What are the main entry points of this codebase?".into());
        insights.push("Which artifacts have the most transitive dependencies?".into());
        insights.push("Are there any circular dependencies between modules?".into());
    }

    insights
}
