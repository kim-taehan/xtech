# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust workspace containing multiple crates under `codex-rs/`. Key directories:

- **`core/`** - Business logic for Codex (the `codex-core` crate)
- **`tui/`** - Fullscreen terminal UI built with Ratatui
- **`cli/`** - Multi-subcommand CLI tool
- **`exec/`** - Headless CLI for automation
- **`app-server/`**, **`app-server-protocol/`** - Server infrastructure and protocol definitions
- **`utils/`** - Shared utility crates
- **`docs/`** - User documentation

Each crate follows the pattern `codex-*` (e.g., `codex-core`, `codex-tui`).

## Build, Test, and Development Commands

Use `just` commands from the `codex-rs/` directory:

```bash
# Format code
just fmt

# Fix lint issues (scoped to project)
just fix -p <crate-name>

# Run tests (uses nextest if available)
just test

# Run clippy
just clippy

# Update Bazel lockfile after Cargo changes
just bazel-lock-update

# Check Bazel lockfile
just bazel-lock-check
```

For Bazel builds:
```bash
bazel run //codex-rs/cli:codex -- --help
bazel test //...
```

## Coding Style & Naming Conventions

- **Formatting**: `rustfmt` with `imports_granularity = "Item"`
- **Clippy rules**: Follow all clippy warnings; use `just fix` to auto-fix
- **Inline format args**: Always use `format!("{var}")` instead of `format!("{}", var)`
- **Collapse if statements**: Prefer `if a && b { ... }` over nested `if` statements
- **Method references**: Use `.map(Self::method)` instead of `.map(|x| x.method())`
- **Boolean/Option parameters**: Avoid `foo(false)`; use enums, named methods, or newtypes instead
- **Literal arguments**: Use `/*param_name*/` comments for opaque positional literals (e.g., `foo(/*timeout=*/ None)`)

## Testing Guidelines

- **Framework**: Standard Rust `#[test]` with `pretty_assertions::assert_eq!`
- **Snapshot tests**: Use `cargo-insta` for UI rendering tests in `codex-tui`
  ```bash
  cargo test -p codex-tui
  cargo insta pending-snapshots -p codex-tui
  cargo insta accept -p codex-tui
  ```
- **Prefer deep equality**: Compare entire objects rather than individual fields
- **Environment**: Avoid mutating process environment; pass dependencies explicitly

## Commit & Pull Request Guidelines

- **Commits**: Use imperative mood ("Add feature" not "Added feature")
- **PRs**: Include clear description of changes and linked issues
- **Bazel lockfile**: Include `MODULE.bazel.lock` updates when changing Rust dependencies
- **Tests**: Ensure relevant tests pass before submitting
- **Large changes**: Run `just fix -p <crate>` before finalizing

## API Development (App-Server Protocol)

- Active API work happens in `v2` module; do not add new surface to `v1`
- Naming: `*Params` for requests, `*Response` for responses, `*Notification` for notifications
- Use `#[serde(rename_all = "camelCase")]` for wire format (except config payloads which use snake_case)
- Export TypeScript types with `#[ts(export_to = "v2/")]`
- Cursor pagination: `cursor`/`limit` on requests, `next_cursor` on responses

## Sandbox & Security Notes

- Never add code referencing `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR`
- These are set automatically by the sandbox environment for tests that need to detect restrictions
