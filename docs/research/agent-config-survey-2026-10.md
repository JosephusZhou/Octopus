# 各 Agent 配置现状调研报告（MCP / Skills / 指令文件）

- 调研日期：2026-10-21（Asia/Shanghai）
- 目的：为优化本应用（AgentBuddy/Octopus）的「项目配置」功能，逐项对照**应用当前实现**与**各 Agent 官方最新现状**，列出差距与建议动作。
- 方法：只读仓库基线（`src-tauri/src/agents.rs`、`src-tauri/src/project_config.rs`）+ 联网检索官方文档/官方仓库；未改任何源码、未编译。
- 确定性标注：
  - **【官方明确】**：官方文档 / 官方仓库 README/docs/源码直接记载。
  - **【社区/半官方】**：仅见社区文章、技术百科或第三方镜像，需二次核实。
  - **【待核实】**：官方未记载、本次未能证实，禁止臆测补全。
- 版本快照（2026-10-21 npm registry / 官方文档）：Claude Code v2.1.x（AGENTS.md 直读需 v2.1.277+）；Codex CLI `@openai/codex` 0.163.0；OpenCode `opencode-ai` 1.19.3；Pi `@earendil-works/pi-coding-agent` 1.3.0。

---

## 0. 摘要（最重要的差距）

1. **Pi 已原生内置 MCP（用户判断属实）**：Pi 自 0.99.0（2026-09-29）起内置 `builtin:mcp`，1.x 默认开启。全局文件 `~/.pi/agent/mcp.json` 与应用现状一致，但**项目级官方位置是 `.pi/mcp.json`（无 `agent/` 层）**，应用当前写的 `.pi/agent/mcp.json` 不被内置 MCP 读取；项目 skills 原生目录是 `.pi/skills/`。
2. **Claude Code 已原生支持项目根 `AGENTS.md`（用户判断属实）**：v2.1.277+ 直读；但默认与 CLAUDE.md **互斥**（有 CLAUDE.md 就不读 AGENTS.md），`claude-md-and-agents-md` 模式才拼接。应用「CLAUDE.md 指针指向 AGENTS.md」的 `@AGENTS.md` import 仍是兼容旧版本的合法兜底，但已非必需。
3. **应用把「Gemini CLI（gcli）」与「Google Antigravity」两套产品混为一个 agent**：两者 MCP/Skills/指令约定完全不同（gcli 用 `~/.gemini/settings.json` + `httpUrl`；Antigravity 用 `~/.gemini/config/mcp_config.json` + `serverUrl`、项目 `.agents/mcp_config.json`）。需先明确注册的 `antigravity` 到底面向哪一个。
4. **应用把「WorkBuddy 桌面办公 Agent」与「CodeBuddy Code CLI」混为一个 agent**：桌面版官方全局 MCP 是 `~/.workbuddy/mcp.json`（无点前缀，应用写成 `.mcp.json`）；CodeBuddy CLI 是另一套 `~/.codebuddy/.mcp.json` / 项目根 `.mcp.json`。
5. **Codex 项目级 MCP 位置 `.codex/config.toml` 正确**，但仅 trusted 项目加载；远程 schema 无 `type` 字段、头字段是 `http_headers`；应用写的 `.codex/instructions.md` 官方现行文档未记载。
6. **OpenCode 项目 MCP 位置/键正确**；但 Full 模式子目录官方现行用复数 `agents/ commands/ plugins/ tools/`，应用写的是单数。
7. **Claude Desktop**：legacy `claude_desktop_config.json` 机制官方确认仍存在，但官方主推 .mcpb 扩展；无项目级、无本地文件 skills（聊天 App skills 走账户同步）。

---

## 1. Pi（`@earendil-works/pi-coding-agent`，仓库 badlogic/pi-mono → earendil-works/pi）

> 官方文档：https://pi.dev/docs/latest/mcp ・ https://pi.dev/docs/latest/configuration ・ https://pi.dev/docs/latest/skills ・ https://pi.dev/docs/latest/settings ・ https://pi.dev/changelog

### 1.1 全局 MCP【官方明确】

- **Pi 已内置 MCP client**：0.99.0（2026-09-29）起作为内置扩展 `builtin:mcp` 提供，1.x 默认开启；可在 `pi config` 关闭、settings 写 `"extensions": ["-builtin:mcp"]` 或单次 `--no-mcp`。
- 全局文件：**`~/.pi/agent/mcp.json`**（agent 目录可用 `PI_CODING_AGENT_DIR` 覆盖），标准 JSON，顶层键 `mcpServers`。**与应用现状一致。**
- 传输：仅 stdio + streamable HTTP，**拒绝 legacy SSE**（SSE 仅第三方扩展支持）。
- 字段：
  - stdio：`command`（可执行文件，非 shell 字符串）、`args[]`、`env{}`、`cwd`。
  - HTTP：`url`、`headers{}`、`oauth{}`；可选 `type: stdio|http|streamable-http`（缺省按 command/url 推断）。
  - 公共：`enabled`、`description`、`timeout`（秒，默认 60）、`exposure`（codemode/deferred/direct/hidden）、`toolExposure`。
  - `env`/`headers` 支持 `${VAR}` 与整值 `!command`；OAuth token 存 `~/.pi/agent/mcp-auth.json`。
