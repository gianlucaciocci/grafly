# grafly

**Scan · Map · Detect modules · Analyze**

Grafly turns a codebase into a **dependency map** — a directed graph of artifacts (files, classes, functions) and their dependencies. It parses source files locally with [tree-sitter](https://tree-sitter.github.io/), detects cohesive modules with the [Leiden algorithm](https://www.nature.com/articles/s41598-019-41695-z), and emits an interactive HTML visualization, a JSON map, and a Markdown report — with no API calls required for code files.

Designed to work as a standalone CLI, a Rust library, and an **MCP server** that any LLM coding assistant can call as a tool.

---

## Ubiquitous Language

Grafly uses the architect's vocabulary — what software architects actually call things:

| Term | Meaning |
|---|---|
| **Artifact** | A unit of code (file, class, function, ...) |
| **Package** | A buildable unit declared in a manifest (`Cargo.toml`, `pyproject.toml`, `package.json`, `go.mod`). Sits above File in the containment hierarchy |
| **Dependency** | A directed relationship between artifacts |
| **Dependency Map** | The full graph of artifacts and dependencies |
| **Module** | A cohesive cluster of artifacts (detected by Leiden) |
| **Hotspot** | An artifact with disproportionately high centrality |
| **Coupling** | A dependency that crosses module boundaries |
| **Visibility** | Source-level access of a declared symbol — `Public`, `Crate`, `Private`, `Unknown`. Drives the public-surface filter on hotspots, couplings, and the artifact HTML |
| **Scan** | Discovering artifacts and dependencies from source |
| **Insight** | A finding the analysis surfaces about the architecture |

---

## Features

- **Local-first** — all code scanning runs with tree-sitter, fully offline
- **Fast** — parallel file scanning via Rayon; single-pass map construction
- **Package layer** — discovers buildable units from project manifests (`Cargo.toml`, `pyproject.toml`, `package.json`, `go.mod`), links each source file to its declaring package, and flags binary entry points
- **Visibility-aware** — detects `Public` / `Crate` / `Private` on each declared symbol (Rust `pub`, Python underscore, JS `export`, Go capitalisation, Java modifiers, TS accessibility) and filters internal helpers out of the architecture view by default
- **Module detection** — Leiden algorithm runs both globally (cross-package modules) and within each package (fine-grained subsystems), so you can see both "where the cross-cuts are" and "what lives inside each crate"
- **Architecture insights** — hotspots, cross-module couplings, suggested insights
- **Interactive path queries** — weighted shortest paths that prefer runtime call chains (`Calls`=1) over file-level import shortcuts (`Imports`=5), and BFS subgraphs with a supernode cap to keep neighborhoods focused
- **Interactive HTML** — vis-network map with module colours, click-to-inspect
- **MCP server** — expose grafly as a tool to Claude Code, Cursor, and any MCP-compatible LLM

---

## Supported Languages

| Language | Extensions |
|---|---|
| Python | `.py` |
| Rust | `.rs` |
| JavaScript | `.js` `.mjs` `.cjs` |
| TypeScript | `.ts` `.tsx` |
| Go | `.go` |
| Java | `.java` |

---

## Installation

```bash
cargo install grafly-cli   # CLI
cargo install grafly-mcp   # MCP server
```

Or build from source (requires Rust ≥ 1.85):

```bash
git clone https://github.com/gianlucaciocci/grafly
cd grafly
cargo build --release
```

---

## CLI Usage

```bash
# Analyze the current directory (writes ./grafly-out/)
grafly analyze .
# `grafly .` is shorthand for the same.

# Specific project
grafly analyze ~/projects/myapp --output ./reports

# Tune module resolution (default 1.0 — higher = more, smaller modules)
grafly analyze . --resolution 0.5

# Deterministic run
grafly analyze . --seed 42

# Choose output formats
grafly analyze . --formats json,html
```

### `grafly analyze` flags

| Flag | Default | Description |
|---|---|---|
| `<PATH>` (positional) | `.` | Directory to scan |
| `-o`, `--output <DIR>` | `./grafly-out` | Output directory |
| `-r`, `--resolution <FLOAT>` | `1.0` | Leiden resolution — higher → more, smaller modules |
| `-s`, `--seed <INT>` | — | Random seed for deterministic module detection |
| `-f`, `--formats <CSV>` | `json,html,html-modules,html-packages,md` | Comma-separated output formats: `json`, `html`, `html-modules`, `html-packages`, `md` |
| `--max-html-nodes <N>` | `800` | Cap on artifacts in the artifact-level HTML (`0` = unlimited) |
| `--max-html-modules <N>` | `100` | Cap on modules in the module-level HTML (`0` = unlimited) |
| `--html-include-ambiguous` | `false` | Show `Ambiguous`-confidence edges in the artifact HTML (always kept in JSON) |
| `--no-ignore` | `false` | Disable all path filtering — scan every file, including hidden dirs, `.gitignore`d paths, `node_modules`, `target`, `.venv`, tests, examples |
| `--include-tests` | `false` | Keep test/example files (`tests/`, `__tests__/`, `examples/`, `*_test.go`, `*.test.ts`, `*Test.java`, etc.). Excluded by default — they're not runtime architecture |
| `--include-imports` | `false` | Keep `Imports` edges in the output. Used for clustering either way; dropped after clustering by default because they create misleading `A → shared_file → B` path shortcuts and inflate hotspot degrees |
| `--no-intra-package-modules` | `false` | Skip the intra-package Leiden pass. By default grafly clusters within each `Package` separately (in addition to the global cross-package modules), surfacing fine-grained subsystems inside each crate/package |
| `--leiden-thorough` | `false` | Use leiden-rs's stock high-quality defaults (`max_iter=100`, `epsilon=1e-10`) instead of grafly's fast defaults (`max_iter=30`, `epsilon=1e-8`, `min_iter=3`). Adds time on large codebases for a sub-percent quality gain |
| `--include-private` | `false` | Show `Visibility::Private` symbols in the artifact HTML, hotspots, and couplings. Hidden by default so the architecture view stays focused on the public surface; always kept in `grafly_knowledge.json` regardless |

Run `grafly analyze --help` for the same list straight from the binary.

Output files (all in `./grafly-out/` by default):

| File | Description |
|---|---|
| `README.md` | Index of all output files, written for both humans and LLM agents |
| `grafly_report.md` | Markdown analysis: packages, modules, hotspots, cross-module couplings, suggested questions — LLM-discoverable |
| `grafly_knowledge.json` | Full directed dependency map with `source_file:line` on every edge |
| `grafly_modules.html` | Interactive module-level overview (Leiden modules as nodes, edges grouped by relationship kind) |
| `grafly_packages.html` | Interactive package-level overview (Cargo/pyproject/package.json/go.mod packages as nodes, cross-package edges; binaries coloured distinctly from libraries) |
| `grafly_artifacts.html` | Interactive artifact-level graph (top-N by degree, Ambiguous edges suppressed for clarity) |
| `SUGGESTED_QUESTIONS.md` | Kickoff list of architectural questions grouped by which grafly file answers them, with a section for an LLM to fill in project-specific versions |

## Make Grafly Discoverable to LLM Agents

After analyzing, install grafly's instructions into your LLM tool's config file
so any agent working in this project knows to consult `./grafly-out/` before
reading source files or running grep:

```bash
grafly install                          # default: Claude Code (writes ./CLAUDE.md)
grafly install --platform cursor        # Cursor (.cursor/rules/grafly.mdc)
grafly install --platform agents        # AGENTS.md for Codex / Aider / OpenCode
grafly install --platform copilot       # .github/copilot-instructions.md
grafly install --platform windsurf      # .windsurfrules
grafly install --platform gemini        # GEMINI.md
grafly install --all                    # all of the above
grafly install --scope global           # ~/.claude/CLAUDE.md (Claude / Agents / Gemini)

grafly uninstall --all                  # cleanly remove all sections
```

Each install appends a marked section (`<!-- grafly-section-start -->` /
`<!-- grafly-section-end -->`) to the target file with the directives the LLM
should follow. Existing content in those files is preserved; `grafly uninstall`
removes just our section.

## Register the MCP Server

The instructions above teach LLM agents *about* grafly's output. To let them
**call grafly's tools live** (query the graph, find paths, export) via the
Model Context Protocol, register `grafly-mcp` in your MCP client:

```bash
grafly mcp install                       # default: Claude Code → ./.mcp.json
grafly mcp install --client cursor       # .cursor/mcp.json
grafly mcp install --client claude-desktop  # global Claude Desktop config
grafly mcp install --client windsurf     # ~/.codeium/windsurf/mcp_config.json
grafly mcp install --client vscode       # .vscode/mcp.json
grafly mcp install --all                 # every supported client

grafly mcp list                          # show where it's registered
grafly mcp uninstall --all               # remove cleanly
```

The MCP server exposes ten tools: `analyze`, `get_artifacts`, `get_modules`,
`get_hotspots`, `get_couplings`, `get_insights`, `export`, `find_path`,
`get_neighbors`, `get_dependents`. The binary path is auto-detected (looks for
`grafly-mcp` next to `grafly`); override with `--bin`.

Other entries in the same config file are preserved — grafly only mutates its
own `grafly` server entry.

### The `/grafly` slash command (Claude Code)

When you run `grafly mcp install` with the Claude Code client, two extra
files are installed automatically:

- `~/.claude/skills/grafly/SKILL.md` — the skill brief that tells Claude how
  to route a user's question to the right MCP tool (e.g. "list the modules"
  → `grafly:get_modules`, "what's a hotspot here" → `grafly:get_hotspots`).
