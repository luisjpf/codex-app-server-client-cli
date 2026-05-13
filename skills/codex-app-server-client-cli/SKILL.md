---
name: codex-app-server-client-cli
description: Use when an agent needs to call a local `codex app-server` through this JSON-first Rust CLI, including one-shot runs, resumable sessions, watch-mode JSONL streaming, approval handling, and operator inspection commands.
---

# Codex App-Server Client CLI

Use this skill when you need to operate `codex app-server` through the `codex-app-server-client-cli` binary.

## Install

Install the CLI from the public GitHub repo:

```bash
cargo install --git https://github.com/luisjpf/codex-app-server-client-cli
```

Confirm the binary is available:

```bash
codex-app-server-client-cli --help
```

The default app-server endpoint is:

```text
ws://127.0.0.1:4500
```

## First Checks

Prefer a health check before running a task:

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 health
```

If the server is on the default URL, `--url` can be omitted when config/env already points there.

## Core Commands

Run a prompt and read the final JSON envelope from stdout:

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run "summarize the workspace"
```

Reuse a known session or alias:

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 resume feature-auth "continue the plan"
```

Stream JSONL events during execution:

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run --watch "summarize the workspace"
```

Inspect server state:

```bash
codex-app-server-client-cli health
codex-app-server-client-cli models list
codex-app-server-client-cli session list
codex-app-server-client-cli approval list
```

## Output Contract

- stdout is machine-readable JSON for final envelopes.
- `run --watch` and `resume --watch` emit JSONL event streams.
- stderr is for diagnostics, logging, and human-readable errors.
- `--pretty` pretty-prints final JSON envelopes.
- `--output text` currently renders pretty JSON, not a separate human UI.

For schema examples, load `references/quickstart.md`.

## Approval Handling

If a non-interactive run exits with status `7`, treat that as `approval_required`.

Inspect stdout for:
- `approval.approval_id`
- `approval.resume_token`
- `approval.summary`
- `approval.requested_action`

Then resolve explicitly:

```bash
codex-app-server-client-cli approval show --id approval-1
codex-app-server-client-cli approval approve --token sess_new:approval-1
codex-app-server-client-cli approval deny --token sess_new:approval-1
```

For the detailed approval workflow, load `references/approvals.md`.

## Safety

- Prefer loopback app-server URLs.
- Do not send bearer tokens over untrusted plain `ws://` network paths.
- Treat pending approval storage as sensitive because it may include command and path details.
- Only WebSocket transport is executable in v1; `stdio`, `unix`, and `off` parse but return unsupported transport errors.

## References

- `references/quickstart.md` for installation, health checks, command examples, config, and output shape.
- `references/approvals.md` for interactive and non-interactive approval workflows.
- `references/examples.md` for shell snippets an agent can adapt.
