# Security Policy

## Reporting a Vulnerability

If you find a security issue in grafly — for example, a parser crash on malicious input, a path-traversal in the install/uninstall flow, or anything else that could compromise a user — **please do not file a public issue.**

Email the maintainer directly: **me@gianlucaciocci.com**

Include:
- A description of the issue
- Steps to reproduce (or a proof-of-concept)
- The version of grafly affected (`grafly --version`)
- Your assessment of impact

You should receive an acknowledgement within **5 business days**. We'll work with you on a fix and disclosure timeline. Once a fix lands, we'll credit you in the release notes unless you'd prefer to remain anonymous.

## Supported Versions

Grafly is pre-1.0; only the latest published version on crates.io receives security fixes.

## Scope

In scope:
- The `grafly`, `grafly-cli`, and `grafly-mcp` binaries and library crates
- Tree-sitter parsing of untrusted source files
- The `grafly install` / `grafly uninstall` flows (which modify user config files)
- The MCP server's tool-call surface

Out of scope:
- Issues in upstream dependencies (`tree-sitter-*`, `leiden-rs`, `rmcp`, etc.) — please report those to the respective projects
- Performance issues that aren't denial-of-service
- Output quality concerns (file a regular issue for those)
