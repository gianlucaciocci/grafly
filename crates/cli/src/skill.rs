//! Claude Code skill installation for grafly.
//!
//! The Claude Code "skill" mechanism lets a user define a slash command
//! that maps to a markdown brief the agent loads on invocation. Installing
//! the grafly skills means:
//!
//! 1. Writing `~/.claude/skills/<name>/SKILL.md` for each skill grafly ships:
//!    - `grafly-ask` — answer architectural / structural codebase questions
//!      via grafly's MCP tools (falling back to the static `grafly-out/` files).
//!    - `grafly-suggest-questions` — bootstrap a project-specific question
//!      list by resolving placeholders in `SUGGESTED_QUESTIONS.md`.
//! 2. Adding a single marker-bracketed registration to `~/.claude/CLAUDE.md`
//!    so the agent knows both slash commands exist.
//!
//! All of the above is installed when `grafly install` runs against the
//! `claude` target (the default). `grafly uninstall --platform claude`
//! removes them cleanly, and the install also sweeps up legacy skill
//! directories from older grafly versions (see [`LEGACY_SKILL_NAMES`]).

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Markers used for the skill registration in `~/.claude/CLAUDE.md`.
/// Different from `grafly-section-*` so the AGENTS.md-style instructions block
/// and the skill registration can coexist in the same file without colliding.
const SKILL_MARKER_START: &str = "<!-- grafly-skill-start -->";
const SKILL_MARKER_END: &str = "<!-- grafly-skill-end -->";

/// The `SKILL.md` content that Claude loads when the user types `/grafly-ask`.
/// Imperative, action-routing — the agent should read this once and immediately
/// know which MCP tool to call for which question.
const SKILL_CONTENT: &str = r#"---
name: grafly-ask
description: Ask any architectural / structural question about the codebase — overview, modules, hotspots, cross-module couplings, "how does X connect to Y", "what depends on Z" — and route it to the right grafly MCP tool. Trigger when the user types `/grafly-ask` or asks structural/architectural questions about the codebase.
---

# grafly-ask skill

When invoked, answer codebase and architecture questions using grafly's tools.
Match the user's intent to the right MCP tool — do NOT read source files first
unless the MCP server is unavailable.

## Tool routing

| User asked about | Call this MCP tool |
|---|---|
| Project overview / architecture / "what does this codebase do" | `grafly-mcp:analyze` |
| List artifacts (files, classes, functions) — optionally by kind or module | `grafly-mcp:get_artifacts` |
| Module / subsystem breakdown | `grafly-mcp:get_modules` |
| Bottlenecks / most-depended-on artifacts | `grafly-mcp:get_hotspots` |
| Cross-module dependencies / design smells / coupling | `grafly-mcp:get_couplings` |
| Suggested questions / things worth investigating | `grafly-mcp:get_insights` |
| Regenerate / write output to disk | `grafly-mcp:export` |
| "What can I ask?" / "Where do I start?" / kick-off questions | Invoke the `/grafly-suggest-questions` skill instead of answering directly |

