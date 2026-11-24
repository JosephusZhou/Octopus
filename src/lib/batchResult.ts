/**
 * 批量操作（应用 / 标签 / 删除）的结果展示决策。
 *
 * 背景：这些操作返回 `BatchSkillResult`，失败或跳过时后端已经给出逐项 `errors`，
 * 但 UI 之前只读 `message` 就直接关窗并退出批量模式，导致用户不知道是哪几项没成功。
 * 这里把判定收敛成纯函数，避免三个页面各写一套。
 */

export interface BatchOutcomeLike {
  failed: number;
  skipped: number;
  message: string;
  errors: string[];
}

export interface BatchDialogDecision {
  /** 失败或跳过非零：保留弹窗，展示 message 与 errors。 */
  keepDialogOpen: boolean;
  /** 只有 failed 为 0 才退出批量模式（跳过属于预期结果）。 */
  exitBatchMode: boolean;
  tone: "success" | "warning";
  /** 弹窗内要列出的逐项明细。 */
  details: string[];
}

export function decideBatchDialog(res: BatchOutcomeLike): BatchDialogDecision {
  const hasLeftovers = res.failed > 0 || res.skipped > 0;
  return {
    keepDialogOpen: hasLeftovers,
    exitBatchMode: res.failed === 0,
    tone: res.failed > 0 ? "warning" : "success",
    details: hasLeftovers ? [...res.errors] : [],
  };
}
