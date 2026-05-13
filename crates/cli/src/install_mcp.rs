//! Register the `grafly-mcp` server in MCP clients' configuration files
//! (Claude Code, Claude Desktop, Cursor, Windsurf, VS Code). Internal helper
//! module — driven by `target::install_target` when the chosen target
//! supports an MCP surface. Not exposed as its own CLI subcommand.
//!
//! Each client expects a JSON configuration file with a server registry under
//! a key like `mcpServers` or `servers`. We merge into the existing file
//! preserving any other servers the user has configured.

use anyhow::{anyhow, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::install::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpClient {
    /// Anthropic Claude Code CLI / IDE extension — project `.mcp.json` or `~/.claude.json`
    ClaudeCode,
    /// Claude Desktop app — global `claude_desktop_config.json`
    ClaudeDesktop,
    /// Cursor IDE — `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global)
    Cursor,
    /// Windsurf — `~/.codeium/windsurf/mcp_config.json` (global only)
    Windsurf,
    /// VS Code — `.vscode/mcp.json` (project only)
    Vscode,
}

impl McpClient {
    fn supports(&self, scope: Scope) -> bool {
        match (self, scope) {
            (McpClient::ClaudeCode, _) => true,
            (McpClient::ClaudeDesktop, Scope::Global) => true,
            (McpClient::ClaudeDesktop, Scope::Project) => false,
            (McpClient::Cursor, _) => true,
            (McpClient::Windsurf, Scope::Global) => true,
            (McpClient::Windsurf, Scope::Project) => false,
            (McpClient::Vscode, Scope::Project) => true,
            (McpClient::Vscode, Scope::Global) => false,
        }
    }

    fn default_scope(&self) -> Scope {
        match self {
            McpClient::ClaudeDesktop | McpClient::Windsurf => Scope::Global,
            _ => Scope::Project,
        }
    }
}

// ── Config location + JSON shape per client ─────────────────────────────────

/// Where the server registry lives inside the JSON file. Most clients use
/// `mcpServers` at the document root; VS Code uses `servers`.
fn servers_key(client: McpClient) -> &'static str {
    match client {
        McpClient::Vscode => "servers",
        _ => "mcpServers",
    }
}

fn config_path(client: McpClient, scope: Scope, root: &Path) -> Result<PathBuf> {
    let scope = if client.supports(scope) {
        scope
    } else {
        // Silently fall back to the only supported scope rather than failing
        // when the user passes `--all`.
        client.default_scope()
    };

    match (client, scope) {
        (McpClient::ClaudeCode, Scope::Project) => Ok(root.join(".mcp.json")),
        (McpClient::ClaudeCode, Scope::Global) => Ok(home_dir()?.join(".claude.json")),

        (McpClient::ClaudeDesktop, _) => claude_desktop_config_path(),

        (McpClient::Cursor, Scope::Project) => Ok(root.join(".cursor").join("mcp.json")),
        (McpClient::Cursor, Scope::Global) => Ok(home_dir()?.join(".cursor").join("mcp.json")),

        (McpClient::Windsurf, _) => Ok(home_dir()?
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json")),

        (McpClient::Vscode, _) => Ok(root.join(".vscode").join("mcp.json")),
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .context("could not resolve home directory (set USERPROFILE or HOME)")
}

#[cfg(target_os = "windows")]
fn claude_desktop_config_path() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| home_dir().ok().map(|h| h.join("AppData").join("Roaming")))
        .context("could not resolve APPDATA")?;
    Ok(appdata.join("Claude").join("claude_desktop_config.json"))
}

#[cfg(target_os = "macos")]
fn claude_desktop_config_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join("Library")
        .join("Application Support")
        .join("Claude")
        .join("claude_desktop_config.json"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn claude_desktop_config_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".config")
        .join("Claude")
        .join("claude_desktop_config.json"))
}

// ── Server entry construction ───────────────────────────────────────────────

const SERVER_NAME: &str = "grafly-mcp";

/// Build the JSON object that represents grafly-mcp in a client's registry.
fn server_entry(bin: &str) -> Value {
    json!({
        "command": bin,
        "args": [],
    })
}

