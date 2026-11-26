import { StrictMode } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ModelConfigAgentPage } from "@/components/pages/model-config/ModelConfigAgentPage";
import ClaudeEnv from "@/components/pages/ClaudeEnv";
import CodexEnv from "@/components/pages/CodexEnv";
import AiProviders from "@/components/pages/AiProviders";
import type {
  AgentModelConfigView,
  ModelConfigAgentId,
} from "@/components/pages/opencode-config/types";
import { bridge } from "./tauriBridge";

const REMOTE_COMMANDS = new Set([
  "fetch_claude_env_remote_models",
  "fetch_codex_env_remote_models",
  "probe_models_endpoint",
]);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function model(id: string) {
  return {
    id,
    name: id,
    modalitiesInput: ["text"],
    modalitiesOutput: ["text"],
    variants: [],
    extraOptions: {},
  };
}

function agentView(agent: ModelConfigAgentId, bound: boolean): AgentModelConfigView {
  return {
    agent,
    installed: true,
    configPath: "~/.agents/models.json",
    credentialPath: "~/.agents/auth.json",
    configExists: true,
    isJsonc: false,
    defaultsSupported: true,
    smallModelSupported: false,
    warnings: [],
    providers: [
      {
        id: "local",
        name: "本地供应商",
        api: agent === "opencode" ? "openai" : "openai-responses",
        package: agent === "opencode" ? "aisdk:@ai-sdk/openai" : null,
        sourceId: bound ? "source-a" : null,
        hasApiKey: true,
        apiKeySource: "config",
        baseUrl: "https://example.invalid/v1",
        whitelist: [],
        blacklist: [],
        models: [model("saved-one"), model("saved-two")],
      },
    ],
  };
}

const aiProvider = {
  id: "source-a",
  name: "来源A",
  providerTypes: ["anthropic", "openai_chat", "openai_responses"],
  baseUrl: "https://example.invalid",
  openaiBaseUrl: "https://example.invalid/v1",
  hasApiKey: true,
  apiKeyCount: 1,
  customModels: [
    { model: "saved-one", aliasId: "" },
    { model: "saved-two", aliasId: "" },
  ],
  notes: "",
  createdAt: 0,
  updatedAt: 0,
  sortOrder: 0,
};

const environment = {
  id: "env-a",
  name: "测试环境",
  slug: "test",
  configDir: "~/.test",
  aliasName: "test",
  isDefault: false,
  source: "managed",
  notes: "",
  aliasInstalled: false,
  dirExists: true,
  hasSettings: true,
  hasSkills: false,
  hasAgents: false,
  mcpServerCount: 0,
  globalMcpServerCount: 0,
  skillCount: 0,
  baseUrl: "https://example.invalid",
  apiKey: "",
  hasApiKey: true,
  hasAuth: true,
  model: "saved-one",
  modelTiers: {},
  modelProvider: "",
  providerId: "source-a",
  createdAt: 0,
  updatedAt: 0,
};

function shellStatus() {
  return {
    aliases: [],
    preview: "",
    message: "",
    blockPresent: false,
    shellConfigPath: "~/.zshrc",
    shellConfigExists: false,
    shellKind: "zsh",
    zshrcPath: "~/.zshrc",
    zshrcExists: false,
  };
}

let savedModels = [model("saved-one"), model("saved-two")];
beforeEach(() => {});
let secretHolder: { promise: Promise<string>; resolve: (v: string) => void } | null = null;

let nativeViewBound = true;

function defaultHandler(cmd: string, payload: Record<string, unknown>): unknown {
  switch (cmd) {
    case "get_agent_model_config": {
      const view = agentView(payload.agent as ModelConfigAgentId, nativeViewBound);
      view.providers[0].models = savedModels;
      return view;
    }
    case "fetch_models_dev_catalog":
      return { providers: [], fetchedAt: 0, fromCache: true };
    case "list_ai_providers":
      return [aiProvider];
    case "get_route_aggregation_status":
      return {
        serverRunning: false,
        listenAddress: "127.0.0.1",
        listenPort: 16888,
        providers: [],
      };
    case "get_route_aggregation_config":
      return { apiKeys: ["route-placeholder"], listenAddress: "127.0.0.1", listenPort: 16888 };
    case "get_route_aggregation_models":
      return ["route-one"];
    case "get_provider_source_models":
      return {
        id: payload.sourceId,
        name: "来源A",
        models: savedModels.map((m) => ({ id: m.id, original: m.id })),
      };
    case "get_provider_delete_impact":
      return ["Claude 环境「测试环境」：~/.test（保留环境，清除连接和模型）"];
    case "get_provider_sync_status":
      return [];
    case "list_claude_environments":
    case "list_codex_environments":
      return [environment];
    case "get_claude_env_shell_status":
    case "get_codex_env_shell_status":
      return shellStatus();
    case "get_claude_env_secret":
    case "get_codex_env_secret":
    case "get_agent_provider_secret":
    case "get_ai_provider_secret":
      return "saved-placeholder";
    case "get_ai_provider_secrets":
      return ["saved-placeholder"];
    case "unbind_agent_provider":
      return null;
    default:
      return null;
  }
}

