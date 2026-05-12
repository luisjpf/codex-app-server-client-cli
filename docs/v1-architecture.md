# Codex app-server client CLI v1 architecture

Status: protocol/background note; not the authoritative operator contract
Updated: 2026-05-12

> For the currently shipped command surface, use `docs/v1-cli-spec.md`.
> This document keeps the protocol/design context and may describe app-server capabilities that the v1 CLI does not fully expose yet.

## Goal

Build a Rust CLI that can talk directly to `codex app-server` for one-shot, automation-friendly Hermes usage without inventing a second backend API.

Non-goals for v1:
- implementing the protocol in this document
- shipping a long-lived TUI
- wrapping every app-server method on day one
- solving internet-facing relay/security beyond documenting current constraints

## Evidence used for this design

This note is based on the protocol surface already demonstrated in three places:

1. `codex-manager-app/docs/codex-app-server-interface.md`
2. `codex-manager-app/app/src/main/java/com/mindysm/codexmanager/CodexRuntime.kt`
3. local `codex-cli 0.128.0` inspection via:
   - `codex app-server --help`
   - `codex app-server generate-json-schema`

That means this design is grounded in what the current app-server already exposes, not a speculative custom API.

## What we already know about the protocol

### Transport

Current app-server transport options from `codex app-server --help`:
- `stdio://` (default)
- `unix://` / `unix://PATH`
- `ws://IP:PORT`
- `off`

For the app-server protocol, the important transports are:
- `ws://...`
- `stdio://`
- `unix://...`
- `off`

For the current Rust CLI implementation, only WebSocket transport is executable in v1.
`stdio`, `unix`, and `off` remain background design context and accepted parser values, but invoking them currently returns an unsupported transport error.

For WebSocket listeners, the existing notes also confirm:
- `GET /readyz`
- `GET /healthz`

The app-server help additionally shows WebSocket auth support for non-loopback listeners:
- capability token mode
- signed bearer token mode

So the Rust client should assume WebSocket auth is part of the real protocol surface, even if the first implementation only needs simple bearer-header support to match the Android app.

### Required handshake

The current known handshake is:
1. open transport
2. send `initialize`
3. wait for `initialize` response
4. send `initialized`
5. only then issue normal requests

Known `initialize` request shape from schema:
- required `clientInfo.name`
- required `clientInfo.version`
- optional `clientInfo.title`
- optional `capabilities.experimentalApi`
- optional `capabilities.optOutNotificationMethods`

Known `initialize` response shape from schema:
- `codexHome`
- `platformFamily`
- `platformOs`
- `userAgent`

Implication: our client should persist handshake metadata in memory for the active session and expose it in JSON output for diagnostics when requested.

### Confirmed core request methods

Already used or documented in `codex-manager-app`:
- `thread/list`
- `thread/start`
- `thread/resume`
- `thread/read`
- `thread/fork`
- `thread/turns/list`
- `thread/name/set`
- `turn/start`
- `turn/steer`
- `turn/interrupt`
- `model/list`
- `fs/readDirectory`
- `fs/readFile`
- `fs/createDirectory`

Important near-term observations:
- `thread/list` supports filtering, sorting, cursors, archived state, cwd filters, provider filters, and source-kind filters.
- `thread/start` and `thread/resume` both accept cwd/approval/sandbox-related settings, which means connection-level defaults and per-command overrides matter.
- `thread/resume` and `thread/fork` have `excludeTurns`, which is useful when a client wants metadata first and then paged history via `thread/turns/list`.
- `turn/start` requires `threadId` and `input`, and can override model, reasoning effort, cwd, approval policy, sandbox policy, personality, service tier, and output schema.
- `model/list` returns rich metadata, not just ids: display name, default effort, supported efforts, hidden/default flags, and input modalities.
- `fs/readDirectory` returns child entry names plus `isDirectory` / `isFile` booleans.
- `fs/readFile` returns base64 payloads.

### Confirmed notifications/events

Already used by the Android client and present in schema:
- `thread/started`
- `turn/started`
- `turn/completed`
- `item/agentMessage/delta`
- `item/plan/delta`
- `item/fileChange/outputDelta`
- `item/fileChange/patchUpdated`
- `item/reasoning/summaryTextDelta`
- `item/reasoning/textDelta`
- `item/completed`
- `command/exec/outputDelta`
- `item/commandExecution/outputDelta`
- `thread/name/updated`
- `thread/status/changed`
- `error`

### Confirmed server->client approval requests

The schema and Android client both show server-initiated JSON-RPC requests for approvals:
- `item/commandExecution/requestApproval`
- `item/fileChange/requestApproval`
- `item/permissions/requestApproval`

This is important for CLI design:
- one-shot commands must not silently hang when approval is needed
- non-interactive Hermes usage needs a deterministic policy for approval requests
- approval events should become machine-readable output and a dedicated exit code

### Practical v1 protocol subset

The full generated schema is much larger than the first CLI needs. For v1, the protocol subset should be:

