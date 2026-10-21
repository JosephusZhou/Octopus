# 各 Agent 项目配置最终落点（MCP / 指令文件 / Skills）

- 整理日期：2026-10-21（Asia/Shanghai）
- 适用范围：本应用「项目配置」初始化（`src-tauri/src/project_config.rs` 的 `AGENT_SPECS`）与全局注册表（`src-tauri/src/agents.rs`）。
- **版本基线**：只面向各 Agent **最新版本**，不做旧版兼容兜底；未来统一向通用目录/通用文档收敛。
- **通用优先原则**：
  1. 指令文件：Agent 原生能读项目根 `AGENTS.md` 就只写 `AGENTS.md`，不再创建 `CLAUDE.md` / `GEMINI.md` 指针。
  2. Skills：Agent 原生能读通用 `.agents/skills` 就不建自己的项目 skills 目录；只有原生位不同的 Agent 才链到自己的目录。
- 确定性标记：**✅ 官方明确**（官方文档/仓库直接记载）；**⚠️ 待核实**（官方未披露，暂采用通用位/通用格式，后续实测修正）。

## 版本快照（2026-10-21 官方文档 / npm registry）

| Agent | 版本基线 | 备注 |
|---|---|---|
| Claude Code | v2.1.x（项目根 `AGENTS.md` 直读需 **v2.1.277+**） | 默认模式 CLAUDE.md 与 AGENTS.md 互斥 |
| Codex CLI | `@openai/codex` 0.163.0 | 项目 `.codex/config.toml` 仅 trusted 加载 |
| OpenCode | `opencode-ai` 1.19.3 | |
| Pi | `@earendil-works/pi-coding-agent` 1.3.0 | 0.99.0 起内置 `builtin:mcp`，1.x 默认开 |
| Antigravity | 闭源 App/IDE/CLI，无公开版本号 ⚠️ | 不是开源 Gemini CLI(gcli) |
| WorkBuddy | 桌面办公版，版本待核实 ⚠️ | 不含 CodeBuddy Code CLI |
| Claude Desktop | legacy `claude_desktop_config.json` 机制仍存，官方主推 .mcpb | 无项目级、无本地文件 skills |

## 项目级落点表（仓库根相对路径）

| Agent | 项目 MCP 文件 | 项目配置目录 | 项目指令文件 | 项目 Skills | 确定性 |
|---|---|---|---|---|---|
| **claude-code** | `.mcp.json`（`mcpServers`，条目不写 `enabled`） | `.claude/`（`commands/`、`agents/`） | `AGENTS.md`（不建 CLAUDE.md） | 原生 `.claude/skills/`（链到通用库） | ✅ |
| **codex** | `.codex/config.toml`（`[mcp_servers.*]`，仅 trusted 加载） | `.codex/` | `AGENTS.md`（不再写 `instructions.md`） | 复用 `.agents/skills` | ✅ |
| **opencode** | `opencode.json`（顶层 `mcp`） | `.opencode/`（`agents/ commands/ plugins/ tools/` 复数） | `AGENTS.md` | 复用 `.agents/skills` | ✅ |
| **antigravity** | `.agents/mcp_config.json`（`mcpServers`，**仅 stdio**） | 不建 `.gemini/`（复用 `.agents/`） | `AGENTS.md`（不建 GEMINI.md） | 复用 `.agents/skills` | MCP 路径 ✅；扩展字段未建模 ⚠️ |
| **workbuddy** | `.workbuddy/mcp.json`（`mcpServers`） | `.workbuddy/`（`rules/`、`skills/`） | `AGENTS.md` | `.workbuddy/skills/`（自建） | 全局路径 ✅；项目远程格式/skills ⚠️ |
| **pi** | `.pi/mcp.json`（`mcpServers`，无 `agent/` 层） | `.pi/`（单层，无 `agent/`） | `AGENTS.md` | 复用 `.agents/skills`（不建 `.pi/skills`） | ✅ |
| claude-desktop | —（不做项目级） | — | — | — | ✅ |

## 全局（用户级）落点表

| Agent | 全局 MCP 文件 | 全局 Skills 根 | 全局指令文件 | 确定性 |
|---|---|---|---|---|
| claude-code | `~/.claude.json` 顶层 `mcpServers` | `~/.claude/skills` | `CLAUDE.md` | ✅ |
| codex | `~/.codex/config.toml` `[mcp_servers.*]`（远程无 `type`，头 `http_headers`） | `~/.agents/skills` | `AGENTS.md` | ✅ |
| opencode | `~/.config/opencode/opencode.{jsonc,json}` 顶层 `mcp`（local `environment`、command 数组） | `~/.config/opencode/skills` | `AGENTS.md` | ✅ |
| antigravity | `~/.gemini/config/mcp_config.json`（`mcpServers`，仅写 stdio `command/args/env`） | `~/.gemini/config/skills`（App）；`~/.gemini/antigravity-cli/skills`（CLI） | `AGENTS.md` | 路径 ✅；远程/oauth 未建模 ⚠️ |
| workbuddy | `~/.workbuddy/mcp.json`（无点前缀，`mcpServers`） | `~/.workbuddy/skills` | `AGENTS.md` | 路径 ✅；远程格式 ⚠️ |
| pi | `~/.pi/agent/mcp.json`（内置 builtin:mcp，仅 stdio + streamable HTTP，拒 SSE） | `~/.pi/agent/skills` | `AGENTS.md` | ✅ |
| claude-desktop | 扫描 `Application Support/Claude*/claude_desktop_config.json` | 无（账户同步） | 无 | 机制 ✅；精确路径 ⚠️ |

## 传输与字段约束（已落地）

- **pi**：仅 stdio + streamable HTTP，拒绝 legacy SSE（写入时按 agent 名拦截报错，不落盘）。
- **antigravity**：本应用只写本地 stdio（`command/args/env`）；远程 `serverUrl`/oauth/`disabled` 等不建模，遇到非 stdio 直接报错不写。
- **codex**：远程不写 `type` 字段，请求头用 `http_headers`。
- **claude-code**：`.mcp.json` 条目内不写官方不存在的 `enabled`，审批走 settings。

## 待核实（暂用通用位/通用格式，不臆造）

1. **WorkBuddy 桌面**：远程 MCP 字段格式、`~/.workbuddy/skills` / `.workbuddy/skills`、是否读 AGENTS.md，官方未披露；暂按通用 http `url`+`headers` 与 AGENTS.md 处理。
2. **Antigravity**：`authProviderType` / oauth / `disabled` / `disabledTools` 扩展字段未建模（本应用只做本地 stdio）。
3. OpenCode `opencode.local.json` 是否被 loader 识别；Pi 内置是否回退扫描 `~/.pi/agent/skills`；Claude Desktop 现行精确落盘路径。

## 来源

逐项官方出处与调研过程见 [`research/agent-config-survey-2026-10.md`](research/agent-config-survey-2026-10.md)；
方言/写器/合并策略的工程规格见 [`AGENT_MCP_SKILLS_MAP.md`](AGENT_MCP_SKILLS_MAP.md)。
