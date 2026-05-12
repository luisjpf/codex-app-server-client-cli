# Codex app-server client CLI v1 product spec

Status: implemented v1 contract for the current binary
Updated: 2026-05-12

> This document is the operator-facing v1 contract for what the Rust CLI actually ships today.
> `docs/v1-architecture.md` remains the protocol/background reference.

## Goal

Build a Rust CLI for `codex app-server` that is:
- automation-first
- safe-by-default but not friction-heavy
- JSON-first for scripts and Hermes
- usable from an interactive terminal when approvals need human input

## Non-goals for this shipped v1

- a full TUI or long-lived shell experience
- broad support for every app-server method on day one
- a standalone documented `watch` verb
- policy editing/inspection commands
- session-scoped YOLO mutation commands
- non-WebSocket transport execution
- package-manager distribution channels beyond the Cargo package and binary
- a bespoke text renderer separate from JSON envelopes

## Locked v1 product decisions

### 1. Command surface

The shipped v1 command surface is hybrid:

High-level verbs:
- `run`
- `resume`
- `approve`
- `deny`

Operator/resource commands:
- `health`
- `models list`
- `session list`
- `session show`
- `session fork`
- `approval list`
- `approval show`
- `approval approve`
- `approval deny`
- `fs ls`
- `fs cat`

Internal `threads` / `turns` commands exist for tests and protocol plumbing but are not part of the supported operator contract.

### 2. Transport and output contract

Use request/response first, with explicit watch/stream mode.

Defaults:
- final command result is a structured JSON object on stdout
- streaming is optional and explicit
- stderr is reserved for logs/tracing/human diagnostics

Current output behavior:
- `run` and `resume` normally emit one final JSON envelope
- `run --watch` and `resume --watch` emit JSON Lines event envelopes
- global output selection is `--output json|jsonl|text`
- `--output text` currently pretty-prints the same JSON envelope instead of using a separate human renderer
- `--output jsonl` on non-streaming commands still emits a single JSON object line

Current transport behavior:
- `ws` is the only executable transport in v1
- `stdio`, `unix`, and `off` are accepted by argument parsing but return unsupported transport errors when invoked

### 3. Session model

Support both reusable and ephemeral sessions.

Session identity is first-class in two forms:
- human-friendly alias/name
- opaque session ID

Examples:
- `resume feature-auth "continue the plan"`
- `session show --id sess_123`

Default selection behavior:
- prefer a repo-aware workspace-scoped default session
- otherwise create a new session

Workspace binding rule:
- bind by project/repo root, not current subdirectory

Ephemeral semantics inside a repo:
- keep workspace identity
- do not inherit prior conversational/task history
- i.e. project-aware, history-clean

### 4. Approval and policy model

The CLI is automation-first with policy gating.

TTY approval UX:
- if stdin and stdout are interactive terminals, prompt inline when approval is required and not auto-approved by policy/YOLO evaluation
- prompt answer `y`/`yes` approves and resumes from the blocked step
- any other answer denies the request and the command exits non-zero

Non-interactive approval UX:
- return a structured approval-required JSON envelope on stdout
- exit with the dedicated approval-required exit code `7`
- include both a stable `approval_id` and a resumable `resume_token`
- persist the pending approval in the local approval store for later `approve` / `deny` commands

Resume semantics after approval:
- default behavior is resume from the blocked step, not rerun from scratch

Underlying model:
- interactive prompting and non-interactive automation both map to the same approval object lifecycle

### 5. YOLO mode

YOLO is in scope for v1, but only through the implemented surfaces.

Implemented forms:
- per-command: `run --yolo ...`
- per-command: `resume --yolo ...`
- per-command disable override: `--no-yolo`
- session-scoped YOLO visibility when the server reports a session as YOLO-enabled

Important limitation:
- the current binary does not ship `session yolo enable` / `session yolo disable`
- docs should describe session-scoped YOLO as observable state, not as a mutable operator command

Default behavior:
- normal mode remains policy-gated
- YOLO is an explicit override or inherited session state

### 6. Approval objects and lifecycle

Approval handling revolves around a stable approval object.

Lifecycle states currently surfaced by the CLI:
- `pending`
- `approved`
- `denied`
- `expired`
- `cancelled`
- `resumed`

Minimum approval object fields in the current envelope:
- `approval_id`
- `session_id`
- `scope`
- `risk_traits`
- `summary`
- `requested_action`
- `requested_at`
- `resume_token`
- `status`

Additional currently surfaced fields:
- `raw_method`
- `request_id`
- `item_id`
- `data`

## Command-level v1 UX

### `run`

Primary high-level command.

Responsibilities:
- select or create a session
- send task input
- optionally stream progress
- return final JSON
- surface approval requests deterministically

Implemented flags:
- `--session <alias-or-id>`
- `--ephemeral`
- `--watch`
- `--yolo`
- `--no-yolo`
- `--cwd <path>`
- `--model <name>`
- `--approval-policy <policy>`
- `--sandbox <mode>`
- global `--output <json|jsonl|text>`
- global `--pretty`

