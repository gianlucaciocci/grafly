mod install;
mod install_mcp;
mod skill;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use install::{Platform, Scope};
use install_mcp::McpClient;
use std::path::PathBuf;

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
    /// Install grafly's instructions into an LLM tool's config so any agent
    /// working in this project uses `./grafly-out/` as the source of truth.
    Install(InstallArgs),
    /// Remove grafly's instructions from an LLM tool's config.
    Uninstall(InstallArgs),
    /// Register / unregister the `grafly-mcp` MCP server in MCP clients.
    #[command(subcommand)]
    Mcp(McpCommand),
}

#[derive(Subcommand)]
enum McpCommand {
    /// Register `grafly-mcp` in an MCP client's config (so the client can call grafly's tools).
    Install(McpInstallArgs),
    /// Remove `grafly-mcp` from an MCP client's config.
    Uninstall(McpInstallArgs),
    /// Show which MCP clients have `grafly-mcp` registered.
    List(McpListArgs),
}

#[derive(Args)]
struct McpInstallArgs {
    /// Target MCP client. Repeatable. Omit to use the default (`claude-code`).
    #[arg(short, long, value_enum)]
    client: Vec<McpClient>,

    /// Install on every supported client.
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Project- or user-global config. Some clients only support one; we
    /// silently fall back to whichever is valid.
    #[arg(short, long, value_enum, default_value_t = Scope::Project)]
    scope: Scope,

    /// Project root for project-scope installs.
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Path to the `grafly-mcp` binary the client should launch.
    /// Defaults to a sibling of the current `grafly` executable, or `grafly-mcp`
    /// on PATH if no sibling is found.
    #[arg(long)]
    bin: Option<PathBuf>,
}

#[derive(Args)]
struct McpListArgs {
    /// Scope to inspect for project-aware clients.
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

    /// Output directory
    #[arg(short, long, default_value = "./grafly-out")]
    output: PathBuf,

    /// Leiden resolution — higher values produce more, smaller modules
    #[arg(short, long, default_value_t = 1.0)]
    resolution: f64,

    /// Random seed for deterministic module detection
    #[arg(short, long)]
    seed: Option<u64>,

    /// Comma-separated output formats: json, html, html-modules, md
    #[arg(short, long, default_value = "json,html,html-modules,md")]
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
}

#[derive(Args)]
struct InstallArgs {
    /// Target LLM tool. Repeatable. Omit to use the default (`claude`).
    #[arg(short, long, value_enum)]
    platform: Vec<Platform>,

    /// Install on every supported platform.
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Where to write — `project` (current directory) or `global` (user home).
    /// `global` only affects platforms that support it (`claude`, `agents`, `gemini`).
    #[arg(short, long, value_enum, default_value_t = Scope::Project)]
    scope: Scope,

    /// Project root for project-scope installs.
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Output directory referenced by the installed instructions.
    /// Should match the `--output` you pass to `grafly analyze`.
    #[arg(long, default_value = "./grafly-out")]
    output_dir: PathBuf,
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
            "analyze" | "install" | "uninstall" | "mcp" | "help"
                | "-h" | "--help" | "-V" | "--version"
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
        Command::Mcp(McpCommand::Install(args)) => run_mcp_install(args),
        Command::Mcp(McpCommand::Uninstall(args)) => run_mcp_uninstall(args),
        Command::Mcp(McpCommand::List(args)) => run_mcp_list(args),
    }
}

// ── Install / Uninstall ──────────────────────────────────────────────────────

fn resolve_platforms(args: &InstallArgs) -> Vec<Platform> {
    if args.all {
        Platform::all().to_vec()
    } else if args.platform.is_empty() {
        vec![Platform::Claude]
    } else {
        args.platform.clone()
    }
}

fn run_install(args: InstallArgs) -> Result<()> {
    let platforms = resolve_platforms(&args);
    let output_str = args.output_dir.to_string_lossy().replace('\\', "/");
    println!("grafly install");
    for platform in platforms {
        let outcome =
            install::install_platform(platform, args.scope, &args.root, &output_str)?;
        println!(
            "  [{:>9}] {:<48} {}",
            outcome.action,
            outcome.platform.display_name(),
            outcome.path.display()
        );
    }
    println!(
        "\nAny LLM agent reading these files will now look for `{}` first when answering codebase questions.",
        output_str
    );
    println!("Run `grafly analyze .` to produce/refresh the analysis.");
    Ok(())
}

