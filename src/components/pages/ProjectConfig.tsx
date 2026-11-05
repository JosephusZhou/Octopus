import { useEffect, useMemo, useState } from "react";
import { useStatusMessage } from "@/lib/useStatusMessage";
import { Toast } from "@/components/Toast";
import { useOverlayDismiss } from "../ui";
import { getAgentIcon } from "../agent-icons";
import { SourceFilterChips, buildSourceOptions, sourceKeyOf } from "./skills/controls";
import {
  AGENT_PROJECT_INFOS,
  type InitMode,
  type CheckResult,
  type InitResult,
  type McpServerDraft,
  type SkillInstallMode,
  type SkillOption,
} from "./project-config/types";
import {
  invokePickProjectFolder,
  invokeCheckProjectConfig,
  invokeInitProjectConfig,
  invokeListMcpServers,
  invokeListSkillOptions,
} from "./project-config/api";
import { Folder } from "lucide-react";

const IconFolder = () => <Folder size={16} strokeWidth={1.8} />;

/** Skill 来源（SkillOption.source）→ 友好显示标签 */
const SOURCE_LABEL: Record<SkillOption["source"], string> = {
  local: "本地",
  github: "GitHub",
  gitcode: "GitCode",
};

/** MCP transport types in fixed display order; only types present in the list are shown. */
const MCP_TYPE_ORDER = ["stdio", "http", "sse"] as const;

/** Build a one-line summary of selected titles: "a、b、c 等 N 个" or empty when none. */
function summarizeSelection(titles: string[]): string {
  if (titles.length === 0) return "";
  const shown = titles.slice(0, 3).join("、");
  return titles.length > 3 ? `${shown} 等 ${titles.length} 个` : shown;
}

