import test from "node:test";
import assert from "node:assert/strict";
import { formatDisplayPath } from "./formatPath.ts";

test("macOS / Linux 用户目录折叠为 ~", () => {
  assert.equal(formatDisplayPath("/Users/alice/projects/Octopus"), "~/projects/Octopus");
  assert.equal(formatDisplayPath("/home/alice/.config"), "~/.config");
  assert.equal(formatDisplayPath("/Users/alice"), "~");
  assert.equal(formatDisplayPath("/Users/alice/"), "~/");
});

test("用户目录以外的绝对路径保持原样", () => {
  assert.equal(formatDisplayPath("/Users"), "/Users");
  assert.equal(formatDisplayPath("/opt/homebrew/bin"), "/opt/homebrew/bin");
  assert.equal(formatDisplayPath("/Volumes/External/octopus"), "/Volumes/External/octopus");
});

test("Windows 用户目录使用环境变量占位符并统一反斜杠", () => {
  assert.equal(
    formatDisplayPath("C:\\Users\\alice\\AppData\\Local\\Octopus\\skills"),
    "%LOCALAPPDATA%\\Octopus\\skills",
  );
  assert.equal(
    formatDisplayPath("C:/Users/alice/AppData/Local/Octopus"),
    "%LOCALAPPDATA%\\Octopus",
  );
  assert.equal(
    formatDisplayPath("C:\\Users\\alice\\AppData\\Roaming\\Claude"),
    "%APPDATA%\\Claude",
  );
  assert.equal(
    formatDisplayPath("C:\\Users\\alice\\.claude\\config.json"),
    "%USERPROFILE%\\.claude\\config.json",
  );
  assert.equal(
    formatDisplayPath("c:\\users\\alice\\appdata\\local\\Octopus"),
    "%LOCALAPPDATA%\\Octopus",
  );
  assert.equal(formatDisplayPath("C:\\Users\\alice"), "%USERPROFILE%");
  assert.equal(formatDisplayPath("C:\\Users\\alice\\AppData\\Local\\"), "%LOCALAPPDATA%\\");
});

test("Windows 非用户目录与外接盘保持原样", () => {
  assert.equal(formatDisplayPath("D:\\data\\project"), "D:\\data\\project");
  assert.equal(formatDisplayPath("\\\\server\\share\\dir"), "\\\\server\\share\\dir");
});

test("已是展示形态的路径保持原样（仅统一分隔符）", () => {
  assert.equal(formatDisplayPath("~/.octopus"), "~/.octopus");
  assert.equal(formatDisplayPath("$HOME/.cache"), "$HOME/.cache");
  assert.equal(formatDisplayPath("%LOCALAPPDATA%/Octopus"), "%LOCALAPPDATA%\\Octopus");
  // 后端已输出反斜杠展示形态时不得二次替换
  assert.equal(formatDisplayPath("%LOCALAPPDATA%\\Octopus"), "%LOCALAPPDATA%\\Octopus");
  assert.equal(
    formatDisplayPath("%USERPROFILE%\\.claude\\config.json"),
    "%USERPROFILE%\\.claude\\config.json",
  );
});

test("空值返回空字符串", () => {
  assert.equal(formatDisplayPath(null), "");
  assert.equal(formatDisplayPath(undefined), "");
  assert.equal(formatDisplayPath("   "), "");
});
