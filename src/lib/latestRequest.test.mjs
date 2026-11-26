import test from "node:test";
import assert from "node:assert/strict";
import { createLatestRequest } from "./latestRequest.ts";

function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("晚返回的旧请求不能覆盖新请求", async () => {
  const gate = createLatestRequest();
  const first = deferred();
  const second = deferred();
  let displayed;
  const a = gate.begin();
  const old = first.promise.then((value) => {
    if (a()) displayed = value;
  });
  const b = gate.begin();
  const fresh = second.promise.then((value) => {
    if (b()) displayed = value;
  });
  second.resolve("new");
  await fresh;
  first.resolve("old");
  await old;
  assert.equal(displayed, "new");
});

test("切换 Agent 或弹窗上下文使原请求失效", () => {
  const gate = createLatestRequest();
  gate.setScope("pi:provider-a");
  const first = gate.begin();
  gate.setScope("opencode:provider-a");
  assert.equal(first(), false);
  const second = gate.begin();
  gate.setScope("opencode:provider-a");
  assert.equal(second(), true);
  gate.setScope("opencode:provider-b");
  assert.equal(second(), false);
});

test("用户编辑或清除供应商后，旧模型和密钥请求不能回填", () => {
  const gate = createLatestRequest();
  const secret = gate.begin();
  gate.invalidate();
  assert.equal(secret(), false);
  const models = gate.begin();
  gate.setScope("unbound");
  assert.equal(models(), false);
});

test("关闭弹窗再打开同一对象不能接收上次请求", () => {
  const gate = createLatestRequest();
  gate.setScope("true:a");
  const old = gate.begin();
  gate.setScope("false:a");
  gate.setScope("true:a");
  assert.equal(old(), false);
  assert.equal(gate.begin()(), true);
});

test("卸载期间不回填，同一实例 StrictMode 重挂载仍可接收在途请求", () => {
  const gate = createLatestRequest();
  const old = gate.begin();
  gate.dispose();
  assert.equal(old(), false);
  gate.activate();
  assert.equal(old(), true);
  gate.invalidate();
  assert.equal(old(), false);
});

test("旧请求失败及 finally 同样不能覆盖新状态", async () => {
  const gate = createLatestRequest();
  const old = gate.begin();
  const current = gate.begin();
  let loading = true;
  let error = "";
  await Promise.reject(new Error("stale failure"))
    .catch((e) => {
      if (old()) error = e.message;
    })
    .finally(() => {
      if (old()) loading = false;
    });
  assert.equal(error, "");
  assert.equal(loading, true);
  if (current()) loading = false;
  assert.equal(loading, false);
});
