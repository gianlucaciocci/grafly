# Contributing to grafly

Thanks for your interest in contributing. This document covers how to set up, what's expected from a PR, and where to file issues.

## Quick start

Grafly is a Rust workspace. `rust-toolchain.toml` pins the channel to `stable` — rustup will auto-install it. In practice the project currently requires Rust **≥ 1.88** because of a transitive `darling` dependency; we don't pin a strict MSRV until we're testing one.

```bash
git clone https://github.com/gianlucaciocci/grafly.git
cd grafly
cargo build --workspace
cargo test --workspace
```

To run the CLI against the repo itself:

```bash
cargo run --release -p grafly-cli -- analyze .
# output lands in ./grafly-out/
```

## Workflow

1. **Find an issue.** All planned work lives in [GitHub Issues](https://github.com/gianlucaciocci/grafly/issues). Look for `good first issue` if you're new. If you want to work on something that isn't tracked, open an issue first so we can discuss scope.
2. **Branch from `main`.** Use a short descriptive branch name (e.g. `path-aware-imports`, `mcp-watch-tool`).
3. **Code.** Keep changes focused — one logical change per PR. Match the surrounding style; the workspace uses `rustfmt` defaults.
4. **Test + lint locally.**
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo build --release --workspace
   ```
5. **Open a PR.** Reference the issue in the description with `Closes #N`. CI must be green before merge.

## What we look for in a PR

- **Small and focused.** A bugfix shouldn't include unrelated refactors.
- **Tests where they make sense.** Scanner changes especially benefit from a fixture test.
- **Updated docs.** If you change a user-facing surface, update `README.md` and any relevant section of `CLAUDE.md`.
- **No `unsafe` without justification.** The project is `#![forbid(unsafe_code)]`-friendly.

## Architectural decisions

The project uses an **Ubiquitous Language** (see `CLAUDE.md` § "Ubiquitous Language") — artifacts, dependencies, modules, hotspots, couplings, insights. Avoid generic graph terminology (node/edge/cluster) in user-facing surfaces; use the domain terms.

Cross-cutting design rationale (benchmark numbers, phase sequencing, weighted-path constraints) lives in the project memory at `C:\Users\gianl\.claude\projects\c--Users-gianl-workspace-grafly\memory\future_improvements.md` — issues link to it where relevant.

## Code of Conduct

This project follows the [Contributor Covenant](https://www.contributor-covenant.org/). Be kind, assume good faith, and report any unacceptable behaviour to the maintainer (see `SECURITY.md` for contact).

## License

By contributing, you agree that your contributions will be licensed under the MIT License (see `LICENSE`).
