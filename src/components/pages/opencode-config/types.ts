/** OpenCode / Pi / Oh-My-Pi 配置页的共享类型。 */

/** 支持可视化模型配置的 agent 标识。 */
export type ModelConfigAgentId = "opencode" | "pi" | "oh-my-pi" | "omo";

export interface AgentVariantView {
  id: string;
  reasoningEffort?: string | null;
  /** 模型 settings 中除 reasoningEffort 外的键，原样往返。 */
  extra: Record<string, unknown>;
  /** 变体顶层未建模键（headers / body 等），原样往返。 */
  overlay?: Record<string, unknown>;
}

export interface AgentModelView {
  id: string;
  name?: string | null;
  limitContext?: number | null;
  limitInput?: number | null;
  limitOutput?: number | null;
  modalitiesInput: string[];
  modalitiesOutput: string[];
  reasoning?: boolean | null;
  /** OpenCode 2.x 写入 capabilities.tools；Pi 忽略。 */
  toolCall?: boolean | null;
  /** 1.x 遗留（模型能力展示），OpenCode 2.x 后端忽略。 */
  attachment?: boolean | null;
  /** 1.x 遗留，OpenCode 2.x 后端忽略。 */
  status?: string | null;
  /** 1.x 遗留（options.thinking.type），展示用；2.x 后端忽略。 */
  thinkingType?: string | null;
  /** 2.x 读自 capabilities.reasoning.budget.max。 */
  thinkingBudgetTokens?: number | null;
  reasoningEffort?: string | null;
  textVerbosity?: string | null;
  variants: AgentVariantView[];
  extraOptions: Record<string, unknown>;
}

export interface AgentProviderView {
  id: string;
  name?: string | null;
  /** OpenCode 2.x `package`（可能带 `aisdk:` 前缀）；Pi 家族为 null。 */
  package?: string | null;
  /** 派生协议：OpenCode 由 package 推导（不落盘）；Pi 是落盘的 api 字段。 */
  api?: string | null;
  hasApiKey: boolean;
  /** auth | config | both | none */
  apiKeySource: string;
  baseUrl?: string | null;
  setCacheKey?: boolean | null;
  timeout?: number | null;
  chunkTimeout?: number | null;
  whitelist: string[];
  blacklist: string[];
  models: AgentModelView[];
}

/** OpenCode 1.x → 2.x 的待迁移项（仅用于提示，不含任何敏感数据）。 */
export interface LegacyMigrationInfo {
  /** 仅存在于 1.x 顶层 `provider` 的供应商 id。 */
  providers: string[];
  /** 密钥只存在于旧版 `auth.json` 的供应商 id。 */
  authProviders: string[];
}

export interface AgentModelConfigView {
  /** opencode | pi | oh-my-pi | omo */
  agent: ModelConfigAgentId;
  configPath: string;
  /** 凭据文件真实路径；展示前须 formatDisplayPath。 */
  credentialPath: string;
  configExists: boolean;
  isJsonc: boolean;
  /** Agent App/CLI 是否已安装（与 agent sniff 同规则；仅配置目录不算）。 */
  installed: boolean;
  /** 是否支持可视化编辑默认模型。 */
  defaultsSupported: boolean;
  /** 是否支持可视化编辑第二模型槽位。 */
  smallModelSupported: boolean;
  model?: string | null;
  /** 第二模型槽位的当前值；按 agent 映射到各自配置键。 */
  smallModel?: string | null;
  /** 第二模型槽位对应的配置键（small_model / modelRoles.fast / categories.quick.model）。 */
  smallModelKey?: string | null;
  /** OmO 专用：omo.jsonc 中固定 model_profile 的当前值。 */
  omoModelProfile?: string | null;
  /** OmO 专用：omo.jsonc 的真实路径（确认提示展示用）。 */
  omoProfilePath?: string | null;
  enabledProviders?: string[] | null;
  disabledProviders?: string[] | null;
  providers: AgentProviderView[];
  warnings: string[];
  /** OpenCode 专用：可一键迁移的 1.x 残留；缺省表示无迁移场景或无需迁移。 */
  legacyMigration?: LegacyMigrationInfo | null;
}

export interface AgentActionResult {
  ok: boolean;
  message: string;
  view?: AgentModelConfigView | null;
}

export interface CatalogReasoningOption {
  type: string;
  values?: string[] | null;
  min?: number | null;
}

export interface CatalogModelSummary {
  id: string;
  name: string;
  limitContext?: number | null;
  limitInput?: number | null;
  limitOutput?: number | null;
  modalitiesInput: string[];
  modalitiesOutput: string[];
  reasoning: boolean;
  reasoningOptions: CatalogReasoningOption[];
  toolCall: boolean;
  attachment: boolean;
  status?: string | null;
}

export interface CatalogProvider {
  id: string;
  name: string;
  env: string[];
  npm?: string | null;
  models: CatalogModelSummary[];
}

export interface ModelsDevCatalog {
  fetchedAt: number;
  fromCache: boolean;
  providers: CatalogProvider[];
}

export interface ProbeModelsResult {
  ok: boolean;
  message: string;
  modelIds: string[];
}

export interface SetDefaultsPayload {
  model?: string | null;
  smallModel?: string | null;
  enabledProviders?: string[] | null | undefined;
  disabledProviders?: string[] | null | undefined;
  /** OmO 专用：是否同步默认模型到 omo.jsonc 的 model_profile。 */
  syncOmoModelProfile?: boolean | null;
}

export interface UpsertProviderPayload {
  id: string;
  previousId?: string | null;
  name?: string | null;
  /** OpenCode 2.x 的 `package`；Pi 家族忽略。 */
  package?: string | null;
  /** Pi 家族 API 格式；OpenCode 后端忽略（协议由 package 推导）。 */
  api?: string | null;
  baseUrl?: string | null;
  timeout?: number | null;
  chunkTimeout?: number | null;
  /** 三态：undefined 不改；"" 清除；有值则写入 providers.<id>.settings.apiKey（OpenCode）
   *  或对应 agent 的密钥文件（Pi 家族） */
  apiKey?: string | null;
}

export interface UpsertModelPayload {
  providerId: string;
  id: string;
  previousId?: string | null;
  name?: string | null;
  limitContext?: number | null;
  limitInput?: number | null;
  limitOutput?: number | null;
  modalitiesInput?: string[] | null;
  modalitiesOutput?: string[] | null;
  reasoning?: boolean | null;
  /** OpenCode 2.x 写入 capabilities.tools；Pi 忽略。 */
  toolCall?: boolean | null;
  /** 2.x 写入 capabilities.reasoning.budget.max。 */
  thinkingBudgetTokens?: number | null;
  reasoningEffort?: string | null;
  textVerbosity?: string | null;
  variants?: AgentVariantView[] | null;
  extraOptions?: Record<string, unknown> | null;
  replaceExtraOptions?: boolean | null;
}

export const MODALITY_OPTIONS = ["text", "image", "pdf", "audio", "video"] as const;
export type Modality = (typeof MODALITY_OPTIONS)[number];

/** Pi models.json 仅接受这两种输入模态。 */
export const PI_MODALITY_OPTIONS = ["text", "image"] as const;

export const EFFORT_PRESETS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
