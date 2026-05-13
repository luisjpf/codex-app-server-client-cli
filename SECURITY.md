# Security Policy

## Supported versions

This project is currently pre-1.0. Security fixes, if needed, will be applied to the latest commit on `main`.

## Reporting a vulnerability

Please do not open a public issue for suspected security vulnerabilities.

Instead, report them privately to:
- `luis@luisj.me`

Include:
- a description of the issue
- impact assessment
- reproduction steps or proof of concept
- any suggested remediation

## Operational notes

- The default server URL is loopback-only: `ws://127.0.0.1:4500`.
- If you point the CLI at a non-loopback server, prefer authenticated connections and avoid exposing bearer tokens over untrusted plain-`ws://` links.
- Pending approvals may store command context locally to support non-interactive resume flows; treat local config storage as sensitive.
