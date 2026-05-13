//! `grafly install` — wires grafly's instructions into an LLM tool's config
//! so any agent working in this project knows about `./grafly-out/` and uses
//! it as the source of truth for codebase questions.
//!
//! For each supported platform we either:
//! - **Append/replace a marked section** in the platform's instruction file
//!   (`CLAUDE.md`, `AGENTS.md`, `.github/copilot-instructions.md`, etc.), or
//! - **Create a dedicated rules file** if the platform uses one (Cursor's
//!   `.cursor/rules/grafly.mdc`, Windsurf's `.windsurfrules`).
//!
//! All edits are bracketed with `<!-- grafly-section-start -->` /
//! `<!-- grafly-section-end -->` so `grafly uninstall` can clean them out
//! without disturbing the rest of the file.

use anyhow::{Context, Result};
use clap::ValueEnum;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Platform {
    /// Anthropic Claude Code — writes `CLAUDE.md`
    Claude,
    /// Generic AGENTS.md — works for Codex, Aider, OpenCode, Factory, etc.
    Agents,
    /// Cursor IDE — writes `.cursor/rules/grafly.mdc`
    Cursor,
    /// GitHub Copilot Chat / VS Code — writes `.github/copilot-instructions.md`
    Copilot,
    /// Windsurf — writes `.windsurfrules`
    Windsurf,
    /// Gemini CLI — writes `GEMINI.md`
    Gemini,
}

impl Platform {
    pub fn display_name(&self) -> &'static str {
        match self {
            Platform::Claude => "Claude Code",
            Platform::Agents => "AGENTS.md (Codex / Aider / OpenCode / generic)",
            Platform::Cursor => "Cursor",
            Platform::Copilot => "GitHub Copilot",
            Platform::Windsurf => "Windsurf",
            Platform::Gemini => "Gemini CLI",
        }
    }

    pub fn all() -> &'static [Platform] {
        &[
            Platform::Claude,
            Platform::Agents,
            Platform::Cursor,
            Platform::Copilot,
            Platform::Windsurf,
            Platform::Gemini,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Write into the current project (e.g. `./CLAUDE.md`)
    Project,
    /// Write into the user's global config (e.g. `~/.claude/CLAUDE.md`)
    Global,
}

// ── Markers ──────────────────────────────────────────────────────────────────
//
// Every marked-section format wraps the grafly block with these. The actual
// comment delimiters depend on the file format (Markdown HTML comments work
// for .md / .mdc; for `.windsurfrules` we use the same since it's also
// freeform markdown).

const MARKER_START: &str = "<!-- grafly-section-start -->";
const MARKER_END: &str = "<!-- grafly-section-end -->";

// ── Content blocks ───────────────────────────────────────────────────────────

