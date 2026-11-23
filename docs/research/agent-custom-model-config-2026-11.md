# WorkBuddy / Trae CN / Qoder CN 自定义模型「配置文件可写性」调研

- 调研日期：2026-11-11（Asia/Shanghai）
- 目的：回答「能否通过改写配置文件修改这三个 Agent 的自定义模型」，为 Octopus 的模型配置能力（`ModelConfigAgent`）判断是否值得接入。
- 方法：只读本机已安装客户端（`/Applications/*.app` 内的 asar 与资源包）、只读本机运行时配置（不含任何密钥内容）、对照官方文档，并逐行阅读开源项目 [`yetone/magpie`](https://github.com/yetone/magpie) 的对应实现。**未修改任何第三方配置，未改动本仓库源码。**
- 2026-11-12 修订：Qoder CN 的结论由「不可行」改为「**可行**」。原判断只看到 IDE 的 BYOK 加密表，漏掉了 CLI/agent 运行时读取的 `settings.json` 顶层 `providers`。修正依据见 §3。
- 相关文档：[`agent-config-survey-2026-10.md`](./agent-config-survey-2026-10.md)（MCP / Skills / 指令文件口径）；本文只覆盖**模型配置**。
- 确定性标注：
  - **【官方明确】**：官方文档直接记载。
  - **【代码实证】**：本机客户端代码/数据文件只读核对得到。
  - **【待核实】**：本次未能证实，禁止臆测补全。

---

## 0. 结论摘要

| Agent | 是否有可写的模型配置文件 | 结论 | 关键依据 |
|---|---|---|---|
| **WorkBuddy**（腾讯桌面版） | 有：`~/.workbuddy/models.json`（用户级）、`<repo>/.workbuddy/models.json`（项目级） | ✅ **可行**，可安全代写 | 【官方明确】+【代码实证】文件监听热加载 |
| **Trae CN**（字节，国内版） | 无 | ❌ **不可行** | 【代码实证】自定义模型是账号侧数据，本地仅 `state.vscdb` 缓存；API Key 为密文 |
| **Qoder CN**（阿里，国内版） | 有：`~/.qoder-cn/settings.json` 顶层 `providers` + `model.name` | ⚠️ **可行（未文档化）** | 【代码实证 + 本机日志】桌面版 agent 运行时确实读它；magpie 已用此法打通 |

一句话结论：**WorkBuddy 与 Qoder CN 都存在可写的模型配置文件（前者官方文档化并热加载，后者未文档化但被客户端运行时读取）；只有 Trae CN 的自定义模型完全绑定服务端，本地仅有加密缓存，无法代写。**

---

## 1. WorkBuddy：可行

### 1.1 路径与覆盖规则【代码实证】

`CustomModelsProvider.getUserConfigPath()` 与 `resolveDefaultConfigDir()` 的解析顺序：

1. 环境变量 `WORKBUDDY_CONFIG_DIR`（其次 `CODEBUDDY_CONFIG_DIR`）→ `<该目录>/models.json`
2. 否则产品配置的 `dataFolderName`（WorkBuddy 桌面版为 `.workbuddy`）→ `~/.workbuddy/models.json`
3. 项目级：`<workspace>/.workbuddy/models.json`（同一份代码通过 `replaceBrandName` 把 `.codebuddy` 替换为 `.workbuddy`，WorkBuddy 构建即写 `.workbuddy`）【代码实证，项目级路径未实机写入验证】

展示口径（遵循仓库 a 类规范）：macOS/Linux `~/.workbuddy/models.json`，Windows `%USERPROFILE%\.workbuddy\models.json`。

### 1.2 文件结构【官方明确 + 代码实证】

`extractLocalModels()` 同时接受两种根结构：

```jsonc
// 形式 A：裸数组（客户端自身 saveLocalCustomModel 写的就是这个）
[ { "id": "...", ... } ]

// 形式 B：对象包装（支持 availableModels 白名单，推荐代写时使用）
{ "models": [ { "id": "...", ... } ], "availableModels": ["id1", "id2"] }
```

- `models` 为模型条目数组；`availableModels` 为字符串数组，控制模型下拉里可见的 id 集合；缺省时由 `collectCustomModelIds()` 从 `models` 推导，等价于「全部可见」。
- 顶层多余字段会被忽略，不报错。

### 1.3 条目字段【代码实证：`isValidLocalCustomModel` / `normalizeCustomModel` / 设置页表单】

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `id` | string | **是** | 非空；不需要自己加 `custom-local:` 前缀 |
| `url` | string | **是** | 接口地址；`useCustomProtocol: false` 时按标准 `/chat/completions` 校验补全 |
| `apiKey` | string | **是**（本机回环地址除外） | 支持 `${ENV_VAR}` 展开；指向 `127.0.0.1` / `localhost` 时可留空 |
| `name` | string | 否 | 下拉显示名，缺省用 `id` |
| `vendor` | string | 否 | 供应商分组/图标，自定义一般写 `Custom` |
| `maxInputTokens` | number | 否 | 也接受数字字符串，内部 `coerceOptionalNumber` 归一 |
| `maxOutputTokens` | number | 否 | 同上 |
| `temperature` | number | 否 | 同上 |
| `supportsToolCall` | bool | 否 | 工具调用能力标记 |
| `supportsImages` | bool | 否 | 图片输入能力标记 |
| `supportsReasoning` | bool | 否 | 推理模型标记 |
| `onlyReasoning` | bool | 否 | 仅推理模式 |
| `useCustomProtocol` | bool | 否 | 跳过路径校验/补全，原样使用 `url`（网关/代理封装场景） |
| `reasoning` | object | 否 | `{ canDisableThinking, defaultEffort | effort, supportedEfforts }` |
| `disabled` | bool | 否 | 保留但置灰 |

加载时 `decorateCustomModels()` 会自动补 `tags: ["custom"]`、按产品特性把 id 改写为 `custom-local:<id>` 并把原 id 写入 `aliases`、把 `url` 标记为 replace 模式——**这些都不需要写进文件**。

写入示例（密钥一律用环境变量占位，禁止落明文）：

```json
{
  "models": [
    {
      "id": "octopus-route-claude",
      "name": "Octopus 路由 · Claude",
      "vendor": "Custom",
      "url": "http://127.0.0.1:16888/v1",
      "apiKey": "${OCTOPUS_ROUTE_API_KEY}",
      "supportsToolCall": true,
      "supportsImages": true,
      "supportsReasoning": true,
      "maxInputTokens": 200000,
      "maxOutputTokens": 8192
    }
  ]
}
```

### 1.4 生效机制【代码实证】

- 用户级与项目级各注册一个 `fs.watch`，命中 `models.json` 后 debounce 触发 `productManager.sync(true)`，再走 `CustomModelsProvider` 合并进产品配置 → **不需要重启客户端**。
- 官方文档亦说明「模型配置允许自行接入第三方大模型……配置参数（含 API Key）仅保存在本地 workbuddy/models.json 中，不上传云端」。
- 官方另一处文档同时强调「无需手动编辑配置文件」，指的是**不必**手改；文件可写性由腾讯云 1831 文档与客户端代码共同证实。

### 1.5 代写必须遵守的不变量（对 Octopus 的实现约束）

1. **读-改-写**：客户端自己也会写该文件，必须读出现状 → 只增删目标条目 → 整体写回，保留未知字段与 `availableModels`。
2. **原子写**：临时文件 + rename，避免半截 JSON 被 watcher 读到（复用 `platform` / `pi_model_config` 的 `atomic_write` 模式）。
3. **幂等 upsert**：以 `id` 为主键合并，同 id 覆盖、异 id 保留。
4. **不写 `custom-local:` 前缀、不手写 `tags`**：交给客户端归一，否则会与 `decorateCustomModels` 的别名逻辑打架。
5. **密钥处理**：该文件是**明文**存储。代写时默认走 `${ENV}` 形式并在 UI 明确提示「Key 会以明文保存在 WorkBuddy 的 models.json 中」；若用户要求直填，也必须复用 Octopus 的按需读取模式，列表 DTO 只回传存在性标志。

### 1.6 风险

- 企业版策略 `EnableEnterpriseCustomModelPolicy` 可在策略关闭时过滤本地自定义模型（`EnterpriseCustomModelPolicyFilterProvider` 会把 `tags` 含 `custom` 的模型从可用列表里摘掉），此时写入成功但下拉不可见。
- 文件被客户端覆盖的窗口期：客户端在 UI 里编辑模型时会整份重写，Octopus 若在此时写入会互相覆盖；建议写入前重读一次并做冲突检测。
- 「当前选中的模型 / 默认模型」不在该文件内，属客户端会话状态【待核实具体落点】；Octopus 只负责「有哪些模型可选」。

---

## 2. Trae CN：不可行

### 2.1 没有模型配置文件【代码实证】

- `~/Library/Application Support/Trae CN/User/settings.json` 只有主题、代理、浮动窗口等 6 个键，**无任何模型项**。
- `~/.trae-cn/` 下无 `models.json`（只有 `mcps/`、`skills/`、`user_rules/`、`memory/` 等）。
- 项目级 `.trae/` 只承载 `mcp.json`、`skills/`、`rules/`，无模型文件。

### 2.2 自定义模型的真实落点：账号侧 + 本地缓存【代码实证】

`~/Library/Application Support/Trae CN/User/globalStorage/state.vscdb`（VS Code 全局状态库）中的键：

```
<uid>:AI.agent.model.model_list_map          ← 按账号缓存（本机 166 KB）
<uid>:AI.agent.model.session_selected_model
AI.modelSelectionReset.state.v1:[...]
```

`model_list_map` 按 agent 分组（`assistant` / `personal_assistant` / `solo_agent` / `solo_coder` / …），自定义模型条目长这样：

```jsonc
{
  "name": "custom_openai_compatible//<vendor>/<model>",
  "provider": "custom_openai_compatible",
  "base_url": "http://127.0.0.1:3425/v1/chat/completions",
  "is_custom_base_url": true,
  "custom_model_id": 3202946050,      // 服务端分配的整型 ID
  "config_source": 3,
  "use_remote_service": false,
  "ak": "<120 字符 base64>",          // 解码后 90 字节二进制，可打印率 0.38 → 密文
  "sk": ""
}
```

判断依据：

1. **键名带 uid 前缀** → 数据按账号隔离，不是纯本地配置。
2. **`custom_model_id` 是服务端分配的整型**，本地无生成逻辑。
3. **`ak` 是密文**：120 字符 base64 → 90 字节二进制，非明文 Key（明文 Key 会是可读 ASCII）。
4. 全量扫描 `Trae CN.app` 的 `out/`、`extensions/`、`node_modules/` 均找不到 `model_list_map` / `config_source` / `custom_model_id` 的读写代码；`custom_model` 在 `out/main.js` 里只作为 WebSocket 路径出现（`wss://<apiHost>/custom_model`）→ 模型目录由服务端下发。

### 2.3 结论

写 `state.vscdb` 或许能让条目短暂出现，但：

- 客户端启动/切账号会从服务端刷新并覆盖本地缓存；
- 无法构造可用凭据（`ak` 是密文，加密密钥在客户端内部）；
- 应用运行时 SQLite 处于 WAL 状态，外部写入有损坏风险（会破坏整个 IDE 的全局状态）。

**因此 Trae CN 不提供文件级接入路径**；官方唯一入口是「设置 → 模型 → 添加模型」（模型服务商预设或自定义配置：API 格式 / 请求地址 / 模型 ID / 鉴权）。

---

## 3. Qoder CN：可行（未文档化，靠 `settings.json` 的 `providers`）

> 本节结论于 2026-11-12 修订。原「不可行」的判断是错的：只看到了 IDE 的 BYOK 加密表，漏了 CLI/agent 运行时读的 `providers`。

### 3.1 两套并存的机制，别混为一谈【代码实证】

| 机制 | 落点 | 凭据 | 谁读 | 可代写 |
|---|---|---|---|---|
| 官方 BYOK 目录（设置 → 模型 → 添加模型） | `main.sqlite` 的 `byok_model_profiles` / `byok_model_credentials` | `encrypted_payload BLOB` 密文 | IDE 的 BYOK 向导 + daemon IPC | ❌ 无法构造密文 |
| **自定义供应商（本文重点）** | `~/.qoder-cn/settings.json` 顶层 `providers` + `model.name` | **明文** `apiKey` | CLI / 桌面版 agent 运行时 | ✅ 可写 |

官方 CLI 文档那句「不要在 `settings.json` 中手工配置 BYOK」，针对的是**第一套**（预置供应商 + Coding Plan 的账号级目录）；`providers` 属于第二套，官方配置项全表里**没有记载**它（未文档化），但客户端确实在读写。

### 3.2 文件结构【代码实证：magpie `internal/agent/qoder.go` + 本机 asar】

```jsonc
{
  "providers": {
    "magpie": {                                  // key 即供应商 id，会被拼进模型引用
      "displayName": "magpie",                   // 可选，UI 显示名
      "protocol": "openai",                      // openai | anthropic
      "baseUrl": "http://127.0.0.1:3425/v1",
      "apiKey": "<明文>",                         // 客户端会原样作为 Bearer
      "model": "qoder/dfmodel",                  // 该供应商的默认模型 id
      "models": [
        {
          "model": "qoder/dfmodel",
          "displayName": "DeepSeek-Flash",
          "contextWindow": 200000,
          "maxOutputTokens": 8192,
          "capabilities": {
            "tools": true,
            "vision": false,
            "thinking": {
              "modes": ["enabled"],
              "supportsEffort": true,
              "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]
            }
          }
        }
      ]
    }
  },
  "model": {
    "name": "magpie/qoder/dfmodel",             // 必须写成 "<providerId>/<modelId>" 才真正切过去
    "reasoningEffort": "high",                   // 旧式全局档位
    "preferences": {                              // 按模型记的档位（新版优先读这里）
      "magpie/qoder/dfmodel": { "reasoning": { "effort": "high" } }
    }
  }
}
```

补充字段（客户端会读，但 magpie 刻意不写、并原样保留用户设置）：`routing`（compact / summary / subagent 各用哪个模型）、`firstPayloadTimeoutMs`、`streamIdleTimeoutMs`、`type`、`authType`、`anthropic` 块。

模型引用规则：`model.name` 是 `"<providers 的 key>/<models[].model>"`；只写 `providers` 不改 `model.name`，等于只把模型挂上货架、没有真正切过去。

### 3.3 生效方式【代码实证】

- 会话启动时读一次配置；**已在跑的会话保持原模型，新会话用新配置**，不需要重启应用（magpie 的提示原文：`reads its settings as a session starts — open sessions keep the model they have; new ones use this`）。
- 日志里能直接验证：命中自定义供应商时 `provider` 字段被归一成 `"custom"`，`model` 为完整引用。

**本机实证（最硬的证据）**：`~/.qoder-cn/logs/sessions/**/*.jsonl` 中有 143 条

```json
{"provider":"custom","model":"magpie/qoder/dfmodel","stop_reason":"tool_use","input_tokens":106167}
```

而本机 **未安装 Qoder CLI**（`which qodercn` 为空），说明这些会话来自 `Qoder CN.app` 桌面版——即**桌面版的 agent 运行时确实读 `~/.qoder-cn/settings.json` 的 `providers`**。magpie 自身的 backup 也记录了当时 `agents.qoder-cn.model = "magpie/workbuddy-ai/hy3"`，说明 2026-11-04/05 这条链路在本机真实跑通过。

### 3.4 magpie 的具体做法（可复用的编辑语义）

`internal/agent/qoder.go` + `internal/edit/json.go`：

1. **路径**：`$QODERCN_CONFIG_DIR`，否则 `~/.qoder-cn`，文件名固定 `settings.json`；全局版同构（`~/.qoder`，`qodercli`）。
2. **只动自己那一格**：写入目标是 `providers.magpie`，用点路径 `SetJSON` 覆盖；用户自己的 `providers.mine` 原样保留（测试里有断言）。
3. **保留用户改过的字段**：`qoderKept()` 在覆盖前读回 `displayName`、`routing`、超时、`type`、`authType`、`anthropic`，写回时带上——因为 Qoder 1.1.70 会读这些，早期版本每次同步都被抹掉（magpie issue #2030）。
4. **格式保全**：`edit.SetJSON` 走 `tidwall/jsonc`，**保留注释、键序、缩进、CRLF、BOM**，写入用 `WriteAtomic`（临时文件 + rename）。测试里明确断言 `// the user's` 注释在写后仍在。
5. **切换与回滚**：切到 magpie 前把用户原来的 `model.name` 存进 stash；断开时删 `providers.magpie` 并还原 `model.name`（本机现在的 `providers: {}` 就是它断开后留下的空对象）。
6. **档位处理**：优先写 `model.preferences["<ref>"].reasoning.effort`；对没有自己档位的模型，把 `model.reasoningEffort` 一起写；并把 `generation.reasoning` 这类旧字段清掉，避免和新字段打架。
7. **不在运行时硬改**：检测到 Qoder 进程在跑时只提示「新会话才生效」，不阻塞写入。

### 3.5 限制与风险

- **未文档化**：`providers` 不在官方配置项全表里，属于客户端内部 schema，随版本变动风险高于 WorkBuddy 的 `models.json`。magpie 因此专门做了「保留未知字段」的兼容层。
- **套餐门槛**：magpie 文档标注 `needs a Qoder CN plan with BYOK`；自定义供应商只对**已登录且套餐含 BYOK** 的账号开放（【第三方实证】，本机账号曾经可用）。
- **明文凭据**：`apiKey` 在 `settings.json` 里是明文，与 WorkBuddy 同级风险，写前必须提示。
- **不要碰 `mcp-router.json`**：仍是运行时路由状态（含本机凭据），不得读取/展示/备份。
- **与 BYOK 表并存**：不要在 `byok_model_*` 里伪造行；两套机制各自独立，混写只会让 UI 状态错乱。

### 3.6 附：BYOK 加密表（仅供识别，不要写）

`~/Library/Application Support/com.qodercn.app.stable/main.sqlite`：

| 表 | 关键列 |
|---|---|
| `byok_model_profiles` | `profile_id, account_id, provider_key, configuration_kind, endpoint_url, protocol_style, model_key, is_vision, is_reasoning, available_context_windows_json, efforts_json, ...` |
| `byok_model_credentials` | `profile_id, account_id, payload_version, encrypted_payload BLOB, generation` |
| `byok_model_capability_metadata` | 能力元数据 |
| `chat_model_preferences` | 可见性 / 上下文窗口 / 推理强度 |

- 凭据列为 `encrypted_payload BLOB`，写入只经 daemon IPC（`create_byok_config` / `update_byok_config` / `validate_byok_model`）。
- 官方 CLI 文档原话：「请通过 `/model` 的 Custom 向导配置 BYOK。**不要在 `settings.json` 中手工配置 BYOK**；Provider、模型和凭证字段由当前账号可用的 BYOK 目录决定。」
- 官方 IDE 侧同样只走「设置 → 模型 → 添加模型」（预置供应商或 OpenAI/Anthropic Compatible + Base URL + API Key + Model ID）。

即使把 profile 行写进 `byok_model_profiles`，也无法生成可用的 `encrypted_payload`，模型会在调用时鉴权失败；且直接改 `main.sqlite` 会与应用运行时写入冲突。**这条路不要走**，走 §3.2 的 `providers`。

补充：`~/.qoder-cn/mcp-router.json` 是运行时路由状态（含本机凭据），**不得读取、展示或备份**（与 `AGENT_MCP_SKILLS_MAP.md` 的既有约束一致）。

---

## 4. 对 Octopus 的含义

现状：`src-tauri/src/agent_model_config.rs` 的 `ModelConfigAgent` 只覆盖 `opencode` / `pi` / `oh-my-pi` / `omo`，三者之外的 agent 没有模型配置入口。

建议动作：

> 落地方案已单独成文：[`docs/plans/2026-11-model-config-workbuddy-qoder-cn.md`](../plans/2026-11-model-config-workbuddy-qoder-cn.md)。

1. **WorkBuddy（优先）**：新增 `ModelConfigAgent::Workbuddy`，后端复用 `pi_model_config.rs` 的读写骨架（`atomic_write` + 读改写 + 保留未知字段 + `camelCase` DTO），路径解析放 `platform.rs` / 该模块内，不散落 `cfg!(windows)`。
2. **Qoder CN（次优先，风险可控）**：新增 `ModelConfigAgent::QoderCn`，只写 `~/.qoder-cn/settings.json` 的 `providers.<id>` 与 `model.name`；必须做到 ① 只覆盖自己的 provider 槽位，保留用户其它 provider；② 保留用户在自己槽位上改过的 `displayName` / `routing` / 超时 / `authType` / `anthropic`；③ 支持 JSONC（注释、尾逗号）读写并保全格式；④ 写入前提示「该字段未文档化，升级可能失效」与「apiKey 明文」。
3. **Trae CN 不做文件写入**：只做「只读说明 + 引导打开客户端设置页（设置 → 模型）」，不得写 `state.vscdb`。
4. 接入前先补 `agents.rs` 侧的路径常量与 `docs/AGENT_MCP_SKILLS_MAP.md` 的模型章节，保持「注册表 → 写入器 → 文档 → 前端」的顺序。
5. 三个 agent 的模型配置路径都属于 a 类常驻展示，统一走 `formatDisplayPath()`。

---

## 5. 待实测项

| 项 | 当前确定性 | 验证方法（安全） |
|---|---|---|
| WorkBuddy 项目级路径确为 `<repo>/.workbuddy/models.json` | 【代码实证】 | 在测试仓库创建 `.workbuddy/models.json`，重启客户端看下拉是否出现；不动用户级文件 |
| WorkBuddy 写入后无需重启即生效 | 【代码实证】 | 客户端运行时改用户级文件，观察下拉与日志 `Loaded custom models config from user` |
| WorkBuddy 明文 Key 与 `${ENV}` 两种形式的实际行为 | 【官方明确】+【代码实证】 | 用回环地址 + 空 Key 先验证通路，再验证 `${ENV}` 展开 |
| Trae `state.vscdb` 写入是否被服务端覆盖 | 【待核实】 | **不建议实测**（破坏 IDE 全局状态）；仅靠代码与数据结构判断 |
| Qoder CN `providers` 是否被桌面版读取 | 【已实证】本机日志 `provider:"custom"` + `model:"magpie/qoder/dfmodel"`，且未安装 CLI | 无需再测；若要复验：写一条回环地址 provider + `model.name`，新建会话看日志 |
| Qoder CN 套餐是否必须含 BYOK | 【第三方实证】magpie 文档标注 | 换一个免费账号试写一条 provider，看是否被忽略/报错 |
| Qoder CN `providers` 在版本升级后的稳定性 | 【待核实】未文档化 | 记录当前版本 0.5.7；升级后复跑同一写入，观察是否仍被读取 |

---

## 6. 来源

### 官方文档

- WorkBuddy 模型配置：https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Model
- WorkBuddy Enterprise 模型配置（含 models.json 说明）：https://cloud.tencent.com/document/product/1831/134445 ・ https://cloud.tencent.com/document/product/1831/137451
- WorkBuddy × CloudBase 配置文件接入：https://docs.cloudbase.net/ai/ai-tools/workbuddy
- Trae CN 自定义模型：https://docs.trae.cn/ide_custom-models
- Trae CN 企业内置模型：https://docs.volcengine.com/docs/trae-cn/model-settings
- Qoder CN IDE 自定义模型：https://docs.qoder.cn/qoder/custom-models
- Qoder CN CLI 自定义模型（BYOK 禁令）：https://docs.qoder.cn/cli/custom-models
- Qoder CN CLI 配置分层：https://docs.qoder.cn/cli/settings
- Qoder CN CLI 配置项全表：https://docs.qoder.cn/cli/settings-reference

### 开源实现（magpie，MIT）

- 仓库：https://github.com/yetone/magpie
- `internal/agent/qoder.go`：Qoder / Qoder CN 的 `providers` 写入、`model.name` 切换、档位写 `model.preferences`、保留用户字段
- `internal/agent/qoder_test.go`：JSONC 注释保全、其它 provider 不受影响、断开还原等断言
- `internal/edit/json.go`：`GetJSON` / `SetJSON` / `DelJSON`（点路径 + `tidwall/jsonc` + 原子写）
- `docs/reference.md`：`Qoder CN (CLI) → ~/.qoder-cn/settings.json（model, effort）`，并注明 `needs a Qoder CN plan with BYOK`
- 官方文档（Qoder CN CLI 自定义模型 / 配置项全表）

### 本机证据（只读，不含密钥内容）

- `~/.qoder-cn/logs/sessions/**/*.jsonl`：143 条 `"provider":"custom","model":"magpie/qoder/dfmodel"`
- `/Applications/WorkBuddy.app/Contents/Resources/app.asar`：`CustomModelsProvider` / `resolveDefaultConfigDir` / `extractLocalModels` / `isValidLocalCustomModel` / `decorateCustomModels` / `EnterpriseCustomModelPolicyFilterProvider`
- `~/.workbuddy/models.json`（当前为空数组 `[]`，由客户端创建）
- `/Applications/Trae CN.app/Contents/Resources/app/out/`（`main.js` 中 `custom_model` 仅作 WebSocket 路径）
- `~/Library/Application Support/Trae CN/User/globalStorage/state.vscdb`（`<uid>:AI.agent.model.model_list_map`）
- `/Applications/Qoder CN.app/Contents/Resources/app.asar`（`byok*` IPC 事件、settings 合并键 `providers`）
- `~/Library/Application Support/com.qodercn.app.stable/main.sqlite`（`byok_model_*`、`chat_model_preferences` 表结构）
