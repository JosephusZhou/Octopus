import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatDisplayPath } from "@/lib/formatPath";
import {
  CheckCircle2,
  ChevronRight,
  Copy,
  Download,
  ExternalLink,
  FileText,
  FolderOpen,
  Github,
  Info,
  MessageSquare,
  RefreshCw,
  Sparkles,
  XCircle,
} from "lucide-react";

// ===== 常量 =====
const GITHUB_OWNER = "JosephusZhou";
const GITHUB_REPO = "Octopus";
const REPO_URL = `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}`;
const ISSUES_URL = `${REPO_URL}/issues`;
const RELEASES_URL = `${REPO_URL}/releases`;
const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;

// ===== 工具函数 =====
/** 在系统默认浏览器中打开外部链接（通过 Tauri command） */
async function openExternal(url: string): Promise<void> {
  try {
    await invoke("open_external_url", { url });
  } catch {
    // fallback：开发模式下 window.open 可能仍然有效
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

/** 复制文本到剪贴板 */
async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

// ===== 类型 =====
/** 后端 `get_about_info` 返回的元信息 */
interface AboutInfo {
  version: string;
  platform: string;
  arch: string;
  configDir: string;
}

/** 后端 `reveal_octopus_config_dir` 返回的结果 */
interface AboutActionResult {
  ok: boolean;
  message: string;
  path: string;
}

interface ReleaseInfo {
  tagName: string;
  name: string;
  publishedAt: string;
  htmlUrl: string;
  body: string;
}

interface UpdateCheckResult {
  currentVersion: string;
  hasUpdate: boolean;
  latest: ReleaseInfo | null;
}

type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date"; current: string }
  | { kind: "new-version"; current: string; latest: ReleaseInfo }
  | { kind: "error"; message: string };

// ===== 关于页面 =====
export default function About() {
  const [info, setInfo] = useState<AboutInfo | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus>({ kind: "idle" });
  const [copied, setCopied] = useState<string>("");
  const [configDirNotice, setConfigDirNotice] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);

  // 版本 / 平台 / 架构统一由后端提供（编译期常量），不在前端做兜底
  useEffect(() => {
    let disposed = false;
    (async () => {
      try {
        const data = await invoke<AboutInfo>("get_about_info");
        if (!disposed) setInfo(data);
      } catch {
        if (!disposed) setInfo(null);
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  // 复制按钮反馈
  const handleCopy = async (text: string, label: string) => {
    const ok = await copyToClipboard(text);
    if (ok) {
      setCopied(label);
      setTimeout(() => setCopied((prev) => (prev === label ? "" : prev)), 1500);
    }
  };

  // 检查更新：请求与版本比较都在 Rust 侧完成
  const handleCheckUpdate = async () => {
    if (!info) return;
    setUpdateStatus({ kind: "checking" });
    try {
      const res = await invoke<UpdateCheckResult>("check_latest_release");
      if (res.hasUpdate && res.latest) {
        setUpdateStatus({
          kind: "new-version",
          current: res.currentVersion,
          latest: res.latest,
        });
      } else {
        setUpdateStatus({ kind: "up-to-date", current: res.currentVersion });
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setUpdateStatus({ kind: "error", message: msg });
    }
  };

  // 打开配置目录：失败时给出可见提示
  const handleRevealConfigDir = async () => {
    setConfigDirNotice(null);
    try {
      const res = await invoke<AboutActionResult>("reveal_octopus_config_dir");
      if (!res.ok) {
        setConfigDirNotice({ ok: false, message: res.message });
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setConfigDirNotice({ ok: false, message: msg });
    }
  };

  const configDir = formatDisplayPath(info?.configDir);
  const versionLabel = info ? `v${info.version}` : "—";
  const systemInfo = info ? `${info.platform} · ${info.arch}` : "—";

  return (
    <>
      <div className="content-header">
        <div className="content-header-bar">
          <h1 className="content-title">
            <span className="title-crumb">
              设置<span className="title-sep">/</span>
            </span>
            关于
          </h1>
        </div>
      </div>
      <div className="content-body">
        {/* ===== 应用信息头部 ===== */}
        <div className="about-app-header">
          <img
            src="/octopus-about-light.png"
            alt="Octopus"
            className="about-app-logo theme-icon-light"
          />
          <img src="/octopus-about-dark.png" alt="" className="about-app-logo theme-icon-dark" />
          <div className="about-app-info">
            <div className="about-app-name">Octopus</div>
            <div className="about-app-tagline">统一管理 AI Agent 生态配置的桌面工具</div>
            <div className="about-app-version-row">
              <span className="about-app-version-label">版本</span>
              <span className="about-app-version">{versionLabel}</span>
              <button
                className="about-copy-btn"
                onClick={() => handleCopy(versionLabel, "version")}
                title="复制版本号"
                aria-label="复制版本号"
                disabled={!info}
              >
                {copied === "version" ? (
                  <CheckCircle2 size={14} strokeWidth={2} />
                ) : (
                  <Copy size={14} strokeWidth={2} />
                )}
              </button>
            </div>
          </div>
        </div>

        {/* ===== 版本更新 ===== */}
        <div className="pref-section">
          <div className="pref-section-title about-section-title">
            <RefreshCw size={16} strokeWidth={1.8} />
            <span>版本更新</span>
          </div>
          <div className="pref-card about-update-card">
            <div className="about-update-actions">
              <button
                className="btn btn-secondary"
                onClick={handleCheckUpdate}
                disabled={!info || updateStatus.kind === "checking"}
                title={!info ? "正在读取版本信息" : undefined}
              >
                {updateStatus.kind === "checking" ? (
                  <>
                    <RefreshCw size={15} strokeWidth={1.8} className="spinning" />
                    检查中…
                  </>
                ) : (
                  <>
                    <RefreshCw size={15} strokeWidth={1.8} />
                    检查更新
                  </>
                )}
              </button>
              <button
                className="btn btn-secondary"
                onClick={() => openExternal(RELEASES_URL)}
                title="在浏览器中打开 Releases 页面"
              >
                <ExternalLink size={15} strokeWidth={1.8} />
                打开 Release 页面
              </button>
            </div>

            {/* 状态展示 */}
            {updateStatus.kind === "up-to-date" && (
              <div className="about-update-status about-update-ok">
                <CheckCircle2 size={16} strokeWidth={1.8} />
                <span>当前已是最新版本（v{updateStatus.current}）</span>
              </div>
            )}

            {updateStatus.kind === "new-version" && (
              <div className="about-update-available">
                <div className="about-update-status about-update-new">
                  <Sparkles size={16} strokeWidth={1.8} />
                  <span>
                    发现新版本：<strong>{updateStatus.latest.tagName}</strong>（当前 v
                    {updateStatus.current}）
                  </span>
                </div>
                {updateStatus.latest.body && (
                  <div className="about-update-notes">
                    <div className="about-update-notes-title">更新说明</div>
                    <div className="about-update-notes-body">
                      {updateStatus.latest.body.split("\n").slice(0, 10).join("\n")}
                      {updateStatus.latest.body.split("\n").length > 10 && "\n…"}
                    </div>
                  </div>
                )}
                <div className="about-update-download-row">
                  <button
                    className="btn btn-primary"
                    onClick={() => openExternal(updateStatus.latest.htmlUrl)}
                  >
                    <Download size={15} strokeWidth={1.8} />
                    下载 {updateStatus.latest.tagName}
                  </button>
                  <span className="about-update-date">
                    发布于 {new Date(updateStatus.latest.publishedAt).toLocaleDateString()}
                  </span>
                </div>
              </div>
            )}

            {updateStatus.kind === "error" && (
              <div className="about-update-status about-update-err">
                <XCircle size={16} strokeWidth={1.8} />
                <span>{updateStatus.message}</span>
              </div>
            )}
          </div>
        </div>

        {/* ===== 快速链接 ===== */}
        <div className="pref-section">
          <div className="pref-section-title about-section-title">
            <ExternalLink size={16} strokeWidth={1.8} />
            <span>快速链接</span>
          </div>
          <div className="about-links-grid">
            <AboutLinkItem
              icon={<Github size={18} strokeWidth={1.8} />}
              title="GitHub 仓库"
              desc="浏览源码、Star 项目"
              onClick={() => openExternal(REPO_URL)}
            />
            <AboutLinkItem
              icon={<MessageSquare size={18} strokeWidth={1.8} />}
              title="提交问题"
              desc="报告 Bug 或请求新功能"
              onClick={() => openExternal(ISSUES_URL)}
            />
            <AboutLinkItem
              icon={<RefreshCw size={18} strokeWidth={1.8} />}
              title="更新日志"
              desc="查看所有版本发布记录"
              onClick={() => openExternal(RELEASES_URL)}
            />
            <AboutLinkItem
              icon={<FileText size={18} strokeWidth={1.8} />}
              title="开源协议"
              desc="PolyForm Noncommercial License"
              onClick={() => openExternal(LICENSE_URL)}
            />
          </div>
        </div>

        {/* ===== 技术信息 ===== */}
        <div className="pref-section">
          <div className="pref-section-title about-section-title">
            <Info size={16} strokeWidth={1.8} />
            <span>技术信息</span>
          </div>
          <div className="pref-card">
            <div className="about-tech-row">
              <span className="about-tech-key">操作系统</span>
              <span className="about-tech-val">{systemInfo}</span>
            </div>
            <div className="about-tech-row">
              <span className="about-tech-key">Tauri 版本</span>
              <span className="about-tech-val">2.x</span>
            </div>
            <div className="about-tech-row">
              <span className="about-tech-key">应用版本</span>
              <span className="about-tech-val">{versionLabel}</span>
            </div>
            <div className="about-tech-row">
              <span className="about-tech-key">应用配置目录</span>
              <div className="about-tech-val-row">
                <code className="about-path">{configDir || "—"}</code>
                <button
                  className="about-copy-btn"
                  onClick={() => handleCopy(info?.configDir ?? "", "configDir")}
                  title="复制路径"
                  aria-label="复制路径"
                  disabled={!info?.configDir}
                >
                  {copied === "configDir" ? (
                    <CheckCircle2 size={14} strokeWidth={2} />
                  ) : (
                    <Copy size={14} strokeWidth={2} />
                  )}
                </button>
                <button
                  className="about-copy-btn"
                  onClick={handleRevealConfigDir}
                  title="在文件管理器中打开"
                  aria-label="在文件管理器中打开"
                >
                  <FolderOpen size={14} strokeWidth={2} />
                </button>
              </div>
            </div>
          </div>
          {configDirNotice && !configDirNotice.ok && (
            <div className="about-update-status about-update-err about-config-dir-notice">
              <XCircle size={16} strokeWidth={1.8} />
              <span>{configDirNotice.message}</span>
            </div>
          )}
        </div>

        {/* ===== 版权信息 ===== */}
        <div className="about-footer">
          <div className="about-footer-text">
            © {new Date().getFullYear()} Octopus ·{" "}
            <button className="about-footer-link" onClick={() => openExternal(LICENSE_URL)}>
              PolyForm Noncommercial License
            </button>
          </div>
        </div>
      </div>
    </>
  );
}

// ===== 链接项组件 =====
function AboutLinkItem({
  icon,
  title,
  desc,
  onClick,
}: {
  icon: React.ReactNode;
  title: string;
  desc: string;
  onClick: () => void;
}) {
  return (
    <button className="about-link-item" onClick={onClick} type="button">
      <div className="about-link-icon">{icon}</div>
      <div className="about-link-text">
        <div className="about-link-title">{title}</div>
        <div className="about-link-desc">{desc}</div>
      </div>
      <ChevronRight size={16} strokeWidth={1.8} className="about-link-chevron" />
    </button>
  );
}
