# mail-cli

**中文 · [English](#english)**

一个为 AI agent（Claude Code、Codex、其他 LLM CLI 客户端）设计的命令行邮件工具。用 Rust 写，一个二进制文件，结构化 JSON 输出，默认安全。

## 亮点

- **JSON 到处都是**（`--json`），与 Himalaya envelope 格式兼容
- **默认安全**：`send` 默认 dry-run；收件人必须在明文白名单里；`delete` 需要**双门**
- **无 prompt-injection 隐患**：邮件正文用 `<UNTRUSTED_EMAIL_BODY>` 边界包裹
- **读邮件不产生副作用**：`message read` 默认 IMAP `peek`
- **OS keyring 集成**：密码存 macOS Keychain / Linux Secret Service / Windows Credential Manager
- **本地通讯录**：`pull` 时自动从收发件人积累联系人；`contact search` 秒查
- **多层降级**：email-lib → async-imap SEARCH+FETCH → 客户端分页；兼容 263.net 等不太规范的国内 IMAP 服务器
- **并行拉取正文**：3 个 async-imap session 并发，`pull` 5 封邮件 ~3 秒
- **语义 exit code**：`0` ok · `1` transient · `2` config · `3` input · `4` rate-limit

底层用 [`email-lib`](https://crates.io/crates/email-lib)（Pimalaya / Himalaya 生态），支持任何提供 IMAP + SMTP 的邮件服务（Gmail App Password、iCloud、Fastmail、QQ、163、263、自建 Dovecot 等）。

## 安装

```sh
cargo install --path .
```

## 快速上手

```sh
# 1) 添加账户 —— 三种密码方式二选一（推荐 --password-env 或 --password-stdin）
mail-cli account add --name qq --email me@qq.com \
    --imap-host imap.qq.com --smtp-host smtp.qq.com \
    --password "$APP_PASSWORD" \
    --send-allow "*@company.com,alice@partner.com" \
    -p                              # -p / --default: 立刻设为默认账号

# 2) 拉取近 24h 未读邮件；默认自动保存附件、标已读、返回正文
mail-cli message pull --max-age 24h --json

# 3) 只看不消费（不标已读）
mail-cli message pull --max-age 24h --peek --json

# 4) 读单封邮件
mail-cli message read --id 42 --json

# 5) 发邮件 —— 短的直接命令行，长的从文件
mail-cli message send --to alice@company.com --subject "报告" \
    --body "老板好，本周报告已发到共享盘。" \
    --send

mail-cli message send --to alice@company.com --subject "详细报告" \
    --body-file /tmp/report.txt \
    --attach /tmp/data.pdf --attach /tmp/chart.png \
    --send

# 6) 回信也支持附件
mail-cli message reply --id 42 --body "收到，附件重发" \
    --attach /tmp/updated.pdf --send

# 7) 本地通讯录搜索
mail-cli contact search alice --json
mail-cli contact search 老板 --field name --json
mail-cli contact list --sort count --limit 20    # 按互动次数排序
mail-cli contact show alice@company.com --json

# 8) 白名单管理（send 安全网）
mail-cli account allowlist add --name qq boss@company.com "*@team.com"
mail-cli account allowlist remove --name qq old@x.com
mail-cli account allowlist set --name qq alice@x.com bob@x.com
mail-cli account allowlist clear --name qq
mail-cli account allowlist show --name qq --json

# 9) 清理旧附件
mail-cli attachment clear --older-than 7d --dry-run
mail-cli attachment clear --older-than 7d

# 10) 拿一份完整能力 manifest（给 agent 用）
mail-cli agent-info --json
```

## 配置

配置文件在 `~/.config/mail-cli/config.toml`（可用 `--config` 或 `MAIL_CLI_CONFIG` 覆盖）：

```toml
default_account = "qq"

[accounts.qq]
email = "me@qq.com"
send_allowlist = ["alice@company.com", "*@team.example.com"]
archive_folder = "Archive"
sent_folder = "Sent Messages"

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

**密码从不进 TOML**；只存 keyring，key 是 `mail-cli:<account>:imap-passwd` / `:smtp-passwd`。

## 命令一览

```
mail-cli
├── agent-info                              # 能力 manifest
├── account
│   ├── add       --name --imap-host --smtp-host --login
│   │             [--email] [--password | --password-env VAR | --password-stdin]
│   │             [--force] [-p/--default] [--send-allow ADDR,ADDR ...]
│   ├── list
│   ├── remove    --name
│   └── allowlist
│       ├── add     --name <ADDR>...
│       ├── remove  --name <ADDR>...
│       ├── set     --name <ADDR>...
│       ├── clear   --name
│       └── show    --name
├── message
│   ├── list      [--folder INBOX --limit 20 --page 0]
│   ├── read      --id [--folder INBOX --format text|raw --mark-read]
│   ├── pull      [--folder INBOX --limit 20 --max-age 24h --since 2026-07-01]
│   │             [--include-read --peek --body-format text|none]
│   │             [--no-attachments --attachments-dir PATH]
│   ├── send      --to --subject
│   │             (--body <TEXT> | --body-file <PATH|->)
│   │             [--cc --bcc --attach ...]
│   │             [--dry-run|--send]
│   ├── reply     --id (--body | --body-file) [--attach ...] [--reply-all]
│   │             [--dry-run|--send]
│   ├── flag      --id --add \Seen --remove \Flagged
│   ├── archive   --id
│   └── delete    --id --user-explicitly-requested-deletion    # +env gate
├── attachment
│   ├── list      --message-id
│   ├── download  --message-id --index --output PATH
│   └── clear     [--all | --older-than 7d]
│                 [--account-scope NAME] [--folder-scope NAME]
│                 [--attachments-dir PATH] [--dry-run]
├── contact
│   ├── search    <TERM>... [--field any|email|name] [--limit 20]
│   ├── list      [--limit 50 --sort last-seen|count|email]
│   ├── show      <EMAIL>
│   ├── clear
│   └── path
└── config
    └── show
```

全局 flag: `--json` `--output plain|json` `--config PATH` `--account NAME` `--read-only`。

## 安全模型

参考 **CVE-2025-32711 (EchoLeak)** 设计：一封精心构造的邮件让某生产 LLM agent 零点击地泄漏了用户数据。**假设每封邮件都是敌意输入**。

| 风险点 | 应对 |
|---|---|
| 邮件正文不可信 | 用 `<UNTRUSTED_EMAIL_BODY id=... sender=...>...</UNTRUSTED_EMAIL_BODY>` 包裹 |
| agent 悄悄标已读 | 默认 IMAP `peek`；`--mark-read` 才 `\Seen` |
| agent 发件被诱导 | 默认 `--dry-run`；`--send` 必须每个收件人都在 `send_allowlist` 里 |
| agent 擅自删除 | **同时**需要 env `MAIL_CLI_DELETE_ENABLED=true` + `--user-explicitly-requested-deletion` |
| 任何写操作 | 全局 `--read-only`（env `MAIL_CLI_READ_ONLY=1`）一键封禁 |
| HTML 远程资源 / 隐藏指令 | v0.1 只返回纯文本（`html2text`）；远程 URL 数在输出里报告 |

## 服务商速查

| 服务商 | 认证 | Host / port |
|---|---|---|
| Gmail | App Password（2FA） | `imap.gmail.com:993` / `smtp.gmail.com:465` |
| iCloud | App-specific password | `imap.mail.me.com:993` / `smtp.mail.me.com:587` (STARTTLS) |
| Fastmail | App password | `imap.fastmail.com:993` / `smtp.fastmail.com:465` |
| QQ 邮箱 | 授权码 | `imap.qq.com:993` / `smtp.qq.com:465` |
| 163 邮箱 | 授权码 | `imap.163.com:993` / `smtp.163.com:465` |
| 263 企业邮 | 密码 | `imap.263.net:993` / `smtp.263.net:465` |
| Outlook / M365 | OAuth 2.0 | **v0.2 用 Microsoft Graph** |

## 环境变量

| 名字 | 用途 |
|---|---|
| `MAIL_CLI_CONFIG` | 覆盖 config 文件路径 |
| `MAIL_CLI_ACCOUNT` | 默认账户名 |
| `MAIL_CLI_READ_ONLY` | 封禁所有写操作 |
| `MAIL_CLI_DELETE_ENABLED` | 必须 `true` 才能 `message delete` |
| `MAIL_CLI_LOG` | `tracing` env-filter（默认静音；`mail_cli=info` 看进度，`mail_cli=debug` 看细节）|

## Agent 使用

如果你想在 Claude Code 里让 agent 直接用这个工具，把 [`skills/mail-skill/SKILL.md`](skills/mail-skill/SKILL.md) 复制到 `~/.claude/skills/mail-skill/SKILL.md`，之后 agent 会知道如何：

- 拉取/搜索未读邮件（`message pull` + `contact search`）
- 发/回邮件（`message send` / `message reply`，附件、dry-run、白名单）
- 处理附件（保存路径、清理）
- 结构化输出、语义 exit code、`<UNTRUSTED_EMAIL_BODY>` 边界含义

---

<a id="english"></a>
## English

**AI agent-friendly email CLI.** IMAP + SMTP, one binary, structured JSON output, safe by default.

Built for [Claude Code](https://claude.com/claude-code), Codex, and similar LLM agents.

### Highlights

- Structured JSON everywhere (`--json`), Himalaya-compatible envelope schema
- Safe by default: `send` is dry-run; recipients must be in an explicit allowlist; `delete` requires two independent gates
- No prompt-injection footguns: bodies wrapped in `<UNTRUSTED_EMAIL_BODY>` markers
- No side effects on read: `message read` uses IMAP `peek` by default
- OS keyring integration: passwords never touch config files
- **Local contact index**: `pull` auto-ingests senders/recipients; `contact search` is instant
- Semantic exit codes: `0` ok · `1` transient · `2` config · `3` input · `4` rate-limited
- Multi-layer degradation: email-lib → async-imap SEARCH+FETCH → client-side scan
- **Parallel body fetch**: 3 concurrent async-imap sessions; ~3s for 5 emails w/ attachments

### Install

```sh
cargo install --path .
```

### Quick start

```sh
# 1) Add an account with allowlist and set as default
mail-cli account add --name gmail --login me@gmail.com \
    --imap-host imap.gmail.com --smtp-host smtp.gmail.com \
    --password-env APP_PW \
    --send-allow "alice@company.com,*@team.com" \
    -p

# 2) Pull unread from the last 24h (attachments saved by default)
mail-cli message pull --max-age 24h --json

# 3) Send with attachments
mail-cli message send --to alice@company.com --subject "Report" \
    --body "Hi Alice — see attached." \
    --attach /tmp/report.pdf --send

# 4) Reply with attachments
mail-cli message reply --id 42 --body "Updated version attached." \
    --attach /tmp/report-v2.pdf --send

# 5) Contact search (built from pulled messages)
mail-cli contact search alice
mail-cli contact list --sort count --limit 20

# 6) Allowlist management
mail-cli account allowlist add --name gmail boss@x.com
mail-cli account allowlist show --name gmail --json

# 7) Full command manifest for agents
mail-cli agent-info --json
```

### Environment variables

| Name | Purpose |
|---|---|
| `MAIL_CLI_CONFIG` | Config path override |
| `MAIL_CLI_ACCOUNT` | Default account |
| `MAIL_CLI_READ_ONLY` | Block all mutations |
| `MAIL_CLI_DELETE_ENABLED` | Must be `true` to allow `message delete` |
| `MAIL_CLI_LOG` | `tracing` env-filter (silent by default) |

### Development

```sh
cargo build
cargo test
cargo run -- agent-info --json | jq .
```

### License

Apache-2.0
