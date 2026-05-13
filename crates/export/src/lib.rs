//! grafly-export — write a dependency map to JSON / HTML.

use grafly_core::{DependencyKind, DependencyMap};
use petgraph::graph::{EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Options for the artifact-level HTML export.
#[derive(Debug, Clone)]
pub struct HtmlOptions {
    /// Cap the number of artifacts rendered (top-N by degree). `None` = unlimited.
    pub max_nodes: Option<usize>,
    /// Module names indexed by module_id (from `Modules::names`).
    pub module_names: Vec<String>,
    /// Include `Confidence::Ambiguous` edges in the visualization.
    /// Default: `false`. Ambiguous edges are heuristic matches that often
    /// create supernode shortcuts; they're kept in the JSON but suppressed
    /// from the HTML to keep it readable.
    pub include_ambiguous: bool,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        Self {
            max_nodes: Some(800),
            module_names: Vec::new(),
            include_ambiguous: false,
        }
    }
}

/// Options for the module-level HTML export.
#[derive(Debug, Clone)]
pub struct ModuleHtmlOptions {
    /// Top-N modules to render (by artifact count). `None` = unlimited.
    pub max_modules: Option<usize>,
    /// Module names indexed by module_id.
    pub module_names: Vec<String>,
}

impl Default for ModuleHtmlOptions {
    fn default() -> Self {
        Self {
            max_modules: Some(100),
            module_names: Vec::new(),
        }
    }
}

// ── JSON (always full) ───────────────────────────────────────────────────────

pub fn to_json(map: &DependencyMap) -> Value {
    let artifacts: Vec<Value> = map
        .node_indices()
        .map(|n| {
            let a = &map[n];
            json!({
                "id":             a.id,
                "label":          a.display_label(),
                "kind":           format!("{:?}", a.kind),
                "source_file":    a.source_file,
                "source_line":    a.source_line,
                "module_id":      a.module_id,
                "description":    a.description,
                "is_entry_point": a.is_entry_point,
            })
        })
        .collect();

    let dependencies: Vec<Value> = map
        .edge_indices()
        .map(|e| {
            let (src, dst) = map.edge_endpoints(e).unwrap();
            let d = &map[e];
            json!({
                "source":      map[src].id,
                "target":      map[dst].id,
                "kind":        format!("{:?}", d.kind),
                "confidence":  format!("{:?}", d.confidence),
                "source_line": d.source_line,
            })
        })
        .collect();

    json!({ "artifacts": artifacts, "dependencies": dependencies })
}

pub fn write_json(map: &DependencyMap, path: &Path) -> Result<(), ExportError> {
    let value = to_json(map);
    let s = serde_json::to_string_pretty(&value)?;
    std::fs::write(path, s)?;
    Ok(())
}

// ── HTML (artifact-level, filtered) ──────────────────────────────────────────

pub fn write_html(
    map: &DependencyMap,
    opts: &HtmlOptions,
    path: &Path,
) -> Result<(), ExportError> {
    let (kept_nodes, kept_edges) = select_for_viz(map, opts.max_nodes, opts.include_ambiguous);
    let payload = filtered_artifact_payload(map, &kept_nodes, &kept_edges, &opts.module_names);
    let payload_json = serde_json::to_string(&payload)?;
    std::fs::write(path, build_artifact_html(&payload_json))?;
    Ok(())
}

fn select_for_viz(
    map: &DependencyMap,
    max_nodes: Option<usize>,
    include_ambiguous: bool,
) -> (Vec<NodeIndex>, Vec<EdgeIndex>) {
    let total = map.node_count();
    let cap = max_nodes.filter(|n| *n < total);

    let kept_set: HashSet<NodeIndex> = if let Some(c) = cap {
        let mut by_degree: Vec<(NodeIndex, usize)> = map
            .node_indices()
            .map(|n| {
                let d = map.edges_directed(n, petgraph::Direction::Outgoing).count()
                    + map.edges_directed(n, petgraph::Direction::Incoming).count();
                (n, d)
            })
            .collect();
        by_degree.sort_by(|a, b| b.1.cmp(&a.1));
        by_degree.iter().take(c).map(|(n, _)| *n).collect()
    } else {
        map.node_indices().collect()
    };

    let kept_edges: Vec<EdgeIndex> = map
        .edge_indices()
        .filter(|e| {
            let (s, t) = map.edge_endpoints(*e).unwrap();
            if !kept_set.contains(&s) || !kept_set.contains(&t) {
                return false;
            }
            include_ambiguous || map[*e].confidence != grafly_core::Confidence::Ambiguous
        })
        .collect();

    (kept_set.into_iter().collect(), kept_edges)
}