function visibleModal(): HTMLElement {
  const modal = document.querySelector<HTMLElement>(".modal-overlay.visible");
  if (!modal) throw new Error("没有可见弹窗");
  return modal;
}

function remoteCommands() {
  return bridge.commands().filter((cmd) => REMOTE_COMMANDS.has(cmd));
}

beforeEach(() => {
  savedModels = [model("saved-one"), model("saved-two")];
  nativeViewBound = true;
  bridge.reset();
  bridge.invoke = vi.fn(async (cmd, payload) => defaultHandler(cmd, payload));
});

describe("模型配置页的绑定交互", () => {
  it.each(["opencode", "pi", "oh-my-pi", "omo"] as const)(
    "%s 绑定列表禁止单独增删且编辑模型不请求远端",
    async (agent) => {
      render(<ModelConfigAgentPage agent={agent} />);
      await screen.findByText("本地供应商");

      const add = document.querySelector<HTMLButtonElement>(
        '[data-tooltip="绑定列表由供应商管理维护"]',
      );
      expect(add).not.toBeNull();
      expect(add!.disabled).toBe(true);

      const deletes = [
        ...document.querySelectorAll<HTMLButtonElement>('[data-tooltip="模型列表由供应商维护"]'),
      ];
      expect(deletes).toHaveLength(2);
      expect(deletes.every((b) => b.disabled)).toBe(true);
      expect(document.querySelector('[data-tooltip="删除模型"]')).toBeNull();

      fireEvent.click(
        document.querySelector<HTMLButtonElement>(".oc-model-chip [data-tooltip='编辑模型']")!,
      );
      const modal = visibleModal();
      const picker = within(modal).getByRole("button", { name: "从供应商模型列表选择" });
      expect((picker as HTMLButtonElement).disabled).toBe(true);
      expect(remoteCommands()).toHaveLength(0);
    },
  );

  it("选择来源保存时携带唯一ID并预览已保存模型数量", async () => {
    nativeViewBound = false;
    render(<ModelConfigAgentPage agent="pi" />);
    await screen.findByText("本地供应商");
    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="添加供应商"]')!);
    const modal = visibleModal();
    await within(modal).findByText("选择供应商（可选）");

    fireEvent.click(modal.querySelector<HTMLElement>("#oc-ai-provider")!);
    fireEvent.click(await screen.findByRole("option", { name: /来源A/ }));
    await within(modal).findByText(/本次将导入 2 个模型/);
    fireEvent.change(modal.querySelector<HTMLElement>("#oc-pid")!, {
      target: { value: "brand-new" },
    });

    fireEvent.click(within(modal).getByRole("button", { name: "保存" }));
    await waitFor(() =>
      expect(
        bridge.calls.some(
          (call) => call.cmd === "upsert_agent_provider" && call.payload.sourceId === "source-a",
        ),
      ).toBe(true),
    );
    expect(remoteCommands()).toHaveLength(0);
  });

  it("来源变更事件移除已删除模型，StrictMode 下初始列表仍会加载", async () => {
    render(
      <StrictMode>
        <ModelConfigAgentPage agent="pi" />
      </StrictMode>,
    );
    await screen.findByText("saved-two");

    savedModels = [model("saved-one")];
    await act(async () => bridge.emit("provider-sources-changed"));
    await waitFor(() => expect(screen.queryByText("saved-two")).toBeNull());
    expect(screen.getByText("saved-one")).toBeTruthy();
  });

  it("关闭编辑后晚返回的密钥不会填进新建表单", async () => {
    const secret = deferred<string>();
    secretHolder = secret;
    const original = bridge.invoke!;
    bridge.invoke = vi.fn(async (cmd, payload) => {
      if (cmd === "get_agent_provider_secret") return secret.promise;
      if (cmd === "get_agent_model_config") {
        const unbound = agentView("pi", false);
        unbound.providers[0].models = savedModels;
        return unbound;
      }
      return original(cmd, payload);
    });

    render(<ModelConfigAgentPage agent="pi" />);
    await screen.findByText("本地供应商");
    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="编辑"]')!);
    await waitFor(() => visibleModal());
    fireEvent.click(within(visibleModal()).getByRole("button", { name: "取消" }));

    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="添加供应商"]')!);
    await waitFor(() => visibleModal());
    await actResolve();
    expect((document.querySelector("#oc-key") as HTMLInputElement).value).toBe("");
  });
});