1. Session bootstrap
   - `initialize`
   - `initialized`
   - `model/list`

2. Thread discovery/lifecycle
   - `thread/list`
   - `thread/start`
   - `thread/resume`
   - `thread/read`
   - `thread/fork`
   - `thread/turns/list`

3. Prompt execution
   - `turn/start`
   - `turn/steer`
   - `turn/interrupt`

4. Workspace browsing
   - `fs/readDirectory`
   - `fs/readFile`

5. Streaming/event handling
   - the delta/completion/error notifications listed above

6. Approval handling
   - detect and surface approval requests even if we do not yet implement full interactive approval workflows

Anything outside that subset can remain out-of-scope until the first CLI is stable.

## Crate layout decision

Decision: use a `lib + thin bin` layout from the start.

Recommended shape:

- `src/main.rs` - argument parsing, output dispatch, top-level error handling
- `src/lib.rs` - public entry points for command execution
- `src/cli.rs` - clap command/flag structs
- `src/config.rs` - config file/env/flag resolution
- `src/error.rs` - typed error model and exit-code mapping
- `src/output.rs` - JSON and text rendering
- `src/client/mod.rs` - transport-agnostic client facade
- `src/client/connection.rs` - connection lifecycle + handshake
- `src/client/ws.rs` - WebSocket transport
- `src/client/stdio.rs` - stdio transport/probe path
- `src/protocol/mod.rs` - protocol types used by the CLI
- `src/protocol/messages.rs` - JSON-RPC envelope structs
- `src/protocol/events.rs` - event/notification enum mapping
- `src/commands/mod.rs` - command dispatch
- `src/commands/health.rs`
- `src/commands/models.rs`
- `src/commands/threads.rs`
- `src/commands/turns.rs`
- `src/commands/fs.rs`

Why not a single binary-only file layout?
- the CLI will need reusable request/response types, connection management, and event handling very quickly
- a lib split makes it much easier to test protocol behavior without shelling out through the binary
- Hermes may eventually want this logic embedded in another local tool or reused by integration tests
- the extra structure cost is small now, but a late refactor would be noisy once streaming and approvals exist

Why not a multi-crate workspace yet?
- premature for v1
- there is only one deliverable binary right now
- a single package with internal modules keeps compile/test friction low

So the recommended compromise is: one Cargo package, one installable binary, internal library boundary.

## Module boundary rules

To keep the codebase automation-friendly, enforce these boundaries:

- `cli` knows clap shapes, but not raw WebSocket details.
- `commands` orchestrate one user-visible operation each, but do not parse JSON by hand.
- `client` owns connection state, request ids, send/receive loops, and handshake sequencing.
- `protocol` owns serde types for requests, responses, notifications, and server requests.
- `output` converts domain results/events into stdout payloads.
- `error` is the only place that maps failures to exit codes.
- `config` resolves defaults once up front; downstream modules receive a final resolved config object.

This keeps the command layer simple and makes JSON output consistent across commands.

## Config model

Decision: explicit connection config with three precedence layers.

Precedence order:
1. CLI flags
2. environment variables
3. config file under the user config directory

Use `dirs` for the config root. Proposed path:
- macOS/Linux: `${config_dir}/codex-app-server-client-cli/config.toml`

