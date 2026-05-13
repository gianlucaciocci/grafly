//! grafly-mcp — expose grafly as an MCP server over stdio.
//!
//! Tools: analyze, get_artifacts, get_modules, get_hotspots,
//!        get_couplings, get_insights, export,
//!        find_path, get_neighbors, get_dependents.

use grafly_core::{Confidence, DependencyKind, DependencyMap, MapBuilder};
use grafly_query::{PathOptions, SubgraphOptions, Traversal};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ServiceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Parameter types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct AnalyzeParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Leiden resolution — higher values produce more, smaller modules. Default: 1.0
    resolution: Option<f64>,
    /// Optional seed for deterministic module detection.
    seed: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PathParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Output directory for generated files.
    output: String,
    /// Comma-separated formats: json, html, md
    formats: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetArtifactsParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Filter by artifact kind: Package, File, Class, Struct, Function, Method, Interface, Trait, Enum, Namespace, Import.
    kind: Option<String>,
    /// Filter by module ID (0-indexed).
    module_id: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FindPathParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Source artifact — exact id (e.g. `src/foo.rs::class::Foo`) or unique label.
    from: String,
    /// Target artifact — exact id or unique label.
    to: String,
    /// When true (default), weight edges so Calls is preferred over Imports.
    weighted: Option<bool>,
    /// Maximum hop count (default 20).
    max_hops: Option<usize>,
    /// Exclude edges with confidence worse than this: "Extracted", "Inferred", or "Ambiguous" (default).
    min_confidence: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetNeighborsParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Center artifact — exact id or unique label.
    artifact: String,
    /// BFS depth (default 2).
    depth: Option<usize>,
    /// Include Imports/References edges. Default false to avoid file-level co-occurrence noise.
    include_imports: Option<bool>,
    /// Skip expansion through nodes with degree above this (default 200, supernode mitigation).
    max_degree: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetDependentsParams {
    /// Absolute or relative path to the directory to scan.
    path: String,
    /// Target artifact — exact id or unique label.
    artifact: String,
    /// BFS depth (default 2).
    depth: Option<usize>,
}

// ── Return types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnalyzeSummary {
    artifacts: usize,
    dependencies: usize,
    modules: usize,
    quality: f64,
    hotspots: Vec<HotspotSummary>,
    insights: Vec<String>,
}

#[derive(Serialize)]
struct HotspotSummary {
    label: String,
    degree: usize,
    source_file: String,
}

#[derive(Serialize)]
struct ArtifactSummary {
    id: String,
    label: String,
    kind: String,
    source_file: String,
    source_line: usize,
    module_id: Option<usize>,
}

#[derive(Serialize)]
struct ModuleSummary {
    id: usize,
    name: String,
    size: usize,
    representative_artifacts: Vec<String>,
}

#[derive(Serialize)]
struct CouplingSummary {
    from: String,
    to: String,
    kind: String,
    from_module: usize,
    to_module: usize,
    source_file: String,
    source_line: usize,
}

// ── Pipeline helper ───────────────────────────────────────────────────────────

struct PipelineResult {
    map: DependencyMap,
    modules: grafly_cluster::Modules,
    analysis: grafly_analyze::Analysis,
}

fn run_pipeline(path: PathBuf, resolution: f64, seed: Option<u64>) -> Result<PipelineResult, String> {
    let scan = grafly_scan::scan_dir(&path).map_err(|e| e.to_string())?;

    let mut builder = MapBuilder::new();
    builder.add_scan(scan);
    let mut map = builder.build();

    let modules = grafly_cluster::detect_modules(&mut map, resolution, seed)
        .map_err(|e| e.to_string())?;

    let analysis = grafly_analyze::analyze(&map);

    Ok(PipelineResult {
        map,
        modules,
        analysis,
    })
}

fn json_err(msg: impl std::fmt::Display) -> String {
    format!("{{\"error\": \"{}\"}}", msg)
}

// ── MCP Server ────────────────────────────────────────────────────────────────

struct GraflyServer;