export default function ProjectConfig() {
  const [targetDir, setTargetDir] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [mode, setMode] = useState<InitMode>("symlink");
  const [mcpOptions, setMcpOptions] = useState<McpServerDraft[]>([]);
  const [selectedMcps, setSelectedMcps] = useState<Set<string>>(new Set());
  const [skillOptions, setSkillOptions] = useState<SkillOption[]>([]);
  const [selectedSkills, setSelectedSkills] = useState<Set<string>>(new Set());
  const [skillMode, setSkillMode] = useState<SkillInstallMode>("link");
  const [mcpDialogOpen, setMcpDialogOpen] = useState(false);
  const [mcpTypeFilter, setMcpTypeFilter] = useState<string>("all");
  const [mcpSearch, setMcpSearch] = useState("");
  const [skillsDialogOpen, setSkillsDialogOpen] = useState(false);
  const [skillSourceFilter, setSkillSourceFilter] = useState<string>("all");
  const [skillTagFilter, setSkillTagFilter] = useState<string>("all");
  const [skillSearch, setSkillSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [checkResult, setCheckResult] = useState<CheckResult | null>(null);
  const [initResult, setInitResult] = useState<InitResult | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [statusMsg, setStatusMsg] = useStatusMessage();

  const confirmDismiss = useOverlayDismiss(() => !busy && setConfirmOpen(false), !busy);
  const mcpDialogDismiss = useOverlayDismiss(() => setMcpDialogOpen(false));
  const skillsDialogDismiss = useOverlayDismiss(() => setSkillsDialogOpen(false));

  // Load MCP / Skills pickers from the app's configured data (DB + skills library).
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const [mcps, skills] = await Promise.all([
          invokeListMcpServers(),
          invokeListSkillOptions(),
        ]);
        if (!alive) return;
        setMcpOptions(mcps);
        setSkillOptions(skills);
      } catch (e) {
        if (alive) setStatusMsg(`加载 MCP / Skills 列表失败：${String(e)}`);
      }
    })();
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ===== MCP dialog: type filter + filtered batch selection ===== */

  const mcpTypeOptions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const m of mcpOptions) {
      counts.set(m.type, (counts.get(m.type) ?? 0) + 1);
    }
    const opts: { key: string; label: string; count: number }[] = [
      { key: "all", label: "全部", count: mcpOptions.length },
    ];
    for (const t of MCP_TYPE_ORDER) {
      const count = counts.get(t) ?? 0;
      if (count > 0) opts.push({ key: t, label: t, count });
    }
    // Unknown/custom transport types appended after the known ones
    for (const [t, count] of counts) {
      if (!(MCP_TYPE_ORDER as readonly string[]).includes(t)) {
        opts.push({ key: t, label: t, count });
      }
    }
    return opts;
  }, [mcpOptions]);

  const filteredMcps = useMemo(() => {
    const q = mcpSearch.trim().toLowerCase();
    return mcpOptions.filter((m) => {
      if (mcpTypeFilter !== "all" && m.type !== mcpTypeFilter) return false;
      if (!q) return true;
      return (
        m.title.toLowerCase().includes(q) ||
        m.type.toLowerCase().includes(q) ||
        (m.command && m.command.toLowerCase().includes(q)) ||
        (m.url && m.url.toLowerCase().includes(q))
      );
    });
  }, [mcpOptions, mcpTypeFilter, mcpSearch]);

  const selectFilteredMcps = (on: boolean) => {
    setSelectedMcps((prev) => {
      const next = new Set(prev);
      for (const m of filteredMcps) {
        on ? next.add(m.title) : next.delete(m.title);
      }
      return next;
    });
  };

  // 当前筛选结果是否已被全部选中（用于「全选筛选结果 / 取消全选」切换按钮）。
  const allFilteredMcpsSelected =
    filteredMcps.length > 0 && filteredMcps.every((m) => selectedMcps.has(m.title));

  /* ===== Skills dialog: source + tag filters + filtered batch selection ===== */

  const skillSourceOptions = useMemo(() => buildSourceOptions(skillOptions), [skillOptions]);

  const skillTagOptions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const s of skillOptions) {
      const t = s.tag?.trim() ?? "";
      counts.set(t, (counts.get(t) ?? 0) + 1);
    }
    const opts: { key: string; label: string; count: number }[] = [
      { key: "all", label: "全部", count: skillOptions.length },
    ];
    const untagged = counts.get("") ?? 0;
    if (untagged > 0) opts.push({ key: "", label: "无标签", count: untagged });
    for (const [tag, count] of Array.from(counts.entries()).sort((a, b) =>
      a[0].localeCompare(b[0]),
    )) {
      if (tag === "") continue;
      opts.push({ key: tag, label: tag, count });
    }
    return opts;
  }, [skillOptions]);

  const filteredSkills = useMemo(() => {
    const q = skillSearch.trim().toLowerCase();
    return skillOptions.filter((s) => {
      const sourceOk = skillSourceFilter === "all" || sourceKeyOf(s) === skillSourceFilter;
      const tagOk = skillTagFilter === "all" || (s.tag?.trim() ?? "") === skillTagFilter;
      if (!sourceOk || !tagOk) return false;
      if (!q) return true;
      return (
        s.title.toLowerCase().includes(q) ||
        (s.description && s.description.toLowerCase().includes(q)) ||
        (s.tag && s.tag.toLowerCase().includes(q))
      );
    });
  }, [skillOptions, skillSourceFilter, skillTagFilter, skillSearch]);

  const selectFilteredSkills = (on: boolean) => {
    setSelectedSkills((prev) => {
      const next = new Set(prev);
      for (const s of filteredSkills) {
        on ? next.add(s.id) : next.delete(s.id);
      }
      return next;
    });
  };

  // 当前筛选结果是否已被全部选中（用于「全选筛选结果 / 取消全选」切换按钮）。
  const allFilteredSkillsSelected =
    filteredSkills.length > 0 && filteredSkills.every((s) => selectedSkills.has(s.id));

  /* ===== Page actions ===== */

  const toggleIn = (set: Set<string>, key: string): Set<string> => {
    const next = new Set(set);
    next.has(key) ? next.delete(key) : next.add(key);
    return next;
  };

  const toggleAgent = (name: string) => {
    setSelected((prev) => toggleIn(prev, name));
  };

  const handlePickDir = async () => {
    try {
      const dir = await invokePickProjectFolder();
      if (dir) {
        setTargetDir(dir);
        setInitResult(null);
        setCheckResult(null);
      }
    } catch (e) {
      setStatusMsg(String(e));
    }
  };

  const buildRequest = () => ({
    agents: Array.from(selected).map((name) => ({ name })),
    skillIds: Array.from(selectedSkills),
    mcpServers: mcpOptions.filter((m) => selectedMcps.has(m.title)),
  });

  const doInit = async (overwrite: boolean) => {
    setConfirmOpen(false);
    setBusy(true);
    setStatusMsg("初始化中…");
    try {
      const { agents, skillIds, mcpServers } = buildRequest();
      const result = await invokeInitProjectConfig(
        targetDir,
        agents,
        mode,
        overwrite,
        mcpServers,
        skillIds,
        skillMode,
      );
      setInitResult(result);
      setCheckResult(null);
      setStatusMsg(
        result.errors.length > 0 ? `完成，但有 ${result.errors.length} 个错误` : "初始化完成",
      );
    } catch (e) {
      setStatusMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleInit = async () => {
    if (!targetDir) {
      setStatusMsg("请先选择目标目录");
      return;
    }
    if (selected.size === 0) {
      setStatusMsg("请至少勾选一个 Agent");
      return;
    }
    setBusy(true);
    setStatusMsg("检查中…");
    setInitResult(null);
    try {
      const { agents, skillIds, mcpServers } = buildRequest();
      const result = await invokeCheckProjectConfig(targetDir, agents, mode, skillIds);
      setCheckResult(result);
      if (result.existing.length > 0) {
        setConfirmOpen(true);
        setStatusMsg("");
        setBusy(false);
        return;
      }
      // No conflicts — run init while staying in busy state (single busy lifecycle).
      setStatusMsg("初始化中…");
      const init = await invokeInitProjectConfig(
        targetDir,
        agents,
        mode,
        false,
        mcpServers,
        skillIds,
        skillMode,
      );
      setInitResult(init);
      setCheckResult(null);
      setStatusMsg(
        init.errors.length > 0 ? `完成，但有 ${init.errors.length} 个错误` : "初始化完成",
      );
    } catch (e) {
      setStatusMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const selectedMcpTitles = mcpOptions.filter((m) => selectedMcps.has(m.title)).map((m) => m.title);
  const selectedSkillTitles = skillOptions
    .filter((s) => selectedSkills.has(s.id))
    .map((s) => s.title);

  return (
    <>
      <div className="content-header">
        <h1 className="content-title">
          <span className="title-crumb">
            环境与配置<span className="title-sep">/</span>
          </span>
          项目配置
        </h1>
        <p className="content-desc">为项目目录初始化多个 AI coding agent 的配置文件和目录</p>
      </div>

      <div className={`content-body project-config-body${initResult ? "has-result" : ""}`}>
        <Toast message={statusMsg} />

        {/* 目录选择 */}
        <div className="form-group">
          <label className="form-label">目标目录</label>
          <div className="form-input-with-action">
            <input
              className="form-input"
              value={targetDir}
              readOnly
              placeholder="点击右侧按钮选择项目目录…"
            />
            <button
              className="form-input-action"
              onClick={handlePickDir}
              disabled={busy}
              data-tooltip="选择目录"
            >
              <IconFolder />
            </button>
          </div>
        </div>

        {/* Agent 选择 */}
        <div className="form-group">
          <div className="agent-pick-header">
            <label className="form-label">
              选择 Agent
              {selected.size > 0 && (
                <span className="form-label-optional">已选 {selected.size} 个</span>
              )}
            </label>
            <div className="mcp-agent-actions">
              <button
                type="button"
                className="mcp-agent-action"
                onClick={() =>
                  setSelected(
                    selected.size === AGENT_PROJECT_INFOS.length
                      ? new Set()
                      : new Set(AGENT_PROJECT_INFOS.map((a) => a.name)),
                  )
                }
                disabled={busy}
              >
                {selected.size === AGENT_PROJECT_INFOS.length ? "取消全选" : "全选"}
              </button>
            </div>
          </div>
          <div className="agent-pick-grid">
            {AGENT_PROJECT_INFOS.map((agent) => {
              const sel = selected.has(agent.name);
              const icon = getAgentIcon(agent.name);
              return (
                <button
                  key={agent.name}
                  type="button"
                  className={`agent-pick ${sel ? "selected" : ""}`}
                  onClick={() => toggleAgent(agent.name)}
                  disabled={busy}
                >
                  <span className={`agent-pick-icon ${sel ? "found" : ""}`}>
                    {icon ?? agent.displayName.slice(0, 2).toUpperCase()}
                  </span>
                  <span className="agent-pick-name">{agent.displayName}</span>
                </button>
              );
            })}
          </div>
        </div>

        {/* 模式选择 */}
        <div className="form-group">
          <label className="form-label">初始化模式</label>
          <div className="proj-mode-list">
            {(["symlink", "full"] as InitMode[]).map((m) => (
              <button
                key={m}
                type="button"
                className={`agent-pick proj-mode-option proj-mode-option--plain ${mode === m ? "selected" : ""}`}
                onClick={() => setMode(m)}
                disabled={busy}
              >
                <span className="agent-pick-name">
                  {m === "symlink" ? "软链接模式（推荐）" : "全量模式"}
                </span>
                <span className="proj-mode-option-desc">
                  {m === "symlink"
                    ? "在 .agents/ 存放共享内容（当前为 skills）：勾选 Skills 时各 agent 配置目录的 skills 软链接到 .agents/skills，实现多 agent 共享。Windows 需开启开发者模式。"
                    : "每个 agent 独立创建配置文件（AGENTS.md / MCP 配置），互不依赖、无软链接。（若下方勾选了 Skills，则 skills 仍统一放入 .agents/skills 并为各 agent 创建软链接，以实现多 agent 共享。）"}
                </span>
              </button>
            ))}
          </div>
        </div>

        {/* MCP 选择入口（弹窗） */}
        <div className="form-group">
          <div className="agent-pick-header">
            <label className="form-label">
              选择 MCP（可选）
              {selectedMcps.size > 0 && (
                <span className="form-label-optional">已选 {selectedMcps.size} 个</span>
              )}
            </label>
            <div className="mcp-agent-actions">
              <button
                type="button"
                className="mcp-agent-action"
                onClick={() => {
                  setMcpSearch("");
                  setMcpDialogOpen(true);
                }}
                disabled={busy}
              >
                选择
              </button>
            </div>
          </div>
          <span className="proj-mode-option-desc">
            {selectedMcps.size > 0
              ? summarizeSelection(selectedMcpTitles)
              : mcpOptions.length === 0
                ? "暂无可选 MCP — 可先在「MCP」页添加或嗅探。"
                : "未选择。勾选的 MCP 将按名称合并写入各 Agent 的项目级配置文件（如 .mcp.json / .codex/config.toml / opencode.json）。"}
          </span>
        </div>

        {/* Skills 选择入口（弹窗） */}
        <div className="form-group">
          <div className="agent-pick-header">
            <label className="form-label">
              选择 Skills（可选）
              {selectedSkills.size > 0 && (
                <span className="form-label-optional">已选 {selectedSkills.size} 个</span>
              )}
            </label>
            <div className="mcp-agent-actions">
              <button
                type="button"
                className="mcp-agent-action"
                onClick={() => {
                  setSkillSearch("");
                  setSkillsDialogOpen(true);
                }}
                disabled={busy}
              >
                选择
              </button>
            </div>
          </div>
          <span className="proj-mode-option-desc">
            {selectedSkills.size > 0
              ? summarizeSelection(selectedSkillTitles)
              : skillOptions.length === 0
                ? "暂无可选 Skills — 可先在「Skills」页添加或嗅探。"
                : "未选择。勾选的 Skills 将安装到项目 .agents/skills 目录，所选多个 Agent 均可共享使用。"}
          </span>
        </div>

        {/* Skills 安装方式 */}
        {selectedSkills.size > 0 && (
          <div className="form-group">
            <label className="form-label">Skills 安装方式</label>
            <div className="proj-mode-list">
              {(["link", "copy"] as SkillInstallMode[]).map((sm) => (
                <button
                  key={sm}
                  type="button"
                  className={`agent-pick proj-mode-option proj-mode-option--plain ${skillMode === sm ? "selected" : ""}`}
                  onClick={() => setSkillMode(sm)}
                  disabled={busy}
                >
                  <span className="agent-pick-name">
                    {sm === "link" ? "软链接（推荐）" : "完整复制"}
                  </span>
                  <span className="proj-mode-option-desc">
                    {sm === "link"
                      ? "在 .agents/skills 中创建指向软件技能库的软链接，库中更新后项目内同步生效。Windows 需开启开发者模式。"
                      : "将技能完整复制到 .agents/skills，项目内为独立副本，后续库更新不影响项目。"}
                  </span>
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="proj-init-actions">
          <button
            className="btn btn-primary"
            onClick={handleInit}
            disabled={busy || !targetDir || selected.size === 0}
          >
            {busy ? "处理中…" : "初始化"}
          </button>
        </div>

        {/* 结果展示 */}
        {initResult && (
          <div className="proj-init-result">
            {initResult.created.length > 0 && (
              <div className="proj-init-result-group">
                <div className="proj-init-result-title is-success">
                  已创建 ({initResult.created.length})
                </div>
                {initResult.created.map((p) => (
                  <div key={`c:${p}`} className="proj-init-result-line is-success">
                    {p}
                  </div>
                ))}
              </div>
            )}
            {initResult.skipped.length > 0 && (
              <div className="proj-init-result-group">
                <div className="proj-init-result-title is-muted">
                  已跳过 ({initResult.skipped.length})
                </div>
                {initResult.skipped.map((p) => (
                  <div key={`s:${p}`} className="proj-init-result-line is-muted">
                    {p}
                  </div>
                ))}
              </div>
            )}
            {initResult.errors.length > 0 && (
              <div className="proj-init-result-group">
                <div className="proj-init-result-title is-danger">
                  错误 ({initResult.errors.length})
                </div>
                {initResult.errors.map((p) => (
                  <div key={`e:${p}`} className="proj-init-result-line is-danger">
                    {p}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* MCP 选择弹窗 */}
      <div className={`modal-overlay ${mcpDialogOpen ? "visible" : ""}`} {...mcpDialogDismiss}>
        <div
          className="modal modal-lg mcp-pick-modal"
          data-no-autofocus
          onClick={(e) => e.stopPropagation()}
        >
          <div className="modal-header">
            <span className="modal-title">
              选择 MCP
              {selectedMcps.size > 0 && (
                <span className="form-label-optional"> 已选 {selectedMcps.size} 个</span>
              )}
            </span>
            <button type="button" className="modal-close" onClick={() => setMcpDialogOpen(false)}>
              ✕
            </button>
          </div>
          <div className="modal-body mcp-modal-body mcp-pick-body">
            {mcpOptions.length === 0 ? (
              <span className="proj-mode-option-desc">
                暂无可选 MCP — 可先在「MCP」页添加或嗅探。
              </span>
            ) : (
              <>
                <div className="mcp-pick-top">
                  <div className="mcp-search">
                    <input
                      type="text"
                      className="form-input"
                      placeholder="搜索 MCP 名称 / 类型 / 命令…"
                      value={mcpSearch}
                      onChange={(e) => setMcpSearch(e.target.value)}
                    />
                  </div>
                  <div className="skill-source-filter" role="group" aria-label="按类型筛选">
                    {mcpTypeOptions.map((opt) => {
                      const active = opt.key === mcpTypeFilter;
                      return (
                        <button
                          key={opt.key}
                          type="button"
                          className={`skill-source-chip ${opt.key === "all" ? "skill-source-chip-all" : ""} ${active ? "active" : ""}`}
                          aria-pressed={active}
                          onClick={() => setMcpTypeFilter(opt.key)}
                        >
                          <span className="skill-source-chip-label">{opt.label}</span>
                          <span className="skill-source-chip-count">{opt.count}</span>
                        </button>
                      );
                    })}
                  </div>
                </div>
                <div className="mcp-pick-header-row">
                  <span className="proj-mode-option-desc">
                    筛选结果 {filteredMcps.length} 项（已选{" "}
                    {filteredMcps.filter((m) => selectedMcps.has(m.title)).length}）
                  </span>
                  <div className="mcp-agent-actions">
                    <button
                      type="button"
                      className="mcp-agent-action"
                      onClick={() => selectFilteredMcps(!allFilteredMcpsSelected)}
                      disabled={filteredMcps.length === 0}
                    >
                      {allFilteredMcpsSelected ? "取消全选" : "全选筛选结果"}
                    </button>
                  </div>
                </div>
                <div className="mcp-pick-scroll">
                  {filteredMcps.length === 0 ? (
                    <span className="proj-mode-option-desc mcp-pick-empty">
                      {mcpSearch || mcpTypeFilter !== "all"
                        ? "没有匹配的 MCP。"
                        : "该类型下暂无 MCP。"}
                    </span>
                  ) : (
                    <div className="mcp-pick-list">
                      {filteredMcps.map((mcp) => {
                        const checked = selectedMcps.has(mcp.title);
                        return (
                          <div
                            key={mcp.title}
                            className={`mcp-card selectable ${checked ? "selected" : ""}`}
                            role="checkbox"
                            aria-checked={checked}
                            onClick={() => setSelectedMcps((prev) => toggleIn(prev, mcp.title))}
                          >
                            <div className="mcp-card-header">
                              <div className="mcp-card-main">
                                <div className="mcp-card-title-row">
                                  <span className="mcp-card-title">{mcp.title}</span>
                                  <span className={`skill-tag mcp-tag-${mcp.type}`}>
                                    {mcp.type}
                                  </span>
                                </div>
                              </div>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              </>
            )}
          </div>
          <div className="modal-footer">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => setMcpDialogOpen(false)}
            >
              完成
            </button>
          </div>
        </div>
      </div>

      {/* Skills 选择弹窗 */}
      <div
        className={`modal-overlay ${skillsDialogOpen ? "visible" : ""}`}
        {...skillsDialogDismiss}
      >
        <div
          className="modal modal-lg skill-pick-modal"
          data-no-autofocus
          onClick={(e) => e.stopPropagation()}
        >
          <div className="modal-header">
            <span className="modal-title">
              选择 Skills
              {selectedSkills.size > 0 && (
                <span className="form-label-optional"> 已选 {selectedSkills.size} 个</span>
              )}
            </span>
            <button
              type="button"
              className="modal-close"
              onClick={() => setSkillsDialogOpen(false)}
            >
              ✕
            </button>
          </div>
          <div className="modal-body mcp-modal-body mcp-pick-body">
            {skillOptions.length === 0 ? (
              <span className="proj-mode-option-desc">
                暂无可选 Skills — 可先在「Skills」页添加或嗅探。
              </span>
            ) : (
              <>
                <div className="mcp-search">
                  <input
                    type="text"
                    className="form-input"
                    placeholder="搜索 Skill 名称 / 描述 / 标签…"
                    value={skillSearch}
                    onChange={(e) => setSkillSearch(e.target.value)}
                  />
                </div>
                <div className="skill-pick-columns">
                  {/* 左列：筛选条件 */}
                  <aside className="skill-pick-col skill-pick-col-filters">
                    <div className="skill-pick-col-label">来源</div>
                    <SourceFilterChips
                      options={skillSourceOptions}
                      active={skillSourceFilter}
                      onSelect={setSkillSourceFilter}
                    />
                    {skillTagOptions.length > 1 && (
                      <>
                        <div className="skill-pick-col-label" style={{ marginTop: 12 }}>
                          标签
                        </div>
                        <div className="skill-tag-filter" role="group" aria-label="按标签筛选">
                          {skillTagOptions.map((opt) => {
                            const active = opt.key === skillTagFilter;
                            return (
                              <button
                                key={opt.key || "__untagged"}
                                type="button"
                                className={`skill-source-chip skill-source-chip-tag ${active ? "active" : ""}`}
                                aria-pressed={active}
                                data-tooltip={opt.label}
                                onClick={() => setSkillTagFilter(opt.key)}
                              >
                                <span className="skill-source-chip-label">{opt.label}</span>
                                <span className="skill-source-chip-count">{opt.count}</span>
                              </button>
                            );
                          })}
                        </div>
                      </>
                    )}
                  </aside>
                  {/* 右列：筛选结果 + 卡片列表（仅此区域滚动） */}
                  <section className="skill-pick-col skill-pick-col-list">
                    <div className="mcp-pick-header-row">
                      <span className="proj-mode-option-desc">
                        筛选结果 {filteredSkills.length} 项（已选{" "}
                        {filteredSkills.filter((s) => selectedSkills.has(s.id)).length}）
                      </span>
                      <div className="mcp-agent-actions">
                        <button
                          type="button"
                          className="mcp-agent-action"
                          onClick={() => selectFilteredSkills(!allFilteredSkillsSelected)}
                          disabled={filteredSkills.length === 0}
                        >
                          {allFilteredSkillsSelected ? "取消全选" : "全选筛选结果"}
                        </button>
                      </div>
                    </div>
                    {filteredSkills.length === 0 ? (
                      <span className="proj-mode-option-desc mcp-pick-empty">
                        {skillSearch || skillSourceFilter !== "all" || skillTagFilter !== "all"
                          ? "没有匹配的 Skill。"
                          : "当前筛选条件下暂无 Skills。"}
                      </span>
                    ) : (
                      <div className="mcp-pick-scroll">
                        <div className="mcp-pick-list">
                          {filteredSkills.map((skill) => {
                            const checked = selectedSkills.has(skill.id);
                            const sourceCls = `skill-tag skill-tag-${skill.source}`;
                            return (
                              <div
                                key={skill.id}
                                className={`mcp-card selectable ${checked ? "selected" : ""}`}
                                role="checkbox"
                                aria-checked={checked}
                                onClick={() =>
                                  setSelectedSkills((prev) => toggleIn(prev, skill.id))
                                }
                                data-tooltip={skill.description || undefined}
                              >
                                <div className="mcp-card-header">
                                  <div className="mcp-card-main">
                                    <div className="mcp-card-title-row">
                                      <span className="mcp-card-title">{skill.title}</span>
                                      <span className={sourceCls}>
                                        {SOURCE_LABEL[skill.source]}
                                      </span>
                                      {skill.tag?.trim() ? (
                                        <span className="skill-tag skill-tag-custom">
                                          {skill.tag.trim()}
                                        </span>
                                      ) : null}
                                    </div>
                                  </div>
                                </div>
                              </div>
                            );
                          })}
                        </div>
                      </div>
                    )}
                  </section>
                </div>
              </>
            )}
          </div>
          <div className="modal-footer">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => setSkillsDialogOpen(false)}
            >
              完成
            </button>
          </div>
        </div>
      </div>

      {/* 确认覆盖 modal */}
      <div className={`modal-overlay ${confirmOpen ? "visible" : ""}`} {...confirmDismiss}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-header">
            <span className="modal-title">检测到已存在的文件</span>
            <button
              type="button"
              className="modal-close"
              onClick={() => !busy && setConfirmOpen(false)}
              disabled={busy}
            >
              ✕
            </button>
          </div>
          <div className="modal-body">
            <p className="proj-confirm-hint">
              以下路径已存在。「跳过」保留原样；「覆盖」仅替换文件内容与可安全删除的软链接/空目录，
              <strong>不会删除非空真实目录</strong>（以免误伤用户数据）。 勾选的 MCP
              不受此影响，始终以按名称合并的方式写入。
            </p>
            <div className="proj-confirm-list">
              {checkResult?.existing.map((item) => (
                <div key={item.path} className="proj-confirm-item">
                  {item.isDir ? "📁" : "📄"} {item.path}
                </div>
              ))}
            </div>
          </div>
          <div className="modal-footer">
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => setConfirmOpen(false)}
              disabled={busy}
            >
              取消
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void doInit(false)}
              disabled={busy}
            >
              跳过已存在
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => void doInit(true)}
              disabled={busy}
            >
              覆盖
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
