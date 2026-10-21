# AgentBuddy — 各 Agent MCP / Skills 配置方案

> **范围**：macOS / Windows 全局配置 + 项目级配置（与当前代码一致）
> **修订日期**：2026-10-13（v4）
> **目的**：作为 `agents.rs` / `mcp_config.rs` / `skills.rs` / `project_config.rs`
> 的**人类可读 mirror**，说明每个 Agent 的路径、方言、读写策略。
>
> **状态**：**全部已实现**——本文不是方案/研究稿，而是当前落盘代码的规格说明。
> 改 agents 路径或方言时，**先改 `agents.rs`**，再同步本文。
>
> **修订摘要（相对 v3 2026-08-14）**：
> - Agent 注册表收窄为 7 个：`codex` / `claude-code` / `claude-desktop` / `opencode` /
>   `antigravity` / `workbuddy` / `pi`（历史下线的 agent 由启动时配置升级统一清理残留）
> - McpPath 移除 `CodebuddyMcp` 变体
> - 模型配置后端仅保留 OpenCode / Pi 两个通用后端（`opencode_config` / `pi_model_config`）
> - 项目级 MCP 写入：经 `project_config::init_project_config` 的 `AGENT_SPECS`
>   落地（不再分散在适配器代码里）
> - §10 本机校验摘要删除（机器相关，迁入测试 fixtures）

---

## 1. 安全与写作约定

1. 本文只记录**路径、字段名、schema、合并策略**。
2. **禁止**写入 token、API Key、密码、本机完整密钥配置。
3. Agent 标识统一使用 `agents.rs` 中的 `name` 字段（如 `claude-code`）。
4. 「全局应用」默认写**用户级**配置；项目级路径见 §8。
5. 文件写入统一走 **temp file + rename**（原子写），解析保留原 JSON 其它顶层键。

---

## 2. 与嗅探的对齐

| # | `name` | `display_name` | 嗅探 config root | `scan_app_support` |
|---|--------|----------------|------------------|--------------------|
| 1 | `codex` | Codex | `~/.codex` | false |
| 2 | `claude-code` | Claude Code | `~/.claude`（MCP 主存 `~/.claude.json`，**不**在 `~/.claude/` 内） | false |
| 3 | `claude-desktop` | Claude Desktop | `~/Library/Application Support/Claude*`（扫描含 `claude_desktop_config.json` 的目录） | true |
| 4 | `opencode` | OpenCode | `~/.config/opencode` | false |
| 5 | `antigravity` | Antigravity | `~/.gemini` | false |
| 6 | `workbuddy` | WorkBuddy | `~/.workbuddy` | false |
| 7 | `pi` | Pi | `~/.pi` | false |

规则（与 `sniff.rs` / `agents.rs` 一致）：

- `found == true` 仅当 App 或 CLI 二者至少其一存在；config dir 单独存在不算
- CLI 解析：静态 `bin_paths` + `PATH`（`search_names`）；App 路径排在 CLI 前
- PATH / cache 解析会过滤 `cmux` 等 temp-dir 下的 shim 包装
- Codex CLI + ChatGPT App **共享** `~/.codex`
- Claude Desktop config 用 `scan_app_support` 在 `~/Library/Application Support` 扫 `claude_desktop_config.json`

---

## 3. 统一方言 Taxonomy

后续适配器按 `McpDialect` / `McpPath` 分发（见 `agents.rs`），不再有 if-else 复制逻辑。

### 3.1 MCP 方言（`agents::McpDialect`）

| variant | 顶层键 | 文件形态 | 代表 Agent |
|---------|--------|----------|------------|
| `TomlMcpServers` | `[mcp_servers.*]` | TOML | `codex` |
| `ClaudeJsonUser` | `mcpServers` | JSON | `claude-code`（独立方言，因为文件位置特殊：`~/.claude.json` 而非 `~/.claude/*.json`） |
| `JsonMcpServers` | `mcpServers` | JSON / JSONC（按 spec） | `claude-desktop` / `workbuddy` / `pi` |
| `JsonMcp` | `mcp`（**不是** `mcpServers`） | JSON / JSONC（`json5` 解析） | `opencode` |
| `JsonAntigravity` | `mcpServers`，**仅写 stdio**（远程 `serverUrl` 不建模） | JSON | `antigravity`（闭源 Google App/IDE/CLI，非开源 gcli） |

