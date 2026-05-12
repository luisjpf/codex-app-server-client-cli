# Codex app-server client CLI v1 operator notes

Status: current operator guidance for the shipped v1 binary
Updated: 2026-05-12

## Purpose

These notes are for humans and automation calling the CLI directly. They focus on:
- JSON-first automation behavior
- approval handling in interactive and non-interactive contexts
- the exact packaging/release scope that is safe to document today

## Command availability matrix

Supported operator-facing commands:
- `run`
- `resume`
- `approve`
- `deny`
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

Supported streaming entry points:
- `run --watch`
- `resume --watch`

Not part of the supported operator surface:
- standalone `watch`
- `policy inspect`
- `session resume`
- `session yolo enable`
- `session yolo disable`
- hidden `threads` / `turns` debug commands

## JSON-first automation notes

### Final output defaults

The CLI is JSON-first.

For non-streaming commands, stdout carries one final envelope with this top-level shape:
- `ok`
- `command`
- `session`
- `data`
- `meta`

Examples:
- `run`
- `resume`
- `approve`
- `deny`
- `session list/show/fork`
- `approval list/show/approve/deny`
- `models list`
- `fs ls`
- `fs cat`
- `health`

### Output flags

Global output flags:
- `--output json`
- `--output jsonl`
- `--output text`
- `--pretty`

Current behavior details:
- `json` is the default
- `jsonl` on a non-streaming command still emits a single JSON object line
- `text` currently pretty-prints the same JSON envelope; it is not a separate human renderer
- `--pretty` only affects final-envelope JSON formatting
- watch mode ignores final-envelope formatting and always emits JSONL event lines

### stderr contract

Treat stderr as diagnostics only:
- tracing/logging
- clap help and usage failures
- human-readable failure context for non-approval errors

Do not scrape stderr for approval flow state. Approval-required state is represented on stdout plus exit code `7`.

## Approval behavior

### Interactive path

If stdin and stdout are terminals, approval requests use an inline prompt unless policy/YOLO evaluation already allows auto-approval.

Prompt shape:

```text
Approval required
  scope: command_execution
  session: sess_new
  summary: Run npm test
  action: ["npm","test"]
  risk_traits: shell_exec
Approve and resume from the blocked step? [y/N]
```

Behavior:
- `y` or `yes` approves and resumes from the blocked step
- any other answer denies the request
- denial exits non-zero

### Non-interactive path

If the CLI is not attached to interactive stdio, approval requests become structured machine output.

Contract:
- stdout contains an approval-required JSON envelope
- process exits with code `7`
- the envelope includes both `approval_id` and `resume_token`
- the pending approval is persisted locally for later resolution

Typical workflow:

```bash
codex-app-server-client-cli run "needs approval"
status=$?
if [ "$status" -eq 7 ]; then
  codex-app-server-client-cli approval list
fi
```

### Resolving pending approvals

Supported resolution forms:

```bash
codex-app-server-client-cli approve --id approval-1
codex-app-server-client-cli approve --token sess_new:approval-1
codex-app-server-client-cli deny --id approval-1
codex-app-server-client-cli approval approve --id approval-1
codex-app-server-client-cli approval deny --token sess_new:approval-1
```

Important behaviors:
- `approve` resumes by default
- `approve --no-resume` marks the request approved in the local store and returns `resumed: false`
- `deny` clears the pending approval from the local store after sending a denied approval response
- `resume <resume-token>` can continue the blocked step without providing new input

### Local approval store

Pending approvals are stored under the user config directory at:
- macOS: `~/Library/Application Support/codex-app-server-client-cli/pending-approvals.json`
- Linux: `${XDG_CONFIG_HOME:-~/.config}/codex-app-server-client-cli/pending-approvals.json`

Operational notes:
- approvals are keyed by `resume_token`
- lookup by `approval_id` is allowed when that ID is unique
- if multiple pending approvals share the same `approval_id`, operators must use the more specific `resume_token`

## Session and watch notes

### Session selection

`run` uses repo-aware default selection:
- if the current `--cwd` resolves inside a repository and an existing session matches that repo root, reuse it
- otherwise create a new reusable session
- `--ephemeral` creates a history-clean session while preserving workspace identity

### Watch mode

Current supported operator watch flow is flag-driven, not subcommand-driven:
- `run --watch`
- `resume --watch`

Event guarantees worth depending on:
- one JSON object per line
- stable normalized `type`
- preserved arrival order
- raw protocol method preserved as `protocol_method`
- terminal completion or terminal error is emitted on stdout, not as a separate stderr-only signal

## YOLO notes

Supported operator controls:
- `run --yolo`
- `resume --yolo`
- `run --no-yolo`
- `resume --no-yolo`

Current limitations:
- there is no documented `session yolo enable` or `session yolo disable` command
- session-scoped YOLO is observable in returned session/policy metadata when the server reports it

## Packaging and release notes for the implemented v1 subset

Safe release/documentation claims today:
- the project ships as a single Cargo package
- the installable binary name is `codex-app-server-client-cli`
- source build and `cargo install --path .` are the documented installation paths
- WebSocket transport is the only implemented execution transport
- the v1 operator contract covers the commands listed in this file and in `docs/v1-cli-spec.md`

Claims to avoid in release notes today:
- Homebrew, apt, npm, Docker, or other package-manager distribution
- stdio/unix transport execution support
- standalone `watch` command support
- policy inspection/editing commands
- session-scoped YOLO mutation commands
- a custom human-oriented text renderer

## Suggested operator quick checks

```bash
cargo run -- --help
cargo run -- run --help
cargo run -- resume --help
cargo run -- approval --help
cargo run -- session --help
cargo run -- health
```

Use `./scripts/check.sh` for the full local quality gate.