fn run_uninstall(args: InstallArgs) -> Result<()> {
    let platforms = resolve_platforms(&args);
    println!("grafly uninstall");
    for platform in platforms {
        let outcome = install::uninstall_platform(platform, args.scope, &args.root)?;
        println!(
            "  [{:>9}] {:<48} {}",
            outcome.action,
            outcome.platform.display_name(),
            outcome.path.display()
        );
    }
    Ok(())
}

// ── MCP install / uninstall / list ───────────────────────────────────────────

fn resolve_mcp_clients(args: &McpInstallArgs) -> Vec<McpClient> {
    if args.all {
        McpClient::all().to_vec()
    } else if args.client.is_empty() {
        vec![McpClient::ClaudeCode]
    } else {
        args.client.clone()
    }
}

fn run_mcp_install(args: McpInstallArgs) -> Result<()> {
    let clients = resolve_mcp_clients(&args);
    let bin = args
        .bin
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(install_mcp::default_mcp_bin);

    let installs_claude_code = clients.iter().any(|c| *c == McpClient::ClaudeCode);

    println!("grafly mcp install");
    println!("  binary : {}", bin);
    for client in clients {
        let outcome = install_mcp::install_mcp(client, args.scope, &args.root, &bin)?;
        println!(
            "  [{:>9}] {:<28} {}",
            outcome.action,
            outcome.client.display_name(),
            outcome.path.display()
        );
    }

    // When Claude Code is among the targets, also install the `/grafly` skill
    // so the user has a slash command that routes to the MCP tools.
    if installs_claude_code {
        for o in skill::install_claude_skill()? {
            println!("  [{:>9}] {:<28} {}", o.action, o.label, o.path.display());
        }
    }

    println!(
        "\nAny MCP-aware client reading those configs can now call `grafly-mcp`'s tools \
         (analyze, get_artifacts, get_modules, get_hotspots, get_couplings, get_insights, export)."
    );
    if installs_claude_code {
        println!(
            "In Claude Code, type `/grafly` to invoke the skill — it routes the user's question \
             to the right MCP tool."
        );
    }
    Ok(())
}

fn run_mcp_uninstall(args: McpInstallArgs) -> Result<()> {
    let clients = resolve_mcp_clients(&args);
    let uninstalls_claude_code = clients.iter().any(|c| *c == McpClient::ClaudeCode);

    println!("grafly mcp uninstall");
    for client in clients {
        let outcome = install_mcp::uninstall_mcp(client, args.scope, &args.root)?;
        println!(
            "  [{:>9}] {:<28} {}",
            outcome.action,
            outcome.client.display_name(),
            outcome.path.display()
        );
    }
    if uninstalls_claude_code {
        for o in skill::uninstall_claude_skill()? {
            println!("  [{:>9}] {:<28} {}", o.action, o.label, o.path.display());
        }
    }
    Ok(())
}

fn run_mcp_list(args: McpListArgs) -> Result<()> {
    println!("grafly mcp list");
    for (client, path) in install_mcp::list_mcp(args.scope, &args.root)? {
        match path {
            Some(p) => println!("  [registered ] {:<22} {}", client.display_name(), p.display()),
            None => println!("  [    -      ] {:<22} —", client.display_name()),
        }
    }
    Ok(())
}

// ── Analyze ──────────────────────────────────────────────────────────────────