Default behavior:
- in a repo, prefer the repo-bound session
- if none exists, create one
- if `--ephemeral`, create a history-clean session bound to the same workspace identity

### `resume`

Primary command for continuing a reusable session.

Responsibilities:
- resolve by alias or ID
- continue the session with new input, or resume a blocked step from a pending approval token

Arguments and flags:
- positional `SESSION`
- optional positional `INPUT`
- `--watch`
- `--yolo`
- `--no-yolo`
- `--cwd <path>`
- `--model <name>`
- `--approval-policy <policy>`
- `--sandbox <mode>`
- global `--output <json|jsonl|text>`
- global `--pretty`

Special case:
- if `SESSION` resolves to a pending approval `resume_token`, `INPUT` may be omitted and the CLI resumes the blocked step directly

### Watch/streaming behavior

The current v1 binary does not expose a standalone supported `watch` command.

Supported streaming entry points:
- `run --watch`
- `resume --watch`

Contract:
- emit one JSON object per line
- preserve arrival order
- keep a stable normalized `type`
- include `protocol_method` and normalized helper fields such as `thread_id`, `turn_id`, `item_id`, or `delta` when available

### `approve` / `deny`

Approval control commands for non-interactive or split workflows.

Responsibilities:
- resolve a pending approval by `approval_id` or `resume_token`
- optionally trigger automatic resumption

Current behavior:
- `approve` resumes automatically unless `--no-resume` is specified
- `approve --no-resume` marks the approval approved in the local store and returns a final envelope with `resumed: false`
- `deny` sends a denied approval response and clears the pending approval from the local store

Both top-level and resource-style forms are supported:
- `approve --id ...`
- `deny --token ...`
- `approval approve --id ...`
- `approval deny --token ...`

### `session ...`

Supported operator subcommands:
- `session list`
- `session show`
- `session fork`

Not part of shipped v1:
- `session resume`
- `session yolo enable`
- `session yolo disable`

### `approval ...`

Supported operator subcommands:
- `approval list`
- `approval show`
- `approval approve`
- `approval deny`

### Other supported resource commands

- `models list`
- `fs ls`
- `fs cat`
- `health`

## Result schema conventions

### Non-streaming success envelope

```json
{
  "ok": true,
  "command": "run",
  "session": {
    "alias": "feature-auth",
    "ephemeral": false,
    "id": "sess_cli",
    "last_active_at": null,
    "repo_root": "/repo",
    "workspace_root": "/tmp/repo",
    "yolo": false
  },
  "data": {
    "output": {
      "summary": "cli completed"
    },
    "turn_id": "turn_cli"
  },
  "meta": {
    "session_selection": {
      "lifecycle": "created",
      "reason": "no_workspace_match"
    },
    "policy": {
      "last_approval": null,
      "yolo": {
        "effective": false,
        "session_enabled": false,
        "source": "default_policy"
      }
    },
    "server": {
      "handshake_complete": true,
      "transport_open": true
    }
  }
}
```

### Approval-required envelope

```json
{
  "ok": false,
  "command": "run",
  "error": {
    "code": "approval_required",
    "message": "run: server requested approval before execution can continue"
  },
  "approval": {
    "approval_id": "approval-1",
    "session_id": "sess_new",
    "scope": "command_execution",
    "risk_traits": ["shell_exec"],
    "summary": "Run npm test",
    "requested_action": "[\"npm\",\"test\"]",
    "requested_at": "2026-05-11T20:15:00Z",
    "resume_token": "sess_new:approval-1",
    "status": "pending"
  }
}
```

### Streaming event envelope

```json
{"type":"turn.started","sequence":1,"protocol_method":"turn/started","data":{"threadId":"thread-1","turnId":"turn-1"},"thread_id":"thread-1","turn_id":"turn-1"}
{"type":"item.agent_message.delta","sequence":2,"protocol_method":"item/agentMessage/delta","data":{"itemId":"item-1","delta":"Hello"},"item_id":"item-1","delta":"Hello"}
{"type":"turn.completed","sequence":3,"protocol_method":"turn/completed","data":{"threadId":"thread-1","turnId":"turn-1","status":"completed"},"thread_id":"thread-1","turn_id":"turn-1"}
```

## Exit-code expectations

The operator-facing guarantees worth documenting in this shipped v1 are:
- success exits `0`
- approval-required exits `7`
- machine consumers should not need stderr scraping to detect the approval-required state

Other non-zero exits remain typed and stable inside the implementation, but only the approval-required path is called out here as part of the external operator contract.

## Docs and release scope

The documented release subset for v1 is:
- one Cargo package
- one installable binary: `codex-app-server-client-cli`
- WebSocket transport execution
- JSON-first final envelopes
- JSONL watch mode through `run --watch` / `resume --watch`
- resumable approval objects with local pending-approval persistence

For operator playbooks and packaging/release notes, see `docs/v1-operator-notes.md`.
