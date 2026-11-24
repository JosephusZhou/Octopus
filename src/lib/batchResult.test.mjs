import test from "node:test";
import assert from "node:assert/strict";
import { decideBatchDialog } from "./batchResult.ts";

test("全部成功：关闭弹窗并退出批量模式", () => {
  const d = decideBatchDialog({
    failed: 0,
    skipped: 0,
    message: "已以软链接覆盖应用 3 个技能",
    errors: [],
  });
  assert.equal(d.keepDialogOpen, false);
  assert.equal(d.exitBatchMode, true);
  assert.equal(d.tone, "success");
  assert.deepEqual(d.details, []);
});

test("有失败：保留弹窗、列出 errors，且不退出批量模式", () => {
  const d = decideBatchDialog({
    failed: 2,
    skipped: 0,
    message: "以软链接覆盖应用 1 个成功，2 个失败",
    errors: ["「a」：权限不足", "「b」：目录不存在"],
  });
  assert.equal(d.keepDialogOpen, true);
  assert.equal(d.exitBatchMode, false);
  assert.equal(d.tone, "warning");
  assert.deepEqual(d.details, ["「a」：权限不足", "「b」：目录不存在"]);
});

test("只有跳过（failed=0）：保留弹窗说明跳过，但仍退出批量模式", () => {
  const d = decideBatchDialog({
    failed: 0,
    skipped: 2,
    message: "已删除 1 个技能，跳过 2 个（不存在）",
    errors: ["「x」：技能不存在", "「y」：技能不存在"],
  });
  assert.equal(d.keepDialogOpen, true);
  assert.equal(d.exitBatchMode, true);
  assert.equal(d.tone, "success");
  assert.equal(d.details.length, 2);
});

test("失败与跳过并存：保留弹窗且不退出批量模式", () => {
  const d = decideBatchDialog({
    failed: 1,
    skipped: 1,
    message: "已删除 0 个技能，跳过 1 个（不存在），1 个失败",
    errors: ["「x」：技能不存在", "「y」：权限不足"],
  });
  assert.equal(d.keepDialogOpen, true);
  assert.equal(d.exitBatchMode, false);
  assert.equal(d.tone, "warning");
  assert.equal(d.details.length, 2);
});

test("失败但 errors 为空：仍保留弹窗，不退出批量模式", () => {
  const d = decideBatchDialog({
    failed: 1,
    skipped: 0,
    message: "1 个失败",
    errors: [],
  });
  assert.equal(d.keepDialogOpen, true);
  assert.equal(d.exitBatchMode, false);
  assert.deepEqual(d.details, []);
});
