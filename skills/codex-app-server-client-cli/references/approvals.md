# Approval Workflows

## Interactive Approval

If both stdin and stdout are terminals, the CLI prompts inline unless policy evaluation already permits auto-approval.

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

`y` or `yes` approves and resumes from the blocked step. Any other answer denies the request.

## Non-Interactive Approval

If stdio is non-interactive, an approval request produces:

- stdout JSON envelope with `ok: false`
- error code `approval_required`
- process exit code `7`
- persisted pending approval for later resolution

Typical envelope:

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

## Resolution Commands

Inspect pending approvals:

```bash
codex-app-server-client-cli approval list
codex-app-server-client-cli approval show --id approval-1
```

Approve and resume:

```bash
codex-app-server-client-cli approve --id approval-1
codex-app-server-client-cli approval approve --token sess_new:approval-1
```

Approve without resuming:

```bash
codex-app-server-client-cli approval approve --id approval-1 --no-resume
```

Deny:

```bash
codex-app-server-client-cli deny --token sess_new:approval-1
codex-app-server-client-cli approval deny --id approval-1
```

## Pending Approval Store

Pending approvals are stored under the user config directory:

- macOS: `~/Library/Application Support/codex-app-server-client-cli/pending-approvals.json`
- Linux: `${XDG_CONFIG_HOME:-~/.config}/codex-app-server-client-cli/pending-approvals.json`

Approvals are keyed by `resume_token`. Lookup by `approval_id` is allowed when unique. If multiple pending approvals share an `approval_id`, use the more specific `resume_token`.