### 3.2 MCP 路径（`agents::McpPath`）

| variant | 含义 | 用到 |
|---------|------|------|
| `Fixed(&'static str)` | 相对用户主目录的固定路径，如 `.codex/config.toml` | `codex` / `claude-code` / `antigravity` / `workbuddy` / `pi` |
| `OpencodeConfig` | `~/.config/opencode/opencode.{jsonc,json}`（存在优先，缺省 `.json`） | `opencode` |
| `ClaudeDesktopScan` | `~/Library/Application Support` 下扫 `claude_desktop_config.json` | `claude-desktop` |

### 3.3 Skills 根（来自 `agents::skills_roots`）

| Agent | Skills roots | 是否支持 |
|-------|--------------|----------|
| `codex` | `~/.agents/skills`（现行官方 USER 位，不保留 legacy `~/.codex/skills`） | ✅ |
| `claude-code` | `~/.claude/skills` | ✅ |
| `claude-desktop` | — | ❌（skills 走账户同步，无本地文件目录） |
| `opencode` | `~/.config/opencode/skills` | ✅ |
| `antigravity` | `~/.gemini/config/skills`（主）, `~/.gemini/antigravity-cli/skills`（CLI 根，兼容扫描） | ✅ |
| `workbuddy` | `~/.workbuddy/skills`（半官方，待核实） | ✅ |
| `pi` | `~/.pi/agent/skills` | ✅ |

### 3.4 写策略（已实现于 `mcp_config.rs`）

| 行为 | 说明 |
|------|------|
| 合并方式 | JSON 对象按 server `title` 合并；TOML 表按 `mcp_servers.<title>` 合并 |
| 其它键 | 保留（不动文件其它顶层字段，如 Claude Code `~/.claude.json` 的大状态文件） |
| 原子写 | temp file + rename（避免半截写入） |
| 共享根去重 | 同一物理配置根的多个 agent 同时勾选 → 只写一次（canonical 取注册表最靠前且被选中的 agent） |
| JSONC | OpenCode 用 `json5` 解析；重写为 pretty JSON（注释不保留） |
| Sniff 合并 | 磁盘优先（`appliedAgents` 反映磁盘实际状态）；`__agentbuddy` 前缀的内置冒烟 server 忽略 |

---

## 4. 总览对照表

| sniff `name` | MCP 文件 | 方言 | Skills 根 |
|--------------|----------|------|-----------|
| `codex` | `~/.codex/config.toml` | `TomlMcpServers` | `~/.agents/skills` |
| `claude-code` | `~/.claude.json` 顶层 `mcpServers` | `ClaudeJsonUser` | `~/.claude/skills` |
| `claude-desktop` | `~/Library/Application Support/Claude*/claude_desktop_config.json` | `JsonMcpServers` | — |
| `opencode` | `~/.config/opencode/opencode.{jsonc,json}` | `JsonMcp` | `~/.config/opencode/skills` |
| `antigravity` | `~/.gemini/config/mcp_config.json` | `JsonAntigravity` | `~/.gemini/config/skills` + `~/.gemini/antigravity-cli/skills` |
| `workbuddy` | `~/.workbuddy/mcp.json`（无点前缀） | `JsonMcpServers` | `~/.workbuddy/skills`（待核实） |
| `pi` | `~/.pi/agent/mcp.json`（内置 builtin:mcp） | `JsonMcpServers` | `~/.pi/agent/skills` |

---

## 5. 分 Agent 详解

### 5.1 Codex (`codex`)