If no `grafly-mcp:*` tool is available (the MCP server isn't connected), fall back:
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

## If the user invokes `/grafly-ask` with no further context

Treat it as "give me a tour": call `grafly-mcp:analyze` on `.`, then summarise the
top 3 modules and top 3 hotspots in 5-7 sentences. Offer follow-up directions
based on what you saw.
"#;

/// `SKILL.md` content for `/grafly-suggest-questions`. A focused, narrower
/// skill: bootstrap a question list for a codebase the user just analysed.
/// Useful even without the MCP server connected — works entirely from the
/// static files in `./grafly-out/`.
const SUGGEST_QUESTIONS_SKILL_CONTENT: &str = r#"---
name: grafly-suggest-questions
description: Generate a project-specific list of architectural questions the user can ask about their codebase, using grafly's analysis. Trigger when the user types `/grafly-suggest-questions`, or asks "what can I ask?" / "where do I start?" / "give me onboarding questions" about the codebase.
---

# grafly-suggest-questions skill

When invoked, produce a project-specific list of architectural questions the
user can ask leveraging grafly's analysis. The output should feel like a
curated menu of "things worth investigating in *this* codebase", not the
generic template.

## Workflow

1. **Check that grafly output exists.** Look for `./grafly-out/grafly_report.md`
   and `./grafly-out/SUGGESTED_QUESTIONS.md`. If they're missing, tell the user
   to run `grafly analyze .` first and stop. (If the user wants to actually
   answer one of the suggested questions afterwards, invoke `/grafly-ask`.)
2. **Read the placeholder template.** Open `./grafly-out/SUGGESTED_QUESTIONS.md`
   and skim the structure. The placeholders (`<ARTIFACT>`, `<MODULE>`,
   `<PACKAGE>`) need to be resolved to real names from this project.
3. **Read the report for context.** Open `./grafly-out/grafly_report.md` and
   pull out:
   - Top packages from the **Packages** section (names, descriptions)
   - Top modules from the **Modules** section (names, sizes)
   - Top hotspots from the **Hotspots** section (high-degree artifacts)
   - Notable cross-module couplings (interesting bridges between subsystems)
4. **For deeper specificity, optionally consult `grafly_knowledge.json`** — or
   call `grafly-mcp:get_artifacts` / `grafly-mcp:get_hotspots` / `grafly-mcp:get_couplings`
   if the MCP server is connected.
5. **Append a "Project-specific questions" section to `SUGGESTED_QUESTIONS.md`.**
   Look for the marker `<!-- Append project-specific questions below this line -->`
   in the file and add a new dated section *below* it. Every question MUST be
   prefixed with `/grafly-ask ` (with the trailing space) so the user can
   copy/paste each line straight into Claude Code as a slash command. Resolve
   every `<PLACEHOLDER>` to a real name — for example:
   - `- /grafly-ask What does the <PACKAGE> contain?` → `- /grafly-ask What does the nautilus-execution crate contain?`
   - `- /grafly-ask Who calls <FUNCTION>?` → `- /grafly-ask Who calls get_message_bus?`
   Pick 15-25 questions across categories, with a bias toward what looks most
   architecturally interesting *in this codebase* (e.g. unusually large
   modules, surprising couplings, god-object-shaped hotspots).
6. **Surface the top 10 as a menu in the chat.** End your reply with a numbered
   list of the 10 highest-leverage questions you appended. Invite the user to
   pick one to dive into — you'll then call the appropriate `grafly-mcp:*` tool or
   read the appropriate file to answer it.

## What to avoid

- Don't invent artifacts/modules/packages — every name you write must come from
  `grafly_report.md` or `grafly_knowledge.json`.
- Don't dump all 100+ placeholder questions resolved — curate. Quality > quantity.
- Don't overwrite the existing template content. Append below the marker.
- Don't include `Ambiguous`-confidence-derived insights without flagging them.

## Citation requirement

When you mention a specific dependency, hotspot, or coupling in either the
appended file or the chat menu, cite `source_file:source_line` where possible —
those are already in the data.
"#;

/// Resolve the home directory in an OS-portable way.
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .context("could not resolve home directory (set USERPROFILE or HOME)")
}

/// All Claude Code skills shipped by `grafly install --platform claude`,
/// keyed by their slash-command name. Iterating this list keeps install and
/// uninstall symmetric — adding a new skill is one row here, not a refactor.
const SKILLS: &[(&str, &str)] = &[
    ("grafly-ask", SKILL_CONTENT),
    ("grafly-suggest-questions", SUGGEST_QUESTIONS_SKILL_CONTENT),
];

/// Older skill directory names that grafly used to install. Listed here so
/// `grafly install` and `grafly uninstall` clean them up on upgrade —
/// otherwise a user who installed an older grafly would be left with a stale
/// `/grafly` slash command pointing at out-of-date content.
const LEGACY_SKILL_NAMES: &[&str] = &["grafly"];

fn skill_file_path(name: &str) -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".claude")
        .join("skills")
        .join(name)
        .join("SKILL.md"))
}

fn registration_file_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude").join("CLAUDE.md"))
}