/// The core directive shown to every LLM agent. Kept deliberately terse and
/// imperative — these are *system instructions*, not documentation. Long
/// explanatory prose tends to get summarised away by agents before it
/// influences behaviour; short numbered rules survive.
fn grafly_block_body(output_dir: &str) -> String {
    format!(
        "## grafly

This project has a precomputed dependency map at `{out}/` produced by [grafly](https://grafly.net).

Rules:
- ALWAYS read `{out}/grafly_report.md` before answering codebase or architecture questions, before reading source files, and before running grep/glob.
- If the `grafly` MCP server is connected (tools `grafly:analyze`, `grafly:get_artifacts`, `grafly:get_modules`, `grafly:get_hotspots`, `grafly:get_couplings`, `grafly:get_insights`, `grafly:export`), PREFER those over the static files — they're live, query-able, and return data with `source_file`/`source_line` baked in.
- For \"how does X relate to Y\" / \"what calls Z\" / \"what depends on W\", query `{out}/grafly_knowledge.json` directly (or the MCP tools). Every dependency has `source_file` and `source_line` — cite them as `path:line` in your answer.
- Trust confidence: `Extracted` (AST-direct) > `Inferred` (resolved by receiver-aware name lookup) > `Ambiguous` (hint only — never assert as fact).
- After modifying code in this session, run `grafly analyze .` (or call `grafly:analyze` via MCP) to refresh the map.
- When the user asks an open-ended onboarding question (\"what can I ask?\", \"where do I start?\", \"give me onboarding questions\"), read `{out}/SUGGESTED_QUESTIONS.md` and append a dated \"Project-specific questions\" section below the marker `<!-- Append project-specific questions below this line -->`, resolving every `<ARTIFACT>` / `<MODULE>` / `<PACKAGE>` placeholder to a real name from `{out}/grafly_report.md`. Then surface the top 10 as a numbered menu in chat. (Claude Code users: the `/grafly-suggest-questions` slash command does exactly this.)",
        out = output_dir
    )
}

/// Wrap the body in markers so we can find and remove it cleanly.
fn marked_block(output_dir: &str) -> String {
    format!(
        "{}\n{}\n{}",
        MARKER_START,
        grafly_block_body(output_dir),
        MARKER_END
    )
}

/// Cursor's `.mdc` format requires YAML frontmatter to mark a rule as
/// always-applied. The rest of the file is the same markdown block.
fn cursor_mdc(output_dir: &str) -> String {
    format!(
        "---
description: Use grafly's precomputed dependency map for codebase questions.
alwaysApply: true
---

{}
",
        marked_block(output_dir)
    )
}

// ── File paths per platform ──────────────────────────────────────────────────

fn target_path(platform: Platform, scope: Scope, project_root: &Path) -> Result<PathBuf> {
    let p = match (platform, scope) {
        (Platform::Claude, Scope::Project) => project_root.join("CLAUDE.md"),
        (Platform::Claude, Scope::Global) => {
            home_dir()?.join(".claude").join("CLAUDE.md")
        }
        (Platform::Agents, Scope::Project) => project_root.join("AGENTS.md"),
        (Platform::Agents, Scope::Global) => {
            home_dir()?.join(".agents").join("AGENTS.md")
        }
        (Platform::Cursor, _) => project_root.join(".cursor").join("rules").join("grafly.mdc"),
        (Platform::Copilot, _) => project_root
            .join(".github")
            .join("copilot-instructions.md"),
        (Platform::Windsurf, _) => project_root.join(".windsurfrules"),
        (Platform::Gemini, Scope::Project) => project_root.join("GEMINI.md"),
        (Platform::Gemini, Scope::Global) => {
            home_dir()?.join(".gemini").join("GEMINI.md")
        }
    };
    Ok(p)
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .context("could not resolve home directory (set USERPROFILE or HOME)")
}

// ── Install / uninstall a single platform ────────────────────────────────────

pub struct InstallOutcome {
    pub platform: Platform,
    pub path: PathBuf,
    pub action: &'static str, // "created", "updated", "unchanged"
}

pub fn install_platform(
    platform: Platform,
    scope: Scope,
    project_root: &Path,
    output_dir: &str,
) -> Result<InstallOutcome> {
    let path = target_path(platform, scope, project_root)?;

    // Cursor's .mdc has frontmatter and is a dedicated file — write directly,
    // overwriting whatever was there (it's our file).
    if platform == Platform::Cursor {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let content = cursor_mdc(output_dir);
        let existed = path.exists();
        fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        return Ok(InstallOutcome {
            platform,
            path,
            action: if existed { "updated" } else { "created" },
        });
    }

    // All other targets are append/replace inside an existing or new markdown
    // file. Use the markers to splice cleanly.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let new_block = marked_block(output_dir);
    let action = if path.exists() {
        let existing = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if let Some(updated) = replace_section(&existing, &new_block) {
            if updated == existing {
                "unchanged"
            } else {
                fs::write(&path, updated)
                    .with_context(|| format!("writing {}", path.display()))?;
                "updated"
            }
        } else {
            // No marker block yet — append.
            let mut updated = existing;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !updated.is_empty() {
                updated.push('\n');
            }
            updated.push_str(&new_block);
            updated.push('\n');
            fs::write(&path, updated)
                .with_context(|| format!("writing {}", path.display()))?;
            "updated"
        }
    } else {
        let content = format!("{}\n", new_block);
        fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        "created"
    };

    Ok(InstallOutcome {
        platform,
        path,
        action,
    })
}

pub struct UninstallOutcome {
    pub platform: Platform,
    pub path: PathBuf,
    pub action: &'static str, // "removed", "deleted", "absent"
}

pub fn uninstall_platform(
    platform: Platform,
    scope: Scope,
    project_root: &Path,
) -> Result<UninstallOutcome> {
    let path = target_path(platform, scope, project_root)?;
    if !path.exists() {
        return Ok(UninstallOutcome {
            platform,
            path,
            action: "absent",
        });
    }

    // Cursor's .mdc is ours; delete the whole file.
    if platform == Platform::Cursor {
        fs::remove_file(&path)
            .with_context(|| format!("removing {}", path.display()))?;
        return Ok(UninstallOutcome {
            platform,
            path,
            action: "deleted",
        });
    }

    let existing = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let trimmed = remove_section(&existing);
    if trimmed == existing {
        return Ok(UninstallOutcome {
            platform,
            path,
            action: "absent",
        });
    }

    // If the file is now empty (or whitespace only), delete it; we created it.
    if trimmed.trim().is_empty() {
        fs::remove_file(&path)
            .with_context(|| format!("removing {}", path.display()))?;
        Ok(UninstallOutcome {
            platform,
            path,
            action: "deleted",
        })
    } else {
        fs::write(&path, trimmed)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(UninstallOutcome {
            platform,
            path,
            action: "removed",
        })
    }
}

// ── Section splicing helpers ────────────────────────────────────────────────

/// Replace the existing marked section with `new_block`. Returns `None` if no
/// marked section is present.
fn replace_section(existing: &str, new_block: &str) -> Option<String> {
    let start = existing.find(MARKER_START)?;
    let end_rel = existing[start..].find(MARKER_END)?;
    let end = start + end_rel + MARKER_END.len();
    // Trim trailing newline of the section so we don't accumulate blanks.
    let mut before = existing[..start].to_string();
    let mut after = existing[end..].to_string();

    if !before.is_empty() && !before.ends_with('\n') {
        before.push('\n');
    }
    if after.starts_with('\n') {
        after.remove(0);
    }

    let mut out = before;
    out.push_str(new_block);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&after);
    Some(out)
}

/// Remove the marked section. Returns the input unchanged if no markers.
fn remove_section(existing: &str) -> String {
    let Some(start) = existing.find(MARKER_START) else {
        return existing.to_string();
    };
    let Some(end_rel) = existing[start..].find(MARKER_END) else {
        return existing.to_string();
    };
    let end = start + end_rel + MARKER_END.len();

    let mut before = existing[..start].to_string();
    let after = &existing[end..];

    // Strip trailing whitespace before the section (avoid leaving "\n\n\n").
    while matches!(before.chars().last(), Some(c) if c.is_whitespace()) {
        before.pop();
    }
    // Strip leading newlines from what remains after the section.
    let after_trim = after.trim_start_matches('\n');

    if before.is_empty() && after_trim.is_empty() {
        return String::new();
    }
    let mut out = before;
    if !out.is_empty() && !after_trim.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(after_trim);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_section_round_trip() {
        let original = format!(
            "Hello\n\n{}\nold body\n{}\n\nGoodbye\n",
            MARKER_START, MARKER_END
        );
        let new_block = format!("{}\nNEW BODY\n{}", MARKER_START, MARKER_END);
        let updated = replace_section(&original, &new_block).unwrap();
        assert!(updated.contains("NEW BODY"));
        assert!(!updated.contains("old body"));
        assert!(updated.starts_with("Hello"));
        assert!(updated.contains("Goodbye"));
    }

    #[test]
    fn remove_section_cleans_up() {
        let original = format!(
            "Hello\n\n{}\nbody\n{}\n\nGoodbye\n",
            MARKER_START, MARKER_END
        );
        let trimmed = remove_section(&original);
        assert!(!trimmed.contains("grafly-section"));
        assert!(trimmed.contains("Hello"));
        assert!(trimmed.contains("Goodbye"));
    }

    #[test]
    fn remove_section_no_op_when_absent() {
        let original = "Hello\nWorld\n";
        assert_eq!(remove_section(original), original);
    }
}
