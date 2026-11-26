import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";
import { bridge, installTauriBridge } from "./tauriBridge";

/**
 * Component tests use the real `@tauri-apps/api` modules with a webview IPC shim
 * instead of `vi.mock("@tauri-apps/api/core")`. The app resolves `core` through
 * concurrent dynamic `import()` calls, and the mock registry can hand the real
 * module to one of those concurrent requests, which makes tests flaky.
 */
installTauriBridge();

Object.defineProperty(window, "matchMedia", {
  configurable: true,
  writable: true,
  value: vi.fn(() => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  })),
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  bridge.reset();
});
