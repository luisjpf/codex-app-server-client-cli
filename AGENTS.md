# Agent Instructions

This repository ships an installable agent skill for using `codex-app-server-client-cli` from coding agents.

## Agent Skill

The skill lives at:

```text
skills/codex-app-server-client-cli/SKILL.md
```

Install it with GitHub CLI:

```bash
gh skill install luisjpf/codex-app-server-client-cli codex-app-server-client-cli --agent codex --scope user
gh skill install luisjpf/codex-app-server-client-cli codex-app-server-client-cli --agent claude-code --scope user
gh skill install luisjpf/codex-app-server-client-cli codex-app-server-client-cli --agent openclaw --scope user
```

The skill teaches agents how to install and operate the CLI against a local `codex app-server` WebSocket endpoint.

## Development

Use the existing project checks before proposing or committing changes:

```bash
./scripts/check.sh
```

This runs:
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

Keep user-facing command documentation aligned across `README.md`, `docs/v1-cli-spec.md`, and `docs/v1-operator-notes.md` when command behavior changes.

## Scope Notes

Only WebSocket transport is executable in v1. The parser accepts other transport flags, but they intentionally return unsupported transport errors.

Do not document hidden `threads` or `turns` commands as supported operator-facing commands.
