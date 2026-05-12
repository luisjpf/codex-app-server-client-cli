# Contributing

Thanks for your interest in improving `codex-app-server-client-cli`.

## Development workflow

1. Fork the repo or create a feature branch.
2. Make focused changes with tests where appropriate.
3. Run the local checks before opening a PR:

```bash
./scripts/check.sh
```

This runs:
- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

## Pull request guidelines

- Keep PRs small and reviewable.
- Update docs when the user-facing contract changes.
- Avoid expanding the documented v1 surface unless the implementation is also ready to support it.
- Do not commit secrets, tokens, or machine-specific absolute paths.

## Reporting bugs

Open a GitHub issue with:
- what you ran
- expected behavior
- actual behavior
- relevant JSON output or logs
- environment details if the issue looks platform-specific