fn filtered_artifact_payload(
    map: &DependencyMap,
    nodes: &[NodeIndex],
    edges: &[EdgeIndex],
    module_names: &[String],
) -> Value {
    let total_artifacts = map.node_count();
    let total_dependencies = map.edge_count();

    let artifacts: Vec<Value> = nodes
        .iter()
        .map(|&n| {
            let a = &map[n];
            let module_label = a.module_id.and_then(|id| module_names.get(id)).cloned();
            json!({
                "id":             a.id,
                "label":          a.display_label(),
                "kind":           format!("{:?}", a.kind),
                "source_file":    a.source_file,
                "source_line":    a.source_line,
                "module_id":      a.module_id,
                "module_name":    module_label,
                "description":    a.description,
                "is_entry_point": a.is_entry_point,
            })
        })
        .collect();

    let dependencies: Vec<Value> = edges
        .iter()
        .map(|&e| {
            let (src, dst) = map.edge_endpoints(e).unwrap();
            let d = &map[e];
            json!({
                "source":      map[src].id,
                "target":      map[dst].id,
                "kind":        format!("{:?}", d.kind),
                "confidence":  format!("{:?}", d.confidence),
                "source_line": d.source_line,
            })
        })
        .collect();

    json!({
        "artifacts": artifacts,
        "dependencies": dependencies,
        "stats": {
            "shown_artifacts": nodes.len(),
            "shown_dependencies": edges.len(),
            "total_artifacts": total_artifacts,
            "total_dependencies": total_dependencies,
            "filtered": nodes.len() < total_artifacts,
        }
    })
}

// ── HTML (module-level overview) ─────────────────────────────────────────────

/// Write a module-level dependency map: nodes are modules, edges are
/// aggregated cross-module dependencies grouped by `DependencyKind`.
/// This is the bird's-eye view — far more legible than the artifact-level
/// graph for large codebases.
pub fn write_html_modules(
    map: &DependencyMap,
    opts: &ModuleHtmlOptions,
    path: &Path,
) -> Result<(), ExportError> {
    let payload = build_module_payload(map, opts);
    let payload_json = serde_json::to_string(&payload)?;
    std::fs::write(path, build_module_html(&payload_json))?;
    Ok(())
}

fn build_module_payload(map: &DependencyMap, opts: &ModuleHtmlOptions) -> Value {
    // 1) Count artifacts per module
    let mut module_sizes: HashMap<usize, usize> = HashMap::new();
    for n in map.node_indices() {
        if let Some(m) = map[n].module_id {
            *module_sizes.entry(m).or_default() += 1;
        }
    }

    // 2) Pick top-N modules by size
    let mut module_ids: Vec<(usize, usize)> = module_sizes.into_iter().collect();
    module_ids.sort_by(|a, b| b.1.cmp(&a.1));
    let total_modules = module_ids.len();
    let kept_ids: HashSet<usize> = match opts.max_modules {
        Some(n) if n < total_modules => {
            module_ids.iter().take(n).map(|(id, _)| *id).collect()
        }
        _ => module_ids.iter().map(|(id, _)| *id).collect(),
    };
    let shown_modules = kept_ids.len();

    // 3) Aggregate cross-module edges
    // Key: (source_module, target_module) → kind counts
    let mut agg: BTreeMap<(usize, usize), HashMap<String, usize>> = BTreeMap::new();
    let mut total_cross_module = 0usize;
    for e in map.edge_references() {
        let src_mod = map[e.source()].module_id;
        let dst_mod = map[e.target()].module_id;
        let (Some(sm), Some(dm)) = (src_mod, dst_mod) else {
            continue;
        };
        if sm == dm {
            continue;
        }
        if !kept_ids.contains(&sm) || !kept_ids.contains(&dm) {
            continue;
        }
        total_cross_module += 1;
        let kind_label = format!("{:?}", e.weight().kind);
        *agg.entry((sm, dm)).or_default().entry(kind_label).or_default() += 1;
    }

    // 4) Build node and edge JSON
    let module_nodes: Vec<Value> = module_ids
        .iter()
        .filter(|(id, _)| kept_ids.contains(id))
        .map(|(id, size)| {
            let name = opts
                .module_names
                .get(*id)
                .cloned()
                .unwrap_or_else(|| format!("module {}", id));
            json!({
                "id":    id,
                "label": format!("{}: {}", id, name),
                "size":  size,
            })
        })
        .collect();

    let module_edges: Vec<Value> = agg
        .into_iter()
        .map(|((sm, dm), counts)| {
            let total: usize = counts.values().sum();
            let mut breakdown: Vec<(String, usize)> =
                counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
            breakdown.sort_by(|a, b| b.1.cmp(&a.1));
            let dominant = breakdown[0].0.clone();
            let label = breakdown
                .iter()
                .map(|(k, v)| format!("{}×{}", k, v))
                .collect::<Vec<_>>()
                .join(" · ");
            json!({
                "from":     sm,
                "to":       dm,
                "total":    total,
                "dominant": dominant,
                "label":    label,
                "breakdown": counts,
            })
        })
        .collect();

    json!({
        "modules": module_nodes,
        "edges": module_edges,
        "stats": {
            "shown_modules": shown_modules,
            "total_modules": total_modules,
            "cross_module_edges_shown": module_edges.len(),
            "cross_module_dependencies_aggregated": total_cross_module,
            "filtered": shown_modules < total_modules,
        }
    })
}