/// OS-appropriate bare binary name. `.exe` is required on Windows because
/// many MCP clients spawn via Node's `child_process.spawn`, which doesn't
/// consult `PATHEXT` the way `cmd`/`powershell` do.
fn bare_bin_name() -> &'static str {
    if cfg!(windows) { "grafly-mcp.exe" } else { "grafly-mcp" }
}

/// Determine the command grafly should write into a client's MCP registry.
///
/// Preference order:
/// 1. The bare binary name (e.g. `grafly-mcp` on Linux/macOS, `grafly-mcp.exe`
///    on Windows) — when the binary is discoverable on `PATH`. Portable
///    across machines so a committed `.mcp.json` keeps working everywhere.
/// 2. The absolute path to a `grafly-mcp` sibling of the currently running
///    executable — for developers running out of `target/{debug,release}/`
///    when the bare name isn't on PATH.
/// 3. The bare name regardless — degenerate fallback so we always emit
///    *something* runnable rather than nothing.
pub fn default_mcp_bin() -> String {
    let bare = bare_bin_name();

    // Prefer the bare name if it's on PATH. Avoids baking a machine-specific
    // absolute path into a `.mcp.json` that might get checked into git.
    if which_on_path(bare).is_some() {
        return bare.to_string();
    }

    // Fall back to the sibling of the current executable (dev workflow).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(bare);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    // Last resort: emit the bare name and hope the user wires PATH later.
    bare.to_string()
}

/// Minimal `which`-style lookup. Walks `PATH` looking for an executable file
/// with the given name. Doesn't recurse — `PATH` entries are searched in order.
/// Pulled in-tree to avoid taking on the `which` crate just for this.
fn which_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ── Install / uninstall ─────────────────────────────────────────────────────

#[derive(Debug)]
pub struct McpOutcome {
    // `client` is carried so callers can correlate the result back to which
    // client they asked for, even though the CLI currently prints by target.
    #[allow(dead_code)]
    pub client: McpClient,
    pub path: PathBuf,
    pub action: &'static str, // "created" | "updated" | "unchanged" | "removed" | "absent"
}

pub fn install_mcp(
    client: McpClient,
    scope: Scope,
    root: &Path,
    bin: &str,
) -> Result<McpOutcome> {
    let path = config_path(client, scope, root)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let existed = path.exists();
    let mut doc: Value = if existed {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if raw.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(&raw)
                .with_context(|| format!("parsing JSON in {}", path.display()))?
        }
    } else {
        Value::Object(Map::new())
    };

    if !doc.is_object() {
        return Err(anyhow!(
            "config at {} is not a JSON object",
            path.display()
        ));
    }

    let key = servers_key(client);
    let new_entry = server_entry(bin);

    let root_obj = doc.as_object_mut().unwrap();
    let registry = root_obj
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !registry.is_object() {
        return Err(anyhow!(
            "config at {} has non-object `{}`",
            path.display(),
            key
        ));
    }
    let registry_obj = registry.as_object_mut().unwrap();

    let existing_entry = registry_obj.get(SERVER_NAME).cloned();
    if existing_entry.as_ref() == Some(&new_entry) {
        return Ok(McpOutcome {
            client,
            path,
            action: "unchanged",
        });
    }

    registry_obj.insert(SERVER_NAME.to_string(), new_entry);

    let serialized = serde_json::to_string_pretty(&doc)?;
    fs::write(&path, format!("{}\n", serialized))
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(McpOutcome {
        client,
        path,
        action: if existed { "updated" } else { "created" },
    })
}

pub fn uninstall_mcp(client: McpClient, scope: Scope, root: &Path) -> Result<McpOutcome> {
    let path = config_path(client, scope, root)?;
    if !path.exists() {
        return Ok(McpOutcome {
            client,
            path,
            action: "absent",
        });
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(McpOutcome {
            client,
            path,
            action: "absent",
        });
    }

    let mut doc: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing JSON in {}", path.display()))?;

    if !doc.is_object() {
        return Ok(McpOutcome {
            client,
            path,
            action: "absent",
        });
    }

    let key = servers_key(client);
    let root_obj = doc.as_object_mut().unwrap();

    let removed = match root_obj.get_mut(key).and_then(|v| v.as_object_mut()) {
        Some(registry) => registry.remove(SERVER_NAME).is_some(),
        None => false,
    };

    if !removed {
        return Ok(McpOutcome {
            client,
            path,
            action: "absent",
        });
    }

    // Drop the registry key entirely if it's now empty.
    if root_obj
        .get(key)
        .and_then(|v| v.as_object())
        .is_some_and(|o| o.is_empty())
    {
        root_obj.remove(key);
    }

    // If the whole file is now an empty object, delete it (we created it).
    if root_obj.is_empty() {
        fs::remove_file(&path)
            .with_context(|| format!("removing {}", path.display()))?;
        return Ok(McpOutcome {
            client,
            path,
            action: "removed",
        });
    }

    let serialized = serde_json::to_string_pretty(&doc)?;
    fs::write(&path, format!("{}\n", serialized))
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(McpOutcome {
        client,
        path,
        action: "removed",
    })
}

