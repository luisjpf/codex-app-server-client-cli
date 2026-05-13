# Examples

## One-Shot Run

```bash
codex-app-server-client-cli --url ws://127.0.0.1:4500 run "summarize the workspace"
```

## Run In The Current Repository

```bash
codex-app-server-client-cli --cwd "$(pwd)" run "inspect this repo and list risks"
```

## Reuse A Session

```bash
codex-app-server-client-cli run --session feature-auth "continue the implementation"
codex-app-server-client-cli resume feature-auth "continue the implementation"
```

## Ephemeral Session

```bash
codex-app-server-client-cli --cwd "$(pwd)" run --ephemeral "try a scratch plan without prior history"
```

## Watch Mode

```bash
codex-app-server-client-cli run --watch "summarize the workspace"
```

## Health And Inspection

```bash
codex-app-server-client-cli health
codex-app-server-client-cli models list
codex-app-server-client-cli session list
codex-app-server-client-cli session show --alias feature-auth
codex-app-server-client-cli fs ls --path /repo/src
codex-app-server-client-cli fs cat --path /repo/src/main.rs
```

## Shell Approval Pattern

```bash
set +e
output="$(codex-app-server-client-cli run "perform a task that may need approval")"
status=$?
set -e

if [ "$status" -eq 7 ]; then
  printf '%s\n' "$output"
  codex-app-server-client-cli approval list
  exit 7
fi

printf '%s\n' "$output"
exit "$status"
```
