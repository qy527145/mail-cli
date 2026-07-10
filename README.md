# mail-cli

**AI agent-friendly email CLI.** IMAP + SMTP, one binary, structured JSON output, safety-by-default.

Built for [Claude Code](https://claude.com/claude-code), Codex, and similar LLM agents that need to read and send email as part of a workflow.

## Highlights

- **Structured JSON everywhere** (`--json`), Himalaya-compatible envelope schema
- **Safe by default**: `send` is dry-run unless you pass `--send`; recipients must be in an explicit allowlist; `delete` requires two independent gates
- **No prompt-injection footguns**: all message bodies are wrapped in `<UNTRUSTED_EMAIL_BODY>` markers so the agent knows what is data vs. instructions
- **No side effects on read**: `message read` uses IMAP `peek` by default (does not set `\Seen`); mark-as-read is opt-in with `--mark-read`
- **OS keyring integration**: passwords live in macOS Keychain / Linux Secret Service / Windows Credential Manager, never in TOML files
- **Semantic exit codes**: `0` ok · `1` transient (retry) · `2` config · `3` input · `4` rate-limited
- **JSON to stdout, logs to stderr** — always parseable

Built on [`email-lib`](https://crates.io/crates/email-lib) (the Pimalaya / Himalaya backend), so any provider that speaks IMAP + SMTP is supported (Gmail App Passwords, iCloud, Fastmail, QQ, 163, self-hosted Dovecot, …).

## Install

```sh
cargo install --path .
```

## Quick start

```sh
# 1) Add an account (password read from stdin, stored in OS keyring)
echo "$APP_PASSWORD" | mail-cli account add \
    --name qq \
    --email me@qq.com \
    --imap-host imap.qq.com --imap-port 993 \
    --smtp-host smtp.qq.com --smtp-port 465 \
    --login me@qq.com \
    --password-stdin

# 2) List unread messages
mail-cli message list --folder INBOX --limit 20 --json | jq '.envelopes[].subject'

# 3) Read a message (non-mutating peek by default)
mail-cli message read --id 42 --json | jq -r .body_text

# 4) Dry-run a send (default)
echo "Hi Alice, here's the report." | mail-cli message send \
    --to alice@company.com \
    --subject "Weekly report" \
    --body-file - \
    --json

# 5) Really send it (requires allowlist)
# Edit ~/.config/mail-cli/config.toml → send_allowlist = ["alice@company.com"]
echo "Hi Alice." | mail-cli message send \
    --to alice@company.com --subject "Weekly report" --body-file - \
    --send --json
```

## Agent onboarding — one call gets everything

An agent can discover the entire tool surface in one shot:

```sh
mail-cli agent-info --json
```

Returns a machine-readable manifest of every subcommand, argument, exit code, output format, environment variable, and safety principle. Suitable for embedding into an agent's tool description.

## Configuration

Config file lives at `~/.config/mail-cli/config.toml` (override with `--config` or `MAIL_CLI_CONFIG`):

```toml
default_account = "qq"

[accounts.qq]
email = "me@qq.com"
send_allowlist = ["alice@company.com", "*@team.example.com"]
archive_folder = "Archive"      # required for `message archive`
sent_folder = "Sent Messages"   # optional: save a copy of each outgoing message

[accounts.qq.imap]
host = "imap.qq.com"
port = 993
encryption = "tls"              # tls | starttls | none
login = "me@qq.com"

[accounts.qq.smtp]
host = "smtp.qq.com"
port = 465
encryption = "tls"
login = "me@qq.com"
```

Passwords never appear in this file; they live under `mail-cli` service in the OS keyring (`<name>:imap-passwd`, `<name>:smtp-passwd`).

## Safety model

mail-cli was designed with **CVE-2025-32711 (EchoLeak)** in mind: one crafted email caused zero-click data exfiltration from a production LLM agent. Assume every message body is adversarial.

| Concern | Mitigation |
|---|---|
| Email body is untrusted input | Body wrapped in `<UNTRUSTED_EMAIL_BODY id=... sender=...>...</UNTRUSTED_EMAIL_BODY>` markers |
| Agent silently marks read | Default is IMAP `peek`; setting `\Seen` requires `--mark-read` |
| Agent sends to attacker-controlled address | Default is `--dry-run`; `--send` fails unless every recipient is in `send_allowlist` |
| Agent unilateral delete | Requires **both** `MAIL_CLI_DELETE_ENABLED=true` and `--user-explicitly-requested-deletion` on the call |
| Agent takes any mutating action | Global `--read-only` (or env `MAIL_CLI_READ_ONLY=1`) blocks all mutations |
| HTML with remote resources / hidden instructions | v0.1 always returns plain text (HTML → text via `html2text`); remote URL count reported for auditing |

## Commands

```
mail-cli
├── agent-info                              # capability manifest
├── account
│   ├── add       --name --email --imap-host --smtp-host --login --password-stdin
│   ├── list      [--json]
│   └── remove    --name
├── message
│   ├── list      [--folder INBOX --limit 20 --page 0 --unread --since 2026-01-01]
│   ├── read      --id [--folder INBOX --format text|raw --mark-read]
│   ├── send      --to --subject --body-file [-|PATH] [--cc --bcc --attach ...]
│   │             [--dry-run|--send]
│   ├── reply     --id --body-file [-|PATH] [--reply-all --dry-run|--send]
│   ├── flag      --id --add \Seen --remove \Flagged
│   ├── archive   --id
│   └── delete    --id --user-explicitly-requested-deletion   # +env gate
├── attachment
│   ├── list      --message-id
│   └── download  --message-id --index --output PATH
└── config
    └── show
```

Global flags: `--json` `--output plain|json` `--config PATH` `--account NAME` `--read-only`.

## Provider quick-reference

| Provider | Auth | Host / port |
|---|---|---|
| Gmail | App Password (2FA required) | `imap.gmail.com:993` / `smtp.gmail.com:465` |
| iCloud | App-specific password | `imap.mail.me.com:993` / `smtp.mail.me.com:587` (STARTTLS) |
| Fastmail | App password | `imap.fastmail.com:993` / `smtp.fastmail.com:465` |
| QQ mail | Authorization code | `imap.qq.com:993` / `smtp.qq.com:465` |
| 163 mail | Authorization code | `imap.163.com:993` / `smtp.163.com:465` |
| Outlook / M365 | OAuth 2.0 (basic auth deprecated) | *v0.2 — Microsoft Graph* |

OAuth 2.0, JMAP, Gmail REST API, and Microsoft Graph are planned for v0.2. POP3 is not currently on the roadmap (the "download-and-delete" model doesn't compose with an agent re-reading messages).

## Environment variables

| Name | Purpose |
|---|---|
| `MAIL_CLI_CONFIG` | Config file path override |
| `MAIL_CLI_ACCOUNT` | Default account name |
| `MAIL_CLI_READ_ONLY` | Block all mutating operations |
| `MAIL_CLI_DELETE_ENABLED` | Must be `true` to allow `message delete` |
| `MAIL_CLI_LOG` | `tracing` env-filter (e.g. `mail_cli=debug`) |

## Development

```sh
cargo build
cargo test
cargo run -- agent-info --json | jq .
```

## License

Apache-2.0