- 内置 MCP **不读**标准 `.mcp.json` 与 `~/.config/mcp/mcp.json`（那是社区 pi-mcp-adapter 的位置）。

### 1.2 项目级 MCP【官方明确】

- 文件：**`<repo>/.pi/mcp.json`**（**不是** `.pi/agent/mcp.json`，项目侧无 `agent/` 层）；仅在 project trust 后读取。
- 作用域只有 global / project 两层，无私有 local 文件；`pi mcp add --local` 写的就是 `.pi/mcp.json`。
- 合并：同名 server 项目条目**替换**全局条目；项目条目若不含 `command/url/type`，则仅作覆盖项（只能改 `enabled/exposure/toolExposure`，保留全局 env/headers/auth）。
- 项目文件**不允许**给 HTTP server 配 `auth`（OAuth 凭据只写全局）。

### 1.3 Skills【官方明确】

- 用户级：`~/.pi/agent/skills/<name>/SKILL.md`（递归扫描，含 SKILL.md 的目录即 skill 根）。**与应用现状一致。**
- 项目级：**`.pi/skills/<name>/SKILL.md`**（原生）；另兼容 Agent Skills 位置项目 `.agents/skills/`（cwd 向祖先扫描到仓库根停）、全局 `~/.agents/skills/`。
- SKILL.md frontmatter：`name`、`description`（必填）、`license`、`compatibility`、`metadata`、`allowed-tools`、`disable-model-invocation`；遵循 agentskills.io 开放规范，渐进式披露。

### 1.4 指令文件【官方明确】

- Context 文件（**无需 trust**，全局 + cwd 逐级向上继承），每目录取第一个存在者，优先级：
  **`AGENTS.override.md` > `AGENTS.md` > `AGENTS.MD` > `CLAUDE.md` > `CLAUDE.MD`**。
- 全局：`~/.pi/agent/AGENTS.md`（及上述变体）；项目：仓库根 `AGENTS.md`，祖先目录逐级拼接继承。
- 另有 `.pi/SYSTEM.md`（替换系统提示）/ `.pi/APPEND_SYSTEM.md`（追加），trust 后项目覆盖全局、**不合并**；不读 PI.md，不把 README 当指令。

### 1.5 版本线索【官方明确 / registry 实测】

- 当前包 `@earendil-works/pi-coding-agent@1.3.0`（2026-10-15，Node ≥22.19）；旧 `@mariozechner/*` 冻结于 0.73.1，早期 `@mariozechner/pi-agent` 止于 0.9.0；GitHub `badlogic/pi-mono` 301 → `earendil-works/pi`。
- 待核实：0.99.x 的 npm 发布渠道（changelog 版本与旧 scope 对不上）；内置是否回退扫描 `.pi/agent/skills`。

---

## 2. Claude Code（code.claude.com，v2.1.x）

> 官方文档：https://code.claude.com/docs/en/mcp ・ https://code.claude.com/docs/en/mcp-quickstart ・ https://code.claude.com/docs/en/memory ・ https://code.claude.com/docs/en/skills ・ https://code.claude.com/docs/en/claude-directory ・ https://code.claude.com/docs/en/subagents

### 2.1 全局 MCP【官方明确】

- `~/.claude.json` 顶层 `mcpServers`（user scope）；local scope 存 `projects[<绝对路径>].mcpServers`。**与应用现状一致，未改名/迁移。**
- Windows `%USERPROFILE%\.claude.json`；`CLAUDE_CONFIG_DIR` 改址。

### 2.2 项目级 MCP【官方明确】

- 仓库根 **`.mcp.json`**，顶层 `mcpServers`，可提交 git（团队共享）。**位置/格式与应用一致。**
- 条目：stdio `command/args/env`；http `type:"http"`（`streamable-http` 别名）`url/headers`；sse（已弃用）；ws。可选 `headersHelper/oauth/timeout/alwaysLoad`。
- **条目内无官方 `enabled` 字段**：审批/拒绝走 settings 的 `enabledMcpjsonServers` / `enableAllProjectMcpServers` / `disabledMcpjsonServers`。
- 交互式会话首次使用需 approval；未信任目录里项目 settings 的自动批准被忽略，服务器停在 Pending。
- scope 同名取整条、**不跨 scope 合并字段**：Local > Project > User > Plugin > connector，managed 最高。

### 2.3 Skills【官方明确】

- 全局 `~/.claude/skills/<name>/SKILL.md`（**与应用一致**）；项目 **`.claude/skills/<name>/SKILL.md`**（可提交 git）。
- `.claude/commands/*.md` 为 legacy 单文件命令（仍可用，已并入 skills 机制）；`.claude/agents/*.md` 是 subagents 目录（项目可提交 git）。应用 Full 模式子目录 `commands`、`agents` 均仍成立。

