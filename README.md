# codex-app-server-client-cli

Rust CLI for talking to `codex app-server` with JSON-first, automation-friendly output.

Current status: v1 command surface is implemented for the shipped WebSocket flow. The repo no longer reflects the original scaffold-only state.

## What v1 currently ships

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

Streaming is available through:
- `run --watch`
- `resume --watch`

The hidden `threads` and `turns` commands are test/debug plumbing and are not part of the supported operator-facing v1 contract.

## What this README does not promise

These surfaces are intentionally not documented as supported v1 features because they are not implemented as stable operator commands in the current binary:
- a standalone `watch` subcommand
- `policy inspect`
- `session resume`
- `session yolo enable` / `session yolo disable`
- a custom human text renderer beyond pretty-printed JSON
- non-WebSocket transport execution (`stdio`, `unix`, and `off` still parse as flags but return unsupported transport errors in v1)
- package-manager distribution channels such as Homebrew, apt, npm, or Docker images

## Build and install

Build from source:

```bash
cargo build
```

Install the current crate as a local binary:

```bash
cargo install --path .
```

Binary name:

```text
codex-app-server-client-cli
```

## Config

Default config file path is resolved via Rust `dirs::config_dir()`:
- macOS: `~/Library/Application Support/codex-app-server-client-cli/config.toml`
- Linux: `${XDG_CONFIG_HOME:-~/.config}/codex-app-server-client-cli/config.toml`

Current config shape:

```toml
[connection]
transport = "ws"
url = "ws://127.0.0.1:4500"
bearer_token = ""
connect_timeout_ms = 10000
request_timeout_ms = 60000

[session]
model = ""
reasoning_effort = ""
approval_policy = "on-request"
sandbox = "workspace-write"
cwd = ""

[output]
default_format = "json"
```

Environment overrides:
- `CODEX_APP_SERVER_CONFIG`
- `CODEX_APP_SERVER_TRANSPORT`
- `CODEX_APP_SERVER_URL`
- `CODEX_APP_SERVER_BEARER_TOKEN`
- `CODEX_APP_SERVER_CWD`
- `CODEX_APP_SERVER_MODEL`
- `CODEX_APP_SERVER_OUTPUT`

Note: the parser accepts `ws`, `stdio`, `unix`, and `off`, but only `ws` is currently executable in v1.

## Common flows

### Run a prompt

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run "summarize the workspace"
```

Final results default to one JSON object on stdout. A typical success envelope looks like:

```json
{
  "ok": true,
  "command": "run",
  "session": {
    "id": "sess_cli",
    "alias": "feature-auth",
    "repo_root": "/repo",
    "workspace_root": "/tmp/repo",
    "ephemeral": false,
    "yolo": false,
    "last_active_at": null
  },
  "data": {
    "turn_id": "turn_cli",
    "output": {
      "summary": "cli completed"
    }
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

### Reuse or select a session explicitly

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run --session feature-auth "continue the plan"
codex-app-server-client-cli --url ws://127.0.0.1:4500 resume feature-auth "continue the plan"
```

`run` prefers a repo-bound default session when `--cwd` resolves inside a repository. If no workspace match exists, it creates a new reusable session.

### Create a history-clean ephemeral session

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 --cwd /repo/subdir run --ephemeral "scratch this change"
```

Ephemeral sessions keep workspace identity while dropping prior conversational history.

### Stream JSONL events during execution

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run --watch "summarize the workspace"
```

Current watch output is JSON Lines in arrival order. Example:

```json
{"type":"turn.started","sequence":1,"protocol_method":"turn/started","data":{"threadId":"thread-1","turnId":"turn-1"},"thread_id":"thread-1","turn_id":"turn-1"}
{"type":"item.agent_message.delta","sequence":2,"protocol_method":"item/agentMessage/delta","data":{"itemId":"item-1","delta":"Hello"},"item_id":"item-1","delta":"Hello"}
{"type":"turn.completed","sequence":3,"protocol_method":"turn/completed","data":{"threadId":"thread-1","turnId":"turn-1","status":"completed"},"thread_id":"thread-1","turn_id":"turn-1"}
```

There is no standalone documented `watch` subcommand in the current v1 binary. Use `--watch` on `run` or `resume`.

### Non-interactive approval flow

When the server asks for approval and stdio is not interactive, the CLI writes an approval-required envelope to stdout and exits with code `7`:

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run "needs approval"
echo $?
```

Example envelope:

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

Inspect and resolve pending approvals:

```bash
codex-app-server-client-cli approval list
codex-app-server-client-cli approval show --id approval-1
codex-app-server-client-cli approve --id approval-1
# or
codex-app-server-client-cli approval approve --token sess_new:approval-1
```

`approve` resumes by default. Use `--no-resume` when you want to mark an approval approved in the local store without immediately resuming the blocked step:

```bash
codex-app-server-client-cli approval approve --id approval-1 --no-resume
```

To deny a request:

```bash
codex-app-server-client-cli deny --token sess_new:approval-1
# or
codex-app-server-client-cli approval deny --id approval-1
```

### Interactive approval behavior

If both stdin and stdout are terminals and the request is not auto-approved by policy/YOLO evaluation, the CLI prompts inline:

```text
Approval required
  scope: command_execution
  session: sess_new
  summary: Run npm test
  action: ["npm","test"]
  risk_traits: shell_exec
Approve and resume from the blocked step? [y/N]
```

`y`/`yes` approves and resumes from the blocked step. Any other answer denies the request and the command exits non-zero.

### Session and operator inspection commands

```bash
codex-app-server-client-cli session list
codex-app-server-client-cli session show --alias feature-auth
codex-app-server-client-cli session fork --id sess_123
codex-app-server-client-cli models list
codex-app-server-client-cli fs ls --path /repo/src
codex-app-server-client-cli fs cat --path /repo/src/main.rs
codex-app-server-client-cli health
```

## Output notes

- Default output format is JSON.
- `--pretty` pretty-prints final JSON envelopes.
- `--output text` currently renders the same envelope as pretty JSON; it is not a separate human-oriented renderer yet.
- `--output jsonl` on non-streaming commands still produces a single JSON object line.
- `--watch` output is JSONL event output regardless of the final-envelope format setting.
- stderr is reserved for logs, tracing, and human diagnostics rather than machine-readable results.

## Local checks

Run the baseline local checks:

```bash
./scripts/check.sh
```

The script runs:
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

## Docs

- `docs/v1-cli-spec.md` — current implemented v1 contract
- `docs/v1-operator-notes.md` — operator guidance for JSON-first automation, approvals, and release scope
- `docs/v1-architecture.md` — protocol/background note and longer-term context