**配置根**：`~/.codex`（CLI 与 ChatGPT App 共享）

**MCP**

| 项 | 值 |
|----|-----|
| 文件 | `~/.codex/config.toml` |
| 方言 | `TomlMcpServers` |
| 结构 | `[mcp_servers.<server-name>]` |
| stdio | 官方按 `command` 推断，**不写 `type`**；`command`，`args`，可选 `enabled = true`；env 子表 `[mcp_servers.<name>.env]` |
| http | 官方按 `url` 推断，**不写 `type`**；`url` + `http_headers`（不是 `headers`），可选 `enabled = true` |
| CLI | `codex mcp add` / `codex mcp list` |

**Skills**

- 主写位：`~/.agents/skills/<skill>/SKILL.md`（现行官方 USER 位置，不保留 legacy `~/.codex/skills`）
- 项目：仓库 `.agents/skills`（原生读取，不建自建 skills 目录）
- `config.toml` 中可有 `[[skills.config]]`（path / enabled）
- 全局 `~/.codex/AGENTS.md`（不是 Skill，但是配置根一部分）

**多环境**：`codex_env.rs` 通过 `CODEX_HOME` 隔离 config root；同步 MCP 时默认 home 的
`~/.codex/config.toml` → 各 `$CODEX_HOME/config.toml`，**替换** `[mcp_servers]`，保留其它键。

### 5.2 Claude Code (`claude-code`)

**配置根**：`~/.claude`（settings / skills / agents）
**MCP 状态文件**：`~/.claude.json`（**不**在 `~/.claude/` 目录内，路径不对称）

**MCP**

| Scope | 路径 | 说明 |
|-------|------|------|
| User（全局，默认应用目标） | `~/.claude.json` 顶层 `mcpServers` | 所有项目可见 |
| Local（单项目、个人） | `~/.claude.json` → `projects["/abs/path"].mcpServers` | `claude mcp add` 默认 scope |
| Project（团队共享） | 项目根 `.mcp.json` | git 可提交；需信任/批准 |

| 项 | 值 |
|----|-----|
| 方言 | `ClaudeJsonUser`（独立 dialect，因为文件位置特殊） |
| stdio | `command` + `args` + 可选 `env`；可带 `type: "stdio"` |
| http/sse | `type: "http"` / `"sse"` + `url` + 可选 `headers` |
| CLI | `claude mcp add [--scope user\|project\|local] ...` |

**Skills**

| 项 | 值 |
|----|-----|
| 用户 | `~/.claude/skills/<skill-name>/SKILL.md` |
| 项目 | `.claude/skills/<skill-name>/SKILL.md` |
| 格式 | 目录 + 必需 `SKILL.md`（YAML frontmatter + Markdown） |