### 2.4 指令文件（CLAUDE.md / AGENTS.md）【官方明确】

- **原生直读项目根 `AGENTS.md` 需 v2.1.277+**（v2.1.281 修部分会话只读 CLAUDE.md，v2.1.285 插件 ID 改名）。
- 默认模式 `claude-md-or-agents-md`：**有 CLAUDE.md / `.claude/CLAUDE.md` / `CLAUDE.local.md` 就读 CLAUDE.md，不读 AGENTS.md（互斥，不拼接）**；`claude-md-and-agents-md` 模式两者都读、CLAUDE.md 在前拼接。
- 层级：managed policy → `~/.claude/CLAUDE.md` → 项目 CLAUDE.md → `CLAUDE.local.md`；目录向上发现、全部拼接。
- 不读 `AGENTS.local.md`/`AGENTS.override.md`/`.agents/` 目录。

---

## 3. Claude Desktop（传统聊天 App）

> 官方：https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop ・ https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp ・ https://code.claude.com/docs/en/desktop

### 3.1 MCP【官方明确（机制）/ 路径待核实】

- legacy `claude_desktop_config.json`（全局单文件、顶层 `mcpServers`、stdio 为主）机制**官方确认仍存在**，`claude mcp add-from-claude-desktop` 可读；但官方已主推 **.mcpb Desktop Extensions**，帮助中心不再引导手编 JSON。
- 精确落盘路径官方页面未重述（通行 macOS `~/Library/Application Support/Claude/claude_desktop_config.json`、Windows `%APPDATA%\Claude\`），**待核实**；应用的 `Claude*` 通配扫描可保留作兼容。
- **无官方项目级 MCP**（全局单文件，负面证据，确定性中）。

### 3.2 Skills / 指令文件【官方明确】

- 聊天/Cowork 的 skills 走**账户同步**（Customize > Skills），不读本地 `~/.claude/skills`；本地文件 skills 是 Claude Code 特性。应用「不支持本地 skills」结论方向正确，表述应从「云端 VM」更新为「账户同步模型」。
- 无指令文件概念。

---

## 4. Codex CLI（`@openai/codex` 0.163.0）

> 官方：https://developers.openai.com/codex/mcp ・ https://developers.openai.com/codex/config-reference ・ https://developers.openai.com/codex/guides/agents-md ・ https://developers.openai.com/codex/skills

### 4.1 全局 MCP【官方明确】

- `~/.codex/config.toml` 的 `[mcp_servers.*]`，**与应用一致、仍有效**。
- stdio：`command`（字符串）、`args[]`、`env{}`、`env_vars`（父环境白名单）、`cwd`。
- 远程 streamable HTTP：**无 `type` 字段**，出现 `url` 即 HTTP；`url` + `bearer_token_env_var` + `http_headers` / `env_http_headers`（**不是 `headers`**）。

### 4.2 项目级 MCP【官方明确】

- 位置 **`.codex/config.toml`**（与应用一致），同样 `[mcp_servers.*]`；**仅 trusted 项目加载**（`projects.<path>.trust_level`）。
- 项目级被忽略的键：`openai_base_url`、`chatgpt_base_url`、`model_provider(s)`、`notify`、`profile(s)`、`otel` 等（供应商/通知/遥测必须放用户级）。
- 与用户级叠加（project-scoped overrides）；深合并还是整表覆盖**待核实**。

### 4.3 Skills【官方明确】

- USER：**`$HOME/.agents/skills`**（现行官方位置）；REPO：`$CWD/.agents/skills`、`$REPO_ROOT/.agents/skills`；ADMIN：`/etc/codex/skills`。
- **`~/.codex/skills` 未列入现行扫描**（legacy，源码兼容性待核实）——应用全局 skills roots 同时写了 `~/.codex/skills` 与 `~/.agents/skills`，前者应降级为 legacy 兼容。
- 无 `.codex/skills` 项目级目录约定。

### 4.4 指令文件【官方明确】

- 全局：`~/.codex/AGENTS.md`（或 `AGENTS.override.md`，存在则只用它）；项目：git root → CWD 逐层 AGENTS.md，根向下拼接、后者覆盖、32 KiB 上限。
- **官方现行文档未记载 `.codex/instructions.md` / `~/.codex/instructions.md` 自动加载**（早期遗留，待核实）——应用在项目骨架里写 `instructions.md` 无官方依据，建议改为 AGENTS.md 体系。

---

## 5. OpenCode（`opencode-ai` 1.19.3，stable docs）

> 官方：https://opencode.ai/docs/config/ ・ https://opencode.ai/docs/en/mcp-servers ・ https://opencode.ai/docs/en/rules/ ・ https://opencode.ai/docs/skills/

### 5.1 全局 MCP【官方明确】

- `~/.config/opencode/opencode.json`（JSON/JSONC），顶层 `mcp` 键（非 mcpServers）。**与应用一致。**
- local：`type:"local"`、`command`（**数组**）、`cwd`、`environment`（env 键名）、`enabled`、`timeout`。
- remote：`type:"remote"`、`url`、`enabled`、`headers`、`oauth`、`timeout`。

### 5.2 项目级 MCP【官方明确】

- 仓库根 **`opencode.json`**（可安全提交 git，合并非替换），同一 `mcp` schema。**位置/键与应用一致。**
- 官方未记载 `opencode.local.json`（社区 gitignored 惯例，待核实）；`.opencode/opencode.json` 是未发布 V2 预览新增，非现行。

### 5.3 Skills【官方明确】

- 项目 `.opencode/skills/<name>/SKILL.md`；全局 `~/.config/opencode/skills/`（**与应用一致**）；兼容位 `.claude/skills`、`.agents/skills`。

### 5.4 指令文件【官方明确】

- 原生 AGENTS.md：项目根 + 全局 `~/.config/opencode/AGENTS.md`；`instructions` 数组合并文件/glob/URL；每类**首个命中胜出**（非逐层拼接）；无 AGENTS.md 时回退 CLAUDE.md（`~/.claude/CLAUDE.md`）。

### 5.5 目录体系（Full 模式子目录）【官方明确】

- 现行规范用**复数**：`agents/`、`commands/`、`plugins/`、`tools/`（另有 `modes/`、`skills/`、`themes/`）；单数仅向后兼容。应用写的 `agent/ command/ plugin/ tool`（单数）可暂时兼容，建议迁复数。

---

## 6. Gemini CLI（gcli，google-gemini/gemini-cli，开源）

> 官方：https://goo.gle/gemini-cli-docs-mcp ・ https://geminicli.com/docs/reference/configuration ・ https://geminicli.com/docs/cli/gemini-md/ ・ https://geminicli.com/docs/cli/skills/ ・ https://geminicli.com/docs/cli/custom-commands.md

### 6.1 全局 MCP【官方明确】

- `~/.gemini/settings.json` 顶层 `mcpServers`，**仍有效（与应用现状一致）**；另有顶层 `mcp.allowed/excluded/serverCommand`。
- stdio `command/args/env/cwd/timeout/trust`；SSE `url`；Streamable HTTP `httpUrl`；`headers`；`includeTools/excludeTools`；`oauth`/`authProviderType`。传输优先级 `httpUrl > url > command`。

### 6.2 项目级 MCP【官方明确】

- **`<root>/.gemini/settings.json`** 的 `mcpServers`（**与应用现状一致**）；`gemini mcp add -s project` 默认写项目层。
- 四层 settings 合并：System Defaults → User → Workspace → System Overrides；mcpServers 对象合并、同名 Workspace 覆盖 User。

### 6.3 Skills【官方明确】

- User `~/.gemini/skills/`（别名 `~/.agents/skills/`，同层别名优先）；Workspace `.gemini/skills/`（别名 `.agents/skills/`，可提交）。**与应用全局 roots 一致（可补 `~/.agents/skills` 别名）。**

### 6.4 指令文件【官方明确】

- 默认 **GEMINI.md**（全局 `~/.gemini/GEMINI.md` + 项目根/父目录向上扫描，拼接）；**AGENTS.md 非默认**，需在 `context.fileName` 显式列出才读。

### 6.5 Commands【官方明确】

- 全局 `~/.gemini/commands/*.toml`、项目 `.gemini/commands/*.toml`（可提交），与应用 Full 子目录 `commands` 对应（格式为 TOML）。

---

## 7. Google Antigravity（闭源 App / IDE / CLI）

> 官方：https://antigravity.google/docs/mcp/ ・ https://antigravity.google/docs/skills/ ・ https://antigravity.google/docs/rules/ ・ https://antigravity.google/docs/cli/gcli-migration/
> 注意：**Antigravity ≠ Gemini CLI**，约定完全不同。

### 7.1 MCP【官方明确】

- 全局：**`~/.gemini/config/mcp_config.json`**（独立 profile，不再内嵌 settings.json）。
- 项目：**`<root>/.agents/mcp_config.json`**。
- 远程字段是 **`serverUrl`**（**不支持 legacy `url`/`httpUrl`**）；stdio `command/args/env/cwd`；另有 `authProviderType`/`oauth`/`disabled`/`disabledTools`。

### 7.2 Skills【官方明确（分 surface）】

- Workspace 统一 `.agents/skills/<name>/`（可提交 VCS）。
- App/IDE 全局 `~/.gemini/config/skills/`（IDE legacy `~/.gemini/antigravity/skills/`）；**CLI** 全局才是 `~/.gemini/antigravity-cli/skills/`。

### 7.3 指令文件 / Rules【官方明确】

- **AGENTS.md 与 GEMINI.md 原生都读**：项目任意目录 + `.agents/rules/*.md`；全局 `~/.gemini/AGENTS.md`、`~/.gemini/GEMINI.md`、`~/.gemini/config/*`；累积合并、目录级更具体优先；单文件 24KB。

### 7.4 与 gcli 的迁移映射【官方明确】

| 配置 | gcli | Antigravity CLI |
|---|---|---|
| 全局 skills | `~/.gemini/skills/` | `~/.gemini/antigravity-cli/skills/` |
| 工作区 skills | `.gemini/skills/` | `.agents/skills/` |
| 全局 MCP | `~/.gemini/settings.json` | `~/.gemini/config/mcp_config.json` |
| 工作区 MCP | `.gemini/settings.json` | `.agents/mcp_config.json` |
| 远程 schema key | `url`/`httpUrl` | `serverUrl` |

---

## 8. 腾讯 WorkBuddy / CodeBuddy Code CLI（需分产品）

> 桌面官方：https://www.workbuddy.ai/docs/zh/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/MCP-Guide ・ https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Setting
> CodeBuddy CLI 官方：https://www.codebuddy.ai/docs/cli/mcp ・ https://www.codebuddy.ai/docs/cli/codebuddy-dir ・ https://www.workbuddy.ai/docs/cli/memory ・ https://www.workbuddy.ai/docs/cli/skills

### 8.1 WorkBuddy 桌面办公 Agent

- **全局 MCP【官方明确】**：`~/.workbuddy/mcp.json`（**无点前缀**，应用写成 `~/.workbuddy/.mcp.json` 不符）；项目 `.workbuddy/mcp.json`；官方仅给 stdio（`command/args/env`）示例，远程 `url/headers` **待核实**。
- 代码开发模式兼容加载用户级 `.codebuddy`（规则/skills/命令/MCP）；项目命令在 `.codebuddy/commands/`。
- Skills `~/.workbuddy/skills/`、项目 `.workbuddy/skills/`、是否读 AGENTS.md：**仅社区/技术百科出处，待官方复核**。

### 8.2 CodeBuddy Code CLI（腾讯云 1831 口径）

- **MCP【官方明确】**：USER `~/.codebuddy/.mcp.json`（推荐）→ `mcp.json`（deprecated）→ `~/.codebuddy.json`（legacy）；PROJECT 仓库根 `.mcp.json`（推荐）→ `mcp.json`；LOCAL 在用户文件 `projects` 字段；优先级 local > project > user。JSONC，`type: stdio|sse|http` + `url` + `headers`，`${VAR}` 展开，项目级首次连接需批准。
- **指令【官方明确】**：`CODEBUDDY.md`（用户 `~/.codebuddy/CODEBUDDY.md`、项目根或 `.codebuddy/CODEBUDDY.md`）优先，**缺省兜底 AGENTS.md**；项目 rules `.codebuddy/rules/*.md`；私有 `CODEBUDDY.local.md`。
- **Skills【官方明确】**：`~/.codebuddy/skills/`、`.codebuddy/skills/`；commands/agents 在 `.codebuddy/` 下。

---

## 9. 总对照表：当前实现 vs 官方现状

> 「应用」列来自 `agents.rs`（全局）与 `project_config.rs`（项目）；「官方」列来自上文各节。✅ 一致 / ⚠️ 部分差异 / ❌ 不符。

### 9.1 全局（用户级）

| Agent | 应用全局 MCP | 官方全局 MCP | 应用全局 Skills | 官方全局 Skills | 应用指令文件 | 官方指令文件 |
|---|---|---|---|---|---|---|
| codex | `~/.codex/config.toml` `[mcp_servers.*]` TOML ✅ | 同左 ✅（远程无 `type`，头 `http_headers`） | `~/.codex/skills`、`~/.agents/skills` ⚠️ | `$HOME/.agents/skills`（`~/.codex/skills` legacy 待核实） | AGENTS.md ✅ | `~/.codex/AGENTS.md` / `AGENTS.override.md` ✅ |
| claude-code | `~/.claude.json` `mcpServers` ✅ | 同左 ✅ | `~/.claude/skills` ✅ | 同左 ✅ | CLAUDE.md ⚠️ | v2.1.277+ 原生 AGENTS.md，默认与 CLAUDE.md 互斥 |
| claude-desktop | 扫描 `Application Support/Claude*/claude_desktop_config.json` ⚠️ | 机制仍存，路径官方未重述（待核实）；主推 .mcpb | 无 ✅ | 无（账户同步）✅ | 无 ✅ | 无 ✅ |
| opencode | `~/.config/opencode/opencode.{jsonc,json}` 顶层 `mcp` ✅ | 同左 ✅（env 键名 `environment`，command 数组） | `~/.config/opencode/skills` ✅ | 同左（另兼容 .claude/.agents）✅ | AGENTS.md ✅ | 项目根 + `~/.config/opencode/AGENTS.md` ✅ |
| antigravity（gcli 口径） | `~/.gemini/settings.json` `mcpServers`，远程 `httpUrl` ✅ | gcli 同左 ✅；Antigravity App 则是 `~/.gemini/config/mcp_config.json` + `serverUrl` ❌ | `~/.gemini/skills`、`~/.gemini/antigravity-cli/skills` ⚠️ | gcli：`~/.gemini/skills`（别名 `~/.agents/skills`）；Antigravity App：`~/.gemini/config/skills/`，CLI：`~/.gemini/antigravity-cli/skills/` | GEMINI.md ⚠️ | gcli 默认 GEMINI.md（AGENTS.md 需 `context.fileName`）；Antigravity 两者都读 |
| workbuddy | `~/.workbuddy/.mcp.json` `mcpServers` ❌ | 桌面 `~/.workbuddy/mcp.json`（无点）；CodeBuddy CLI `~/.codebuddy/.mcp.json` | `~/.workbuddy/skills` ⚠️ | 桌面待核实；CodeBuddy CLI `~/.codebuddy/skills` | AGENTS.md ⚠️ | 桌面待核实；CodeBuddy CLI CODEBUDDY.md 优先、AGENTS.md 兜底 |
| pi | `~/.pi/agent/mcp.json` `mcpServers`（误认为靠 adapter）✅ | 内置 `builtin:mcp`，同路径同键 ✅ | `~/.pi/agent/skills` ✅ | 同左（另兼容 `~/.agents/skills`）✅ | AGENTS.md ✅ | AGENTS.override.md>AGENTS.md>…>CLAUDE.md ✅ |

### 9.2 项目级（仓库内骨架）

| Agent | 应用项目 MCP | 官方项目 MCP | 应用项目目录/子目录 | 官方项目目录 | 应用根指令文件 | 官方根指令文件 |
|---|---|---|---|---|---|---|
| claude-code | `.mcp.json` ✅ | 同左（首次需 approval，条目无 `enabled`）✅ | `.claude`；commands、agents ✅ | `.claude/`（另 `.claude/skills`）✅ | CLAUDE.md（指向 AGENTS.md 指针）⚠️ | CLAUDE.md 或（v2.1.277+）AGENTS.md，默认互斥 |
| codex | `.codex/config.toml` ✅ | 同左（仅 trusted 加载，忽略 provider 等键）✅ | `.codex`，写 `instructions.md` ❌ | `.codex/`（config.toml/hooks/rules；无 instructions.md） | AGENTS.md ✅ | AGENTS.md（根→CWD 拼接）✅ |
| opencode | 仓库根 `opencode.json` ✅ | 同左（可提交 git）✅ | `.opencode`；`agent/command/plugin/tool`（单数）⚠️ | `.opencode/`；复数 `agents/commands/plugins/tools/` | AGENTS.md ✅ | AGENTS.md ✅ |
| antigravity（gcli 口径） | `.gemini/settings.json` ✅（gcli）；Antigravity App 应为 `.agents/mcp_config.json` ❌ | gcli `.gemini/settings.json`；App `.agents/mcp_config.json` | `.gemini`；commands ⚠️ | gcli `.gemini/`（commands/*.toml）；App 无 `.gemini`，用 `.agents/` | GEMINI.md（指针）⚠️ | gcli GEMINI.md；App AGENTS.md+GEMINI.md |
| workbuddy | 仓库根 `.mcp.json` ⚠️ | 桌面 `.workbuddy/mcp.json`；CodeBuddy CLI 仓库根 `.mcp.json` ✅（CLI） | `.workbuddy`；rules、skills ⚠️ | 桌面待核实；CodeBuddy CLI `.codebuddy/` | AGENTS.md ⚠️ | CodeBuddy CLI CODEBUDDY.md 优先、AGENTS.md 兜底 |
| pi | `.pi/agent/mcp.json` ❌ | **`.pi/mcp.json`**（trust 后，官方正确位置） | `.pi/agent`；skills ❌ | `.pi/`（无 agent 层）；项目 skills `.pi/skills/`（兼容 `.agents/skills/`） | AGENTS.md ✅ | AGENTS.md（无需 trust）✅ |

> 项目 Skills 统一放 `<repo>/.agents/skills` 再链接/复制到各 agent 目录：对 Codex、OpenCode、gcli、Antigravity App、Pi（兼容位）成立；Claude Code 原生是 `.claude/skills`；Pi 原生是 `.pi/skills`。

---

## 10. 差距清单（现状 → 官方 → 建议动作 → 确定性）

| # | Agent | 现状（应用） | 官方现状 | 建议动作 | 确定性 |
|---|---|---|---|---|---|
| D1 | pi | 认为 MCP 非内置、靠 pi-mcp-adapter | 0.99.0+ 内置 `builtin:mcp`，1.x 默认开 | 移除「adapter 依赖」注释/认知；按内置 MCP 对接，保留 adapter 作为可选兼容 | 官方明确 |
| D2 | pi | 项目 MCP 写 `.pi/agent/mcp.json` | `.pi/mcp.json`（trust 后） | 项目 MCP 路径改为 `.pi/mcp.json`；schema 维持 `mcpServers`，补 stdio/HTTP 字段与「项目仅覆盖 enabled/exposure」语义 | 官方明确 |
| D3 | pi | 项目目录 `.pi/agent`、skills 子目录 | 项目 `.pi/`（无 agent 层），项目 skills `.pi/skills/`（兼容 `.agents/skills/`） | 项目骨架目录改 `.pi/`；skills 原生位 `.pi/skills/`，可保留 `.agents/skills` 兼容链接 | 官方明确（回退扫描待核实） |
| D4 | pi | 全局 MCP/skills/AGENTS.md | 全部一致 | 全局 `~/.pi/agent/mcp.json`、`~/.pi/agent/skills`、AGENTS.md 保持 | 官方明确 |
| D5 | pi | 未区分传输 | 仅 stdio + streamable HTTP，拒 SSE；HTTP 用 `url/headers/oauth` | 写入器拒绝 SSE 条目；项目文件不写 auth | 官方明确 |
| D6 | claude-code | 项目根写 CLAUDE.md 指针指向 AGENTS.md | v2.1.277+ 原生直读 AGENTS.md，默认与 CLAUDE.md 互斥 | 双轨：新版本可直接落 AGENTS.md；保留 CLAUDE.md `@AGENTS.md` import 兜底旧版本；勿假设两者拼接 | 官方明确 |
| D7 | claude-code | `.mcp.json` 可能写 `enabled` 字段 | 条目无官方 `enabled`；审批走 settings 列表 | 不在条目内写 enabled；需要时写 `.claude/settings.json` 的 enabledMcpjsonServers 等 | 官方明确（未文档化 enabled 待核实） |
| D8 | claude-code | commands、agents 子目录 | 仍成立（commands legacy、agents=subagents）；新增项目 skills `.claude/skills` | 补项目 skills 目录约定；commands 标注 legacy | 官方明确 |
| D9 | claude-desktop | `Claude*` 通配扫描、「云端 VM 不支持 skills」 | 机制仍存但主推 .mcpb；路径官方未重述；skills 账户同步 | 保留通配扫描作兼容；更新文案为「账户同步，无本地文件 skills」；精确路径待核实 | 机制官方明确 / 路径待核实 |
| D10 | codex | 项目写 `.codex/instructions.md` | 官方无此文件，项目指令走 AGENTS.md | 停止生成 instructions.md；项目指令落 AGENTS.md | 待核实（官方未记载） |
| D11 | codex | 远程 MCP 可能按 `type=remote`/`headers` 写 | 无 type 字段；远程 `url`+`bearer_token_env_var`+`http_headers` | 修正 TOML 写入器 schema；项目级仅 trusted 加载需提示 | 官方明确 |
| D12 | codex | 全局 skills 含 `~/.codex/skills` | 现行 USER 仅 `$HOME/.agents/skills` | `~/.codex/skills` 降级为 legacy 兼容扫描，主打 `~/.agents/skills` | 官方明确 + 社区（兼容待核实） |
| D13 | opencode | Full 子目录用单数 agent/command/plugin/tool | 现行复数 agents/commands/plugins/tools（单数兼容） | 新骨架写复数；读取兼容单数 | 官方明确 |
| D14 | opencode | env 键名/command 形状待核 | local `command` 数组、`environment` 对象 | 写入器对齐字段名 | 官方明确 |
| D15 | antigravity | 一个 agent 同时写 gcli 路径（settings.json/httpUrl）与 Antigravity skills | 两套产品约定不同（见 §6/§7） | 先明确目标产品；若面向 Antigravity App：全局 `~/.gemini/config/mcp_config.json`、项目 `.agents/mcp_config.json`、`serverUrl`、skills `.agents/skills/`、AGENTS.md+GEMINI.md；若面向 gcli：维持现状并补别名 | 官方明确（需产品决策） |
| D16 | workbuddy | 全局 `~/.workbuddy/.mcp.json` | 桌面 `~/.workbuddy/mcp.json`（无点）；CodeBuddy CLI `~/.codebuddy/.mcp.json` | 修正桌面文件名；评估是否拆分出 CodeBuddy CLI；远程格式与 skills/AGENTS.md 待官方复核 | 桌面路径官方明确 / 其余待核实 |
| D17 | 通用 | 项目 skills 统一 `.agents/skills` 链接到各目录 | 各 agent 原生项目 skills 位不同（`.claude/skills`、`.pi/skills`、`.agents/skills` 兼容位） | 链接策略按 agent 原生位落，`.agents/skills` 作为通用兼容源 | 官方明确（逐家） |

---

## 11. 待核实清单

1. Claude Code：`.mcp.json` 条目是否存在未文档化 `enabled`；消费版 Desktop `claude_desktop_config.json` 现行精确路径与是否仍被新版读取；Chat tab 是否保留手编 JSON 入口。
2. Codex：项目级 `.codex/config.toml` 与用户级是深合并还是整表覆盖；`~/.codex/skills` 是否仍被当前版本兼容扫描；`.codex/instructions.md` 是否有隐藏加载。
3. OpenCode：`opencode.local.json` 是否被 loader 识别。
4. Pi：0.99.x npm 发布渠道；`~/.pi/` 是否有未文档化平级目录；内置是否回退扫描 `.pi/agent/skills`；未来是否读项目根 `.mcp.json`。
5. gcli：源码默认 `context.fileName` 数组是否内置 AGENTS.md。
6. WorkBuddy 桌面：远程 MCP 字段、`~/.workbuddy/skills` / `.workbuddy/skills`、是否读 AGENTS.md（官方文档未披露，需官方复核或实测）。

---

## 12. 来源索引

### Pi
- 内置 MCP：https://pi.dev/docs/latest/mcp
- 配置/目录表：https://pi.dev/docs/latest/configuration
- Skills：https://pi.dev/docs/latest/skills
- Settings：https://pi.dev/docs/latest/settings
- 环境变量：https://pi.dev/docs/latest/environment-variables
- CLI：https://pi.dev/docs/latest/cli
- Changelog（0.99.0 内置 MCP）：https://pi.dev/changelog
- 仓库（301 → earendil-works/pi）：https://github.com/badlogic/pi-mono
- 社区扩展（对照）：https://pi.dev/packages/pi-mcp-adapter ・ https://pi.dev/packages/pi-mcp-extension

### Claude Code / Desktop
- MCP reference：https://code.claude.com/docs/en/mcp
- MCP quickstart：https://code.claude.com/docs/en/mcp-quickstart
- Memory（CLAUDE.md/AGENTS.md）：https://code.claude.com/docs/en/memory
- Skills：https://code.claude.com/docs/en/skills
- .claude 目录：https://code.claude.com/docs/en/claude-directory
- Subagents：https://code.claude.com/docs/en/subagents
- Settings：https://code.claude.com/docs/en/settings ・ https://docs.anthropic.com/en/docs/claude-code/settings
- Desktop 共享配置：https://code.claude.com/docs/en/desktop
- 本地 MCP / .mcpb：https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop
- 远程 connector（确认 legacy 机制）：https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp
- 自定义 skills（账户同步）：https://support.claude.com/en/articles/12512198-how-to-create-custom-skills
- Desktop Extensions 博客：https://www.anthropic.com/engineering/desktop-extensions

### Codex
- MCP：https://developers.openai.com/codex/mcp
- 配置参考：https://developers.openai.com/codex/config-reference
- AGENTS.md：https://developers.openai.com/codex/guides/agents-md
- Skills：https://developers.openai.com/codex/skills
- 仓库：https://github.com/openai/codex

### OpenCode
- Config：https://opencode.ai/docs/config/
- MCP servers：https://opencode.ai/docs/en/mcp-servers
- Rules/AGENTS.md：https://opencode.ai/docs/en/rules/
- Skills：https://opencode.ai/docs/skills/
- Agents/Commands/Plugins/Tools：https://opencode.ai/docs/agents ・ https://opencode.ai/docs/commands ・ https://opencode.ai/docs/plugins ・ https://opencode.ai/docs/custom-tools/
- 仓库：https://github.com/sst/opencode

### Gemini CLI（gcli）
- MCP：https://goo.gle/gemini-cli-docs-mcp
- 配置参考：https://geminicli.com/docs/reference/configuration
- 企业版合并：https://google-gemini.github.io/gemini-cli/docs/cli/enterprise.html
- GEMINI.md：https://geminicli.com/docs/cli/gemini-md/
- Skills：https://geminicli.com/docs/cli/skills/ ・ https://geminicli.com/docs/cli/creating-skills/
- Commands：https://geminicli.com/docs/cli/custom-commands.md

### Google Antigravity
- MCP：https://antigravity.google/docs/mcp/
- Skills：https://antigravity.google/docs/skills/
- Rules：https://antigravity.google/docs/rules/
- gcli 迁移映射：https://antigravity.google/docs/cli/gcli-migration/
- Codelab 佐证：https://codelabs.developers.google.com/getting-started-google-antigravity

### 腾讯 WorkBuddy / CodeBuddy CLI
- WorkBuddy 桌面 MCP 指南：https://www.workbuddy.ai/docs/zh/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/MCP-Guide
- WorkBuddy 系统设置（兼容 .codebuddy）：https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Setting
- WorkBuddy 任务栏：https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Task-Bar
- CodeBuddy MCP：https://www.codebuddy.ai/docs/cli/mcp
- .codebuddy 目录：https://www.codebuddy.ai/docs/cli/codebuddy-dir
- Memory/AGENTS.md：https://www.workbuddy.ai/docs/cli/memory
- Skills：https://www.workbuddy.ai/docs/cli/skills
- Sub-agents：https://www.workbuddy.ai/docs/cli/sub-agents
- 腾讯云 1831：https://cloud.tencent.com/document/product/1831/137016 ・ /137020 ・ /137461
- 社区/半官方（需复核）：https://cloud.tencent.com/developer/techpedia/2610/20530 ・ https://developer.cloud.tencent.com/article/2672691 ・ https://developer.cloud.tencent.com/article/2686192 ・ https://developer.cloud.tencent.com/article/2750199
