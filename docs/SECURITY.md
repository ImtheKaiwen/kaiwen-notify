# Security notes

## Assessed risks

| Topic | Status | Notes |
|---|---|---|
| UDP listener (127.0.0.1:47451) | Accepted, documented | Loopback only, no sender authentication. Impact is limited to playing the configured sound and bumping an in-memory counter. A shared-secret token would only stop other *same-user* processes that cannot read the token file, which they can, so it adds little protection for real complexity. Revisit if the listener ever gains side effects. |
| `-ExecutionPolicy Bypass` in hook commands | Accepted | Applies to the app's own bundled script under `%LOCALAPPDATA%\KaiwenNotify\hooks`. Changing the command string would break the working Claude/Codex hooks and the exact-match removal logic. Revisit with a signed script or native sidecar. |
| Hook file edits | Mitigated | Backup before every edit, atomic temp-file replace, exact-command matching, idempotent, corrupt JSON is never overwritten. |
| Codex `notify` | Protected | Never read or written; covered by regression tests. |
| Updater | Mitigated | Official plugin, signature verification always on, install only on explicit user action, no `dangerous*` options in committed config. |
| Tauri capabilities | Least privilege | Event listen/unlisten and dialog open only. |
| Secrets | None in repo | Private updater key lives only in GitHub Actions secrets or the maintainer's machine. `.gitignore` excludes keys and `.env`. CI runs a secret scan. |
| Diagnostics log | Opt-in | Metadata only (reason, idle flag, counts), never prompts or file paths. |

## Reporting

Open a private security advisory on the GitHub repository.