**多环境**：`claude_env.rs` 通过 `CLAUDE_CONFIG_DIR` 隔离 config root；同步 MCP 时默认 home 的
`~/.claude.json` → 各 `$CLAUDE_CONFIG_DIR/.claude.json`，**替换** 顶层 `mcpServers`，保留其它键。
每 env 的 `settings.json` 写 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_MODEL`
（+ 配套 default-model 键）。

### 5.3 Claude Desktop (`claude-desktop`)

**配置根**：在 `~/Library/Application Support` 下扫 `Claude*/claude_desktop_config.json`

**MCP**

| 项 | 值 |
|----|-----|
| 文件 | `…/Claude/claude_desktop_config.json`（多实例时全部扫描） |
| 方言 | `JsonMcpServers` |
| 结构 | 顶层 `mcpServers`；stdio：`command` / `args` / `env` |
| 生效 | 通常需**完全退出并重启** Desktop |

**Skills**

- Claude Desktop 的 skills 走**账户同步**（Customize → Skills 上传、绑定账号），不读本地 `~/.claude/skills`；本地文件 skills 是 Claude Code 特性。
- `agents.rs` 中 `skills_supported: false`，前端 `SKILL_UNSUPPORTED_AGENTS` 同步置灰。
- 无本地磁盘 skills 目录，这是产品模型差异，不是缺口。

### 5.4 OpenCode (`opencode`)

**配置根**：`~/.config/opencode`（**不是** `~/.opencode` 安装目录）

**MCP**

| 项 | 值 |
|----|-----|
| 文件 | `~/.config/opencode/opencode.json`（或 `opencode.jsonc`） |
| 方言 | `JsonMcp` |
| 顶层键 | **`mcp`**（不是 `mcpServers`） |
| local | `"type": "local"`，`command` 是**字符串数组**（含 args），`environment`，`enabled` |
| remote | `"type": "remote"`，`url`，`headers`，`enabled` |
| schema | `"$schema": "https://opencode.ai/config.json"` |

**Skills**

| 项 | 值 |
|----|-----|
| 全局 | `~/.config/opencode/skills/<name>/SKILL.md` |
| 项目 | `.opencode/skills/` 等 |
| 权限 | 可在 `opencode.json` 的 `skill` 字段做 allow/deny |

**模型配置**：`opencode_config.rs` 提供 `get_agent_model_config` / `upsert_agent_model` /
`upsert_agent_provider` 等命令；`~/.config/opencode/opencode.json` 的 `provider` / `model`
条目与 `auth.json`（API Key）独立维护。

### 5.5 Antigravity (`antigravity`)

**性质**：闭源 Google Antigravity App/IDE/CLI（`agy`）；**不是**开源 Gemini CLI（gcli），
本文不再保留 gcli 的 `settings.json` / `httpUrl` 口径。

**配置根**：`~/.gemini`

**MCP**

| 项 | 值 |
|----|-----|
| 文件 | `~/.gemini/config/mcp_config.json` |
| 方言 | `JsonAntigravity`（顶层 `mcpServers`） |
| stdio | `command`（字符串）+ `args[]` + `env{}`；DTO 无 `cwd`，不写；不写 `type` |
| 远程 | **本应用不建模**（`serverUrl`/oauth/`disabled` 等不写）；非 stdio 直接报错不落盘 |
| 读取 | 识别已有文件的 `serverUrl` → http transport 仅用于回显；保留其它键 |

**项目级 MCP**：`<repo>/.agents/mcp_config.json`（**不建** `.gemini` 项目目录）。

**Skills**

| 项 | 值 |
|----|-----|
| 主写位 | `~/.gemini/config/skills/`（App/IDE 全局） |
| 兼容扫描 | `~/.gemini/antigravity-cli/skills/`（CLI 根） |
| 项目 | 原生读 `.agents/skills`，不建自建 skills 目录 |

**提示文件**：`AGENTS.md`（全局 `~/.gemini/AGENTS.md`）。GEMINI.md 也会被读取，但
AgentBuddy 只维护 AGENTS.md，不生成 GEMINI.md 指针。

### 5.6 WorkBuddy (`workbuddy`)

**性质**：腾讯桌面办公版（WorkBuddy.app）；不含 CodeBuddy Code CLI，本文不保留 CodeBuddy 口径。

**配置根**：`~/.workbuddy`

**MCP**

| 项 | 值 |
|----|-----|
| 文件 | `~/.workbuddy/mcp.json`（**无点前缀**，不是 `.workbuddy/.mcp.json`） |
| 方言 | `JsonMcpServers`（顶层 `mcpServers`，stdio：`command`/`args`/`env`） |
| 远程格式 | 官方未披露（待核实）；当前按通用 `type: "http"` + `url` + `headers` 写入 |
| 注意 | 合并时**保留**产品自带 server，禁止整体覆盖文件 |

**Skills**

| 项 | 值 |
|----|-----|
| 全局 | `~/.workbuddy/skills/`（半官方，待核实） |
| 项目 | `.workbuddy/skills/`（Full 模式自建目录并链到 `.agents/skills`） |
| 格式 | 目录 + `SKILL.md` |

**提示文件**：`AGENTS.md`。

### 5.7 Pi (`pi`)

**配置根**：`~/.pi`

**MCP**

| 项 | 值 |
|----|-----|
| 内置扩展 | `builtin:mcp`：0.99.0 起内置，1.x 默认开启；**不**依赖社区 pi-mcp-adapter |
| 全局文件 | `~/.pi/agent/mcp.json`（顶层 `mcpServers`，方言 `JsonMcpServers`） |
| 项目文件 | `<repo>/.pi/mcp.json`（单层 `.pi`，无 `agent` 层） |
| stdio | `command` + `args` + `env` |
| 远程 | streamable HTTP；**仅支持 stdio 与 streamable HTTP，拒绝 legacy SSE**（按 agent 名拦截，不写文件并报错） |

**Skills**

| 项 | 值 |
|----|-----|
| 全局 | `~/.pi/agent/skills/<name>/SKILL.md` |
| 项目 | 兼容 `.agents/skills`，不建 `.pi/skills` 自建目录 |

**模型配置**（`pi_model_config.rs` 后端）：

| 项 | 值 |
|----|-----|
| 模型文件 | `~/.pi/agent/models.json`（JSON，顶层 `providers`） |
| 密钥文件 | `~/.pi/agent/auth.json`（`{ providerId: { type: "api_key", key } }`） |
| 默认模型 | `~/.pi/agent/settings.json` 的 `defaultProvider` / `defaultModel` |

字段映射到通用 DTO：`contextWindow`→`limitContext`、`maxTokens`→`limitOutput`、
`input`→`modalitiesInput`；未建模字段进 `extraOptions` 原样往返。列表 DTO **永不**回传明文 API Key。

---

## 6. AgentBuddy 表单 → 各方言字段映射

UI 模型（`McpManage.tsx`）：

- `title`
- `type`: `stdio` | `http` | `sse`
- `command` / `args[]` / `env{}`
- `url` / `headers{}`

| UI | `ClaudeJsonUser` / `JsonMcpServers`（Claude Code / Desktop / WorkBuddy / Pi） | `JsonMcp`（OpenCode） | `TomlMcpServers`（Codex） | `JsonAntigravity`（Antigravity） |
|----|------------------------------------------------------|------------------------------|-----------------------------|---------------------|
| title | `mcpServers[title]` | `mcp[title]` | `mcp_servers.title` | `mcpServers[title]` |
| stdio | `command` + `args` + `env`；可选 `type: "stdio"` | `type: "local"` + `command: [cmd, ...args]` + `environment` | **不写 `type`**；`command` + `args` + `[.env]` + `enabled = true` | `command`（字符串）+ `args[]` + `env{}`；不写 `type` |
| http | `type: "http"` + `url` + `headers` | `type: "remote"` + `url` + `headers` | **不写 `type`**；`url` + `http_headers` + `enabled = true` | 不建模（仅 stdio，非 stdio 报错不落盘） |
| sse | `type: "sse"` + `url` + `headers` | 映射为 `remote`（或文档等价） | 若官方仅 http，降级为 http 或跳过并提示 | 不支持；不写 |
| enabled | 可选 | 常写 `enabled: true` | `enabled = true` | 无统一字段时省略 |

> Pi 的 SSE 在应用与项目两条路径都按 agent 名拦截：`agent == "pi" && transport == "sse"`
> 直接报错「Pi 内置 MCP 仅支持 stdio 与 streamable HTTP，不支持 SSE」，不写文件。
> `JsonMcpServers` 方言被 claude-desktop / workbuddy / pi 共用，拦截依据是 agent 名而非方言。

---

## 7. 全局应用 / 删除 — 统一算法（已实现）

实现入口：`mcp_config::apply_draft_to_file` / `remove_from_file`，按 `McpDialect` 分派到对应写器。

### 7.1 Apply（保存并应用）

```
输入: McpDraft, selectedAgentNames[]
for agent in selectedAgents:
  spec    = agents::find(agent.name)               # AgentSpec + McpSpec
  path    = spec.mcp.path.resolve(...)             # 固定路径 / scan / 优先选择
  dialect = spec.mcp.dialect
  doc     = read_json_or_toml(path) or empty_doc(dialect)
  entry   = draft_from_ui(dialect, draft)          # 字段映射（见 §6）
  doc     = upsert_by_title(doc, draft.title, entry, dialect)
  atomic_write(path, doc)                          # temp + rename
