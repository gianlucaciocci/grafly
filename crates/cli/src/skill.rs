//! Claude Code skill installation for grafly.
//!
//! The Claude Code "skill" mechanism lets a user define a slash command
//! that maps to a markdown brief the agent loads on invocation. Installing
//! the grafly skill means:
//!
//! 1. Writing `~/.claude/skills/grafly/SKILL.md` — the skill brief that tells
//!    the agent how to answer codebase questions using grafly's MCP tools
//!    (or, as a fallback, the static `grafly-out/` files).
//! 2. Adding a marker-bracketed registration to `~/.claude/CLAUDE.md` so the
//!    agent knows the skill exists and what `/grafly` should do.
//!
//! Both are installed when `grafly mcp install --client claude-code` runs.
//! `grafly mcp uninstall --client claude-code` removes both cleanly.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Markers used for the skill registration in `~/.claude/CLAUDE.md`.
/// Different from `grafly-section-*` so the AGENTS.md-style instructions block
/// and the skill registration can coexist in the same file without colliding.
const SKILL_MARKER_START: &str = "<!-- grafly-skill-start -->";
const SKILL_MARKER_END: &str = "<!-- grafly-skill-end -->";

/// The `SKILL.md` content that Claude loads when the user types `/grafly`.
/// Imperative, action-routing — the agent should read this once and immediately
/// know which MCP tool to call for which question.
const SKILL_CONTENT: &str = r#"---
name: grafly
description: Analyze and query the codebase using grafly's precomputed dependency map and MCP tools. Trigger when the user types `/grafly` or asks structural/architectural questions about the codebase.
---

# grafly skill

When invoked, answer codebase and architecture questions using grafly's tools.
Match the user's intent to the right MCP tool — do NOT read source files first
unless the MCP server is unavailable.

## Tool routing

| User asked about | Call this MCP tool |
|---|---|
| Project overview / architecture / "what does this codebase do" | `grafly:analyze` |
| List artifacts (files, classes, functions) — optionally by kind or module | `grafly:get_artifacts` |
| Module / subsystem breakdown | `grafly:get_modules` |
| Bottlenecks / most-depended-on artifacts | `grafly:get_hotspots` |
| Cross-module dependencies / design smells / coupling | `grafly:get_couplings` |
| Suggested questions / things worth investigating | `grafly:get_insights` |
| Regenerate / write output to disk | `grafly:export` |

If no `grafly:*` tool is available (the MCP server isn't connected), fall back:
1. Read `./grafly-out/grafly_report.md` for the structured summary.
2. Read `./grafly-out/grafly_knowledge.json` for the full graph (artifacts +
   dependencies with `source_file` and `source_line` on every edge).
3. If neither exists, tell the user to run `grafly analyze .` first.

## Calling convention

Most tools take a `path` argument — pass the project root (typically `.`).
For filtered queries (`get_artifacts`, `get_modules`) pass optional `kind` or
`module_id` parameters when the user's intent is specific.

## Citation requirement

Every dependency in grafly's data carries `source_file` and `source_line`.
When you explain a specific call, edge, or coupling, cite it as `path:line`.
Never paraphrase or guess locations — they're already in the data.

## Confidence calibration

- `Extracted` — directly visible in the AST. State as fact.
- `Inferred` — resolved by receiver-aware name lookup (e.g. `Foo::new()` →
  the specific `Foo::method::new` artifact). Usually correct; you can assert,
  but a slight hedge ("likely calls", "appears to") is honest.
- `Ambiguous` — multiple candidates matched. Treat as a hint, never as fact.
  If you mention an `Ambiguous` edge, say so explicitly.

## After answering

If the user's question revealed a hotspot, a cross-module coupling, or a
suggested insight, surface it as a follow-up — they probably want to know.

## If the user invokes `/grafly` with no further context

Treat it as "give me a tour": call `grafly:analyze` on `.`, then summarise the
top 3 modules and top 3 hotspots in 5-7 sentences. Offer follow-up directions
based on what you saw.
"#;

/// Resolve the home directory in an OS-portable way.
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .context("could not resolve home directory (set USERPROFILE or HOME)")
}

fn skill_file_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".claude")
        .join("skills")
        .join("grafly")
        .join("SKILL.md"))
}

fn registration_file_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude").join("CLAUDE.md"))
}

fn registration_block() -> String {
    format!(
        "{}\n# grafly skill\n\
         - **grafly** (`~/.claude/skills/grafly/SKILL.md`) — analyze and query the project's \
         codebase via grafly's MCP server. Trigger: `/grafly`\n\n\
         When the user types `/grafly`, invoke the Skill tool with `skill: \"grafly\"` before \
         doing anything else.\n{}",
        SKILL_MARKER_START, SKILL_MARKER_END
    )
}

/// Outcome of a skill-related file operation.
#[derive(Debug)]
pub struct SkillOutcome {
    pub label: &'static str,
    pub path: PathBuf,
    pub action: &'static str, // "created" | "updated" | "unchanged" | "removed" | "absent"
}

