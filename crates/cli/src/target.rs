//! Unified install target — one row per LLM tool, doing every surface that
//! tool supports (rules file, MCP server registry, Claude Code skills) in a
//! single all-or-nothing call.
//!
//! `Target` is the user-facing CLI enum behind `--platform` on
//! `grafly install` / `uninstall` / `list`. Internally each variant maps to:
//! - an optional `install::Platform` for the rules file (CLAUDE.md, AGENTS.md, …)
//! - an optional `install_mcp::McpClient` for the JSON MCP registry
//! - a boolean for "also wire the Claude Code `/grafly-*` skills"
//!
//! A target may support any subset of those three surfaces. Targets with no
//! rules surface (e.g. Claude Desktop, VS Code) are MCP-only; targets with no
//! MCP surface (e.g. Codex/Aider/Copilot/Gemini) are rules-only. The
//! orchestration here does whatever the target supports and silently skips
//! the rest — the user doesn't get to choose.

use anyhow::Result;
use clap::ValueEnum;
use std::path::{Path, PathBuf};

use crate::install::{
    install_platform, list_marker_path, uninstall_platform, InstallOutcome, Platform, Scope,
    UninstallOutcome,
};
use crate::install_mcp::{install_mcp, list_marker_mcp_path, uninstall_mcp, McpClient, McpOutcome};
use crate::skill::{install_claude_skill, uninstall_claude_skill, SkillOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Anthropic Claude Code — CLAUDE.md + .mcp.json + `/grafly-*` skills.
    Claude,
    /// Anthropic Claude Desktop — claude_desktop_config.json only (no rules file).
    ClaudeDesktop,
    /// Cursor IDE — .cursor/rules/grafly.mdc + .cursor/mcp.json.
    Cursor,
    /// Windsurf — .windsurfrules + ~/.codeium/windsurf/mcp_config.json.
    Windsurf,
    /// VS Code — .vscode/mcp.json only (no equivalent rules file).
    Vscode,
    /// Generic AGENTS.md — Codex / Aider / OpenCode / Factory (no MCP slot).
    Agents,
    /// GitHub Copilot — .github/copilot-instructions.md (no MCP slot).
    Copilot,
    /// Gemini CLI — GEMINI.md (no MCP slot).
    Gemini,
}

impl Target {
    pub fn display_name(self) -> &'static str {
        match self {
            Target::Claude => "Claude Code",
            Target::ClaudeDesktop => "Claude Desktop",
            Target::Cursor => "Cursor",
            Target::Windsurf => "Windsurf",
            Target::Vscode => "VS Code",
            Target::Agents => "AGENTS.md (Codex / Aider / OpenCode / generic)",
            Target::Copilot => "GitHub Copilot",
            Target::Gemini => "Gemini CLI",
        }
    }

    pub fn all() -> &'static [Target] {
        &[
            Target::Claude,
            Target::ClaudeDesktop,
            Target::Cursor,
            Target::Windsurf,
            Target::Vscode,
            Target::Agents,
            Target::Copilot,
            Target::Gemini,
        ]
    }

    /// The rules-file surface this target supports, if any.
    fn rules_platform(self) -> Option<Platform> {
        match self {
            Target::Claude => Some(Platform::Claude),
            Target::Cursor => Some(Platform::Cursor),
            Target::Windsurf => Some(Platform::Windsurf),
            Target::Agents => Some(Platform::Agents),
            Target::Copilot => Some(Platform::Copilot),
            Target::Gemini => Some(Platform::Gemini),
            Target::ClaudeDesktop | Target::Vscode => None,
        }
    }

    /// The MCP-registry surface this target supports, if any.
    fn mcp_client(self) -> Option<McpClient> {
        match self {
            Target::Claude => Some(McpClient::ClaudeCode),
            Target::ClaudeDesktop => Some(McpClient::ClaudeDesktop),
            Target::Cursor => Some(McpClient::Cursor),
            Target::Windsurf => Some(McpClient::Windsurf),
            Target::Vscode => Some(McpClient::Vscode),
            Target::Agents | Target::Copilot | Target::Gemini => None,
        }
    }

    /// Whether this target also installs the Claude Code `/grafly-*` skills.
    /// Only `Target::Claude` does — the skills live in `~/.claude/skills/`
    /// and are recognised exclusively by Claude Code.
    fn installs_claude_skills(self) -> bool {
        matches!(self, Target::Claude)
    }
}

/// Outcome of installing a single target, broken down per surface so the CLI
/// can print one row per touched file. Each `Option` is `None` when the
/// target doesn't support that surface (silently skipped).
pub struct TargetOutcome {
    pub target: Target,
    pub rules: Option<InstallOutcome>,
    pub mcp: Option<McpOutcome>,
    pub skills: Vec<SkillOutcome>,
}

pub fn install_target(
    target: Target,
    scope: Scope,
    root: &Path,
    mcp_bin: &str,
) -> Result<TargetOutcome> {
    let rules = if let Some(platform) = target.rules_platform() {
        Some(install_platform(platform, scope, root)?)
    } else {
        None
    };

    let mcp = if let Some(client) = target.mcp_client() {
        Some(install_mcp(client, scope, root, mcp_bin)?)
    } else {
        None
    };

    let skills = if target.installs_claude_skills() {
        install_claude_skill()?
    } else {
        Vec::new()
    };

    Ok(TargetOutcome {
        target,
        rules,
        mcp,
        skills,
    })
}

pub struct TargetUninstallOutcome {
    pub target: Target,
    pub rules: Option<UninstallOutcome>,
    pub mcp: Option<McpOutcome>,
    pub skills: Vec<SkillOutcome>,
}

pub fn uninstall_target(
    target: Target,
    scope: Scope,
    root: &Path,
) -> Result<TargetUninstallOutcome> {
    let rules = if let Some(platform) = target.rules_platform() {
        Some(uninstall_platform(platform, scope, root)?)
    } else {
        None
    };

    let mcp = if let Some(client) = target.mcp_client() {
        Some(uninstall_mcp(client, scope, root)?)
    } else {
        None
    };

    let skills = if target.installs_claude_skills() {
        uninstall_claude_skill()?
    } else {
        Vec::new()
    };

    Ok(TargetUninstallOutcome {
        target,
        rules,
        mcp,
        skills,
    })
}

/// What `grafly list` shows for one target — paths where each surface is
/// currently installed, or `None` if the surface is absent.
pub struct TargetListing {
    pub target: Target,
    pub rules_path: Option<PathBuf>,
    pub mcp_path: Option<PathBuf>,
}

/// List every supported target with the install state of each of its
/// surfaces. `None` for a surface means "this target supports it but it's
/// not currently installed" *or* "this target doesn't support it at all" —
/// the CLI prints "-" for both, since the difference doesn't matter to the
/// user.
pub fn list_targets(scope: Scope, root: &Path) -> Result<Vec<TargetListing>> {
    let mut out = Vec::with_capacity(Target::all().len());
    for &target in Target::all() {
        let rules_path = target
            .rules_platform()
            .and_then(|p| list_marker_path(p, scope, root));
        let mcp_path = target
            .mcp_client()
            .and_then(|c| list_marker_mcp_path(c, scope, root));
        out.push(TargetListing {
            target,
            rules_path,
            mcp_path,
        });
    }
    Ok(out)
}