返回值: List<{agent, path, ok, message}>           # 仅 ok=true 的写 appliedAgents
```

原则：

1. **只写用户全局**（见 §4 总览表）
2. **同名覆盖，异名保留** — 不动文件中其它顶层键
3. **原子写**，避免半截 JSON/TOML
4. 共享物理根的多个 agent 同时勾选 → `dedupe_write_targets` 一次写入
5. Claude Code → 只动 `~/.claude.json` 的 `mcpServers`，不动其它大字段
6. **partial success 透明**：apply 失败的部分不再把该 agent 标记为已应用
7. `test_mcp_connection` 仅为运行时 probe（stdio 拉起进程 + 发送 `initialize`；
   http/sse POST `initialize`），**永不写** 配置文件

### 7.2 Delete（列表删除 + 可选同步配置）

```
if removeFromAgentConfigs:
  for agent in server.appliedAgents:
    spec = agents::find(agent)
    path = spec.mcp.path.resolve(...)
    doc  = read(path)
    doc  = remove_by_title(doc, server.title, dialect)
    atomic_write(path, doc)
从 SQLite 移除 server 行（不再有 localStorage 镜像）
```

原则：

1. 仅删除目标 server name
2. 不删除整个配置文件
3. WorkBuddy / Claude Desktop 等可能含非用户 server，禁止清空 `mcpServers`

---

## 8. 项目级 MCP / Skills（项目配置页）

「项目配置」初始化（`init_project_config`）时，用户可从 AgentBuddy 已配置的 MCP / Skills 中勾选，
落盘到所选项目目录（与全局应用互不影响）。

### 8.1 项目级 MCP 目标文件（来自 `project_config::AGENT_SPECS`）

| sniff `name` | 项目级 MCP 文件（相对项目根） | 方言 | 自建 skills 目录 |
|--------------|-------------------------------|------|:---:|
| `claude-code` | `.mcp.json`（仓库根） | `JsonMcpServers` | ✅（原生 `.claude/skills`，链到 `.agents/skills`） |
| `codex` | `.codex/config.toml` | `TomlMcpServers` | ❌（原生读 `.agents/skills`） |
| `opencode` | `opencode.json` | `JsonMcp` | ❌（兼容 `.agents/skills`） |
| `antigravity` | `.agents/mcp_config.json`（不建 `.gemini`） | `JsonAntigravity` | ❌（原生 `.agents/skills`） |
| `workbuddy` | `.workbuddy/mcp.json` | `JsonMcpServers` | ✅（`.workbuddy/skills`） |
| `pi` | `.pi/mcp.json`（单层，无 `agent` 层） | `JsonMcpServers` | ❌（兼容 `.agents/skills`，不建 `.pi/skills`） |
| `claude-desktop` | — | ❌（桌面应用，不做项目级） | — |

**写策略**：复用 `mcp_config::apply_draft_to_file` 的方言写器，**按 server title 合并**、保留文件其它键、
原子写；不参与「覆盖/跳过」确认。claude-code 的仓库根 `.mcp.json` 与 workbuddy 的
`.workbuddy/mcp.json` 是两个独立文件，分别写入，不再去重为同一目标。

### 8.2 项目级骨架目录与 Skills

- 统一安装到 `<repo>/.agents/skills/<id>`（完整复制或软链接，源为 `~/.octopus/skills` 技能库）。
- 每个 agent 的 `skills_own_dir` 布尔决定自建 skills 目录：
  - **true**（`claude-code` / `workbuddy`）：Full / Symlink 模式勾选 skills 时把
    `<config_dir>/skills` 相对链接到 `.agents/skills`；
  - **false**（`codex` / `opencode` / `antigravity` / `pi`）：直接依赖通用 `.agents/skills`，
    不建不链自己的 skills 目录。
- 其它 Full 骨架子目录：`claude-code` → `.claude/{commands,agents}`；`opencode` →
  `.opencode/{agents,commands,plugins,tools}`（复数）；`workbuddy` → `.workbuddy/{rules,skills}`；
  `codex` / `antigravity` / `pi` 无额外子目录。
- 提示文件：AGENTS.md 在项目最上方统一写一次；`claude-code` / `antigravity` 的 `root_file` 为
  None，**不生成** CLAUDE.md / GEMINI.md 指针。
- 安全语义：非空真实目录拒绝删除；overwrite 可替换软链接/空目录。

### 8.3 不支持项目级

- `claude-desktop` — 桌面应用，不写项目树

---

## 9. 风险与决策（当前状态）

| 项 | 状态 | 处理 |
|----|------|------|
| Claude Desktop Skills | 账户同步模型，无本地文件目录 | Skills 页置灰；只做 MCP |
| Antigravity MCP | 闭源 App/IDE/CLI，非 gcli | 只写 `~/.gemini/config/mcp_config.json`（`JsonAntigravity`，**仅 stdio**）；远程 `serverUrl`/oauth 不建模；gcli 的 `settings.json`/`httpUrl` 口径已移除 |
| WorkBuddy 远程格式 | 官方未披露（待核实） | 当前按通用 http `url`+`headers` 写 `~/.workbuddy/mcp.json`；`~/.workbuddy/skills` 路径半官方（待核实） |
| Codex Skills | 现行官方 USER 位仅 `~/.agents/skills` | 只写 `~/.agents/skills`，不保留 legacy `~/.codex/skills` |
| Pi MCP | 内置 `builtin:mcp`（0.99.0 起，1.x 默认开） | 不再依赖社区 pi-mcp-adapter；仅 stdio / streamable HTTP，SSE 按 agent 名拦截 |
| Claude Code 大文件 | `~/.claude.json` 含大量状态字段 | 只改 `mcpServers` 键，独立方言 `ClaudeJsonUser` |
| 项目级 MCP | 写项目会进 git / 需信任 | 全局应用默认不写项目；项目级写入由用户在「项目配置」页显式勾选触发 |
| SSE 方言差异 | 各产品支持不一 | Pi 直接拒绝 SSE；其余能映射则映射，否则提示跳过 |
| 密钥 | 配置中常含 token | AgentBuddy 永不日志打印 env/headers 值；API Key 经 `crypto::encrypt` 落 SQLite |
| OpenCode JSONC | 注释 / 尾逗号 | `json5` 解析；重写 pretty JSON（注释不保留，会抛 warn 给用户） |
| partial failure | apply 批次中部分 agent 失败 | UI 只把 ok=true 的 agent 写进 `appliedAgents` |
| Pi 模型配置 | JSON 解析（`~/.pi/agent/models.json`） | 字段映射到统一 DTO，列表 DTO 不回传明文 Key |
| 内部冒烟 server | 嗅探时混入 | 嗅探忽略 `__agentbuddy` 前缀 title |

---

## 10. 来源索引

### 官方 / 准官方

- Claude Code MCP / Settings / Skills：https://code.claude.com/docs/en/mcp 、https://code.claude.com/docs/en/settings 、https://code.claude.com/docs/en/skills
- Claude Desktop MCP：https://modelcontextprotocol.io/docs/develop/connect-local-servers
- Codex Config / MCP / Skills / AGENTS.md：https://developers.openai.com/codex/config-reference 、https://developers.openai.com/codex/mcp 、https://developers.openai.com/codex/skills 、https://developers.openai.com/codex/guides/agents-md
- OpenCode Config / MCP / Skills：https://opencode.ai/docs/config/ 、https://opencode.ai/docs/en/mcp-servers 、https://opencode.ai/docs/skills/
- Antigravity（闭源 App/IDE/CLI，非 gcli）：MCP https://antigravity.google/docs/mcp/ 、Skills https://antigravity.google/docs/skills/ 、Rules https://antigravity.google/docs/rules/
- WorkBuddy 桌面（MCP 指南；远程格式与 skills 路径待官方复核）：https://www.workbuddy.ai/docs/zh/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/MCP-Guide
- Pi（内置 builtin:mcp）：https://pi.dev/docs/latest/mcp 、https://pi.dev/docs/latest/configuration 、https://pi.dev/docs/latest/skills 、仓库 https://github.com/badlogic/pi-mono

### 工程内对照

- 嗅探定义 / Agent 注册表：[`src-tauri/src/agents.rs`](src-tauri/src/agents.rs) （`AgentSpec` / `McpDialect` / `McpPath`）
- 嗅探实现：[`src-tauri/src/sniff.rs`](src-tauri/src/sniff.rs)
- MCP 写器（apply / remove / sniff）：[`src-tauri/src/mcp_config.rs`](src-tauri/src/mcp_config.rs)
- Skills 库：[`src-tauri/src/skills.rs`](src-tauri/src/skills.rs)
- 项目级骨架 + 项目级 MCP：[`src-tauri/src/project_config.rs`](src-tauri/src/project_config.rs) （`AGENT_SPECS`）
- MCP UI 模型：[`src/components/pages/McpManage.tsx`](src/components/pages/McpManage.tsx)
- 项目级 UI：[`src/components/pages/ProjectConfig.tsx`](src/components/pages/ProjectConfig.tsx) +
  [`src/components/pages/project-config/types.ts`](src/components/pages/project-config/types.ts) （`AGENT_PROJECT_INFOS`，与 `project_config.rs` 同步）

---

## 11. 结论（可执行摘要）

1. **5 种 MCP 方言**覆盖 7 个 Agent：`TomlMcpServers` / `ClaudeJsonUser` / `JsonMcpServers` /
   `JsonMcp` / `JsonAntigravity`（其中 `ClaudeJsonUser` 为 Claude Code 独立方言，因其文件位置
   `~/.claude.json` 特殊）。
2. **7 个 Agent**（2026-10）：`codex` / `claude-code` / `claude-desktop` / `opencode` /
   `antigravity` / `workbuddy` / `pi`。历史下线的 agent 由启动时配置升级统一清理残留。
3. **Skills** 主流是 `<root>/skills/<name>/SKILL.md`，项目优先复用通用 `.agents/skills`；
   Claude Desktop 无本地文件 skills（账户同步）；Codex 只写 `~/.agents/skills`，不保留 legacy。
4. **全局应用**默认只写用户级文件；Claude Code 写 `~/.claude.json` 顶层 `mcpServers`，
   Antigravity 写 `~/.gemini/config/mcp_config.json`，**仅 stdio**，远程不建模。
5. **项目级** MCP 由 `project_config::AGENT_SPECS` 决定落地路径；目前支持 6 个 agent
   （claude-code / workbuddy / codex / opencode / antigravity / pi）。
6. **OpenCode** 用 `mcp` 顶层键 + `local`/`remote`，不是 `mcpServers`；JSONC 用 `json5`。
7. **Pi** 内置 `builtin:mcp`（不再依赖 pi-mcp-adapter），用 `JsonMcpServers` 方言，仅 stdio /
   streamable HTTP，拒绝 SSE；模型配置经通用 `agent_model_config` + 各自后端
   （`opencode_config` / `pi_model_config`）。
8. **改动路径**：改 agents / 方言 → 先改 `agents.rs` → 同步本文件 + `AGENT_PROJECT_INFOS`
   （前端 mirror）。