- A marker-bracketed registration in `~/.claude/CLAUDE.md` so Claude knows
  the skill exists.

After install, typing `/grafly` in a Claude Code session invokes the skill,
which uses the MCP tools to answer architecture and codebase questions with
`source_file:line` citations. `grafly mcp uninstall` removes the skill and
registration alongside the MCP entry.

---

## MCP Server

Grafly ships a first-class MCP server (`grafly-mcp`) so LLMs can call it directly as a tool.

### Claude Code setup

Add to your `~/.claude/settings.json` (or project `.claude/settings.json`):

```json
{
  "mcpServers": {
    "grafly": {
      "command": "grafly-mcp",
      "args": []
    }
  }
}
```

### Available tools

| Tool | Description |
|---|---|
| `analyze` | Full pipeline — returns summary with artifact/dependency/module counts, quality score, hotspots, and insights |
| `get_artifacts` | List artifacts, optionally filtered by `kind` or `module_id` |
| `get_modules` | Module breakdown — sizes and representative artifacts |
| `get_hotspots` | High-centrality artifacts that may be architectural bottlenecks |
| `get_couplings` | Cross-module couplings — unexpected dependencies between modules |
| `get_insights` | Suggested insights for architectural review |
| `export` | Write JSON / HTML / Markdown files to a directory |
| `find_path` | Weighted shortest path between two artifacts — prefers `Calls` chains over `Imports` shortcuts |
| `get_neighbors` | Depth-limited BFS subgraph around an artifact (default: runtime edges only, supernode cap at degree 200) |
| `get_dependents` | The artifacts that depend on a given artifact (incoming-direction subgraph) |

