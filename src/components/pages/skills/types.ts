/** SkillsManage 页的类型定义（从页面组件抽出，供页面与 api 层共享）。 */

export type SkillSource = "local" | "github" | "gitcode";

export interface SkillRecord {
  id: string;
  title: string;
  description: string;
  source: SkillSource;
  repoUrl: string;
  githubOwner: string;
  githubRepo: string;
  githubPath: string;
  localPath: string;
  tag: string;
  appliedAgents: string[];
  createdAt: number;
  updatedAt: number;
  updateAvailable: boolean;
  version?: string;
}

export interface SkillDuplicateConflict {
  name: string;
  importVersion: string;
  existingId: string;
  existingVersion: string;
  existingSource: SkillSource;
}

export interface SkillDuplicateCheckResult {
  ok: boolean;
  conflicts: SkillDuplicateConflict[];
  totalDetected: number;
  message: string;
}

export interface ExportDuplicateConflict {
  skillId: string;
  title: string;
  libraryVersion: string;
  targetVersion: string;
}

export interface ExportDuplicateCheckResult {
  ok: boolean;
  conflicts: ExportDuplicateConflict[];
  targetDir: string;
  message: string;
}

export interface AgentResult {
  name: string;
  display_name: string;
  icon: string;
  found: boolean;
}

export interface SkillsListResult {
  skills: SkillRecord[];
  message: string;
}

export interface SniffPreviewItem {
  key: string;
  directory: string;
  title: string;
  description: string;
  sourcePath: string;
  foundAgents: string[];
  status: "import" | "skip_exists" | string;
  statusLabel: string;
}

export interface SniffPreviewResult {
  ok: boolean;
  items: SniffPreviewItem[];
  scannedAgents: number;
  total: number;
  importable: number;
  skipExists: number;
  message: string;
}

export interface SniffImportResult {
  ok: boolean;
  imported: number;
  skipped: number;
  failed: number;
  skills: SkillRecord[];
  message: string;
  errors: string[];
}

export interface SkillUpdateCheckResult {
  skills: SkillRecord[];
  checked: number;
  updates: number;
  message: string;
}

export interface SkillActionResult {
  ok: boolean;
  skill: SkillRecord | null;
  message: string;
}

export interface SkillApplyResult {
  ok: boolean;
  skill: SkillRecord | null;
  linked: number;
  unlinked: number;
  message: string;
  errors: string[];
}

// 与后端 agent_skills_targets 中 supported=false 对齐：这些 Agent 无标准全局 Skills 目录
export const SKILL_UNSUPPORTED_AGENTS = new Set<string>(["claude-desktop"]);

// 批量操作（删除 / 复制 / 应用）的统一聚合返回，skills 为刷新后的整表
export interface BatchSkillResult {
  ok: boolean;
  succeeded: number;
  failed: number;
  skipped: number;
  skills: SkillRecord[];
  message: string;
  errors: string[];
  /** 批量更新时远端仓库已找不到（被移动/删除）的技能，供弹窗勾选删除 */
  missingUpstream?: RepoChangeRemoved[];
}

export type BatchApplyMode = "add" | "replace";
export type SkillInstallMode = "link" | "copy";

/* ===== 仓库差异：远端新增 / 远端已移除 ===== */

export interface RepoChangeInstall {
  agent: string;
  path: string;
  /** true = 软链接；false = 完整复制的真实目录 */
  link: boolean;
}

export interface RepoChangeAdded {
  /** 技能身份（frontmatter name，否则目录名）小写 */
  key: string;
  title: string;
  description: string;
  version: string;
  /** 技能目录在仓库内的相对路径 */
  repoPath: string;
  source: SkillSource;
  owner: string;
  repo: string;
  repoUrl: string;
  hostLabel: string;
  existsInLibrary: boolean;
}

export interface RepoChangeRemoved {
  /** 技能库目录 id */
  skillId: string;
  title: string;
  description: string;
  version: string;
  source: SkillSource;
  owner: string;
  repo: string;
  repoUrl: string;
  hostLabel: string;
  localPath: string;
  appliedAgents: string[];
  installations: RepoChangeInstall[];
}

export interface RepoChangeGroup {
  source: SkillSource;
  owner: string;
  repo: string;
  repoUrl: string;
  hostLabel: string;
  added: RepoChangeAdded[];
  removed: RepoChangeRemoved[];
}

export interface SkillRepoChangesResult {
  ok: boolean;
  groups: RepoChangeGroup[];
  addedCount: number;
  removedCount: number;
  checkedRepos: number;
  failedRepos: string[];
  message: string;
}

/** 勾选导入的一个远端新增候选 */
export interface RepoCandidateInput {
  source: SkillSource;
  owner: string;
  repo: string;
  repoUrl: string;
  path: string;
}