async function actResolve() {
  await act(async () => {
    secretHolder!.resolve("late-placeholder");
    await secretHolder!.promise;
  });
}

describe.each([
  ["Claude", ClaudeEnv, "get_claude_env_secret"],
  ["Codex", CodexEnv, "get_codex_env_secret"],
] as const)("%s 环境绑定交互", (_label, Page, secretCommand) => {
  it("已绑定供应商的模型候选只读已保存列表，不请求远端", async () => {
    render(<Page />);
    await screen.findByText("测试环境");
    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="编辑环境信息"]')!);
    const modal = visibleModal();
    await within(modal).findByDisplayValue("测试环境");

    fireEvent.click(within(modal).getByRole("button", { name: "拉取列表" }));
    await waitFor(() => expect(bridge.commands()).toContain("get_provider_source_models"));
    expect(bridge.commands()).toContain(secretCommand);
    const combo = within(modal).getByDisplayValue("saved-one");
    fireEvent.focus(combo);
    await waitFor(() =>
      expect(modal.querySelectorAll('[role="option"]').length).toBeGreaterThan(1),
    );
    expect(
      [...modal.querySelectorAll('[role="option"]')].map((o) => o.textContent?.trim()),
    ).toEqual(expect.arrayContaining(["saved-one", "saved-two"]));
    expect(remoteCommands()).toHaveLength(0);
  });

  it("来源列表读取失败保留原绑定并阻止静默解绑保存", async () => {
    const original = bridge.invoke!;
    bridge.invoke = vi.fn(async (cmd, payload) =>
      cmd === "list_ai_providers"
        ? Promise.reject(new Error("native unavailable"))
        : original(cmd, payload),
    );
    render(<Page />);
    await screen.findByText("测试环境");
    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="编辑环境信息"]')!);
    const modal = visibleModal();
    const message = await within(modal).findByText(/读取绑定供应商失败/);
    expect(message).toBeTruthy();
    expect(within(modal).getByRole("button", { name: /改为手动配置/ })).toBeTruthy();
    expect(within(modal).getByText(/不会自动解除绑定或改成远端拉取/)).toBeTruthy();
    expect(bridge.calls.some((c) => REMOTE_COMMANDS.has(c.cmd))).toBe(false);
  });
});

it("供应商表单中晚返回的密钥不覆盖用户已输入的值", async () => {
  const secrets = deferred<string[]>();
  const original = bridge.invoke!;
  bridge.invoke = vi.fn(async (cmd, payload) =>
    cmd === "get_ai_provider_secrets" ? secrets.promise : original(cmd, payload),
  );

  render(<AiProviders />);
  await screen.findByText("来源A");
  fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="编辑供应商"]')!);
  const modal = visibleModal();
  const input = within(modal).getAllByRole("textbox")[0] as HTMLInputElement;
  fireEvent.change(input, { target: { value: "user-placeholder" } });
  secrets.resolve(["late-placeholder"]);
  await new Promise((r) => setTimeout(r, 0));
  expect(input.value).toBe("user-placeholder");
});

it("删除供应商前展示关联影响并要求确认", async () => {
  render(<AiProviders />);
  await screen.findByText("来源A");
  fireEvent.click(document.querySelector<HTMLButtonElement>('[data-tooltip="删除供应商"]')!);
  const modal = visibleModal();
  const impact = await within(modal).findByText(/Claude 环境「测试环境」/);
  expect(impact).toBeTruthy();
  expect(bridge.commands()).toContain("get_provider_delete_impact");
});
