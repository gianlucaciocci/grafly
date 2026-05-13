# Security Policy

## Supported Versions

Grafly is pre-1.0; only the latest published version on crates.io receives security fixes.

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report security issues via GitHub's [private vulnerability reporting](https://github.com/gianlucaciocci/grafly/security/advisories/new), or email the maintainer directly. Please include:

- Description of the vulnerability
- Steps to reproduce (or a proof-of-concept)
- The version of grafly affected (`grafly --version`)
- Potential impact
- Suggested fix (if any)

You should receive an acknowledgement within **5 business days**. We'll work with you on a fix and disclosure timeline, and credit you in the release notes unless you'd prefer to remain anonymous.

## Security Model

Grafly is a **local development tool**. It parses source files with tree-sitter and runs the Leiden algorithm in-process. It optionally runs as a local MCP stdio server. **It makes no network calls** — the codebase contains no HTTP client and never reaches the network during analysis, export, or any MCP tool call.

### Threat Surface

| Vector | Mitigation |
|--------|------------|
| Tree-sitter parsing of untrusted source files | tree-sitter parses ASTs — it does not evaluate or execute source code. Grammars are statically linked and bounded. |
| Non-UTF-8 source files | `std::fs::read_to_string` returns an error on invalid UTF-8; the file is skipped and the scan continues with the rest of the tree. No panic, no partial state. |
| Symlink traversal during scan | `ignore::WalkBuilder` does not follow symlinks by default. The default `scan_dir` walker honours `.gitignore`, hidden-file rules, and skips well-known dependency/build directories (`node_modules`, `target`, `__pycache__`, ...). |
| Path traversal in install / uninstall | Target paths (`~/.claude/CLAUDE.md`, `~/.cursor/rules/...`, etc.) are computed deterministically per platform from a fixed table — not derived from untrusted input. The install flow only writes between fixed marker comments (`<!-- grafly-section-start -->` … `<!-- grafly-section-end -->`) so existing user content is preserved. |
| MCP server attack surface | The MCP server communicates over **stdio only**. There is no network listener, no port binding, and no remote transport. Each tool call re-runs the pipeline against a caller-supplied directory path, which is treated as filesystem input only. |
| Pipeline crashes on adversarial input | CPU-bound work in the MCP server runs via `tokio::task::block_in_place` so a slow parse cannot starve the runtime. Errors from any pipeline stage are converted to JSON error responses rather than propagated as panics. |
| XSS via adversarial identifier names in HTML export | The JSON payload embedded in each generated HTML view is unicode-escaped (`<`, `>`, `&` → `<`, `>`, `&`) before being written into a `<script type="application/json">` data tag; the page reads it back via `JSON.parse(textContent)`. A `</script>` substring in any artifact label or id cannot terminate the data tag, and the data tag never executes as JavaScript regardless. |

### Known limitations

(no currently tracked limitations — see the `area:security` label on GitHub Issues for any open items.)

### What grafly does NOT do

- Does not open a network listener (MCP server is stdio only)
- Does not make outbound HTTP requests (the codebase has no HTTP client)
- Does not execute, evaluate, or `cargo run` any source code it parses — tree-sitter operates on ASTs only
- Does not spawn subprocesses with shell expansion (no `shell=true`-equivalent)
- Does not collect telemetry or analytics
- Does not store credentials, tokens, or API keys

### Optional / out-of-scope dependencies

- Tree-sitter grammar crates are upstream Rust crates linked at compile time. Report parser-side issues (panics on adversarial input, etc.) to the respective tree-sitter project as well.
- `leiden-rs` is a published crates.io dependency. Algorithmic issues should be reported upstream.
- `rmcp` provides the MCP server transport. Transport-level issues should be reported upstream.

## Scope

In scope:
- The `grafly`, `grafly-cli`, and `grafly-mcp` binaries and library crates
- The `grafly install` / `grafly uninstall` flows (which modify user config files under `~/`)
- The MCP server's tool-call surface
- HTML / JSON / Markdown export when run against untrusted source code

Out of scope:
- Issues in upstream dependencies (`tree-sitter-*`, `leiden-rs`, `rmcp`, etc.) — please report those upstream
- Performance issues that aren't denial-of-service
- Output quality concerns (file a regular issue for those)
