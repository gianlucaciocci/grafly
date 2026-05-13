mod install;
mod install_mcp;
mod skill;
mod target;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use install::Scope;
use std::path::PathBuf;
use std::time::Duration;
use target::Target;

fn spinner(prefix: &'static str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{prefix} {spinner} {elapsed} {msg}")
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
    );
    pb.set_prefix(prefix);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

fn bar(prefix: &'static str, total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{prefix} [{bar:30.cyan/blue}] {percent:>3}% ({human_pos}/{human_len}) {msg}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.set_prefix(prefix);
    pb
}

#[derive(Parser)]
#[command(
    name = "grafly",
    version,
    about = "Map · cluster · analyze codebases as dependency graphs.",
    long_about = None,
    subcommand_required = true,
    arg_required_else_help = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze a codebase and emit the dependency map (default).
    Analyze(AnalyzeArgs),
    /// Wire grafly into an LLM tool: rules file + MCP server + (for Claude
    /// Code) the `/grafly-*` slash commands. All-or-nothing per target — the
    /// user doesn't get to install just one piece.
    Install(InstallArgs),
    /// Remove every grafly artifact (rules block, MCP entry, Claude Code
    /// skills) from the target's config files.
    Uninstall(UninstallArgs),
    /// Show which targets currently have grafly installed, and where.
    List(ListArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Scope to inspect (project vs user-global). Targets that only support
    /// one scope are reported under that scope regardless.
    #[arg(short, long, value_enum, default_value_t = Scope::Project)]
    scope: Scope,

    /// Project root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Args)]
struct UninstallArgs {
    /// Target LLM tool. Repeatable. Omit to use the default (`claude`).
    #[arg(short, long, value_enum)]
    platform: Vec<Target>,

    /// Uninstall from every supported target.
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Project- or user-global config. Targets that only support one scope
    /// silently fall back to the supported one.
    #[arg(short, long, value_enum, default_value_t = Scope::Project)]
    scope: Scope,

    /// Project root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Args)]
struct AnalyzeArgs {
    /// Directory to scan (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Leiden resolution — higher values produce more, smaller modules
    #[arg(short, long, default_value_t = 1.0)]
    resolution: f64,

    /// Random seed for deterministic module detection
    #[arg(short, long)]
    seed: Option<u64>,

    /// Comma-separated output formats: json, html, html-modules, html-packages, md
    #[arg(short, long, default_value = "json,html,html-modules,html-packages,md")]
    formats: String,

    /// Max artifacts in the artifact-level HTML (0 = unlimited).
    #[arg(long, default_value_t = 800)]
    max_html_nodes: usize,

    /// Max modules in the module-level HTML (0 = unlimited).
    #[arg(long, default_value_t = 100)]
    max_html_modules: usize,

    /// Include `Ambiguous`-confidence edges in the artifact HTML.
    /// They're always kept in `grafly_knowledge.json` regardless.
    #[arg(long, default_value_t = false)]
    html_include_ambiguous: bool,

    /// Disable all path filtering — scan every file, including hidden dirs,
    /// `.gitignore`d paths, `node_modules`, `target`, `.venv`, etc.
    #[arg(long, default_value_t = false)]
    no_ignore: bool,

    /// Use leiden-rs's stock high-quality defaults (max_iter=100, epsilon=1e-10)
    /// instead of grafly's fast defaults (max_iter=20, epsilon=1e-4). Adds
    /// several minutes on large codebases for a sub-percent quality gain.
    #[arg(long, default_value_t = false)]
    leiden_thorough: bool,

    /// Keep `Imports` edges in the dependency map. By default they're filtered
    /// as architectural noise — file-level co-occurrence creates misleading
    /// path shortcuts and bloats hotspot degrees without revealing structure.
    #[arg(long, default_value_t = false)]
    include_imports: bool,

    /// Keep test and example files in the scan. By default `tests/`, `test/`,
    /// `__tests__/`, `examples/`, `benches/` directories and per-language test
    /// filename patterns (`test_*.py`, `*_test.go`, `*.test.ts`, `*Test.java`)
    /// are excluded — they're not part of the runtime architecture and pollute
    /// module/hotspot detection.
    #[arg(long, default_value_t = false)]
    include_tests: bool,

    /// Skip the intra-package Leiden pass. By default grafly clusters within
    /// each `Package` separately (in addition to the global cross-package
    /// modules), surfacing fine-grained subsystems inside each crate/package.
    #[arg(long, default_value_t = false)]
    no_intra_package_modules: bool,

    /// Include private/internal symbols in the artifact HTML, hotspots, and
    /// cross-module couplings. By default `Visibility::Private` artifacts are
    /// filtered out so the architecture view stays focused on the public
    /// surface area. They're always kept in `grafly_knowledge.json` regardless.
    #[arg(long, default_value_t = false)]
    include_private: bool,
}

#[derive(Args)]
struct InstallArgs {
    /// Target LLM tool. Repeatable. Omit to use the default (`claude`).
    /// Each target installs every surface it supports — rules file, MCP
    /// server registry, and (for Claude Code) the `/grafly-*` skills.
    #[arg(short, long, value_enum)]
    platform: Vec<Target>,

    /// Install on every supported target.
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Where to write — `project` (current directory) or `global` (user home).
    /// Targets that only support one scope (e.g. Claude Desktop is global;
    /// VS Code is project) silently fall back to the supported one.
    #[arg(short, long, value_enum, default_value_t = Scope::Project)]
    scope: Scope,

    /// Project root for project-scope installs.
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Path to the `grafly-mcp` binary the MCP-aware targets should launch.
    /// Defaults to the bare name `grafly-mcp` when on PATH, otherwise the
    /// sibling of the current `grafly` executable.
    #[arg(long)]
    bin: Option<PathBuf>,
}

fn main() -> Result<()> {
    // Compat shim: `grafly <path>` → `grafly analyze <path>`. If the first
    // positional argument is not a known subcommand or help/version flag,
    // we inject `analyze` so the historical UX still works.
    let mut argv: Vec<String> = std::env::args().collect();
    if argv.len() > 1 {
        let first = argv[1].as_str();
        let known = matches!(
            first,
            "analyze"
                | "install"
                | "uninstall"
                | "list"
                | "help"
                | "-h"
                | "--help"
                | "-V"
                | "--version"
        );
        if !known {
            argv.insert(1, "analyze".to_string());
        }
    }
    let cli = Cli::parse_from(argv);

    match cli.command {
        Command::Analyze(args) => run_analyze(args),
        Command::Install(args) => run_install(args),
        Command::Uninstall(args) => run_uninstall(args),
        Command::List(args) => run_list(args),
    }
}

// ── Install / Uninstall / List ───────────────────────────────────────────────

fn resolve_targets_install(args: &InstallArgs) -> Vec<Target> {
    if args.all {
        Target::all().to_vec()
    } else if args.platform.is_empty() {
        vec![Target::Claude]
    } else {
        args.platform.clone()
    }
}

fn resolve_targets_uninstall(args: &UninstallArgs) -> Vec<Target> {
    if args.all {
        Target::all().to_vec()
    } else if args.platform.is_empty() {
        vec![Target::Claude]
    } else {
        args.platform.clone()
    }
}

fn run_install(args: InstallArgs) -> Result<()> {
    let targets = resolve_targets_install(&args);
    let bin = args
        .bin
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(install_mcp::default_mcp_bin);

    println!("grafly install");
    println!("  binary : {}", bin);

    let mut installed_claude = false;
    for target in targets {
        if matches!(target, Target::Claude) {
            installed_claude = true;
        }
        let outcome = target::install_target(target, args.scope, &args.root, &bin)?;
        print_install_outcome(&outcome);
    }

    println!(
        "\nAny LLM agent reading these configs will now consult `{}` first when answering codebase questions.",
        install::OUTPUT_DIR
    );
    if installed_claude {
        println!(
            "In Claude Code, type `/grafly-ask` for any architectural question or \
             `/grafly-suggest-questions` to bootstrap a project-specific question list."
        );
    }
    println!("Run `grafly analyze .` to produce/refresh the analysis.");
    Ok(())
}

fn run_uninstall(args: UninstallArgs) -> Result<()> {
    let targets = resolve_targets_uninstall(&args);
    println!("grafly uninstall");
    for target in targets {
        let outcome = target::uninstall_target(target, args.scope, &args.root)?;
        print_uninstall_outcome(&outcome);
    }
    Ok(())
}

fn run_list(args: ListArgs) -> Result<()> {
    println!("grafly list");
    for row in target::list_targets(args.scope, &args.root)? {
        let rules = row
            .rules_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        let mcp = row
            .mcp_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "  {:<48} rules: {}  |  mcp: {}",
            row.target.display_name(),
            rules,
            mcp
        );
    }
    Ok(())
}

fn print_install_outcome(o: &target::TargetOutcome) {
    println!("  {}:", o.target.display_name());
    if let Some(r) = &o.rules {
        println!("    [{:>9}] rules    {}", r.action, r.path.display());
    }
    if let Some(m) = &o.mcp {
        println!("    [{:>9}] mcp      {}", m.action, m.path.display());
    }
    for s in &o.skills {
        println!("    [{:>9}] {:<19} {}", s.action, s.label, s.path.display());
    }
}

fn print_uninstall_outcome(o: &target::TargetUninstallOutcome) {
    println!("  {}:", o.target.display_name());
    if let Some(r) = &o.rules {
        println!("    [{:>9}] rules    {}", r.action, r.path.display());
    }
    if let Some(m) = &o.mcp {
        println!("    [{:>9}] mcp      {}", m.action, m.path.display());
    }
    for s in &o.skills {
        println!("    [{:>9}] {:<19} {}", s.action, s.label, s.path.display());
    }
}

// ── Analyze ──────────────────────────────────────────────────────────────────

fn run_analyze(cli: AnalyzeArgs) -> Result<()> {
    let output_dir = PathBuf::from(install::OUTPUT_DIR);
    println!("grafly {}", env!("CARGO_PKG_VERSION"));
    println!("  target : {}", cli.path.display());
    println!("  output : {}", output_dir.display());

    // ── 1. Scan ───────────────────────────────────────────────────────────────
    println!();
    let pb = spinner("[1/4] scanning");
    let scan_opts = if cli.no_ignore {
        grafly_scan::ScanOptions::unrestricted()
    } else {
        grafly_scan::ScanOptions {
            skip_tests_and_examples: !cli.include_tests,
            ..grafly_scan::ScanOptions::default()
        }
    };
    let scan = grafly_scan::scan_dir_with_options(&cli.path, &scan_opts)
        .with_context(|| format!("failed to scan {}", cli.path.display()))?;
    pb.finish_and_clear();
    println!(
        "[1/4] scanning ... {} artifacts, {} dependencies, {} unresolved refs",
        scan.artifacts.len(),
        scan.dependencies.len(),
        scan.unresolved.len()
    );

    // ── 2. Build map ──────────────────────────────────────────────────────────
    let pb = spinner("[2/4] merging scan");
    let mut builder = grafly_core::MapBuilder::new();
    builder.add_scan(scan);
    let total_unresolved = builder.unresolved_len();
    pb.finish_and_clear();

    let pb = bar("[2/4] resolving refs", total_unresolved as u64);
    let (mut map, stats) = builder.build_with_progress(|done, _total| {
        pb.set_position(done as u64);
    });
    pb.finish_and_clear();
    println!(
        "[2/4] building dependency map ... {} artifacts, {} dependencies ({} unique, {} ambiguous, {} unresolved)",
        map.node_count(),
        map.edge_count(),
        stats.resolved_unique,
        stats.resolved_ambiguous,
        stats.unresolved,
    );

    if map.node_count() == 0 {
        println!("  no source files found — nothing to do.");
        return Ok(());
    }

    // ── 3. Detect modules ─────────────────────────────────────────────────────
    let pb = spinner("[3/4] detecting modules");
    let detection_config = if cli.leiden_thorough {
        grafly_cluster::DetectionConfig::thorough()
    } else {
        grafly_cluster::DetectionConfig::default()
    };
    let mode = if cli.leiden_thorough {
        "thorough"
    } else {
        "fast"
    };
    pb.set_message(format!(
        "(leiden, resolution={}, mode={})",
        cli.resolution, mode
    ));
    let modules = grafly_cluster::detect_modules_with_config(
        &mut map,
        cli.resolution,
        cli.seed,
        &detection_config,
    )
    .context("module detection failed")?;
    pb.finish_and_clear();
    println!(
        "[3/4] detecting modules (leiden, resolution={}, mode={}) ... {} modules (quality = {:.4})",
        cli.resolution,
        mode,
        modules.count(),
        modules.quality
    );

    // ── 3b. Intra-package Leiden ─────────────────────────────────────────────
    // Cluster within each Package's subgraph to surface fine-grained subsystems
    // inside each crate/package. Additive to the global modules above — doesn't
    // mutate Artifact.module_id, just produces a separate per-package partition.
    let intra_package = if cli.no_intra_package_modules {
        None
    } else {
        let pb = spinner("[3b/4] intra-package modules");
        let result =
            grafly_cluster::detect_modules_within_packages(&map, &detection_config, cli.seed)
                .context("intra-package module detection failed")?;
        pb.finish_and_clear();
        let clustered = result.values().filter(|m| !m.members.is_empty()).count();
        let total_intra: usize = result.values().map(|m| m.count()).sum();
        println!(
            "[3b/4] intra-package modules ... {} packages clustered, {} intra-modules total",
            clustered, total_intra
        );
        Some(result)
    };

    // ── 3c. Drop Imports edges (post-cluster) ─────────────────────────────────
    // Imports edges are essential signal for Leiden (they encode file-level
    // co-occurrence — the densest connections in a typical codebase). But for
    // downstream surfaces — hotspots, HTML, path queries — they're noise:
    // they inflate degree counts and create misleading `A → shared_file → B`
    // shortcuts. So we cluster *with* them, then drop them before output.
    if !cli.include_imports {
        let before = map.edge_count();
        map.retain_edges(|frozen, e| {
            frozen
                .edge_weight(e)
                .map(|d| d.kind != grafly_core::DependencyKind::Imports)
                .unwrap_or(false)
        });
        let dropped = before - map.edge_count();
        if dropped > 0 {
            println!(
                "       dropped {} Imports edges (use --include-imports to keep)",
                dropped
            );
        }
    }

    // ── 4. Analyze ────────────────────────────────────────────────────────────
    let pb = spinner("[4/4] analyzing");
    let analysis = grafly_analyze::analyze_with_options(
        &map,
        grafly_analyze::AnalysisOptions {
            include_private: cli.include_private,
        },
    );
    pb.finish_and_clear();
    println!(
        "[4/4] analyzing ... {} hotspots, {} cross-module couplings",
        analysis.hotspots.len(),
        analysis.couplings.len()
    );

    // ── Output ────────────────────────────────────────────────────────────────
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("cannot create output dir {}", output_dir.display()))?;

    let html_opts = grafly_export::HtmlOptions {
        max_nodes: if cli.max_html_nodes == 0 {
            None
        } else {
            Some(cli.max_html_nodes)
        },
        module_names: modules.names.clone(),
        include_ambiguous: cli.html_include_ambiguous,
        include_private: cli.include_private,
    };
    let module_html_opts = grafly_export::ModuleHtmlOptions {
        max_modules: if cli.max_html_modules == 0 {
            None
        } else {
            Some(cli.max_html_modules)
        },
        module_names: modules.names.clone(),
    };
    let package_html_opts = grafly_export::PackageHtmlOptions {
        max_packages: None, // packages are typically O(10-50), no cap needed
        intra_module_counts: intra_package
            .as_ref()
            .map(|m| m.iter().map(|(idx, mods)| (*idx, mods.count())).collect())
            .unwrap_or_default(),
    };

    println!();
    let mut report_path: Option<PathBuf> = None;
    for fmt in cli.formats.split(',').map(str::trim) {
        match fmt {
            "json" => {
                let p = output_dir.join("grafly_knowledge.json");
                grafly_export::write_json(&map, &p)?;
                println!("  wrote {}", p.display());
            }
            "html" => {
                let p = output_dir.join("grafly_artifacts.html");
                grafly_export::write_html(&map, &html_opts, &p)?;
                let note = if cli.max_html_nodes > 0 && map.node_count() > cli.max_html_nodes {
                    format!(
                        " (showing top {} of {} artifacts)",
                        cli.max_html_nodes,
                        map.node_count()
                    )
                } else {
                    String::new()
                };
                println!("  wrote {}{}", p.display(), note);
            }
            "html-modules" => {
                let p = output_dir.join("grafly_modules.html");
                grafly_export::write_html_modules(&map, &module_html_opts, &p)?;
                let note = if cli.max_html_modules > 0 && modules.count() > cli.max_html_modules {
                    format!(
                        " (showing top {} of {} modules)",
                        cli.max_html_modules,
                        modules.count()
                    )
                } else {
                    String::new()
                };
                println!("  wrote {}{}", p.display(), note);
            }
            "html-packages" => {
                let p = output_dir.join("grafly_packages.html");
                grafly_export::write_html_packages(&map, &package_html_opts, &p)?;
                println!("  wrote {}", p.display());
            }
            "md" => {
                let p = output_dir.join("grafly_report.md");
                let md = grafly_report::generate_markdown(
                    &map,
                    &modules,
                    &analysis,
                    intra_package.as_ref(),
                );
                std::fs::write(&p, md)?;
                println!("  wrote {}", p.display());
                report_path = Some(p);
            }
            other => eprintln!("  unknown format '{}' — skipping", other),
        }
    }

    let readme_path = output_dir.join("README.md");
    std::fs::write(&readme_path, grafly_report::generate_output_readme())?;
    println!("  wrote {}", readme_path.display());

    let questions_path = output_dir.join("SUGGESTED_QUESTIONS.md");
    std::fs::write(
        &questions_path,
        grafly_report::generate_suggested_questions(),
    )?;
    println!("  wrote {}", questions_path.display());

    println!("\ndone.");

    if report_path.is_some() {
        println!("\nNext up.");
        println!("======================================================");
        println!("Run `grafly install` and start asking questions to your LLM with:");
        println!("  - /grafly-ask              (Claude Code)");
        println!("  - /grafly-suggest-questions (Claude Code)");
        println!(
            "  - other LLMs: just ask architecture questions — the installed\n\
             \x20   rules file routes them to ./grafly-out automatically."
        );
        println!("======================================================");
    }
    Ok(())
}