// ── HTML templates ───────────────────────────────────────────────────────────

const COMMUNITY_PALETTE: &str = r##"[
  "#e63946","#2a9d8f","#e9c46a","#264653","#f4a261",
  "#a8dadc","#457b9d","#1d3557","#f1faee","#6d6875",
  "#b5838d","#e5989b","#ffb4a2","#ffcdb2","#80b918",
  "#007200","#38b000","#70e000","#ccff33","#aacc00"
]"##;

/// Edge colours by `DependencyKind`. Synced between both HTML views.
const KIND_COLORS: &str = r##"{
  "Contains":   "#5c6b73",
  "Imports":    "#7b9acc",
  "References": "#9a78d0",
  "Calls":      "#2a9d8f",
  "Extends":    "#e76f51",
  "Implements": "#f4a261",
  "Uses":       "#888888"
}"##;

fn build_artifact_html(payload_json: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Grafly — Dependency Map</title>
<script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0f0f1a; color: #e0e0e0; font-family: system-ui, sans-serif; }}
  #graph {{ width: 100vw; height: 100vh; }}
  #panel {{
    position: fixed; top: 12px; left: 12px; z-index: 10;
    background: rgba(15,15,26,.88); border: 1px solid #333;
    border-radius: 10px; padding: 14px 18px; min-width: 240px;
    backdrop-filter: blur(6px);
  }}
  #panel h2 {{ font-size: 1rem; color: #a0c4ff; margin-bottom: 8px; }}
  #panel p  {{ font-size: .78rem; color: #aaa; line-height: 1.5; }}
  #panel .warn {{ color: #f4a261; font-weight: 500; }}
  #info {{ margin-top: 10px; font-size: .78rem; color: #ccc; line-height: 1.4; }}
  #legend {{ margin-top: 10px; font-size: .72rem; color: #aaa; }}
  #legend span {{ display: inline-block; margin-right: 8px; }}
  #legend i {{ display: inline-block; width: 10px; height: 2px; vertical-align: middle; margin-right: 2px; }}
</style>
</head>
<body>
<div id="panel">
  <h2>grafly</h2>
  <p id="stats">Loading…</p>
  <div id="info"></div>
  <div id="legend"></div>
</div>
<div id="graph"></div>
<script>
const DATA   = {payload_json};
const COLORS = {palette};
const KIND   = {kind_colors};

const shapeFor = kind => ({{
  File: 'diamond', Class: 'box', Struct: 'box',
  Interface: 'hexagon', Trait: 'hexagon', Enum: 'triangle',
  Function: 'dot', Method: 'dot', Import: 'ellipse', Namespace: 'square',
}})[kind] ?? 'dot';