Initial config shape:

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
stream_format = "jsonl"
```

Suggested env vars:
- `CODEX_APP_SERVER_URL`
- `CODEX_APP_SERVER_BEARER_TOKEN`
- `CODEX_APP_SERVER_TRANSPORT`
- `CODEX_APP_SERVER_CWD`
- `CODEX_APP_SERVER_MODEL`
- `CODEX_APP_SERVER_OUTPUT`

Why this model fits Hermes:
- a one-shot CLI should be runnable with a single command and zero interactive setup
- env vars are easy for Hermes to inject per call
- config file provides stable local defaults for repeated usage
- per-command flags still allow override without editing disk state

## Initial command surface

The first binary should optimize for one-shot automation, not a shell-like REPL.

Recommended initial commands:

- `health`
  - connect, handshake, optionally hit `/healthz` or `/readyz` for ws URLs
  - return server metadata

- `models list`
  - wrap `model/list`

- `threads list`
  - wrap `thread/list`
  - expose filtering flags later without changing architecture

- `threads start --cwd <path>`
  - wrap `thread/start`

- `threads resume --thread-id <id>`
  - wrap `thread/resume`

- `threads read --thread-id <id>`
  - wrap `thread/read`

- `turns start --thread-id <id> --prompt <text>`
  - wrap `turn/start`
  - support non-streaming and streaming modes

- `turns interrupt --thread-id <id>`
  - wrap `turn/interrupt`

- `fs ls --path <path>`
  - wrap `fs/readDirectory`

- `fs cat --path <path>`
  - wrap `fs/readFile`

This is enough for Hermes to inspect server state, create/resume threads, and submit prompts.

## Dependency decisions

Recommended initial dependencies:

- `tokio`
  - yes
  - needed for async connection lifecycle, timers, and stream handling

- `tokio-tungstenite`
  - yes
  - primary WebSocket transport implementation

- `clap`
  - yes, with derive
  - clear command tree and stable automation ergonomics

- `serde`
  - yes, with derive
  - protocol types and config loading

- `serde_json`
  - yes
  - essential even though not listed in the task; JSON-RPC and JSON output both need it

- `anyhow`
  - yes, but only at the binary boundary
  - convenient top-level context in `main.rs`

- `thiserror`
  - yes
  - typed library/domain errors

- `tracing`
  - yes
  - structured diagnostics without polluting stdout

- `tracing-subscriber`
  - yes
  - log initialization and env-based filtering

- `dirs`
  - yes
  - config path resolution

Also likely needed immediately:
- `futures-util` for stream/sink helpers
- `reqwest` only if we want `health` to call `/healthz` and `/readyz` directly over HTTP instead of treating health as handshake-only
- `toml` for config parsing

Dependency policy:
- keep the runtime small
- prefer serde-first typed schemas over ad hoc `Value` traversal in most paths
- do not add a second CLI framework, state machine framework, or config framework unless a real pain point appears

## JSON output conventions

Decision: make JSON the default output for commands intended for Hermes automation.

### Non-streaming commands

Write exactly one JSON object to stdout on success.

Proposed envelope:

```json
{
  "ok": true,
  "command": "threads list",
  "data": {"threads": []},
  "meta": {
    "server": {
      "url": "ws://127.0.0.1:4500",
      "user_agent": "codex-cli 0.128.0"
    }
  }
}
```

On failure:

```json
{
  "ok": false,
  "command": "turns start",
  "error": {
    "code": "protocol.approval_required",
    "message": "Server requested approval for a command execution",
    "details": {
      "method": "item/commandExecution/requestApproval"
    }
  }
}
```

Rules:
- stdout is reserved for the final machine-readable payload
- tracing/logging goes to stderr only
- field names stay stable and snake_case
- `data` is command-specific but wrapped in a stable envelope
- `error.code` is a stable programmatic string, not copied raw from human text

### Streaming commands

For commands that watch turn progress, use JSON Lines on stdout.

Proposed event shapes:

```json
{"type":"turn.started","thread_id":"thr_123","turn_id":"turn_456"}
{"type":"item.agent_message.delta","item_id":"item_1","delta":"Hello"}
{"type":"item.plan.delta","item_id":"item_2","delta":"1. Inspect repo"}
{"type":"item.completed","item_id":"item_1","item_kind":"agent_message"}
{"type":"turn.completed","thread_id":"thr_123","turn_id":"turn_456"}
```

Rules:
- one JSON object per line
- preserve arrival order
- normalize raw protocol method names into a small stable client event taxonomy where useful
- include raw method names in a field like `protocol_method` if normalization could hide detail
- finish with a terminal event (`turn.completed`, `turn.interrupted`, or `error`)

### Text mode

Text mode can exist for humans, but should be explicitly requested with `--output text`.
Defaulting to JSON reduces ambiguity for Hermes.

## Exit-code policy

Decision: use a small fixed exit-code table and keep it stable.

Proposed mapping:

- `0` success
- `1` unexpected internal failure
- `2` CLI usage or argument validation error
- `3` connection failure or auth failure
- `4` protocol/server error response
- `5` interrupted, cancelled, or timed out
- `6` local config or local I/O failure
- `7` approval required but not resolved in non-interactive mode

Examples:
- could not open socket -> `3`
- server returned JSON-RPC error -> `4`
- SIGINT during streaming wait -> `5`
- bad config file parse -> `6`
- `item/*/requestApproval` received during one-shot non-interactive run -> `7`

This gives Hermes enough signal to branch without scraping stderr text.

## Recommended v1 implementation stance

When implementation starts, bias toward:
- one connection per command invocation
- one request flow per invocation
- no daemon mode yet
- explicit streaming mode for turn execution
- explicit non-interactive behavior for approvals
- typed protocol structs for the v1 subset only

Avoid in v1:
- caching state across invocations
- a custom local database
- optimistic support for every generated schema method
- automatic approval decisions
- hidden retries that can duplicate turns

## Open questions to defer, not solve now

These are real but should not block the first CLI:
- whether health should mean HTTP probe, handshake probe, or both
- whether bearer auth alone is enough, or if ws capability-token/JWT flows need first-class support immediately
- how much of `thread/read` vs `thread/turns/list` should be exposed directly to users
- whether to keep protocol method names raw in CLI command names or wrap them in friendlier verbs
- whether output schemas should be exposed in v1 CLI flags or only later

## Final recommendation

Build the first Rust client as a single Cargo package with a reusable library core and a thin CLI binary.
Target the confirmed app-server subset already proven by `codex-manager-app` and the generated schema: initialize, model listing, thread lifecycle, turn execution, directory browsing, streaming notifications, and approval detection.
Default all automation-oriented commands to structured JSON output and keep a stable exit-code contract so Hermes can call the binary predictably.