fn has_grafly_entry(path: &Path, client: McpClient) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    doc.get(servers_key(client))
        .and_then(|v| v.get(SERVER_NAME))
        .is_some()
}

/// Return the config-file path where `grafly-mcp` is currently registered for
/// `client`, if the server entry is present. `None` for "not installed".
/// Used by `grafly list` so it can render one row per target.
pub fn list_marker_mcp_path(client: McpClient, scope: Scope, root: &Path) -> Option<PathBuf> {
    let path = config_path(client, scope, root).ok()?;
    if has_grafly_entry(&path, client) {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tmp_dir(suffix: &str) -> PathBuf {
        let mut p = env::temp_dir();
        p.push(format!("grafly-mcp-test-{}-{}", suffix, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn install_creates_file_when_absent() {
        let dir = tmp_dir("create");
        let outcome =
            install_mcp(McpClient::ClaudeCode, Scope::Project, &dir, "grafly-mcp").unwrap();
        assert_eq!(outcome.action, "created");
        let content: Value =
            serde_json::from_str(&fs::read_to_string(&outcome.path).unwrap()).unwrap();
        assert_eq!(
            content.pointer("/mcpServers/grafly-mcp/command"),
            Some(&Value::String("grafly-mcp".into()))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_preserves_other_servers() {
        let dir = tmp_dir("preserve");
        let path = dir.join(".mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other-mcp","args":[]}}}"#,
        )
        .unwrap();

        install_mcp(McpClient::ClaudeCode, Scope::Project, &dir, "grafly-mcp").unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.pointer("/mcpServers/other").is_some(), "other should survive");
        assert!(content.pointer("/mcpServers/grafly-mcp").is_some(), "grafly-mcp added");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_idempotent_when_already_correct() {
        let dir = tmp_dir("idem");
        install_mcp(McpClient::ClaudeCode, Scope::Project, &dir, "grafly-mcp").unwrap();
        let second =
            install_mcp(McpClient::ClaudeCode, Scope::Project, &dir, "grafly-mcp").unwrap();
        assert_eq!(second.action, "unchanged");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_removes_entry_and_preserves_others() {
        let dir = tmp_dir("uninstall");
        let path = dir.join(".mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other-mcp","args":[]},"grafly-mcp":{"command":"grafly-mcp","args":[]}}}"#,
        )
        .unwrap();

        let outcome = uninstall_mcp(McpClient::ClaudeCode, Scope::Project, &dir).unwrap();
        assert_eq!(outcome.action, "removed");

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.pointer("/mcpServers/grafly-mcp").is_none());
        assert!(content.pointer("/mcpServers/other").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_deletes_file_when_only_grafly_present() {
        let dir = tmp_dir("delete");
        install_mcp(McpClient::ClaudeCode, Scope::Project, &dir, "grafly-mcp").unwrap();
        let outcome = uninstall_mcp(McpClient::ClaudeCode, Scope::Project, &dir).unwrap();
        assert_eq!(outcome.action, "removed");
        assert!(!outcome.path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn vscode_uses_servers_key_not_mcp_servers() {
        let dir = tmp_dir("vscode");
        install_mcp(McpClient::Vscode, Scope::Project, &dir, "grafly-mcp").unwrap();
        let content: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(".vscode/mcp.json")).unwrap())
                .unwrap();
        assert!(content.get("servers").is_some(), "vscode uses `servers`");
        assert!(content.get("mcpServers").is_none(), "not `mcpServers`");
        let _ = fs::remove_dir_all(&dir);
    }
}