#[tool_router(server_handler)]
impl GraflyServer {
    /// Run the full grafly pipeline on a directory.
    /// Returns artifact/dependency/module counts, quality score, hotspots,
    /// and insights about the codebase architecture.
    #[tool(description = "Run the full grafly pipeline on a directory. Returns a JSON summary \
        with artifact/dependency/module counts, Leiden quality score, hotspots \
        (high-centrality artifacts), and insights about the codebase architecture.")]
    fn analyze(&self, Parameters(p): Parameters<AnalyzeParams>) -> String {
        let path = PathBuf::from(&p.path);
        let resolution = p.resolution.unwrap_or(1.0);

        let result = tokio::task::block_in_place(|| run_pipeline(path, resolution, p.seed));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let num_modules = r
                    .map
                    .node_indices()
                    .filter_map(|n| r.map[n].module_id)
                    .max()
                    .map(|m| m + 1)
                    .unwrap_or(0);

                let summary = AnalyzeSummary {
                    artifacts: r.map.node_count(),
                    dependencies: r.map.edge_count(),
                    modules: num_modules,
                    quality: r.modules.quality,
                    hotspots: r
                        .analysis
                        .hotspots
                        .iter()
                        .map(|h| HotspotSummary {
                            label: h.label.clone(),
                            degree: h.degree,
                            source_file: h.source_file.clone(),
                        })
                        .collect(),
                    insights: r.analysis.insights.clone(),
                };
                serde_json::to_string_pretty(&summary).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Return artifacts, optionally filtered by kind or module.
    #[tool(description = "List artifacts in the dependency map, optionally filtered by kind \
        (Package, File, Class, Struct, Enum, Interface, Trait, Function, Method, Namespace, Import) \
        or by module ID. Useful for exploring what lives in a specific module or finding \
        all classes/functions in the codebase.")]
    fn get_artifacts(&self, Parameters(p): Parameters<GetArtifactsParams>) -> String {
        let path = PathBuf::from(&p.path);

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let artifacts: Vec<ArtifactSummary> = r
                    .map
                    .node_indices()
                    .filter_map(|n| {
                        let a = &r.map[n];
                        let kind_str = format!("{:?}", a.kind);
                        if let Some(ref k) = p.kind {
                            if !kind_str.eq_ignore_ascii_case(k) {
                                return None;
                            }
                        }
                        if let Some(m) = p.module_id {
                            if a.module_id != Some(m) {
                                return None;
                            }
                        }
                        Some(ArtifactSummary {
                            id: a.id.clone(),
                            label: a.label.clone(),
                            kind: kind_str,
                            source_file: a.source_file.clone(),
                            source_line: a.source_line,
                            module_id: a.module_id,
                        })
                    })
                    .collect();

                serde_json::to_string_pretty(&artifacts).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Return module breakdown with sizes and representative artifact labels.
    #[tool(description = "Return the module breakdown detected by Leiden clustering. \
        Each module entry includes its ID, size (artifact count), and a sample of \
        representative artifact labels. Use this to understand the high-level structure \
        of a codebase.")]
    fn get_modules(&self, Parameters(p): Parameters<PathParams>) -> String {
        let path = PathBuf::from(&p.path);

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let num_modules = r
                    .map
                    .node_indices()
                    .filter_map(|n| r.map[n].module_id)
                    .max()
                    .map(|m| m + 1)
                    .unwrap_or(0);

                let mut modules: Vec<ModuleSummary> = (0..num_modules)
                    .map(|id| {
                        let reps: Vec<String> = r
                            .map
                            .node_indices()
                            .filter(|&n| r.map[n].module_id == Some(id))
                            .take(5)
                            .map(|n| r.map[n].label.clone())
                            .collect();
                        ModuleSummary {
                            id,
                            name: r.modules.name_of(id).to_string(),
                            size: r
                                .map
                                .node_indices()
                                .filter(|&n| r.map[n].module_id == Some(id))
                                .count(),
                            representative_artifacts: reps,
                        }
                    })
                    .collect();

                modules.sort_by(|a, b| b.size.cmp(&a.size));
                serde_json::to_string_pretty(&modules).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Return hotspots — high-centrality artifacts that may be bottlenecks.
    #[tool(description = "Return hotspots — high-centrality artifacts whose degree is more \
        than 2 standard deviations above the mean. These are likely architectural \
        bottlenecks or utility modules that many other components depend on.")]
    fn get_hotspots(&self, Parameters(p): Parameters<PathParams>) -> String {
        let path = PathBuf::from(&p.path);

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let hotspots: Vec<HotspotSummary> = r
                    .analysis
                    .hotspots
                    .iter()
                    .map(|h| HotspotSummary {
                        label: h.label.clone(),
                        degree: h.degree,
                        source_file: h.source_file.clone(),
                    })
                    .collect();
                serde_json::to_string_pretty(&hotspots).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Return cross-module couplings.
    #[tool(description = "Return cross-module couplings — dependencies between artifacts in \
        different modules. These highlight unexpected coupling between modules and are \
        good candidates for architectural review.")]
    fn get_couplings(&self, Parameters(p): Parameters<PathParams>) -> String {
        let path = PathBuf::from(&p.path);

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let couplings: Vec<CouplingSummary> = r
                    .analysis
                    .couplings
                    .iter()
                    .map(|c| CouplingSummary {
                        from: c.from_label.clone(),
                        to: c.to_label.clone(),
                        kind: c.kind.clone(),
                        from_module: c.from_module,
                        to_module: c.to_module,
                        source_file: c.source_file.clone(),
                        source_line: c.source_line,
                    })
                    .collect();
                serde_json::to_string_pretty(&couplings).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Return suggested insights about the codebase.
    #[tool(description = "Return a list of insights about the codebase, generated from \
        the dependency map structure. Use these as starting points for architectural \
        review or onboarding conversations.")]
    fn get_insights(&self, Parameters(p): Parameters<PathParams>) -> String {
        let path = PathBuf::from(&p.path);

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => serde_json::to_string_pretty(&r.analysis.insights)
                .unwrap_or_else(|e| json_err(e)),
        }
    }

    /// Export the dependency map to files (JSON, HTML, Markdown).
    #[tool(description = "Export the dependency map to one or more file formats. \
        Supported formats (comma-separated): json, html, html-modules, html-packages, md. \
        json — raw map data; html — artifact-level interactive graph (top-N by degree); \
        html-modules — module-level overview (modules as nodes, aggregated cross-module \
        edges grouped by relationship kind); html-packages — package-level overview \
        (Cargo/pyproject/package.json/go.mod packages as nodes, cross-package edges); \
        md — Markdown report with packages, modules, hotspots, and insights. \
        Returns the list of written file paths.")]
    fn export(&self, Parameters(p): Parameters<ExportParams>) -> String {
        let path = PathBuf::from(&p.path);
        let output = PathBuf::from(&p.output);
        let formats: Vec<&str> = p
            .formats
            .as_deref()
            .unwrap_or("json,html,md")
            .split(',')
            .map(str::trim)
            .collect();

        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                if let Err(e) = std::fs::create_dir_all(&output) {
                    return json_err(e);
                }

                let analysis = r.analysis;
                let map = r.map;
                let modules = r.modules;
                let html_opts = grafly_export::HtmlOptions {
                    max_nodes: Some(800),
                    module_names: modules.names.clone(),
                    include_ambiguous: false,
                };
                let module_html_opts = grafly_export::ModuleHtmlOptions {
                    max_modules: Some(100),
                    module_names: modules.names.clone(),
                };
                let mut written: Vec<String> = Vec::new();

                // Always emit README.md alongside any other format
                let readme_path = output.join("README.md");
                if let Err(e) = std::fs::write(&readme_path, grafly_report::generate_output_readme()) {
                    return json_err(e);
                }
                written.push(readme_path.to_string_lossy().to_string());

                for fmt in formats {
                    let result: Result<(), String> = match fmt {
                        "json" => {
                            let p = output.join("grafly_knowledge.json");
                            grafly_export::write_json(&map, &p)
                                .map_err(|e| e.to_string())
                                .map(|_| written.push(p.to_string_lossy().to_string()))
                        }
                        "html" => {
                            let p = output.join("grafly_artifacts.html");
                            grafly_export::write_html(&map, &html_opts, &p)
                                .map_err(|e| e.to_string())
                                .map(|_| written.push(p.to_string_lossy().to_string()))
                        }
                        "html-modules" => {
                            let p = output.join("grafly_modules.html");
                            grafly_export::write_html_modules(&map, &module_html_opts, &p)
                                .map_err(|e| e.to_string())
                                .map(|_| written.push(p.to_string_lossy().to_string()))
                        }
                        "html-packages" => {
                            let p = output.join("grafly_packages.html");
                            let pkg_opts = grafly_export::PackageHtmlOptions::default();
                            grafly_export::write_html_packages(&map, &pkg_opts, &p)
                                .map_err(|e| e.to_string())
                                .map(|_| written.push(p.to_string_lossy().to_string()))
                        }
                        "md" => {
                            let p = output.join("grafly_report.md");
                            let md = grafly_report::generate_markdown(&map, &modules, &analysis, None);
                            std::fs::write(&p, md)
                                .map_err(|e| e.to_string())
                                .map(|_| written.push(p.to_string_lossy().to_string()))
                        }
                        other => Err(format!("unknown format: {}", other)),
                    };
                    if let Err(e) = result {
                        return json_err(e);
                    }
                }

                serde_json::to_string_pretty(&written).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Find the shortest path between two artifacts.
    #[tool(description = "Find the shortest path between two artifacts in the dependency map. \
        By default uses weighted edges so the path prefers high-confidence call chains \
        (Calls=1) over file-level co-occurrence (Imports=5, References/Uses=10) — \
        important for message-bus architectures where the call chain reveals \
        the mediation that imports don't. Returns a JSON object with the full path, \
        per-hop kind/confidence/source_line, and total weight. Pass `weighted=false` \
        for raw shortest path by hop count. Pass `min_confidence` to exclude \
        Inferred/Ambiguous edges entirely.")]
    fn find_path(&self, Parameters(p): Parameters<FindPathParams>) -> String {
        let path = PathBuf::from(&p.path);
        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let from = match grafly_query::resolve(&r.map, &p.from) {
                    Ok(n) => n,
                    Err(e) => return json_err(e),
                };
                let to = match grafly_query::resolve(&r.map, &p.to) {
                    Ok(n) => n,
                    Err(e) => return json_err(e),
                };
                let min_conf = match p.min_confidence.as_deref() {
                    Some("Extracted") | Some("extracted") => Confidence::Extracted,
                    Some("Inferred") | Some("inferred") => Confidence::Inferred,
                    Some("Ambiguous") | Some("ambiguous") | None => Confidence::Ambiguous,
                    Some(other) => return json_err(format!("unknown min_confidence: {}", other)),
                };
                let opts = PathOptions {
                    weighted: p.weighted.unwrap_or(true),
                    max_hops: Some(p.max_hops.unwrap_or(20)),
                    allowed_kinds: None,
                    min_confidence: min_conf,
                };
                match grafly_query::find_path(&r.map, from, to, &opts) {
                    Some(path) => serde_json::to_string_pretty(&path).unwrap_or_else(|e| json_err(e)),
                    None => "null".to_string(),
                }
            }
        }
    }

    /// Get the BFS neighborhood of an artifact.
    #[tool(description = "Return a depth-limited subgraph centered on an artifact. \
        By default only follows runtime-meaningful edges (Calls/Extends/Implements/Contains) \
        and skips expansion through supernodes (degree > 200) to avoid pulling in \
        thousands of unrelated nodes via common types like `from_str` or `UUID4`. \
        Set `include_imports=true` to also follow Imports/References edges. \
        Returns artifacts, dependencies, and a list of supernodes that were treated \
        as boundary nodes.")]
    fn get_neighbors(&self, Parameters(p): Parameters<GetNeighborsParams>) -> String {
        let path = PathBuf::from(&p.path);
        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let center = match grafly_query::resolve(&r.map, &p.artifact) {
                    Ok(n) => n,
                    Err(e) => return json_err(e),
                };
                let allowed_kinds = if p.include_imports.unwrap_or(false) {
                    None
                } else {
                    Some(vec![
                        DependencyKind::Calls,
                        DependencyKind::Extends,
                        DependencyKind::Implements,
                        DependencyKind::Contains,
                    ])
                };
                let opts = SubgraphOptions {
                    depth: p.depth.unwrap_or(2),
                    direction: Traversal::Outgoing,
                    allowed_kinds,
                    max_degree: Some(p.max_degree.unwrap_or(200)),
                };
                let sub = grafly_query::neighbors(&r.map, center, &opts);
                serde_json::to_string_pretty(&sub).unwrap_or_else(|e| json_err(e))
            }
        }
    }

    /// Get the artifacts that depend on the target (incoming subgraph).
    #[tool(description = "Return the artifacts that depend on a target artifact — \
        the incoming-direction BFS subgraph. Answers the question \"who uses this?\". \
        Same defaults as get_neighbors (runtime edges only, supernode cap at degree 200).")]
    fn get_dependents(&self, Parameters(p): Parameters<GetDependentsParams>) -> String {
        let path = PathBuf::from(&p.path);
        let result = tokio::task::block_in_place(|| run_pipeline(path, 1.0, None));

        match result {
            Err(e) => json_err(e),
            Ok(r) => {
                let target = match grafly_query::resolve(&r.map, &p.artifact) {
                    Ok(n) => n,
                    Err(e) => return json_err(e),
                };
                let sub = grafly_query::ancestors(&r.map, target, p.depth.unwrap_or(2));
                serde_json::to_string_pretty(&sub).unwrap_or_else(|e| json_err(e))
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = GraflyServer.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
