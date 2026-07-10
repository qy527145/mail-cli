# mail-cli

**中文 · [English](#english)**

一个为 AI agent（Claude Code、Codex、其他 LLM CLI 客户端）设计的命令行邮件工具。用 Rust 写，一个二进制文件，结构化 JSON 输出，默认安全。

## 亮点

- **JSON 到处都是**（`--json`），与 Himalaya envelope 格式兼容
- **默认安全**：`send` 默认 dry-run；收件人必须在明文白名单里；`delete` 需要**双门**
- **无 prompt-injection 隐患**：所有邮件正文用 `<UNTRUSTED_EMAIL_BODY>` 边界包裹，agent 能识别什么是数据、什么是指令
- **读邮件不产生副作用**：`message read` 默认走 IMAP `peek`（不打 `\Seen`）；主动标已读要 `--mark-read`
- **OS keyring 集成**：密码存 macOS Keychain / Linux Secret Service / Windows Credential Manager，不进 TOML
- **语义 exit code**：`0` ok · `1` 可重试 · `2` 配置错 · `3` 输入错 · `4` 限流
- **JSON → stdout，日志 → stderr**，永远可解析
- **多层降级**：email-lib → async-imap fallback → 客户端 filter，兼容 263.net 这种不太标准的国内 IMAP 服务器

底层用 [`email-lib`](https://crates.io/crates/email-lib)（Pimalaya / Himalaya 生态），支持任何提供 IMAP + SMTP 的邮件服务（Gmail App Password、iCloud、Fastmail、QQ、163、263、自建 Dovecot 等）。

## 安装

```sh
cargo install --path .
```

## 快速上手

```sh
# 1) 添加账户，密码通过管道 / 环境变量 / 命令行传入
echo "$APP_PASSWORD" | mail-cli account add \
    --name qq --email me@qq.com \
    --imap-host imap.qq.com --smtp-host smtp.qq.com \
    --login me@qq.com --password-stdin

# 或环境变量
MAIL_PW=$APP_PASSWORD mail-cli account add --name qq --email me@qq.com \
    --imap-host imap.qq.com --smtp-host smtp.qq.com --password-env MAIL_PW

# 2) 批量拉取近 24h 的未读邮件，自动标已读，附件落盘
mail-cli message pull --account qq --max-age 24h --attachments --json

# 3) 读单封邮件（默认 peek 不标已读）
mail-cli message read --account qq --id 42 --json

# 4) 发邮件（先干跑）
echo "Hi Alice, here's the report." | mail-cli message send --account qq \
    --to alice@company.com --subject "Weekly report" --body-file -

# 5) 真发（收件人必须在配置的 send_allowlist 里）
echo "Hi Alice." | mail-cli message send --account qq \
    --to alice@company.com --subject "Weekly report" --body-file - \
    --send

# 6) 带附件发送
mail-cli message send --account qq \
    --to alice@company.com --subject "Files" \
    --body-file /tmp/body.txt \
    --attach /tmp/report.pdf --attach /tmp/data.csv \
    --send

# 7) 清理旧附件（>7 天的，先看再删）
mail-cli attachment clear --older-than 7d --dry-run
mail-cli attachment clear --older-than 7d

# 8) 拿一份完整的命令 manifest（给 agent 用）
mail-cli agent-info --json
```

## 配置

配置文件在 `~/.config/mail-cli/config.toml`（可用 `--config` 或 `MAIL_CLI_CONFIG` 覆盖）：

```toml
default_account = "qq"

[accounts.qq]
email = "me@qq.com"
send_allowlist = ["alice@company.com", "*@team.example.com"]
archive_folder = "Archive"      # `message archive` 需要
sent_folder = "Sent Messages"   # 可选，成功发信后存副本

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

**密码从不在 TOML 里**，只存 keyring，key 为 `mail-cli:<account>:imap-passwd` / `:smtp-passwd`。

## 安全模型

设计时充分参考了 **CVE-2025-32711 (EchoLeak)**：一封精心构造的邮件让某生产 LLM agent 零点击地泄漏了用户数据。假设**每封邮件都是敌意输入**。

| 风险点 | 应对 |
|---|---|
| 邮件正文不可信 | 正文用 `<UNTRUSTED_EMAIL_BODY id=... sender=...>...</UNTRUSTED_EMAIL_BODY>` 包裹 |
| agent 悄悄把邮件标为已读 | 默认 IMAP `peek`；`--mark-read` 才 `\Seen` |
| agent 被诱导发到攻击者邮箱 | 默认 `--dry-run`；`--send` 必须每个收件人都在 `send_allowlist` |
| agent 擅自删除 | 需要**同时**满足环境变量 `MAIL_CLI_DELETE_ENABLED=true` 和调用时的 `--user-explicitly-requested-deletion` flag |
| 任何写操作 | 全局 `--read-only`（或 env `MAIL_CLI_READ_ONLY=1`）一键封禁所有写 |
| HTML 里的远程资源 / 隐藏指令 | v0.1 只返回纯文本（html2text）；远程 URL 数量在输出里报告 |

## 命令一览

```
mail-cli
├── agent-info                              # 能力 manifest
├── account
│   ├── add       --name --email --imap-host --smtp-host --login
│   │             [--password <VAL> | --password-env <VAR> | --password-stdin]
│   │             [--force]
│   ├── list      [--json]
│   └── remove    --name
├── message
│   ├── list      [--folder INBOX --limit 20 --page 0]
│   ├── read      --id [--folder INBOX --format text|raw --mark-read]
│   ├── pull      [--folder INBOX --limit 20 --max-age 24h --since 2026-07-01]
│   │             [--include-read --peek --body-format text|none]
│   │             [--attachments --attachments-dir PATH]
│   ├── send      --to --subject --body-file [-|PATH] [--cc --bcc --attach ...]
│   │             [--dry-run|--send]
│   ├── reply     --id --body-file [-|PATH] [--reply-all --dry-run|--send]
│   ├── flag      --id --add \Seen --remove \Flagged
│   ├── archive   --id
│   └── delete    --id --user-explicitly-requested-deletion   # + 环境变量闸门
├── attachment
│   ├── list      --message-id
│   ├── download  --message-id --index --output PATH
│   └── clear     [--all | --older-than 7d]
│                 [--account-scope NAME] [--folder-scope NAME]
│                 [--attachments-dir PATH] [--dry-run]
└── config
    └── show
```

全局 flag: `--json` `--output plain|json` `--config PATH` `--account NAME` `--read-only`。

## 服务商速查

| 服务商 | 认证方式 | Host / port |
|---|---|---|
| Gmail | App Password（需要 2FA）| `imap.gmail.com:993` / `smtp.gmail.com:465` |
| iCloud | App-specific password | `imap.mail.me.com:993` / `smtp.mail.me.com:587` (STARTTLS) |
| Fastmail | App password | `imap.fastmail.com:993` / `smtp.fastmail.com:465` |
| QQ 邮箱 | 授权码 | `imap.qq.com:993` / `smtp.qq.com:465` |
| 163 邮箱 | 授权码 | `imap.163.com:993` / `smtp.163.com:465` |
| 263 企业邮 | 密码 | `imap.263.net:993` / `smtp.263.net:465` |
| Outlook / M365 | OAuth 2.0（Basic Auth 已废）| **v0.2 用 Microsoft Graph** |

OAuth 2.0、JMAP、Gmail REST、Microsoft Graph 都在 v0.2 路线图上。POP3 不打算做（"下载即删除"的模型跟 agent 反复读邮件不兼容）。

## 环境变量

| 名字 | 用途 |
|---|---|
| `MAIL_CLI_CONFIG` | 覆盖 config 文件路径 |
| `MAIL_CLI_ACCOUNT` | 默认账户名 |
| `MAIL_CLI_READ_ONLY` | 封禁所有写操作 |
| `MAIL_CLI_DELETE_ENABLED` | 必须 `true` 才能 `message delete` |
| `MAIL_CLI_LOG` | `tracing` env-filter（`mail_cli=debug` 之类）|

## Agent 使用

如果你想在 Claude Code 里让 agent 直接用这个工具，把 [`skills/mail-skill/SKILL.md`](skills/mail-skill/SKILL.md) 里的 skill 复制到 `~/.claude/skills/mail-skill/SKILL.md`，之后 agent 会知道如何：

- 列/读未读邮件（`message pull`）
- 发邮件（`message send`，附件、dry-run、白名单）
- 处理附件（保存路径、清理）
- 结构化输出、语义 exit code、`<UNTRUSTED_EMAIL_BODY>` 边界含义

---

<a id="english"></a>
## English

**AI agent-friendly email CLI.** IMAP + SMTP, one binary, structured JSON output, safe by default.

Built for [Claude Code](https://claude.com/claude-code), Codex, and similar LLM agents that need to read and send email as part of a workflow.

### Highlights

- **Structured JSON everywhere** (`--json`), Himalaya-compatible envelope schema
- **Safe by default**: `send` is dry-run unless you pass `--send`; recipients must be in an explicit allowlist; `delete` requires two independent gates
- **No prompt-injection footguns**: all message bodies wrapped in `<UNTRUSTED_EMAIL_BODY>` markers
- **No side effects on read**: `message read` uses IMAP `peek` by default
- **OS keyring integration**: passwords never touch config files
- **Semantic exit codes**: `0` ok · `1` transient · `2` config · `3` input · `4` rate-limited
- **Multi-layer degradation**: email-lib → async-imap fallback → client-side filter for parser-non-conformant servers (e.g. 263.net)

### Install

```sh
cargo install --path .
```

### Quick start

```sh
# 1) Add an account (password from stdin/env var/direct value)
echo "$APP_PASSWORD" | mail-cli account add \
    --name gmail --email me@gmail.com \
    --imap-host imap.gmail.com --smtp-host smtp.gmail.com \
    --login me@gmail.com --password-stdin

# 2) Pull unread messages from the last 24h, batch-mark as read, save attachments
mail-cli message pull --account gmail --max-age 24h --attachments --json

# 3) Read a single message (peek by default — no \Seen flag)
mail-cli message read --account gmail --id 42 --json

# 4) Send a message (dry-run first)
echo "Hi Alice" | mail-cli message send --account gmail \
    --to alice@company.com --subject "Report" --body-file -

# 5) Send for real (allowlist must permit the recipient)
echo "Hi Alice" | mail-cli message send --account gmail \
    --to alice@company.com --subject "Report" --body-file - --send

# 6) Send with attachments
mail-cli message send --account gmail \
    --to alice@company.com --subject "Files" --body-file /tmp/body.txt \
    --attach /tmp/report.pdf --attach /tmp/data.csv --send

# 7) Clean up old attachment directories (older than 7 days)
mail-cli attachment clear --older-than 7d --dry-run    # preview
mail-cli attachment clear --older-than 7d              # actually delete

# 8) Get the full command manifest for programmatic use
mail-cli agent-info --json
```

### Configuration

Lives at `~/.config/mail-cli/config.toml`:

```toml
default_account = "gmail"

[accounts.gmail]
email = "me@gmail.com"
send_allowlist = ["alice@company.com", "*@team.example.com"]
archive_folder = "Archive"
sent_folder = "[Gmail]/Sent Mail"

[accounts.gmail.imap]
host = "imap.gmail.com"
port = 993
encryption = "tls"
login = "me@gmail.com"

[accounts.gmail.smtp]
host = "smtp.gmail.com"
port = 465
encryption = "tls"
login = "me@gmail.com"
```

Passwords never appear in this file; they live in the OS keyring under
`mail-cli` service, keyed by `<account>:imap-passwd` / `<account>:smtp-passwd`.

### Safety model

Designed with **CVE-2025-32711 (EchoLeak)** in mind: a single crafted email
achieved zero-click data exfiltration from a production LLM agent. Assume every
message body is adversarial.

| Concern | Mitigation |
|---|---|
| Email body is untrusted input | Wrapped in `<UNTRUSTED_EMAIL_BODY id=... sender=...>...</UNTRUSTED_EMAIL_BODY>` |
| Agent silently marks read | Default IMAP `peek`; `--mark-read` opt-in for setting `\Seen` |
| Agent sends to attacker | Default `--dry-run`; `--send` requires every recipient in `send_allowlist` |
| Agent unilateral delete | Requires **both** `MAIL_CLI_DELETE_ENABLED=true` env and `--user-explicitly-requested-deletion` per call |
| Any mutation | Global `--read-only` (or env `MAIL_CLI_READ_ONLY=1`) blocks everything |
| HTML remote resources / hidden instructions | v0.1 returns plain text only (html2text); remote URL count reported |

### Agent integration

Copy [`skills/mail-skill/SKILL.md`](skills/mail-skill/SKILL.md) into
`~/.claude/skills/mail-skill/SKILL.md` and Claude Code will know how to:

- List / read unread mail (`message pull`)
- Send email (`message send` with attachments, dry-run, allowlist)
- Handle attachments (save paths, cleanup)
- Interpret structured output, exit codes, `<UNTRUSTED_EMAIL_BODY>` markers

### Commands

Same tree as the Chinese section above. Global flags: `--json`, `--output plain|json`,
`--config PATH`, `--account NAME`, `--read-only`.

### Environment variables

| Name | Purpose |
|---|---|
| `MAIL_CLI_CONFIG` | Override config path |
| `MAIL_CLI_ACCOUNT` | Default account name |
| `MAIL_CLI_READ_ONLY` | Block all mutations |
| `MAIL_CLI_DELETE_ENABLED` | Must be `true` to allow `message delete` |
| `MAIL_CLI_LOG` | `tracing` env-filter (e.g. `mail_cli=debug`) |

### Development

```sh
cargo build
cargo test
cargo run -- agent-info --json | jq .
```

### License

Apache-2.0