### Example tool call

```json
{
  "tool": "analyze",
  "arguments": {
    "path": "/home/user/projects/myapp",
    "resolution": 1.0,
    "seed": 42
  }
}
```

Response:

```json
{
  "artifacts": 312,
  "dependencies": 847,
  "modules": 7,
  "quality": 0.4821,
  "hotspots": [
    { "label": "DatabaseClient", "degree": 34, "source_file": "src/db/client.py" }
  ],
  "insights": [
    "`DatabaseClient` (src/db/client.py) is a hotspot with 34 connections — consider splitting it.",
    "`AuthMiddleware` (module 2) couples to `ReportGenerator` (module 5) via `Imports` — is this intentional?"
  ]
}
```

### Path queries

```json
{
  "tool": "find_path",
  "arguments": {
    "path": "/home/user/projects/myapp",
    "from": "DataActorCore",
    "to": "ExecutionEngine"
  }
}
```

By default the path is weighted so `Calls` edges (weight 1) are strongly preferred
over `Imports` edges (weight 5) and `References`/`Uses` (weight 10). In a message-bus
architecture this routes the answer through the actual mediation chain instead of
taking a file-level import shortcut. Set `weighted: false` for raw shortest path by
hop count. Each hop in the response carries its `DependencyKind`, `Confidence`,
and `source_line` so callers can distinguish hard-evidence chains from inferred ones.