const nodes = new vis.DataSet(DATA.artifacts.map(a => ({{
  id:    a.id,
  label: a.label,
  title: `${{a.kind}}<br>${{a.source_file}}:${{a.source_line}}${{a.module_id != null ? '<br>module ' + a.module_id + (a.module_name ? ' — ' + a.module_name : '') : ''}}`,
  color: a.module_id != null ? COLORS[a.module_id % COLORS.length] : '#555',
  shape: shapeFor(a.kind),
  size:  a.kind === 'File' ? 18 : 10,
}})));

const edges = new vis.DataSet(DATA.dependencies.map((d, i) => ({{
  id:     i,
  from:   d.source,
  to:     d.target,
  label:  d.kind,
  title:  `${{d.kind}} · ${{d.confidence}} · L${{d.source_line}}`,
  arrows: 'to',
  dashes: d.confidence === 'Inferred' || d.confidence === 'Ambiguous',
  color:  {{ color: KIND[d.kind] || '#555', highlight: '#fff' }},
  font:   {{ color: '#aaa', size: 9, align: 'middle' }},
  width:  d.confidence === 'Extracted' ? 1.5 : 1,
}})));

const s = DATA.stats;
document.getElementById('stats').innerHTML = s.filtered
  ? `<span class="warn">Showing top ${{s.shown_artifacts.toLocaleString()}} of ${{s.total_artifacts.toLocaleString()}} artifacts</span><br>${{s.shown_dependencies.toLocaleString()}} of ${{s.total_dependencies.toLocaleString()}} dependencies`
  : `${{s.shown_artifacts.toLocaleString()}} artifacts · ${{s.shown_dependencies.toLocaleString()}} dependencies`;

document.getElementById('legend').innerHTML =
  Object.entries(KIND).map(([k,v]) => `<span><i style="background:${{v}}"></i>${{k}}</span>`).join('') +
  `<br><span style="color:#777">dashed = Inferred/Ambiguous</span>`;

const net = new vis.Network(
  document.getElementById('graph'),
  {{ nodes, edges }},
  {{
    physics: {{
      solver: 'forceAtlas2Based',
      forceAtlas2Based: {{ gravitationalConstant: -60, springLength: 120 }},
      stabilization: {{ iterations: 250 }},
    }},
    nodes: {{ font: {{ color: '#fff', size: 12 }} }},
    edges: {{ smooth: {{ type: 'continuous' }} }},
  }}
);

net.on('click', params => {{
  if (!params.nodes.length) return;
  const a = DATA.artifacts.find(x => x.id === params.nodes[0]);
  if (!a) return;
  document.getElementById('info').innerHTML =
    `<strong>${{a.label}}</strong><br>
     Kind: ${{a.kind}}<br>
     File: ${{a.source_file}}:${{a.source_line}}<br>
     Module: ${{a.module_id != null ? a.module_id + (a.module_name ? ' — ' + a.module_name : '') : '—'}}`;
}});
</script>
</body>
</html>"##,
        payload_json = payload_json,
        palette = COMMUNITY_PALETTE,
        kind_colors = KIND_COLORS
    )
}

fn build_module_html(payload_json: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Grafly — Module Overview</title>
<script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0f0f1a; color: #e0e0e0; font-family: system-ui, sans-serif; }}
  #graph {{ width: 100vw; height: 100vh; }}
  #panel {{
    position: fixed; top: 12px; left: 12px; z-index: 10;
    background: rgba(15,15,26,.9); border: 1px solid #333;
    border-radius: 10px; padding: 14px 18px; min-width: 280px;
    backdrop-filter: blur(6px);
  }}
  #panel h2 {{ font-size: 1rem; color: #a0c4ff; margin-bottom: 8px; }}
  #panel p  {{ font-size: .78rem; color: #aaa; line-height: 1.5; }}
  #panel .warn {{ color: #f4a261; font-weight: 500; }}
  #info {{ margin-top: 10px; font-size: .78rem; color: #ccc; line-height: 1.4; }}
  #legend {{ margin-top: 10px; font-size: .72rem; color: #aaa; }}
  #legend span {{ display: inline-block; margin-right: 8px; }}
  #legend i {{ display: inline-block; width: 12px; height: 3px; vertical-align: middle; margin-right: 2px; }}
