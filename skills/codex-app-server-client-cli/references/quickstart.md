# Quickstart

## Install From GitHub

```bash
cargo install --git https://github.com/luisjpf/codex-app-server-client-cli
```

Confirm:

```bash
codex-app-server-client-cli --help
```

## Health Check

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 health
```

## Config

Default config path:

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

Useful environment overrides:

- `CODEX_APP_SERVER_CONFIG`
- `CODEX_APP_SERVER_TRANSPORT`
- `CODEX_APP_SERVER_URL`
- `CODEX_APP_SERVER_BEARER_TOKEN`
- `CODEX_APP_SERVER_CWD`
- `CODEX_APP_SERVER_MODEL`
- `CODEX_APP_SERVER_OUTPUT`

## Commands

Supported high-level commands:

- `run`
- `resume`
- `approve`
- `deny`

Supported operator/resource commands:

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

Streaming entry points:

- `run --watch`
- `resume --watch`

Not supported as stable v1 operator commands:

- standalone `watch`
- `policy inspect`
- `session resume`
- `session yolo enable`
- `session yolo disable`
- hidden `threads` and `turns` debug commands

## Output Shape

Non-streaming commands write one JSON envelope on stdout.

Typical top-level fields:

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
      "last_approval": null
    },
    "server": {
      "handshake_complete": true,
      "transport_open": true
    }
  }
}
```

Watch mode emits one JSON object per line in arrival order. Each event includes a normalized `type` and preserves the raw protocol method in `protocol_method`.