fn registration_block() -> String {
    format!(
        "{}\n# grafly skills\n\
         - **grafly-ask** (`~/.claude/skills/grafly-ask/SKILL.md`) — answer any architectural / \
         structural question about the project's codebase (overview, modules, hotspots, couplings, \
         path queries) via grafly's MCP server, falling back to `./grafly-out/grafly_report.md`. \
         Trigger: `/grafly-ask`\n\
         - **grafly-suggest-questions** (`~/.claude/skills/grafly-suggest-questions/SKILL.md`) — \
         generate a project-specific list of architectural questions, resolving placeholders in \
         `./grafly-out/SUGGESTED_QUESTIONS.md` to real artifact/module/package names. \
         Trigger: `/grafly-suggest-questions` or natural-language asks like \"what can I ask about this codebase?\"\n\n\
         When the user types one of those slash commands, invoke the Skill tool with the \
         matching `skill:` argument before doing anything else.\n{}",
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

/// Human-readable label for install/uninstall output. Skill names that don't
/// match a known skill fall back to a generic label — defensive default,
/// the [`SKILLS`] list is the single source of truth.
fn skill_label(name: &str) -> &'static str {
    match name {
        "grafly-ask" => "Claude Code skill (/grafly-ask)",
        "grafly-suggest-questions" => "Claude Code skill (/grafly-suggest-questions)",
        "grafly" => "Claude Code skill (/grafly — legacy)",
        _ => "Claude Code skill",
    }
}

/// Best-effort: remove the SKILL.md (and prune the empty parent directory) for
/// any [`LEGACY_SKILL_NAMES`] entry that exists on disk. Used by both install
/// (to clean up after an upgrade) and uninstall.
///
/// Adds one [`SkillOutcome`] per legacy directory that existed; absent legacy
/// entries are silently ignored so the install summary stays clean.
fn cleanup_legacy_skills(outcomes: &mut Vec<SkillOutcome>) -> Result<()> {
    for name in LEGACY_SKILL_NAMES {
        let path = skill_file_path(name)?;
        if !path.exists() {
            continue;
        }
        fs::remove_file(&path)
            .with_context(|| format!("removing legacy skill {}", path.display()))?;
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        outcomes.push(SkillOutcome {
            label: skill_label(name),
            path,
            action: "removed",
        });
    }
    Ok(())
}

/// Install all Claude Code skills shipped by grafly: write each `SKILL.md`
/// and add a single registration block (covering every skill) to
/// `~/.claude/CLAUDE.md`. Returns one [`SkillOutcome`] per file written.
pub fn install_claude_skill() -> Result<Vec<SkillOutcome>> {
    let mut outcomes: Vec<SkillOutcome> = Vec::with_capacity(SKILLS.len() + 2);

    // 0. Remove anything from a prior grafly version that we no longer ship.
    cleanup_legacy_skills(&mut outcomes)?;

    // 1. Every SKILL.md
    for (name, content) in SKILLS {
        let skill_path = skill_file_path(name)?;
        if let Some(parent) = skill_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let action = if skill_path.exists() {
            let current = fs::read_to_string(&skill_path).unwrap_or_default();
            if current == *content {
                "unchanged"
            } else {
                fs::write(&skill_path, *content)
                    .with_context(|| format!("writing {}", skill_path.display()))?;
                "updated"
            }
        } else {
            fs::write(&skill_path, *content)
                .with_context(|| format!("writing {}", skill_path.display()))?;
            "created"
        };
        outcomes.push(SkillOutcome {
            label: skill_label(name),
            path: skill_path,
            action,
        });
    }

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

/// Uninstall every Claude Code skill shipped by grafly: remove each
/// `SKILL.md` and the (single) registration block. Returns one
/// [`SkillOutcome`] per file touched.
pub fn uninstall_claude_skill() -> Result<Vec<SkillOutcome>> {
    let mut outcomes: Vec<SkillOutcome> = Vec::with_capacity(SKILLS.len() + 2);

    // Also sweep up legacy skill names we no longer ship.
    cleanup_legacy_skills(&mut outcomes)?;

    for (name, _) in SKILLS {
        let skill_path = skill_file_path(name)?;
        let action = if skill_path.exists() {
            fs::remove_file(&skill_path)
                .with_context(|| format!("removing {}", skill_path.display()))?;
            // Best-effort: prune empty parent dir we created (e.g. skills/grafly-ask).
            if let Some(parent) = skill_path.parent() {
                let _ = fs::remove_dir(parent);
            }
            "removed"
        } else {
            "absent"
        };
        outcomes.push(SkillOutcome {
            label: skill_label(name),
            path: skill_path,
            action,
        });
    }

    // Registration block
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