fn run_analyze(cli: AnalyzeArgs) -> Result<()> {
    println!("grafly {}", env!("CARGO_PKG_VERSION"));
    println!("  target : {}", cli.path.display());
    println!("  output : {}", cli.output.display());

    // ── 1. Scan ───────────────────────────────────────────────────────────────
    print!("\n[1/4] scanning ...");
    let scan_opts = if cli.no_ignore {
        grafly_scan::ScanOptions::unrestricted()
    } else {
        grafly_scan::ScanOptions::default()
    };
    let scan = grafly_scan::scan_dir_with_options(&cli.path, &scan_opts)
        .with_context(|| format!("failed to scan {}", cli.path.display()))?;
    println!(
        " {} artifacts, {} dependencies, {} unresolved refs",
        scan.artifacts.len(),
        scan.dependencies.len(),
        scan.unresolved.len()
    );

    // ── 2. Build map ──────────────────────────────────────────────────────────
    print!("[2/4] building dependency map ...");
    let mut builder = grafly_core::MapBuilder::new();
    builder.add_scan(scan);
    let (mut map, stats) = builder.build_with_stats();
    println!(
        " {} artifacts, {} dependencies ({} unique, {} ambiguous, {} unresolved)",
        map.node_count(),
        map.edge_count(),
        stats.resolved_unique,
        stats.resolved_ambiguous,
        stats.unresolved
    );

    if map.node_count() == 0 {
        println!("  no source files found — nothing to do.");
        return Ok(());
    }

    // ── 3. Detect modules ─────────────────────────────────────────────────────
    print!(
        "[3/4] detecting modules (leiden, resolution={}) ...",
        cli.resolution
    );
    let modules = grafly_cluster::detect_modules(&mut map, cli.resolution, cli.seed)
        .context("module detection failed")?;
    println!(
        " {} modules (quality = {:.4})",
        modules.count(),
        modules.quality
    );

    // ── 4. Analyze ────────────────────────────────────────────────────────────
    print!("[4/4] analyzing ...");
    let analysis = grafly_analyze::analyze(&map);
    println!(
        " {} hotspots, {} cross-module couplings",
        analysis.hotspots.len(),
        analysis.couplings.len()
    );

    // ── Output ────────────────────────────────────────────────────────────────
    std::fs::create_dir_all(&cli.output)
        .with_context(|| format!("cannot create output dir {}", cli.output.display()))?;

    let html_opts = grafly_export::HtmlOptions {
        max_nodes: if cli.max_html_nodes == 0 {
            None
        } else {
            Some(cli.max_html_nodes)
        },
        module_names: modules.names.clone(),
        include_ambiguous: cli.html_include_ambiguous,
    };
    let module_html_opts = grafly_export::ModuleHtmlOptions {
        max_modules: if cli.max_html_modules == 0 {
            None
        } else {
            Some(cli.max_html_modules)
        },
        module_names: modules.names.clone(),
    };

    println!();
    let mut report_path: Option<PathBuf> = None;
    for fmt in cli.formats.split(',').map(str::trim) {
        match fmt {
            "json" => {
                let p = cli.output.join("grafly_knowledge.json");
                grafly_export::write_json(&map, &p)?;
                println!("  wrote {}", p.display());
            }
            "html" => {
                let p = cli.output.join("grafly_artifacts.html");
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
                let p = cli.output.join("grafly_modules.html");
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
            "md" => {
                let p = cli.output.join("grafly_report.md");
                let md = grafly_report::generate_markdown(&map, &modules, &analysis);
                std::fs::write(&p, md)?;
                println!("  wrote {}", p.display());
                report_path = Some(p);
            }
            other => eprintln!("  unknown format '{}' — skipping", other),
        }
    }

    let readme_path = cli.output.join("README.md");
    std::fs::write(&readme_path, grafly_report::generate_output_readme())?;
    println!("  wrote {}", readme_path.display());

    println!("\ndone.");
    if report_path.is_some() {
        println!(
            "\nNext steps — make this analysis discoverable to LLM agents:\n\
             \n  1. Append grafly's rules to your LLM tool's instructions file:\n\
             \n         grafly install                  # default: Claude Code (./CLAUDE.md)\n\
             \n         grafly install --all            # every supported platform\n\
             \n  2. Register the MCP server so agents can call grafly's tools live:\n\
             \n         grafly mcp install              # default: Claude Code (./.mcp.json)\n\
             \n         grafly mcp install --all        # every supported MCP client\n"
        );
    }
    Ok(())
}