/// Install the Claude Code skill: write `SKILL.md` and add the registration
/// block to `~/.claude/CLAUDE.md`. Returns the outcomes for both operations.
pub fn install_claude_skill() -> Result<Vec<SkillOutcome>> {
    let mut outcomes = Vec::with_capacity(2);

    // 1. SKILL.md
    let skill_path = skill_file_path()?;
    if let Some(parent) = skill_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let action = if skill_path.exists() {
        let current = fs::read_to_string(&skill_path).unwrap_or_default();
        if current == SKILL_CONTENT {
            "unchanged"
        } else {
            fs::write(&skill_path, SKILL_CONTENT)
                .with_context(|| format!("writing {}", skill_path.display()))?;
            "updated"
        }
    } else {
        fs::write(&skill_path, SKILL_CONTENT)
            .with_context(|| format!("writing {}", skill_path.display()))?;
        "created"
    };
    outcomes.push(SkillOutcome {
        label: "Claude Code skill",
        path: skill_path,
        action,
    });

    // 2. Registration in ~/.claude/CLAUDE.md
    let reg_path = registration_file_path()?;
    if let Some(parent) = reg_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let new_block = registration_block();
    let reg_action = if reg_path.exists() {
        let existing = fs::read_to_string(&reg_path)
            .with_context(|| format!("reading {}", reg_path.display()))?;
        if let Some(updated) = replace_marked_section(&existing, &new_block) {
            if updated == existing {
                "unchanged"
            } else {
                fs::write(&reg_path, updated)
                    .with_context(|| format!("writing {}", reg_path.display()))?;
                "updated"
            }
        } else {
            let mut updated = existing;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !updated.is_empty() {
                updated.push('\n');
            }
            updated.push_str(&new_block);
            updated.push('\n');
            fs::write(&reg_path, updated)
                .with_context(|| format!("writing {}", reg_path.display()))?;
            "updated"
        }
    } else {
        fs::write(&reg_path, format!("{}\n", new_block))
            .with_context(|| format!("writing {}", reg_path.display()))?;
        "created"
    };
    outcomes.push(SkillOutcome {
        label: "Claude Code skill registration",
        path: reg_path,
        action: reg_action,
    });

    Ok(outcomes)
}

/// Uninstall the Claude Code skill: remove SKILL.md and the registration block.
pub fn uninstall_claude_skill() -> Result<Vec<SkillOutcome>> {
    let mut outcomes = Vec::with_capacity(2);

    // 1. SKILL.md
    let skill_path = skill_file_path()?;
    let action = if skill_path.exists() {
        fs::remove_file(&skill_path)
            .with_context(|| format!("removing {}", skill_path.display()))?;
        // Best-effort: prune empty parent directories we created (skills/grafly).
        if let Some(parent) = skill_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        "removed"
    } else {
        "absent"
    };
    outcomes.push(SkillOutcome {
        label: "Claude Code skill",
        path: skill_path,
        action,
    });

    // 2. Registration block
    let reg_path = registration_file_path()?;
    let reg_action = if reg_path.exists() {
        let existing = fs::read_to_string(&reg_path)
            .with_context(|| format!("reading {}", reg_path.display()))?;
        let trimmed = remove_marked_section(&existing);
        if trimmed == existing {
            "absent"
        } else if trimmed.trim().is_empty() {
            fs::remove_file(&reg_path)
                .with_context(|| format!("removing {}", reg_path.display()))?;
            "removed"
        } else {
            fs::write(&reg_path, trimmed)
                .with_context(|| format!("writing {}", reg_path.display()))?;
            "removed"
        }
    } else {
        "absent"
    };
    outcomes.push(SkillOutcome {
        label: "Claude Code skill registration",
        path: reg_path,
        action: reg_action,
    });

    Ok(outcomes)
}

// ── Marker-section splice helpers (local copies — different markers from install.rs) ──

fn replace_marked_section(existing: &str, new_block: &str) -> Option<String> {
    let start = existing.find(SKILL_MARKER_START)?;
    let end_rel = existing[start..].find(SKILL_MARKER_END)?;
    let end = start + end_rel + SKILL_MARKER_END.len();

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

fn remove_marked_section(existing: &str) -> String {
    let Some(start) = existing.find(SKILL_MARKER_START) else {
        return existing.to_string();
    };
    let Some(end_rel) = existing[start..].find(SKILL_MARKER_END) else {
        return existing.to_string();
    };
    let end = start + end_rel + SKILL_MARKER_END.len();

    let mut before = existing[..start].to_string();
    let after = &existing[end..];

    while matches!(before.chars().last(), Some(c) if c.is_whitespace()) {
        before.pop();
    }
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
    fn skill_marker_round_trip() {
        let original = format!(
            "Existing CLAUDE.md content\n\n{}\nold reg\n{}\n\nMore stuff\n",
            SKILL_MARKER_START, SKILL_MARKER_END
        );
        let new_block = format!("{}\nnew reg\n{}", SKILL_MARKER_START, SKILL_MARKER_END);
        let updated = replace_marked_section(&original, &new_block).unwrap();
        assert!(updated.contains("new reg"));
        assert!(!updated.contains("old reg"));
        assert!(updated.contains("Existing CLAUDE.md content"));
        assert!(updated.contains("More stuff"));
    }

    #[test]
    fn skill_marker_remove_preserves_surrounding() {
        let original = format!(
            "Existing.\n\n{}\nthe block\n{}\n\nAfter.\n",
            SKILL_MARKER_START, SKILL_MARKER_END
        );
        let trimmed = remove_marked_section(&original);
        assert!(!trimmed.contains("grafly-skill"));
        assert!(trimmed.contains("Existing."));
        assert!(trimmed.contains("After."));
    }

    #[test]
    fn skill_marker_remove_noop_when_absent() {
        let original = "Hello\nWorld\n";
        assert_eq!(remove_marked_section(original), original);
    }
}