---

## Rust Library Usage

Add to `Cargo.toml`:

```toml
[dependencies]
grafly = "0.1"
```

```rust
use std::path::Path;

// 1. Scan
let scan = grafly::scan::scan_dir(Path::new("./src"))?;

// 2. Build dependency map
let mut builder = grafly::MapBuilder::new();
builder.add_scan(scan);
let mut map = builder.build();

// 3. Detect modules
grafly::cluster::detect_modules(&mut map, 1.0, None)?;

// 4. Analyze
let analysis = grafly::analyze::analyze(&map);

// 5. Query — weighted shortest path between two artifacts
let from = grafly::query::resolve(&map, "DataActorCore")?;
let to   = grafly::query::resolve(&map, "ExecutionEngine")?;
if let Some(path) = grafly::query::find_path(&map, from, to, &Default::default()) {
    println!("{} hops, weight {}", path.total_hops, path.total_weight);
}

// 6. Export
grafly::export::write_html(&map, Path::new("map.html"))?;
```

---

## Architecture

```
grafly/
├── crates/
│   ├── core/      Artifact, Dependency, DependencyMap, MapBuilder — shared types
│   ├── scan/      tree-sitter parsers (Python, Rust, JS, TS, Go, Java) + Rayon walker
│   ├── cluster/   Leiden module detection via leiden-rs (detect_modules)
│   ├── analyze/   Hotspots · couplings · insights
│   ├── query/     find_path · neighbors · ancestors · descendants
│   ├── report/    Markdown report generator
│   ├── export/    JSON + interactive HTML (vis-network)
│   ├── cli/       grafly binary (clap)
│   └── mcp/       grafly-mcp binary (rmcp stdio server)
```

**Pipeline**:

```
scan_dir(path)
  → ScanResult { artifacts, dependencies }
  → MapBuilder           → DependencyMap (petgraph DiGraph)
  → detect_modules()     → module_id assigned to each artifact
  → analyze()            → Analysis { hotspots, couplings, insights }
  → query()              → Path / Subgraph — weighted shortest path, BFS
  → export / report
```

---

## Contributing

**Prerequisites**:

1. Rust ≥ 1.85
2. Build:

```bash
git clone https://github.com/gianlucaciocci/grafly
cd grafly
cargo build
```

All dependencies (including `leiden-rs` for module detection) resolve from crates.io.

### Adding a new language

1. Add the `tree-sitter-<lang>` crate to `crates/scan/Cargo.toml` and the workspace `Cargo.toml`.
2. Create `crates/scan/src/<lang>.rs` following the pattern in `python.rs`.
3. Register the extension in `crates/scan/src/lib.rs`.
4. Update the supported languages table in this README and in `CLAUDE.md`.

---

## License

MIT — see [LICENSE](LICENSE).

---

*[grafly.net](https://grafly.net) · me@gianlucaciocci.com*