</style>
</head>
<body>
<div id="panel">
  <h2>grafly — module overview</h2>
  <p id="stats">Loading…</p>
  <div id="info"></div>
  <div id="legend"></div>
</div>
<div id="graph"></div>
<script>
const DATA = {payload_json};
const KIND = {kind_colors};
const COLORS = {palette};

const maxSize = Math.max(1, ...DATA.modules.map(m => m.size));
const minSize = Math.min(...DATA.modules.map(m => m.size));
const sizeOf = sz => 14 + Math.log2(1 + sz) * 6;

const nodes = new vis.DataSet(DATA.modules.map(m => ({{
  id:    m.id,
  label: m.label,
  title: `${{m.label}}<br>${{m.size}} artifacts`,
  color: COLORS[m.id % COLORS.length],
  shape: 'dot',
  size:  sizeOf(m.size),
  font:  {{ color: '#fff', size: 13 }},
}})));

const maxWeight = Math.max(1, ...DATA.edges.map(e => e.total));
const widthOf = total => 1 + Math.log2(1 + total) * 1.5;

const edges = new vis.DataSet(DATA.edges.map((e, i) => ({{
  id:     i,
  from:   e.from,
  to:     e.to,
  label:  e.label,
  title:  e.label,
  arrows: 'to',
  width:  widthOf(e.total),
  color:  {{ color: KIND[e.dominant] || '#888', highlight: '#fff', opacity: 0.7 }},
  font:   {{ color: '#bbb', size: 10, align: 'middle', strokeWidth: 0 }},
  smooth: {{ type: 'curvedCW', roundness: 0.15 }},
}})));

const s = DATA.stats;
document.getElementById('stats').innerHTML = s.filtered
  ? `<span class="warn">Showing top ${{s.shown_modules.toLocaleString()}} of ${{s.total_modules.toLocaleString()}} modules</span><br>${{s.cross_module_edges_shown.toLocaleString()}} cross-module edges (${{s.cross_module_dependencies_aggregated.toLocaleString()}} dependencies aggregated)`
  : `${{s.shown_modules.toLocaleString()}} modules · ${{s.cross_module_edges_shown.toLocaleString()}} cross-module edges (${{s.cross_module_dependencies_aggregated.toLocaleString()}} dependencies aggregated)`;

document.getElementById('legend').innerHTML =
  Object.entries(KIND).map(([k,v]) => `<span><i style="background:${{v}}"></i>${{k}}</span>`).join('') +
  `<br><span style="color:#777">edge width ∝ log(total dependencies)</span>`;

const net = new vis.Network(
  document.getElementById('graph'),
  {{ nodes, edges }},
  {{
    physics: {{
      solver: 'forceAtlas2Based',
      forceAtlas2Based: {{ gravitationalConstant: -80, springLength: 180, avoidOverlap: 0.5 }},
      stabilization: {{ iterations: 400 }},
    }},
    interaction: {{ hover: true, tooltipDelay: 200 }},
  }}
);

net.on('click', params => {{
  if (params.nodes.length) {{
    const m = DATA.modules.find(x => x.id === params.nodes[0]);
    if (!m) return;
    document.getElementById('info').innerHTML =
      `<strong>${{m.label}}</strong><br>Artifacts: ${{m.size}}`;
  }} else if (params.edges.length) {{
    const e = DATA.edges[params.edges[0]];
    if (!e) return;
    const from = DATA.modules.find(m => m.id === e.from);
    const to = DATA.modules.find(m => m.id === e.to);
    const breakdownLines = Object.entries(e.breakdown)
      .sort((a,b) => b[1]-a[1])
      .map(([k,v]) => `&nbsp;&nbsp;${{k}}: ${{v}}`).join('<br>');
    document.getElementById('info').innerHTML =
      `<strong>${{from.label}} → ${{to.label}}</strong><br>
       Total: ${{e.total}} dependencies<br>${{breakdownLines}}`;
  }}
}});
</script>
</body>
</html>"##,
        payload_json = payload_json,
        palette = COMMUNITY_PALETTE,
        kind_colors = KIND_COLORS,
    )
}

// quiet unused-import warning if DependencyKind is unused at link time
#[allow(dead_code)]
fn _ensure_kind_used() -> DependencyKind {
    DependencyKind::Calls
}
