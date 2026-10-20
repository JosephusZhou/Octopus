/** SkillsManage 页的 Tauri 命令封装（从页面组件抽出）。 */

import type {
  SkillsListResult,
  SniffPreviewResult,
  SniffImportResult,
  SkillUpdateCheckResult,
  SkillActionResult,
  SkillApplyResult,
  BatchSkillResult,
  BatchApplyMode,
  SkillInstallMode,
  SkillDuplicateCheckResult,
  ExportDuplicateCheckResult,
  SkillRepoChangesResult,
  RepoCandidateInput,
} from "./types";

export async function invokeListSkills(): Promise<SkillsListResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("list_skills") as Promise<SkillsListResult>;
}

export async function invokePreviewSniff(): Promise<SniffPreviewResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("preview_sniff_skills") as Promise<SniffPreviewResult>;
}

export async function invokeImportSniffed(keys: string[]): Promise<SniffImportResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("import_sniffed_skills", { keys }) as Promise<SniffImportResult>;
}

export async function invokeCheckUpdates(): Promise<SkillUpdateCheckResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("check_skill_updates") as Promise<SkillUpdateCheckResult>;
}

/** 扫描已跟踪远程仓库的新增候选与远端已移除技能（只读，不写盘）。 */
export async function invokeScanRepoChanges(): Promise<SkillRepoChangesResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("scan_skill_repo_changes") as Promise<SkillRepoChangesResult>;
}

/** 导入勾选的远端新增候选；tag 为空表示不打标签。 */
export async function invokeImportRepoCandidates(
  selections: RepoCandidateInput[],
  tag: string,
): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("import_repo_candidates", { selections, tag }) as Promise<BatchSkillResult>;
}

export async function invokePickLocal(tag: string): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("pick_and_add_skill_local", { tag }) as Promise<SkillActionResult>;
}

export async function invokeAddGithub(url: string, tag: string): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("add_skill_github", { url, tag }) as Promise<SkillActionResult>;
}

export async function invokeAddGitcode(url: string, tag: string): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("add_skill_gitcode", { url, tag }) as Promise<SkillActionResult>;
}

export async function invokeCheckExportDuplicates(
  skillIds: string[],
): Promise<ExportDuplicateCheckResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("check_export_duplicates", { skillIds }) as Promise<ExportDuplicateCheckResult>;
}

export async function invokeExportSkillsToDir(
  skillIds: string[],
  installMode: SkillInstallMode,
  targetDir: string,
  overwriteIds?: string[],
): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("export_skills_to_dir", {
    skillIds,
    installMode,
    targetDir,
    overwriteIds,
  }) as Promise<BatchSkillResult>;
}

export async function invokeUpdateSkill(skillId: string): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("update_skill", { skillId }) as Promise<SkillActionResult>;
}

// 批量更新：后端按仓库分组并发克隆，同仓库多技能共享一次克隆
export async function invokeUpdateSkillsBatch(skillIds: string[]): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("update_skills_batch", { ids: skillIds }) as Promise<BatchSkillResult>;
}

export async function invokeDeleteSkill(
  skillId: string,
  deleteAgentCopies: boolean,
): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("delete_skill", {
    skillId,
    deleteAgentCopies,
  }) as Promise<SkillActionResult>;
}

export async function invokeApplySkill(
  skillId: string,
  agents: string[],
  tag: string,
  installMode: SkillInstallMode,
): Promise<SkillApplyResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("apply_skill_to_agents", {
    skillId,
    agents,
    tag,
    installMode,
  }) as Promise<SkillApplyResult>;
}

export async function invokeAddLocalPath(
  path: string,
  tag: string,
  overwriteIds?: string[],
): Promise<SkillActionResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("add_skill_local", { path, tag, overwriteIds }) as Promise<SkillActionResult>;
}

export async function invokeCheckLocalDuplicate(path: string): Promise<SkillDuplicateCheckResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("check_skill_local_duplicate", { path }) as Promise<SkillDuplicateCheckResult>;
}

export async function invokeBatchDelete(
  skillIds: string[],
  deleteAgentCopies: boolean,
): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("batch_delete_skills", {
    skillIds,
    deleteAgentCopies,
  }) as Promise<BatchSkillResult>;
}

export async function invokeBatchApply(
  skillIds: string[],
  agents: string[],
  mode: BatchApplyMode,
  installMode: SkillInstallMode,
): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("batch_apply_skills_to_agents", {
    skillIds,
    agents,
    mode,
    installMode,
  }) as Promise<BatchSkillResult>;
}

export async function invokeBatchSetTag(
  skillIds: string[],
  tag: string,
): Promise<BatchSkillResult> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("batch_set_skill_tag", {
    skillIds,
    tag,
  }) as Promise<BatchSkillResult>;
}
