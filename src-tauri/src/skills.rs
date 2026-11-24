//! Skills library under `~/.octopus/skills` + sniff/apply status across agents.
//! Spec: docs/AGENT_MCP_SKILLS_MAP.md (dir.SKILL.md dialect).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db;

/* ===== Public types ===== */

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSource {
    Local,
    Github,
    Gitcode,
}

impl SkillSource {
    /// Whether this source tracks a remote git repository (github/gitcode).
    fn is_remote(&self) -> bool {
        matches!(self, SkillSource::Github | SkillSource::Gitcode)
    }
}

impl Default for SkillSource {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    /// Directory name under ~/.octopus/skills (stable id)
    pub id: String,
    /// Display title (frontmatter `name` or dir name)
    pub title: String,
    /// Short description from SKILL.md frontmatter / first paragraph
    pub description: String,
    pub source: SkillSource,
    /// e.g. https://github.com/owner/repo
    #[serde(default)]
    pub repo_url: String,
    #[serde(default)]
    pub github_owner: String,
    #[serde(default)]
    pub github_repo: String,
    /// Relative path inside the github repo when skill is a subdir
    #[serde(default)]
    pub github_path: String,
    /// Absolute path of the skill directory in the library
    pub local_path: String,
    /// User-assigned tag/category (empty = untagged)
    #[serde(default)]
    pub tag: String,
    /// Agent sniff `name`s that currently have this skill installed
    #[serde(default)]
    pub applied_agents: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// For github skills: whether a remote update is available
    #[serde(default)]
    pub update_available: bool,
    /// Version from SKILL.md frontmatter (empty if absent)
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListResult {
    pub skills: Vec<SkillRecord>,
    /// 技能库真实绝对路径；前端展示时须脱敏。
    pub library_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSniffResult {
    pub skills: Vec<SkillRecord>,
    pub scanned_agents: usize,
    pub found_entries: usize,
    pub imported: usize,
    pub message: String,
}

/// One skill discovered in an Agent's skills dir, for the sniff preview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffPreviewItem {
    /// Stable selection key = absolute source path of the skill dir.
    pub key: String,
    /// Directory name (used as preferred library id on import).
    pub directory: String,
    pub title: String,
    pub description: String,
    /// Absolute path of the discovered skill dir.
    pub source_path: String,
    /// Agent sniff names where this exact source dir was found.
    pub found_agents: Vec<String>,
    /// "import" | "skip_exists"
    pub status: String,
    pub status_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffPreviewResult {
    pub ok: bool,
    pub items: Vec<SniffPreviewItem>,
    pub scanned_agents: usize,
    pub total: usize,
    pub importable: usize,
    pub skip_exists: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffImportResult {
    pub ok: bool,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    pub skills: Vec<SkillRecord>,
    pub message: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateCheckResult {
    pub skills: Vec<SkillRecord>,
    pub checked: usize,
    pub updates: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillActionResult {
    pub ok: bool,
    pub skill: Option<SkillRecord>,
    pub message: String,
    /// 非致命失败项（如删除后各 Agent 副本清理失败），逐条可展示。
    #[serde(default)]
    pub errors: Vec<String>,
}

/// One conflict discovered when importing a local skill that already exists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDuplicateConflict {
    /// Frontmatter `name` (or dir name) of the skill being imported.
    pub name: String,
    /// Version of the importing skill (from frontmatter `version`, empty if absent).
    pub import_version: String,
    /// Library id of the existing skill.
    pub existing_id: String,
    /// Version of the existing skill in the library.
    pub existing_version: String,
    /// Source of the existing skill (local / github / gitcode).
    pub existing_source: SkillSource,
}

/// Result of checking a local path for duplicate skills before import.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDuplicateCheckResult {
    pub ok: bool,
    /// Conflicts against the library (may be multiple if the path has several skill dirs).
    pub conflicts: Vec<SkillDuplicateConflict>,
    /// Display names repeated inside this batch; only the first copy gets imported.
    pub batch_duplicates: Vec<String>,
    /// Total skill dirs detected in the given path (0 = not a valid skill path).
    pub total_detected: usize,
    pub message: String,
}

/// One conflict when exporting a skill to a directory where it already exists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDuplicateConflict {
    /// Skill id (directory name) being exported.
    pub skill_id: String,
    /// Skill display title.
    pub title: String,
    /// Version in the library (empty if absent).
    pub library_version: String,
    /// Version at the target directory (empty if absent).
    pub target_version: String,
}

/// Result of checking export target for duplicate skills.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDuplicateCheckResult {
    pub ok: bool,
    pub conflicts: Vec<ExportDuplicateConflict>,
    pub target_dir: String,
    pub message: String,
}

/// Result of applying a skill to a set of agents.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillApplyResult {
    pub ok: bool,
    /// Refreshed record with up-to-date `applied_agents` / `tag`.
    pub skill: Option<SkillRecord>,
    /// Agents newly installed (or already present) this run.
    pub linked: usize,
    /// Agents whose stale entry was removed this run.
    pub unlinked: usize,
    pub message: String,
    /// Per-agent non-fatal failures (unsupported agent, IO error, …).
    pub errors: Vec<String>,
}

/// 批量操作（删除 / 软链接到目录 / 应用到 Agent）的统一聚合返回。
/// `skills` 为操作后重新加载的整表，前端可直接 setSkills 刷新，保持与
/// sniff / migrate 一致的数据流。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSkillResult {
    /// 全部选中项均成功时为真；有任一失败 / 跳过则为假。
    pub ok: bool,
    /// 成功处理的技能个数。
    pub succeeded: usize,
    /// 失败的技能个数（软链接场景下「目标已存在同名」计入 skipped，不计此项）。
    pub failed: usize,
    /// 软链接到目录时因目标已存在而跳过的个数；其它操作恒为 0。
    pub skipped: usize,
    /// 操作后刷新的技能整表。
    pub skills: Vec<SkillRecord>,
    pub message: String,
    /// 逐项失败原因（含技能标题前缀）。
    pub errors: Vec<String>,
    /// 批量更新时远端仓库已找不到（被移动 / 删除）的技能，供前端弹窗勾选删除。
    #[serde(default)]
    pub missing_upstream: Vec<RepoChangeRemoved>,
}

/* ===== Repo drift: added upstream / removed upstream preview ===== */

/// 远端仓库里发现、但技能库尚未导入的候选技能。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoChangeAdded {
    /// 稳定选择键：技能身份（frontmatter name，否则目录名）小写。
    pub key: String,
    pub title: String,
    pub description: String,
    pub version: String,
    /// 技能目录在仓库内的相对路径（克隆根起算，含分隔符 `/`）。
    pub repo_path: String,
    pub source: SkillSource,
    pub owner: String,
    pub repo: String,
    pub repo_url: String,
    pub host_label: String,
    /// 该身份是否已存在于技能库（来自其它仓库）；为真时不可重复导入。
    pub exists_in_library: bool,
}

/// 技能在某个 Agent skills 目录下的落盘副本（软链接 / 真实目录）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoChangeInstall {
    pub agent: String,
    pub path: String,
    /// true = 软链接；false = 完整复制的真实目录。
    pub link: bool,
}

/// 库内已有、但远端仓库已不再提供的技能（候选删除）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoChangeRemoved {
    /// 技能库目录 id（稳定）。
    pub skill_id: String,
    pub title: String,
    pub description: String,
    pub version: String,
    pub source: SkillSource,
    pub owner: String,
    pub repo: String,
    pub repo_url: String,
    pub host_label: String,
    /// 技能库内绝对路径。
    pub local_path: String,
    /// 当前已应用到的 Agent 名（库 id 或 frontmatter 名安装均计入）。
    pub applied_agents: Vec<String>,
    /// 各 Agent 目录下的实际落盘副本。
    pub installations: Vec<RepoChangeInstall>,
}

/// 单个仓库（source/owner/repo）的新增与已移除差异。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoChangeGroup {
    pub source: SkillSource,
    pub owner: String,
    pub repo: String,
    pub repo_url: String,
    pub host_label: String,
    pub added: Vec<RepoChangeAdded>,
    pub removed: Vec<RepoChangeRemoved>,
}

/// `scan_skill_repo_changes` 的返回：按仓库归组的新增候选与已移除项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRepoChangesResult {
    pub ok: bool,
    pub groups: Vec<RepoChangeGroup>,
    pub added_count: usize,
    pub removed_count: usize,
    /// 成功克隆并比对的仓库数。
    pub checked_repos: usize,
    /// 克隆 / 取 HEAD 失败的仓库描述（owner/repo: 原因）。
    pub failed_repos: Vec<String>,
    pub message: String,
}

/* ===== Meta registry (source info, persisted in SQLite) ===== */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillMetaEntry {
    #[serde(default)]
    pub source: SkillSource,
    #[serde(default)]
    pub repo_url: String,
    #[serde(default)]
    pub github_owner: String,
    #[serde(default)]
    pub github_repo: String,
    #[serde(default)]
    pub github_path: String,
    /// User-assigned tag/category (empty = untagged)
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    /// last known remote commit / etag hint for update checks
    #[serde(default)]
    pub remote_ref: String,
    #[serde(default)]
    pub local_ref: String,
    /// Content hash of the installed skill dir, for per-skill update detection.
    #[serde(default)]
    pub content_hash: String,
}

/* ===== Paths ===== */

fn app_dir() -> Result<PathBuf, String> {
    crate::config::app_dir()
}

pub fn skills_library_dir() -> Result<PathBuf, String> {
    let dir = app_dir()?.join("skills");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("创建 skills 目录失败: {}", e))?;
    }
    Ok(dir)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn expand_tilde(path: &str) -> PathBuf {
    crate::platform::expand_home(path).unwrap_or_else(|_| PathBuf::from(path))
}

fn default_local_meta() -> SkillMetaEntry {
    let ts = now_secs();
    SkillMetaEntry {
        source: SkillSource::Local,
        created_at: ts,
        updated_at: ts,
        ..Default::default()
    }
}

/* ===== SKILL.md parsing ===== */

#[derive(Debug, Default)]
struct SkillDoc {
    name: String,
    description: String,
    version: String,
}

fn parse_skill_md(content: &str) -> SkillDoc {
    let mut doc = SkillDoc::default();
    let trimmed = content.trim_start_matches('\u{feff}');

    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let fm_lines: Vec<&str> = fm.lines().collect();
            let mut i = 0;
            while i < fm_lines.len() {
                let line = fm_lines[i].trim();
                if line.is_empty() || line.starts_with('#') {
                    i += 1;
                    continue;
                }
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim().to_ascii_lowercase();
                    let val_raw = v.trim();
                    // YAML block scalars: `description: |` / `>` collect the indented body.
                    if val_raw == "|"
                        || val_raw == ">"
                        || val_raw.starts_with("|-")
                        || val_raw.starts_with(">-")
                        || val_raw.starts_with("|+")
                        || val_raw.starts_with(">+")
                    {
                        let folded = val_raw.starts_with('>');
                        let mut block: Vec<String> = Vec::new();
                        i += 1;
                        while i < fm_lines.len() {
                            let raw = fm_lines[i];
                            if raw.trim().is_empty() {
                                block.push(String::new());
                                i += 1;
                                continue;
                            }
                            // Block ends when indentation returns to column 0 (next key).
                            if !raw.starts_with(' ') && !raw.starts_with('\t') {
                                break;
                            }
                            block.push(raw.trim_start().to_string());
                            i += 1;
                        }
                        let joined = if folded {
                            block.join(" ")
                        } else {
                            block.join("\n")
                        };
                        let value = joined.trim().to_string();
                        match key.as_str() {
                            "name" => doc.name = value,
                            "version" => doc.version = value,
                            "description" => {
                                // Block scalars are often a lead sentence followed by a
                                // structured trigger list (no blank-line separator). Keep the
                                // card blurb to the author's first line for literal `|` blocks;
                                // fold-and-summarize `>` blocks that flow as one paragraph.
                                doc.description = if folded {
                                    first_paragraph_summary(&value)
                                } else {
                                    let lead = block
                                        .iter()
                                        .map(|l| l.trim())
                                        .find(|l| !l.is_empty())
                                        .unwrap_or("");
                                    first_paragraph_summary(lead)
                                };
                            }
                            _ => {}
                        }
                        continue;
                    }
                    let mut val = val_raw.to_string();
                    // strip surrounding quotes
                    if (val.starts_with('"') && val.ends_with('"'))
                        || (val.starts_with('\'') && val.ends_with('\''))
                    {
                        val = val[1..val.len() - 1].to_string();
                    }
                    match key.as_str() {
                        "name" => doc.name = val,
                        "version" => doc.version = val,
                        "description" => doc.description = val,
                        _ => {}
                    }
                }
                i += 1;
            }
            // body after frontmatter — use first non-empty paragraph if no description
            if doc.description.is_empty() {
                let body = rest[end + 4..].trim();
                doc.description = first_paragraph_summary(body);
            }
            return doc;
        }
    }

    // No frontmatter: first # heading as name, first paragraph as description
    for line in trimmed.lines() {
        let t = line.trim();
        if t.starts_with("# ") && doc.name.is_empty() {
            doc.name = t.trim_start_matches('#').trim().to_string();
            continue;
        }
        if !t.is_empty() && !t.starts_with('#') && doc.description.is_empty() {
            doc.description = t.to_string();
            break;
        }
    }
    doc
}

fn first_paragraph_summary(body: &str) -> String {
    let mut lines = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if t.starts_with('#') {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        lines.push(t);
        if lines.join(" ").len() > 240 {
            break;
        }
    }
    let mut s = lines.join(" ");
    if s.chars().count() > 200 {
        s = s.chars().take(200).collect::<String>() + "…";
    }
    s
}

fn read_skill_doc(skill_dir: &Path) -> SkillDoc {
    let skill_md = skill_dir.join("SKILL.md");
    let alt = skill_dir.join("skill.md");
    let path = if skill_md.exists() {
        skill_md
    } else if alt.exists() {
        alt
    } else {
        return SkillDoc::default();
    };
    match fs::read_to_string(&path) {
        Ok(content) => parse_skill_md(&content),
        Err(_) => SkillDoc::default(),
    }
}

fn is_skill_dir(path: &Path) -> bool {
    path.is_dir() && (path.join("SKILL.md").exists() || path.join("skill.md").exists())
}

/// 技能展示名：frontmatter `name`，缺省回落到目录名。
fn skill_display_name(skill_dir: &Path) -> String {
    let doc = read_skill_doc(skill_dir);
    if !doc.name.trim().is_empty() {
        return doc.name.trim().to_string();
    }
    skill_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 判重用的身份键：展示名小写，并去掉开头的保留前缀 `.` / `-`。
///
/// 读取侧把 `.` 前缀目录当作内部条目（staging、隐藏项）跳过，写入侧
/// `sanitize_skill_id` 也会归一掉首部 `.`；判重必须用同一口径，否则同名技能
/// 会被反复导入成 `.hidden`、`.hidden-2` 这类列表里看不见的副本。
fn skill_identity_key(skill_dir: &Path) -> String {
    identity_key_of(&skill_display_name(skill_dir))
}

fn identity_key_of(name: &str) -> String {
    name.trim()
        .trim_start_matches(|c| c == '.' || c == '-')
        .to_lowercase()
}

/// Resolve a library skill directory by id: the id must be a single plain path
/// component, stay inside the skills library, and contain a SKILL.md.
/// Used by project-level init to install library skills into `<repo>/.agents/skills`.
pub(crate) fn library_skill_dir(id: &str) -> Result<PathBuf, String> {
    let id = id.trim();
    if !is_plain_entry_name(id) {
        return Err(format!("非法的技能标识: {}", id));
    }
    let lib = skills_library_dir()?;
    let dir = lib.join(id);
    if !dir.starts_with(&lib) || !is_skill_dir(&dir) {
        return Err(format!("技能不存在或缺少 SKILL.md: {}", id));
    }
    Ok(dir)
}

/// Returns true when an entry is a visible skill directory (including a directory symlink).
pub(crate) fn is_visible_skill_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    !name.starts_with('.') && !name.starts_with("__octopus") && is_skill_dir(path)
}

/// Count installed skills in a concrete skills root using the same rules as skills management.
pub(crate) fn count_skills_in_root(root: &Path) -> usize {
    fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| is_visible_skill_dir(&entry.path()))
        .count()
}

/* ===== Agent skills roots ===== */

struct AgentSkillsTarget {
    agent: &'static str,
    /// Prefer first existing path when scanning; write to first entry
    roots: Vec<PathBuf>,
    supported: bool,
}

fn agent_skills_targets() -> Vec<AgentSkillsTarget> {
    // 单一数据源：从 `agents` 注册表派生（roots 相对 ~，用 expand_tilde 展开）。
    crate::agents::agents()
        .iter()
        .map(|spec| AgentSkillsTarget {
            agent: spec.name,
            roots: spec.skills_roots.iter().map(|&r| expand_tilde(r)).collect(),
            supported: spec.skills_supported,
        })
        .collect()
}

/// Map skill_id → set of agent names that have it installed.
fn scan_applied_agents() -> HashMap<String, BTreeSet<String>> {
    let mut map: HashMap<String, BTreeSet<String>> = HashMap::new();
    // Shared root（shared_root 标识相同的 agent）— 任一路径有 skill 则标记同根所有 agent
    for target in agent_skills_targets() {
        if !target.supported {
            continue;
        }
        for root in &target.roots {
            if !root.is_dir() {
                continue;
            }
            let entries = match fs::read_dir(root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !is_skill_dir(&path) {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().to_string();
                if id.starts_with('.') || id.starts_with("__octopus") {
                    continue;
                }
                map.entry(id).or_default().insert(target.agent.to_string());
            }
        }
    }
    map
}

/// Count skills installed in one agent's skills roots (unique dir names across all roots;
/// 0 for unknown/unsupported agents). Mirrors `scan_applied_agents`'s entry filtering.
pub(crate) fn count_agent_skills(agent: &str) -> usize {
    let Some(spec) = crate::agents::find(agent) else {
        return 0;
    };
    if !spec.skills_supported {
        return 0;
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    for root in spec.skills_roots.iter().map(|&r| expand_tilde(r)) {
        if !root.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_skill_dir(&path) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if id.starts_with('.') || id.starts_with("__octopus") {
                continue;
            }
            names.insert(id);
        }
    }
    names.len()
}

/// One skill installed in an agent's skills roots (for the agent detail view).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillInfo {
    /// Directory name (stable id within the agent's skills root).
    pub id: String,
    /// Display title from SKILL.md frontmatter (falls back to `id`).
    pub title: String,
    /// Description from SKILL.md frontmatter (may be empty).
    pub description: String,
    /// Absolute path of the skill directory.
    pub path: String,
}

/// List skills installed in one agent's skills roots (unique dir names across all roots;
/// empty for unknown/unsupported agents). Mirrors `count_agent_skills`'s entry filtering.
pub(crate) fn list_agent_skills(agent: &str) -> Vec<AgentSkillInfo> {
    let Some(spec) = crate::agents::find(agent) else {
        return vec![];
    };
    if !spec.skills_supported {
        return vec![];
    }
    let mut by_id: std::collections::BTreeMap<String, AgentSkillInfo> = Default::default();
    for root in spec.skills_roots.iter().map(|&r| expand_tilde(r)) {
        if !root.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_skill_dir(&path) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if id.starts_with('.') || id.starts_with("__octopus") {
                continue;
            }
            by_id.entry(id.clone()).or_insert_with(|| {
                let doc = read_skill_doc(&path);
                AgentSkillInfo {
                    title: if doc.name.is_empty() {
                        id.clone()
                    } else {
                        doc.name
                    },
                    description: doc.description,
                    id,
                    path: path.to_string_lossy().to_string(),
                }
            });
        }
    }
    by_id.into_values().collect()
}

/* ===== List library ===== */

fn sanitize_skill_id(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // 首部 `.` 是读取侧的保留前缀（隐藏条目）：归一掉，避免导入后技能在列表、
    // 应用状态扫描和计数里全部不可见（见 REVIEW_FINDINGS D-001）。
    let s = mapped
        .trim_start_matches(|c| c == '.' || c == '-')
        .trim_end_matches('-')
        .to_string();
    if s.is_empty() {
        format!("skill-{}", now_secs())
    } else {
        s
    }
}

fn build_record(
    id: &str,
    skill_dir: &Path,
    meta: &SkillMetaEntry,
    applied: &[String],
    update_available: bool,
) -> SkillRecord {
    let doc = read_skill_doc(skill_dir);
    let title = if !doc.name.is_empty() {
        doc.name
    } else {
        id.to_string()
    };
    SkillRecord {
        id: id.to_string(),
        title,
        description: doc.description,
        source: meta.source.clone(),
        repo_url: meta.repo_url.clone(),
        github_owner: meta.github_owner.clone(),
        github_repo: meta.github_repo.clone(),
        github_path: meta.github_path.clone(),
        local_path: skill_dir.to_string_lossy().to_string(),
        tag: meta.tag.clone(),
        applied_agents: applied.to_vec(),
        created_at: if meta.created_at > 0 {
            meta.created_at
        } else {
            now_secs()
        },
        updated_at: if meta.updated_at > 0 {
            meta.updated_at
        } else {
            now_secs()
        },
        update_available,
        version: doc.version,
    }
}

pub fn list_skills() -> Result<SkillsListResult, String> {
    // One-time: fold legacy skills-meta.json into SQLite if present.
    let _ = db::migrate_skills_meta_json_if_present();

    let lib = skills_library_dir()?;
    let mut meta_map = db::load_skill_meta_map().unwrap_or_default();
    let applied_map = scan_applied_agents();
    let mut skills = Vec::new();

    let entries = fs::read_dir(&lib).map_err(|e| format!("读取 skills 目录失败: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_skill_dir(&path) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if id.starts_with('.') {
            continue;
        }

        let entry_meta = if let Some(existing) = meta_map.get(&id).cloned() {
            existing
        } else {
            let created = default_local_meta();
            if let Err(e) = db::upsert_skill_meta(&id, &created) {
                eprintln!("[skills] ensure meta for {} failed: {}", id, e);
            }
            meta_map.insert(id.clone(), created.clone());
            created
        };

        let mut applied: Vec<String> = applied_map
            .get(&id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        applied.sort_by_key(|a| crate::agents::display_order(a));

        // Also try matching by frontmatter name if dir differs
        let doc = read_skill_doc(&path);
        if !doc.name.is_empty() && doc.name != id {
            if let Some(extra) = applied_map.get(&doc.name) {
                for a in extra {
                    if !applied.contains(a) {
                        applied.push(a.clone());
                    }
                }
                applied.sort_by_key(|a| crate::agents::display_order(a));
            }
        }

        skills.push(build_record(
            &id,
            &path,
            &entry_meta,
            &applied,
            false, // refresh on explicit check
        ));
    }

    skills.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

    let count = skills.len();
    Ok(SkillsListResult {
        skills,
        library_path: lib.to_string_lossy().into_owned(),
        message: if count == 0 {
            "技能库为空，可通过「添加」或「嗅探」导入".into()
        } else {
            format!("已加载 {} 个技能", count)
        },
    })
}

/* ===== Copy helpers ===== */

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("源路径不是目录: {}", src.display()));
    }
    fs::create_dir_all(dst).map_err(|e| format!("创建目标目录失败: {}", e))?;

    let mut stack = vec![src.to_path_buf()];
    while let Some(current) = stack.pop() {
        let rel = current
            .strip_prefix(src)
            .map_err(|e| format!("路径计算失败: {}", e))?;
        let target = dst.join(rel);
        if current != src {
            fs::create_dir_all(&target).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        for entry in fs::read_dir(&current).map_err(|e| format!("读取目录失败: {}", e))? {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            let path = entry.path();
            let name = entry.file_name();
            // skip VCS noise when importing
            if name == ".git" || name == ".DS_Store" {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|e| format!("读取文件类型失败 {}: {}", path.display(), e))?;
            if file_type.is_symlink() {
                copy_symlink(&path, &target.join(&name))?;
            } else if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let dest_file = target.join(&name);
                fs::copy(&path, &dest_file).map_err(|e| {
                    format!(
                        "复制文件失败 {} → {}: {}",
                        path.display(),
                        dest_file.display(),
                        e
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// 复制软链接本体，不跟随链接目标。
///
/// 跟随目录软链会成环（`loop -> .`）或把技能目录外的整棵树搬进库里；跟随文件软链
/// 则会把链接实体化成普通文件，与导出/应用时的软链策略不一致。Windows 无创建链接
/// 权限时仅对文件链接降级为复制内容，目录链接跳过。
fn copy_symlink(src: &Path, dst: &Path) -> Result<(), String> {
    let link_target =
        fs::read_link(src).map_err(|e| format!("读取软链接失败 {}: {}", src.display(), e))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link_target, dst)
            .map_err(|e| format!("创建软链接失败 {}: {}", dst.display(), e))
    }
    #[cfg(not(unix))]
    {
        if crate::platform::symlink_any(&link_target, dst).is_ok() {
            return Ok(());
        }
        match fs::canonicalize(src) {
            Ok(target) if target.is_file() => fs::copy(&target, dst)
                .map(|_| ())
                .map_err(|e| format!("复制软链接目标失败 {}: {}", dst.display(), e)),
            _ => Ok(()),
        }
    }
}

/// 用已准备好的 `staging` 原子替换 `dest`：先把 `dest` 改名到 `backup`，替换失败则还原。
/// 覆盖导入/导出的唯一替换入口，避免“先删后拷”在失败时毁掉用户已有内容。
fn replace_path_with_staged(staging: &Path, dest: &Path, backup: &Path) -> Result<(), String> {
    let had_dest = fs::symlink_metadata(dest).is_ok();
    if had_dest {
        fs::rename(dest, backup).map_err(|e| format!("备份已有条目失败: {}", e))?;
    }
    if let Err(e) = fs::rename(staging, dest) {
        let _ = remove_path_entry(staging);
        if had_dest {
            if let Err(restore) = fs::rename(backup, dest) {
                return Err(format!("替换失败: {}；恢复原条目失败: {}", e, restore));
            }
        }
        return Err(format!("替换失败: {}", e));
    }
    if had_dest {
        remove_path_entry(backup).map_err(|e| format!("清理旧条目失败: {}", e))?;
    }
    Ok(())
}

/// 同一父目录下的临时路径对（staging / backup），点号前缀确保不会被技能列表读到。
fn staging_paths(parent: &Path, name: &str) -> (PathBuf, PathBuf) {
    let nonce = format!("{}-{}", std::process::id(), now_secs());
    (
        parent.join(format!(".octopus-{}-staging-{}", name, nonce)),
        parent.join(format!(".octopus-{}-backup-{}", name, nonce)),
    )
}

fn unique_skill_id(preferred: &str, lib: &Path) -> String {
    let base = sanitize_skill_id(preferred);
    if !lib.join(&base).exists() {
        return base;
    }
    for i in 2..1000 {
        let candidate = format!("{}-{}", base, i);
        if !lib.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{}-{}", base, now_secs())
}

fn import_skill_dir(
    src: &Path,
    preferred_id: Option<&str>,
    source: SkillSource,
    git: Option<GitRepoRef>,
    tag: &str,
    overwrite_id: Option<&str>,
) -> Result<SkillRecord, String> {
    if !is_skill_dir(src) {
        return Err(format!("目录缺少 SKILL.md: {}", src.display()));
    }
    let lib = skills_library_dir()?;
    let doc = read_skill_doc(src);
    let preferred = preferred_id
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if !doc.name.is_empty() {
                Some(doc.name.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            src.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("skill-{}", now_secs()))
        });

    // 覆盖导入：id 必须是库内合法目录名；先复制到库内 staging，再原子替换，
    // 复制失败时保留库里原有技能（不再“先删后拷”）。
    let id = match overwrite_id {
        Some(oid) => {
            if !is_plain_entry_name(oid) {
                return Err(format!("非法的技能标识: {}", oid));
            }
            oid.to_string()
        }
        None => unique_skill_id(&preferred, &lib),
    };
    let dest = lib.join(&id);
    if dest.parent() != Some(lib.as_path()) {
        return Err("非法的技能路径".into());
    }
    let (staging, backup) = staging_paths(&lib, &id);
    // 覆盖前记下旧 meta：内容会被替换，但「标签 + 远端来源」描述的是这个技能的归属，
    // 不该因为一次本地覆盖就丢失（D-019）。
    let old_meta = overwrite_id.and_then(|oid| db::get_skill_meta(oid).ok().flatten());
    if fs::symlink_metadata(&staging).is_ok() || fs::symlink_metadata(&backup).is_ok() {
        return Err("技能导入临时路径已存在，请稍后重试".into());
    }
    if let Err(e) = copy_dir_recursive(src, &staging) {
        let _ = remove_path_entry(&staging);
        return Err(e);
    }
    replace_path_with_staged(&staging, &dest, &backup)?;

    let ts = now_secs();
    let mut entry = SkillMetaEntry {
        source: source.clone(),
        repo_url: git.as_ref().map(|g| g.repo_url.clone()).unwrap_or_default(),
        github_owner: git.as_ref().map(|g| g.owner.clone()).unwrap_or_default(),
        github_repo: git.as_ref().map(|g| g.repo.clone()).unwrap_or_default(),
        github_path: git.as_ref().map(|g| g.path.clone()).unwrap_or_default(),
        tag: tag.trim().to_string(),
        created_at: ts,
        updated_at: ts,
        remote_ref: String::new(),
        local_ref: String::new(),
        // Best-effort baseline; empty on failure is fine (checks recompute).
        content_hash: hash_skill_dir(&dest).unwrap_or_default(),
    };
    if let Some(old) = old_meta {
        // 本次没给标签 → 沿用原标签；本地覆盖不携带远端定位 → 沿用原来源与仓库信息，
        // 否则「检查更新 / 更新技能」会静默失效。
        if entry.tag.is_empty() {
            entry.tag = old.tag;
        }
        if !source.is_remote() {
            entry.source = old.source;
            entry.repo_url = old.repo_url;
            entry.github_owner = old.github_owner;
            entry.github_repo = old.github_repo;
            entry.github_path = old.github_path;
            entry.remote_ref = old.remote_ref;
            entry.local_ref = old.local_ref;
        }
    }
    db::upsert_skill_meta(&id, &entry)?;

    let applied_map = scan_applied_agents();
    let mut applied: Vec<String> = applied_map
        .get(&id)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    if !doc.name.is_empty() {
        if let Some(extra) = applied_map.get(&doc.name) {
            for a in extra {
                if !applied.contains(a) {
                    applied.push(a.clone());
                }
            }
        }
    }
    applied.sort_by_key(|a| crate::agents::display_order(a));

    Ok(build_record(&id, &dest, &entry, &applied, false))
}

/* ===== Add: local ===== */

/// 技能库身份索引：小写 frontmatter `name` 与目录 id → (id, version, source)。
/// 本地导入的判重与重名检测共用同一口径。
fn library_identity_index(lib: &Path) -> HashMap<String, (String, String, SkillSource)> {
    let mut existing_map: HashMap<String, (String, String, SkillSource)> = HashMap::new();
    if let Ok(entries) = fs::read_dir(lib) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_skill_dir(&path) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if id.starts_with('.') {
                continue;
            }
            let doc = read_skill_doc(&path);
            let meta = db::get_skill_meta(&id)
                .unwrap_or_default()
                .unwrap_or_default();
            let version = doc.version.clone();
            let source = meta.source.clone();
            // Index by dir id（与判重键同口径，去掉首部保留前缀）
            let id_key = identity_key_of(&id);
            if !id_key.is_empty() {
                existing_map.insert(id_key, (id.clone(), version.clone(), source.clone()));
            }
            // Index by frontmatter name
            let name_key = identity_key_of(&doc.name);
            if !name_key.is_empty() {
                existing_map.insert(name_key, (id, version, source));
            }
        }
    }
    existing_map
}

/// 启动迁移结果：点号目录改名情况，供升级步骤记录日志。
#[derive(Debug, Default)]
pub struct HiddenSkillMigration {
    pub renamed: Vec<(String, String)>,
    pub skipped: Vec<String>,
    /// 一并改名的 Agent 副本条目数（best-effort）。
    pub agent_entries: usize,
}

/// 修复前，名字以 `.` 开头的技能会被原样落库成 `~/.octopus/skills/.name`，而读取侧
/// 把所有 `.` 前缀目录当作内部条目跳过，于是这些技能在界面上永久不可见（D-015）。
/// 本函数把它们改名成归一 id，并同步 DB meta 与各 Agent 下的同名副本；可重复执行。
///
/// 规则：
/// - 只处理库根的直接子目录，且确实含 SKILL.md；
/// - 跳过内部临时目录（`.octopus-*`）与 VCS 元数据；
/// - 目标 id 已存在时不动任何数据，记入 `skipped`（自动合并有丢文件风险）；
/// - 目录改名成功但 meta 写入失败时回滚目录名，避免“目录已改名、meta 孤立”。
pub fn migrate_hidden_skill_dirs() -> Result<HiddenSkillMigration, String> {
    let lib = skills_library_dir()?;
    let entries: Vec<PathBuf> = fs::read_dir(&lib)
        .map_err(|e| format!("读取 skills 目录失败: {}", e))?
        .flatten()
        .map(|entry| entry.path())
        .collect();

    let mut report = HiddenSkillMigration::default();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_migratable_hidden_skill(name) || !is_skill_dir(&path) {
            continue;
        }
        let new_id = sanitize_skill_id(name);
        if new_id == name {
            continue;
        }
        let dest = lib.join(&new_id);
        if fs::symlink_metadata(&dest).is_ok() {
            report.skipped.push(name.to_string());
            continue;
        }

        let old_meta =
            db::get_skill_meta(name).map_err(|e| format!("读取技能元信息失败 {}: {}", name, e))?;
        fs::rename(&path, &dest).map_err(|e| format!("迁移技能目录失败 {}: {}", name, e))?;

        if let Some(meta) = old_meta {
            if let Err(e) = db::upsert_skill_meta(&new_id, &meta) {
                // 回滚目录名，保持“库目录与 meta 同名”的不变量。
                let _ = fs::rename(&dest, &path);
                eprintln!("[octopus] 技能目录迁移跳过 {}：{}", name, e);
                report.skipped.push(name.to_string());
                continue;
            }
            let _ = db::delete_skill_meta(name);
        }

        report.agent_entries += rename_agent_entries_for_legacy(name, &new_id);
        report.renamed.push((name.to_string(), new_id));
    }

    if !report.renamed.is_empty() || !report.skipped.is_empty() {
        eprintln!(
            "[octopus] 技能目录迁移：改名 {} 个，跳过 {} 个，Agent 副本 {} 个",
            report.renamed.len(),
            report.skipped.len(),
            report.agent_entries
        );
    }
    Ok(report)
}

/// 遗留的“点号技能目录”判定：内部 staging/backup（`.octopus-*`）与 VCS 元数据不算。
fn is_migratable_hidden_skill(name: &str) -> bool {
    name.starts_with('.') && !name.starts_with(".octopus-") && name != ".git" && name != ".DS_Store"
}

/// 把各 Agent skills 根下的旧名字条目改名成新 id（best-effort，目标已存在则保留）。
fn rename_agent_entries_for_legacy(old: &str, new: &str) -> usize {
    let mut moved = 0usize;
    let mut handled_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for target in agent_skills_targets() {
        if !target.supported {
            continue;
        }
        for root in &target.roots {
            if !handled_roots.insert(root.clone()) {
                continue;
            }
            let from = root.join(old);
            if fs::symlink_metadata(&from).is_err() {
                continue;
            }
            let to = root.join(new);
            if fs::symlink_metadata(&to).is_ok() {
                continue;
            }
            if fs::rename(&from, &to).is_ok() {
                moved += 1;
            }
        }
    }
    moved
}

/// 本地导入时单个技能的处理方式。
#[derive(Debug, PartialEq, Eq)]
enum LocalImportPlan<'a> {
    /// 库内无同名技能，正常导入。
    Import,
    /// 库内已有同名技能，用户勾选了覆盖：替换该 id 的目录。
    Overwrite(&'a str),
    /// 库内已有同名技能且用户未勾选覆盖：跳过，不再生成 `name-2` 副本。
    Skip,
}

fn local_import_plan<'a>(
    key: &str,
    existing_keys: &HashSet<String>,
    overwrite_ids: &'a HashMap<String, String>,
) -> LocalImportPlan<'a> {
    if !existing_keys.contains(key) {
        return LocalImportPlan::Import;
    }
    match overwrite_ids.get(key) {
        Some(id) => LocalImportPlan::Overwrite(id.as_str()),
        None => LocalImportPlan::Skip,
    }
}

/// Check a local path for skill dirs and detect duplicates against the library.
/// Returns a list of conflicts (by frontmatter `name` or dir name match).
pub fn check_skill_local_duplicate(path: String) -> Result<SkillDuplicateCheckResult, String> {
    let src = expand_tilde(path.trim());
    if !src.exists() {
        return Ok(SkillDuplicateCheckResult {
            ok: false,
            conflicts: Vec::new(),
            batch_duplicates: Vec::new(),
            total_detected: 0,
            message: format!("路径不存在: {}", src.display()),
        });
    }

    // Collect skill dirs to import
    let skill_dirs: Vec<PathBuf> = if is_skill_dir(&src) {
        vec![src.clone()]
    } else if src.is_dir() {
        let mut dirs = Vec::new();
        if let Ok(entries) = fs::read_dir(&src) {
            for entry in entries.flatten() {
                let p = entry.path();
                if is_skill_dir(&p) {
                    dirs.push(p);
                }
            }
        }
        dirs
    } else {
        return Ok(SkillDuplicateCheckResult {
            ok: false,
            conflicts: Vec::new(),
            batch_duplicates: Vec::new(),
            total_detected: 0,
            message: "请选择包含 SKILL.md 的技能目录".into(),
        });
    };

    if skill_dirs.is_empty() {
        return Ok(SkillDuplicateCheckResult {
            ok: false,
            conflicts: Vec::new(),
            batch_duplicates: Vec::new(),
            total_detected: 0,
            message: "所选目录中未找到包含 SKILL.md 的技能".into(),
        });
    }

    let total = skill_dirs.len();

    let lib = skills_library_dir()?;
    let existing_map = library_identity_index(&lib);

    let mut conflicts = Vec::new();
    let mut batch_duplicates = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for dir in &skill_dirs {
        let doc = read_skill_doc(dir);
        let name = skill_display_name(dir);
        let key = skill_identity_key(dir);
        if key.is_empty() {
            continue;
        }
        // 同一批里的同名技能也要预告：导入时只有第一个会落库。
        if !seen.insert(key.clone()) {
            batch_duplicates.push(name);
            continue;
        }
        if let Some((existing_id, existing_version, existing_source)) = existing_map.get(&key) {
            conflicts.push(SkillDuplicateConflict {
                name: name.clone(),
                import_version: doc.version.clone(),
                existing_id: existing_id.clone(),
                existing_version: existing_version.clone(),
                existing_source: existing_source.clone(),
            });
        }
    }

    Ok(SkillDuplicateCheckResult {
        ok: true,
        conflicts,
        batch_duplicates,
        total_detected: total,
        message: String::new(),
    })
}

pub fn add_skill_local(
    path: String,
    tag: String,
    overwrite_ids: Option<Vec<String>>,
) -> Result<SkillActionResult, String> {
    let src = expand_tilde(path.trim());
    if !src.exists() {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: format!("路径不存在: {}", src.display()),
        });
    }

    let lib = skills_library_dir()?;

    // 覆盖表：身份键 -> 库内目录 id。前端勾选时传的是目录 id，但源技能的身份键
    // 可能命中库内技能的「目录 id」或「frontmatter name」任意一个（扫描导入以目录名
    // 落库时两者不同，见 D-017），所以两个键都要登记；同一 id 下 id 键后写，
    // 因此优先命中 id 键。
    let overwrite_map: HashMap<String, String> = match &overwrite_ids {
        Some(ids) if !ids.is_empty() => {
            let mut map = HashMap::new();
            for id in ids {
                // 覆盖目标只能是库内单层目录名，拒绝 `../` 之类的穿越标识。
                if !is_plain_entry_name(id) {
                    continue;
                }
                let dir = lib.join(id);
                if !is_skill_dir(&dir) {
                    continue;
                }
                let name_key = skill_identity_key(&dir);
                if !name_key.is_empty() {
                    map.insert(name_key, id.clone());
                }
                let id_key = identity_key_of(id);
                if !id_key.is_empty() {
                    map.insert(id_key, id.clone());
                }
            }
            map
        }
        _ => HashMap::new(),
    };

    // 库里已有同名技能且未勾选覆盖时跳过，而不是生成 `name-2` 副本。
    let mut existing_keys: HashSet<String> = library_identity_index(&lib).into_keys().collect();

    // If path itself is a skill dir
    if is_skill_dir(&src) {
        let display_name = skill_display_name(&src);
        let overwrite_id =
            match local_import_plan(&skill_identity_key(&src), &existing_keys, &overwrite_map) {
                LocalImportPlan::Import => None,
                LocalImportPlan::Overwrite(id) => Some(id),
                LocalImportPlan::Skip => {
                    return Ok(SkillActionResult {
                        errors: Vec::new(),
                        ok: false,
                        skill: None,
                        message: format!("技能「{}」已存在于技能库，已跳过", display_name),
                    });
                }
            };
        let skill = import_skill_dir(&src, None, SkillSource::Local, None, &tag, overwrite_id)?;
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: true,
            message: format!("已导入本地技能「{}」", skill.title),
            skill: Some(skill),
        });
    }

    // If path contains nested skill dirs (one level)
    if src.is_dir() {
        let mut imported = Vec::new();
        let mut skipped = 0usize;
        let mut failed = 0usize;
        let mut errors: Vec<String> = Vec::new();
        // 更深层级的技能不会被导入，必须显式告诉用户，不能静默丢掉。
        let nested = count_nested_skill_dirs(&src);
        for entry in fs::read_dir(&src).map_err(|e| format!("读取目录失败: {}", e))? {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            let p = entry.path();
            if !is_skill_dir(&p) {
                continue;
            }
            let key = skill_identity_key(&p);
            let overwrite_id = match local_import_plan(&key, &existing_keys, &overwrite_map) {
                LocalImportPlan::Import => None,
                LocalImportPlan::Overwrite(id) => Some(id),
                LocalImportPlan::Skip => {
                    skipped += 1;
                    continue;
                }
            };
            // 单项失败不能中断整批：记入 failed/errors，其余技能照常导入。
            match import_skill_dir(&p, None, SkillSource::Local, None, &tag, overwrite_id) {
                Ok(record) => {
                    imported.push(record);
                    // 同一批里重复出现的同名技能也按重复处理
                    existing_keys.insert(key);
                }
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{}: {}", skill_display_name(&p), e));
                }
            }
        }
        if imported.is_empty() {
            let mut message = if failed > 0 {
                format!("所选目录中没有技能导入成功，失败 {} 个", failed)
            } else if skipped > 0 {
                format!("所选目录中的 {} 个技能均已存在，已跳过", skipped)
            } else {
                "所选目录中未找到包含 SKILL.md 的技能".into()
            };
            if failed > 0 && skipped > 0 {
                message.push_str(&format!("，跳过 {} 个重复技能", skipped));
            }
            if nested > 0 {
                message.push_str(&format!(
                    "；发现 {} 个技能在更深层级，未导入（只扫描一层）",
                    nested
                ));
            }
            return Ok(SkillActionResult {
                errors,
                ok: false,
                skill: None,
                message,
            });
        }
        let mut message = format!("已从本地导入 {} 个技能", imported.len());
        if skipped > 0 {
            message.push_str(&format!("，跳过 {} 个重复技能", skipped));
        }
        if failed > 0 {
            message.push_str(&format!("，失败 {} 个", failed));
        }
        if nested > 0 {
            message.push_str(&format!(
                "；另有 {} 个技能在更深层级，未导入（只扫描一层）",
                nested
            ));
        }
        return Ok(SkillActionResult {
            errors,
            ok: failed == 0,
            skill: imported.into_iter().next(),
            message,
        });
    }

    Ok(SkillActionResult {
        errors: Vec::new(),
        ok: false,
        skill: None,
        message: "请选择包含 SKILL.md 的技能目录".into(),
    })
}

/// 统计 `root` 下会被“只扫一层”忽略的技能目录数（深度 2..=3）。
/// 一层子目录由导入逻辑处理，这里只数深层候选；跳过 VCS/依赖目录与符号链接。
fn count_nested_skill_dirs(root: &Path) -> usize {
    let mut count = 0usize;
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        if depth >= 3 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                // 文件与符号链接都不下探（软链可能成环）。
                continue;
            }
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" || name == ".github" {
                continue;
            }
            let path = entry.path();
            if is_skill_dir(&path) {
                if depth + 1 >= 2 {
                    count += 1;
                }
                continue; // 已识别的技能目录不再下探
            }
            queue.push((path, depth + 1));
        }
    }
    count
}

/// Native folder picker. Returns selected path or cancel.
pub fn pick_local_skill_folder(tag: String) -> Result<SkillActionResult, String> {
    match crate::platform::pick_folder("选择 Skill 目录（需包含 SKILL.md）") {
        Ok(Some(path)) => add_skill_local(path.to_string_lossy().to_string(), tag, None),
        Ok(None) => Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "已取消选择".into(),
        }),
        Err(e) => Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: e,
        }),
    }
}

/* ===== Delete: remove a library skill (dir + metadata) ===== */

/// 计算某技能在各 Agent skills 目录下需要清理的条目名集合：
/// 库 id 本身，加上 frontmatter `name`（部分 Agent 以展示名安装）。
/// 二者都要求是「纯目录名」，防止路径穿越。
fn managed_entry_names(id: &str, skill_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = vec![id.to_string()];
    let doc = read_skill_doc(skill_dir);
    if !doc.name.is_empty() && doc.name != id && is_plain_entry_name(&doc.name) {
        names.push(doc.name);
    }
    names
}

/// 删除该技能在所有 Agent skills 目录下的副本（软链接 / 真实目录 / 悬空链接）。
/// 共享物理根（shared_root 标识相同者）只处理一次；错误不致命，逐条收集。
/// 返回清理成功的「物理根」数量，供上层提示。
///
/// `managed_names` 通常由 `managed_entry_names` 生成，已含库 id 与 frontmatter 名。
fn remove_skill_agent_copies(managed_names: &[String], errors: &mut Vec<String>) -> usize {
    let targets = agent_skills_targets();
    let mut handled_roots: BTreeSet<PathBuf> = BTreeSet::new();
    let mut removed_roots = 0usize;
    for target in &targets {
        if !target.supported {
            continue;
        }
        for root in &target.roots {
            if handled_roots.contains(root) {
                continue;
            }
            handled_roots.insert(root.clone());
            if !root.is_dir() {
                continue;
            }
            let mut removed_here = false;
            for name in managed_names {
                match remove_agent_skill_entry(root, name) {
                    Ok(true) => removed_here = true,
                    Ok(false) => {}
                    Err(e) => errors.push(format!("{}：{}", target.agent, e)),
                }
            }
            if removed_here {
                removed_roots += 1;
            }
        }
    }
    removed_roots
}

/// 删除单个技能的核心逻辑，供单卡与批量复用。
///
/// - `delete_agent_copies`：为真时，一并清理各 Agent skills 目录下的副本，
///   避免删除库目录后残留悬空软链接。
/// - 先在库目录仍存在时算出 `managed_names`（含 frontmatter 名），再删库目录，
///   最后按名清理各 Agent 副本，保证展示名安装也能被移除。
/// - 返回是否真的存在并删除了库目录；`false` 表示该技能本就不存在（调用方据此
///   区分「已删除」与「不存在」，不再把空操作报告成成功）。
fn delete_skill_core(
    id: &str,
    delete_agent_copies: bool,
    errors: &mut Vec<String>,
) -> Result<bool, String> {
    let lib = skills_library_dir()?;
    let dir = lib.join(id);
    // Ensure the resolved path stays within the library root.
    if !dir.starts_with(&lib) {
        return Err("非法的技能路径".into());
    }
    let existed = fs::symlink_metadata(&dir).is_ok();
    if !existed {
        return Ok(false);
    }

    // 库目录尚在时先解析受管条目名（含 frontmatter 名），删除后便无从读取。
    let managed = if delete_agent_copies {
        Some(managed_entry_names(id, &dir))
    } else {
        None
    };

    fs::remove_dir_all(&dir).map_err(|e| format!("删除技能目录失败: {}", e))?;
    // Best-effort metadata cleanup; disk removal already succeeded.
    let _ = db::delete_skill_meta(id);

    if let Some(names) = managed {
        remove_skill_agent_copies(&names, errors);
    }
    Ok(true)
}

/// 删除技能（可选一并清理各 Agent 副本）。
///
/// - 技能不存在时返回 `ok:false` +「技能不存在」，不再把空操作报告成成功；
/// - 副本清理为尽力而为：库目录删除成功即 `ok:true`，但失败项逐条进入 `errors`，
///   不只是压进 message。
pub fn delete_skill(
    skill_id: String,
    delete_agent_copies: bool,
) -> Result<SkillActionResult, String> {
    let id = skill_id.trim().to_string();
    if id.is_empty() {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "缺少技能标识".into(),
        });
    }
    // Guard against path traversal: id must be a plain directory name.
    if !is_plain_entry_name(&id) {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "非法的技能标识".into(),
        });
    }

    let mut errors: Vec<String> = Vec::new();
    let removed = delete_skill_core(&id, delete_agent_copies, &mut errors)?;
    if !removed {
        return Ok(SkillActionResult {
            ok: false,
            skill: None,
            message: "技能不存在".into(),
            errors: Vec::new(),
        });
    }

    let message = if errors.is_empty() {
        "已删除技能".into()
    } else {
        format!("已删除技能，但 {} 项副本清理未成功", errors.len())
    };
    Ok(SkillActionResult {
        ok: true,
        skill: None,
        message,
        errors,
    })
}

/* ===== Apply: sync a library skill into agents' skills dirs ===== */

/// How a library skill is installed into an Agent's skills directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInstallMode {
    Link,
    Copy,
}

impl SkillInstallMode {
    /// Missing or unrecognised wire values retain the historical link behavior.
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("copy") => Self::Copy,
            _ => Self::Link,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Link => "软链接",
            Self::Copy => "完整复制",
        }
    }
}

/// A directory-entry name must be a single, plain path component — never a
/// traversal token — before we join it onto an agent's skills root.
fn is_plain_entry_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

/// Remove a filesystem entry without following a symlink.
fn remove_path_entry(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("读取目标项失败: {}", e))?;
    if meta.file_type().is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("删除技能目录失败: {}", e))
    } else {
        fs::remove_file(path).map_err(|e| format!("删除技能项失败: {}", e))
    }
}

/// Install `target` under `<root>/<entry_name>` using the requested mode.
///
/// The new entry is prepared before an existing target is moved aside. If the
/// final replacement fails, the original entry is restored so an Agent's
/// existing skill is not lost to an IO or permissions error.
fn ensure_skill_install(
    root: &Path,
    entry_name: &str,
    target: &Path,
    mode: SkillInstallMode,
) -> Result<(), String> {
    if !is_plain_entry_name(entry_name) {
        return Err("非法的技能标识".into());
    }
    fs::create_dir_all(root).map_err(|e| format!("创建 skills 目录失败: {}", e))?;
    let destination = root.join(entry_name);
    // Defense in depth: resolved path must stay directly under root.
    if destination.parent() != Some(root) {
        return Err("非法的技能路径".into());
    }
    let nonce = format!("{}-{}", std::process::id(), now_secs());
    let staging = root.join(format!(".octopus-{}-staging-{}", entry_name, nonce));
    let backup = root.join(format!(".octopus-{}-backup-{}", entry_name, nonce));

    if fs::symlink_metadata(&staging).is_ok() || fs::symlink_metadata(&backup).is_ok() {
        return Err("技能安装临时路径已存在，请稍后重试".into());
    }

    // Copy 模式与导入/导出共用同一套复制语义（软链按链接保留，不跟随、不实体化）。
    let prepare = match mode {
        SkillInstallMode::Link => crate::platform::symlink_any(target, &staging),
        SkillInstallMode::Copy => copy_dir_recursive(target, &staging),
    };
    if let Err(e) = prepare {
        let _ = remove_path_entry(&staging);
        return Err(format!("创建{}失败: {}", mode.label(), e));
    }

    let had_destination = fs::symlink_metadata(&destination).is_ok();
    if had_destination {
        if let Err(e) = fs::rename(&destination, &backup) {
            let _ = remove_path_entry(&staging);
            return Err(format!("备份已有技能项失败: {}", e));
        }
    }

    if let Err(e) = fs::rename(&staging, &destination) {
        let _ = remove_path_entry(&staging);
        if had_destination {
            if let Err(restore_err) = fs::rename(&backup, &destination) {
                return Err(format!(
                    "安装失败: {}；恢复原技能项失败: {}",
                    e, restore_err
                ));
            }
        }
        return Err(format!("安装{}失败: {}", mode.label(), e));
    }

    if had_destination {
        remove_path_entry(&backup).map_err(|e| format!("清理旧技能项失败: {}", e))?;
    }
    Ok(())
}

/// Remove `<root>/<entry_name>` whether it is a symlink, real dir, or file.
/// Returns whether anything was removed.
fn remove_agent_skill_entry(root: &Path, entry_name: &str) -> Result<bool, String> {
    if !is_plain_entry_name(entry_name) {
        return Err("非法的技能标识".into());
    }
    let path = root.join(entry_name);
    if path.parent() != Some(root) {
        return Err("非法的技能路径".into());
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_dir() {
                fs::remove_dir_all(&path).map_err(|e| format!("删除技能目录失败: {}", e))?;
            } else {
                // symlink or file
                fs::remove_file(&path).map_err(|e| format!("删除技能项失败: {}", e))?;
            }
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Update a skill's tag and sync its installation across agents.
///
/// - `agents`: the desired set of agent sniff-names that should have this skill.
/// - Newly desired agents get the requested installation under their first skills root (shared
///   roots（shared_root 标识相同者）只写一次）。掉了右括号修正
/// - Agents that were previously applied but are now unchecked have their entry
///   removed (symlink or real copy), unless the same root is still needed by a
///   desired agent.
/// - Agents never applied and not desired are left completely untouched.
/// 安装同步结果（不含 record 重建，交由调用方）。
struct InstallationSyncOutcome {
    linked: usize,
    unlinked: usize,
    errors: Vec<String>,
}

/// 将库技能的安装同步到「恰好等于 `desired`」的最终态：
/// - `desired` 是最终应用集合（共享物理根的删除保护仍按此集合计算）；
/// - 之前已应用但不在 `desired` 的 Agent 移除其条目（保护仍被占用的根）。
/// - `install_targets` 是本次需要重装的 Agent；批量追加时仅包含新增目标，
///   避免改动其它已应用 Agent 的安装方式。
/// 不触碰 tag / meta。`lib_skill_dir` 须为库内、已校验的技能目录。
/// 记录重建（applied_after 扫描）由调用方用 `applied_after_for` 完成。
fn sync_skill_installations(
    id: &str,
    lib_skill_dir: &Path,
    desired: &BTreeSet<String>,
    install_targets: &BTreeSet<String>,
    install_mode: SkillInstallMode,
) -> InstallationSyncOutcome {
    let targets = agent_skills_targets();

    // Candidate entry names we manage: the library id, plus the frontmatter
    // name when it differs (some agents install under the display name).
    let managed_names = managed_entry_names(id, lib_skill_dir);

    let applied_before = scan_applied_agents();
    let doc_name = read_skill_doc(lib_skill_dir).name;
    let mut current: BTreeSet<String> = applied_before.get(id).cloned().unwrap_or_default();
    if !doc_name.is_empty() && doc_name != id {
        if let Some(extra) = applied_before.get(&doc_name) {
            current.extend(extra.iter().cloned());
        }
    }
    let to_unlink: BTreeSet<String> = current.difference(desired).cloned().collect();

    let mut errors: Vec<String> = Vec::new();

    // Install desired agents (dedupe by physical root for shared configs).
    let mut linked = 0usize;
    let mut ensured_roots: BTreeSet<PathBuf> = BTreeSet::new();
    let mut failed_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for agent in install_targets {
        let target = match targets.iter().find(|t| t.agent == agent.as_str()) {
            Some(t) => t,
            None => {
                errors.push(format!("未知 Agent：{}", agent));
                continue;
            }
        };
        if !target.supported {
            errors.push(format!("{} 暂不支持 Skills 目录，已跳过", agent));
            continue;
        }
        let root = match target.roots.first() {
            Some(r) => r.clone(),
            None => {
                errors.push(format!("{} 无可用 skills 目录", agent));
                continue;
            }
        };
        if ensured_roots.contains(&root) {
            linked += 1;
            continue;
        }
        if failed_roots.contains(&root) {
            continue; // error already recorded for this shared root
        }
        match ensure_skill_install(&root, id, lib_skill_dir, install_mode) {
            Ok(()) => {
                ensured_roots.insert(root);
                linked += 1;
            }
            Err(e) => {
                failed_roots.insert(root);
                errors.push(format!("{}：{}", agent, e));
            }
        }
    }

    // Remove entries for de-selected agents, protecting roots still in use.
    let mut protected: BTreeSet<PathBuf> = BTreeSet::new();
    for agent in desired {
        if let Some(t) = targets.iter().find(|t| t.agent == agent.as_str()) {
            for r in &t.roots {
                protected.insert(r.clone());
            }
        }
    }

    let mut unlinked = 0usize;
    let mut removed_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for agent in &to_unlink {
        let target = match targets.iter().find(|t| t.agent == agent.as_str()) {
            Some(t) => t,
            None => continue,
        };
        let mut removed_any = false;
        for root in &target.roots {
            if protected.contains(root) {
                continue;
            }
            if removed_roots.contains(root) {
                removed_any = true;
                continue;
            }
            let mut removed_here = false;
            for name in &managed_names {
                match remove_agent_skill_entry(root, name) {
                    Ok(true) => removed_here = true,
                    Ok(false) => {}
                    Err(e) => errors.push(format!("{}：{}", agent, e)),
                }
            }
            if removed_here {
                removed_roots.insert(root.clone());
                removed_any = true;
            }
        }
        if removed_any {
            unlinked += 1;
        }
    }

    InstallationSyncOutcome {
        linked,
        unlinked,
        errors,
    }
}

/// 以一次全新扫描重建某技能的 applied Agent 列表（含 frontmatter 名匹配、排序）。
fn applied_after_for(id: &str, lib_skill_dir: &Path) -> Vec<String> {
    let doc_name = read_skill_doc(lib_skill_dir).name;
    let applied_after = scan_applied_agents();
    let mut applied: Vec<String> = applied_after
        .get(id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    if !doc_name.is_empty() && doc_name != id {
        if let Some(extra) = applied_after.get(&doc_name) {
            for a in extra {
                if !applied.contains(a) {
                    applied.push(a.clone());
                }
            }
        }
    }
    applied.sort_by_key(|a| crate::agents::display_order(a));
    applied
}

pub fn apply_skill_to_agents(
    skill_id: String,
    agents: Vec<String>,
    tag: String,
    install_mode: SkillInstallMode,
) -> Result<SkillApplyResult, String> {
    let id = skill_id.trim().to_string();
    if !is_plain_entry_name(&id) {
        return Ok(SkillApplyResult {
            ok: false,
            skill: None,
            linked: 0,
            unlinked: 0,
            message: "非法的技能标识".into(),
            errors: vec![],
        });
    }

    let lib = skills_library_dir()?;
    let lib_skill_dir = lib.join(&id);
    if !lib_skill_dir.starts_with(&lib) || !is_skill_dir(&lib_skill_dir) {
        return Ok(SkillApplyResult {
            ok: false,
            skill: None,
            linked: 0,
            unlinked: 0,
            message: "技能不存在或缺少 SKILL.md".into(),
            errors: vec![],
        });
    }

    // 1) Sync installations first: a failed install must not leave the tag changed
    //    (the dialog is one action; partial failure keeps the old tag so a retry is safe).
    let desired: BTreeSet<String> = agents
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    let outcome = sync_skill_installations(&id, &lib_skill_dir, &desired, &desired, install_mode);

    // 2) Persist the tag only when the install step reported no errors.
    let mut meta = db::get_skill_meta(&id)
        .unwrap_or_default()
        .unwrap_or_else(default_local_meta);
    if outcome.errors.is_empty() {
        meta.tag = tag.trim().to_string();
        meta.updated_at = now_secs();
        db::upsert_skill_meta(&id, &meta)?;
    }

    // 3) Rebuild the record with a fresh applied scan.
    let applied = applied_after_for(&id, &lib_skill_dir);
    let record = build_record(&id, &lib_skill_dir, &meta, &applied, false);

    let message = if outcome.errors.is_empty() {
        format!(
            "已更新「{}」：以{}应用 {} 个、解除 {} 个 Agent",
            record.title,
            install_mode.label(),
            outcome.linked,
            outcome.unlinked
        )
    } else {
        format!(
            "「{}」部分完成：以{}应用 {} 个、解除 {} 个，{} 项未成功；标签未更新",
            record.title,
            install_mode.label(),
            outcome.linked,
            outcome.unlinked,
            outcome.errors.len()
        )
    };

    Ok(SkillApplyResult {
        ok: outcome.errors.is_empty(),
        skill: Some(record),
        linked: outcome.linked,
        unlinked: outcome.unlinked,
        message,
        errors: outcome.errors,
    })
}

/* ===== Batch operations (delete / export / apply) ===== */

/// 去重并保留原始顺序地规整一批技能标识；过滤掉非法（含穿越）项。
fn sanitize_id_batch(ids: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for raw in ids {
        let id = raw.trim().to_string();
        if !is_plain_entry_name(&id) {
            continue;
        }
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

/// 批量删除技能（可选一并清理各 Agent 副本）。逐项容错：任一失败不中断，
/// 记入 `errors`；最后统一刷新整表返回。
pub fn batch_delete_skills(
    skill_ids: Vec<String>,
    delete_agent_copies: bool,
) -> Result<BatchSkillResult, String> {
    let ids = sanitize_id_batch(&skill_ids);
    if ids.is_empty() {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择有效技能".into(),
            errors: vec![],
        });
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for id in &ids {
        let mut copy_errors: Vec<String> = Vec::new();
        match delete_skill_core(id, delete_agent_copies, &mut copy_errors) {
            Ok(true) => {
                succeeded += 1;
                // 副本清理失败不影响删除结果，但需逐条提示。
                for e in copy_errors {
                    errors.push(format!("「{}」副本清理：{}", id, e));
                }
            }
            Ok(false) => {
                // 不存在不属于失败，也不算成功，单独计入 skipped。
                skipped += 1;
                errors.push(format!("「{}」：技能不存在", id));
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("「{}」：{}", id, e));
            }
        }
    }

    let list = list_skills()?;
    let mut parts: Vec<String> = vec![format!("已删除 {} 个技能", succeeded)];
    if skipped > 0 {
        parts.push(format!("跳过 {} 个（不存在）", skipped));
    }
    if failed > 0 {
        parts.push(format!("{} 个失败", failed));
    }
    Ok(BatchSkillResult {
        missing_upstream: Vec::new(),
        ok: failed == 0,
        succeeded,
        failed,
        skipped,
        skills: list.skills,
        message: parts.join("，"),
        errors,
    })
}

/// 批量应用模式：追加（并入现有应用）或覆盖（以选中 Agent 为最终态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchApplyMode {
    Add,
    Replace,
}

/// 批量把选中技能应用到一组 Agent。
/// - `Add`：把 `agents` 并入每个技能已应用的 Agent（不解除任何现有应用）。
/// - `Replace`：把每个技能的应用最终态设为恰好 `agents`（会解除多余项）。
/// tag 一律不改动；逐项按 `install_mode` 同步。逐项容错。
pub fn batch_apply_skills_to_agents(
    skill_ids: Vec<String>,
    agents: Vec<String>,
    mode: BatchApplyMode,
    install_mode: SkillInstallMode,
) -> Result<BatchSkillResult, String> {
    let ids = sanitize_id_batch(&skill_ids);
    if ids.is_empty() {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择有效技能".into(),
            errors: vec![],
        });
    }

    let selected_agents: BTreeSet<String> = agents
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    // 覆盖模式允许清空（= 从所有 Agent 解除）；追加模式空选无意义。
    if selected_agents.is_empty() && mode == BatchApplyMode::Add {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择要应用的 Agent".into(),
            errors: vec![],
        });
    }

    let lib = skills_library_dir()?;
    // 追加模式需要每个技能的当前应用集，先做一次全量扫描避免重复 IO。
    let applied_map = scan_applied_agents();

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for id in &ids {
        let lib_skill_dir = lib.join(id);
        if !lib_skill_dir.starts_with(&lib) || !is_skill_dir(&lib_skill_dir) {
            failed += 1;
            errors.push(format!("「{}」：技能不存在或缺少 SKILL.md", id));
            continue;
        }

        // 计算该技能的目标最终态。
        let desired: BTreeSet<String> = if mode == BatchApplyMode::Replace {
            selected_agents.clone()
        } else {
            // 并入现有应用：库 id 与 frontmatter 名两处都算作已应用。
            let mut cur: BTreeSet<String> = applied_map.get(id).cloned().unwrap_or_default();
            let doc_name = read_skill_doc(&lib_skill_dir).name;
            if !doc_name.is_empty() && doc_name != *id {
                if let Some(extra) = applied_map.get(&doc_name) {
                    cur.extend(extra.iter().cloned());
                }
            }
            cur.extend(selected_agents.iter().cloned());
            cur
        };

        let install_targets = if mode == BatchApplyMode::Replace {
            &desired
        } else {
            &selected_agents
        };
        let outcome =
            sync_skill_installations(id, &lib_skill_dir, &desired, install_targets, install_mode);
        if outcome.errors.is_empty() {
            succeeded += 1;
        } else {
            failed += 1;
            for e in outcome.errors {
                errors.push(format!("「{}」：{}", id, e));
            }
        }
    }

    let list = list_skills()?;
    // 覆盖模式 + 空 Agent 集合 = 从所有 Agent 解除，此时说「应用 N 个」与事实相反。
    let clearing = mode == BatchApplyMode::Replace && selected_agents.is_empty();
    let message = match (failed == 0, clearing) {
        (true, true) => format!("已从所有 Agent 解除 {} 个技能", succeeded),
        (true, false) => {
            let verb = if mode == BatchApplyMode::Replace {
                "覆盖应用"
            } else {
                "追加应用"
            };
            format!("已以{}{} {} 个技能", install_mode.label(), verb, succeeded)
        }
        (false, true) => format!("解除 {} 个成功，{} 个失败", succeeded, failed),
        (false, false) => {
            let verb = if mode == BatchApplyMode::Replace {
                "覆盖应用"
            } else {
                "追加应用"
            };
            format!(
                "以{}{} {} 个成功，{} 个失败",
                install_mode.label(),
                verb,
                succeeded,
                failed
            )
        }
    };
    Ok(BatchSkillResult {
        missing_upstream: Vec::new(),
        ok: failed == 0,
        succeeded,
        failed,
        skipped: 0,
        skills: list.skills,
        message,
        errors,
    })
}

/// 批量设置选中技能的标签（tag）。只更新 SQLite meta 的 `tag` / `updated_at`，
/// 不触碰软链接、不改动库目录，也不影响各 Agent 的应用状态。逐项容错。
///
/// - `tag` 会做 trim；允许传空串表示「清除标签」。
/// - 技能不在库中（缺失或非技能目录）计入 `failed` 并记录原因。
pub fn batch_set_skill_tag(
    skill_ids: Vec<String>,
    tag: String,
) -> Result<BatchSkillResult, String> {
    let ids = sanitize_id_batch(&skill_ids);
    if ids.is_empty() {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择有效技能".into(),
            errors: vec![],
        });
    }

    let normalized_tag = tag.trim().to_string();
    let lib = skills_library_dir()?;
    let ts = now_secs();

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for id in &ids {
        let lib_skill_dir = lib.join(id);
        if !lib_skill_dir.starts_with(&lib) || !is_skill_dir(&lib_skill_dir) {
            failed += 1;
            errors.push(format!("「{}」：技能不存在或缺少 SKILL.md", id));
            continue;
        }
        let mut meta = db::get_skill_meta(id)
            .unwrap_or_default()
            .unwrap_or_else(default_local_meta);
        meta.tag = normalized_tag.clone();
        meta.updated_at = ts;
        match db::upsert_skill_meta(id, &meta) {
            Ok(()) => succeeded += 1,
            Err(e) => {
                failed += 1;
                errors.push(format!("「{}」：{}", id, e));
            }
        }
    }

    let list = list_skills()?;
    let action = if normalized_tag.is_empty() {
        "清除标签"
    } else {
        "设置标签"
    };
    let message = if failed == 0 {
        format!("已为 {} 个技能{}", succeeded, action)
    } else {
        format!("{} {} 个成功，{} 个失败", action, succeeded, failed)
    };
    Ok(BatchSkillResult {
        missing_upstream: Vec::new(),
        ok: failed == 0,
        succeeded,
        failed,
        skipped: 0,
        skills: list.skills,
        message,
        errors,
    })
}

/* ===== Apply an existing library skill into a chosen directory ===== */

/// 弹出系统文件夹选择器，返回用户选定并校验通过的目标根目录。
/// - `Ok(Some(root))`：选定的有效目录；
/// - `Ok(None)`：用户取消（不视为错误）；
/// - `Err(msg)`：选择器调用失败或目录无效。
fn pick_target_folder(prompt: &str) -> Result<Option<PathBuf>, String> {
    match crate::platform::pick_folder(prompt)? {
        None => Ok(None),
        Some(path) => {
            let target_root = expand_tilde(&path.to_string_lossy());
            if !target_root.is_dir() {
                return Err("目标目录无效".into());
            }
            Ok(Some(target_root))
        }
    }
}

/// 单技能导出到目录的结果。目标已有同名条目属于“按用户选择跳过”，不是失败，
/// 因此用枚举表达，调用方不得再靠错误文案分类。
#[derive(Debug)]
enum DirInstallOutcome {
    Installed,
    Exists {
        existing_version: String,
        import_version: String,
    },
}

/// 在 `target_root` 下把库技能 `id` 安装为 `<target_root>/<id>`。
/// 未选择覆盖时遇到同名条目返回 `DirInstallOutcome::Exists`（不写盘）。
fn install_skill_into_dir(
    id: &str,
    target_root: &Path,
    install_mode: SkillInstallMode,
    overwrite: bool,
) -> Result<DirInstallOutcome, String> {
    if !is_plain_entry_name(id) {
        return Err("非法的技能标识".into());
    }
    let lib = skills_library_dir()?;
    let src = lib.join(id);
    if !src.starts_with(&lib) || !is_skill_dir(&src) {
        return Err("技能目录不存在或缺少 SKILL.md".into());
    }
    if !target_root.is_dir() {
        return Err("目标目录不存在或不是目录".into());
    }
    let dest = target_root.join(id);
    // Defense in depth: resolved dest must stay directly under target_root.
    if dest.parent() != Some(target_root) {
        return Err("非法的技能路径".into());
    }
    // 已存在同名条目且未选择覆盖：跳过（带版本信息，供调用方汇总展示）。
    if fs::symlink_metadata(&dest).is_ok() && !overwrite {
        return Ok(DirInstallOutcome::Exists {
            existing_version: read_skill_doc(&dest).version,
            import_version: read_skill_doc(&src).version,
        });
    }
    // 覆盖也走 staging：先准备好新条目，再原子替换，失败时保留用户已有目录。
    let (staging, backup) = staging_paths(target_root, id);
    if fs::symlink_metadata(&staging).is_ok() || fs::symlink_metadata(&backup).is_ok() {
        return Err("导出临时路径已存在，请稍后重试".into());
    }
    let source = fs::canonicalize(&src).unwrap_or(src);
    let prepared = match install_mode {
        SkillInstallMode::Link => crate::platform::symlink_any(&source, &staging),
        SkillInstallMode::Copy => copy_dir_recursive(&source, &staging),
    };
    if let Err(e) = prepared {
        let _ = remove_path_entry(&staging);
        return Err(format!("创建{}失败: {}", install_mode.label(), e));
    }
    replace_path_with_staged(&staging, &dest, &backup)?;
    Ok(DirInstallOutcome::Installed)
}

/// Pick a target folder and check for duplicate skills at the target.
/// Returns conflicts with version info; frontend shows a dialog for user to choose.
pub fn check_export_duplicates(
    skill_ids: Vec<String>,
) -> Result<ExportDuplicateCheckResult, String> {
    let ids = sanitize_id_batch(&skill_ids);
    if ids.is_empty() {
        return Ok(ExportDuplicateCheckResult {
            ok: false,
            conflicts: Vec::new(),
            target_dir: String::new(),
            message: "未选择有效技能".into(),
        });
    }

    let target_root = match pick_target_folder("选择应用目标目录") {
        Ok(Some(root)) => root,
        Ok(None) => {
            return Ok(ExportDuplicateCheckResult {
                ok: false,
                conflicts: Vec::new(),
                target_dir: String::new(),
                message: "已取消选择".into(),
            });
        }
        Err(msg) => {
            return Ok(ExportDuplicateCheckResult {
                ok: false,
                conflicts: Vec::new(),
                target_dir: String::new(),
                message: msg,
            });
        }
    };

    let lib = skills_library_dir()?;
    let mut conflicts = Vec::new();
    for id in &ids {
        let src = lib.join(id);
        if !is_skill_dir(&src) {
            continue;
        }
        let dest = target_root.join(id);
        if fs::symlink_metadata(&dest).is_ok() {
            let lib_doc = read_skill_doc(&src);
            let dest_doc = read_skill_doc(&dest);
            conflicts.push(ExportDuplicateConflict {
                skill_id: id.clone(),
                title: if !lib_doc.name.is_empty() {
                    lib_doc.name
                } else {
                    id.clone()
                },
                library_version: lib_doc.version,
                target_version: dest_doc.version,
            });
        }
    }

    Ok(ExportDuplicateCheckResult {
        ok: true,
        conflicts,
        target_dir: target_root.to_string_lossy().to_string(),
        message: String::new(),
    })
}

/// Export skills to a directory, optionally overwriting specified skill ids.
pub fn export_skills_to_dir(
    skill_ids: Vec<String>,
    install_mode: SkillInstallMode,
    target_dir: String,
    overwrite_ids: Vec<String>,
) -> Result<BatchSkillResult, String> {
    let ids = sanitize_id_batch(&skill_ids);
    if ids.is_empty() {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择有效技能".into(),
            errors: vec![],
        });
    }

    let target_root = PathBuf::from(&target_dir);
    let overwrite_set: BTreeSet<String> = overwrite_ids.iter().cloned().collect();

    let mut succeeded = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for id in &ids {
        let overwrite = overwrite_set.contains(id);
        match install_skill_into_dir(id, &target_root, install_mode, overwrite) {
            Ok(DirInstallOutcome::Installed) => succeeded += 1,
            Ok(DirInstallOutcome::Exists {
                existing_version,
                import_version,
            }) => {
                skipped += 1;
                errors.push(format!(
                    "「{}」：目标已存在同名条目（existing_version={}|import_version={}）",
                    id, existing_version, import_version
                ));
            }
            Err(msg) => {
                failed += 1;
                errors.push(format!("「{}」：{}", id, msg));
            }
        }
    }

    let list = list_skills()?;
    let mut parts: Vec<String> = vec![format!("已{} {} 个", install_mode.label(), succeeded)];
    if skipped > 0 {
        parts.push(format!("跳过 {} 个（同名已存在）", skipped));
    }
    if failed > 0 {
        parts.push(format!("失败 {} 个", failed));
    }
    // 跳过是用户“不勾选覆盖”的预期结果，不参与成败判定。
    Ok(BatchSkillResult {
        missing_upstream: Vec::new(),
        ok: failed == 0,
        succeeded,
        failed,
        skipped,
        skills: list.skills,
        message: parts.join("，"),
        errors,
    })
}

/* ===== Add: git repo (GitHub / GitCode) ===== */

/// Supported git hosting platforms for remote skill import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHost {
    Github,
    Gitcode,
}

impl GitHost {
    fn domain(&self) -> &'static str {
        match self {
            GitHost::Github => "github.com",
            GitHost::Gitcode => "gitcode.com",
        }
    }

    fn source(&self) -> SkillSource {
        match self {
            GitHost::Github => SkillSource::Github,
            GitHost::Gitcode => SkillSource::Gitcode,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            GitHost::Github => "GitHub",
            GitHost::Gitcode => "GitCode",
        }
    }
}

#[derive(Debug, Clone)]
struct GitRepoRef {
    host: GitHost,
    owner: String,
    repo: String,
    /// optional subpath inside repo
    path: String,
    repo_url: String,
    /// branch or empty (default)
    branch: String,
}

/// Parse a git repo reference for the given host. Accepts:
/// - https://{domain}/owner/repo(.git)
/// - https://{domain}/owner/repo/tree|blob/branch/path
/// - {domain}/owner/repo
/// - owner/repo (shorthand — assumes `host`)
/// - git@{domain}:owner/repo.git
fn parse_git_url(input: &str, host: GitHost) -> Result<GitRepoRef, String> {
    let domain = host.domain();
    let raw = input.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(format!("{} 仓库地址不能为空", host.label()));
    }

    let mut s = raw.to_string();
    let ssh_prefix = format!("git@{}:", domain);
    if let Some(rest) = s.strip_prefix(&ssh_prefix) {
        s = format!("https://{}/{}", domain, rest.trim_end_matches(".git"));
    }
    let domain_prefix = format!("{}/", domain);
    if s.starts_with(&domain_prefix) {
        s = format!("https://{}", s);
    }

    if !s.contains("://") && s.matches('/').count() == 1 {
        // owner/repo shorthand
        let parts: Vec<&str> = s.splitn(2, '/').collect();
        let repo = parts[1].trim_end_matches(".git").to_string();
        return Ok(GitRepoRef {
            host,
            owner: parts[0].to_string(),
            repo: repo.clone(),
            path: String::new(),
            repo_url: format!("https://{}/{}/{}", domain, parts[0], repo),
            branch: String::new(),
        });
    }

    let url = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let url = url.trim_start_matches("www.");
    if !url.starts_with(&domain_prefix) {
        return Err(format!("仅支持 {} 仓库地址", host.label()));
    }
    let rest = &url[domain_prefix.len()..];
    let segs: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if segs.len() < 2 {
        return Err(format!(
            "无法解析 {} 仓库，请使用 owner/repo 或完整 URL",
            host.label()
        ));
    }
    let owner = segs[0].to_string();
    let repo = segs[1].trim_end_matches(".git").to_string();
    let mut branch = String::new();
    let mut path = String::new();
    if segs.len() >= 4 && (segs[2] == "tree" || segs[2] == "blob") {
        branch = segs[3].to_string();
        if segs.len() > 4 {
            path = segs[4..].join("/");
            if segs[2] == "blob" {
                // drop file name — take parent as skill path later
                if let Some(parent) = Path::new(&path).parent() {
                    path = parent.to_string_lossy().to_string();
                    if path == "." {
                        path.clear();
                    }
                }
            }
        }
    }
    Ok(GitRepoRef {
        host,
        owner: owner.clone(),
        repo: repo.clone(),
        path,
        repo_url: format!("https://{}/{}/{}", domain, owner, repo),
        branch,
    })
}

fn find_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if is_skill_dir(root) {
        out.push(root.to_path_buf());
        return out;
    }
    // shallow BFS (depth <= 3) to find skill packages
    let mut queue: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        if depth > 3 {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" || name == ".github" {
                continue;
            }
            if path.is_dir() {
                if is_skill_dir(&path) {
                    out.push(path);
                } else if depth < 3 {
                    queue.push((path, depth + 1));
                }
            }
        }
    }
    out
}

fn git_clone(url: &str, branch: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        let _ = fs::remove_dir_all(dest);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建临时目录失败: {}", e))?;
    }

    let mut args = vec!["clone".to_string(), "--depth".into(), "1".into()];
    if !branch.is_empty() {
        args.push("--branch".into());
        args.push(branch.to_string());
    }
    args.push(url.to_string());
    args.push(dest.to_string_lossy().to_string());

    let output = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| format!("执行 git 失败（请确认已安装 Git）: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone 失败: {}", err.trim()));
    }
    Ok(())
}

fn temp_clone_dir(owner: &str, repo: &str) -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join("octopus-skills");
    fs::create_dir_all(&base).map_err(|e| format!("创建临时目录失败: {}", e))?;
    Ok(base.join(format!("{}-{}-{}", owner, repo, now_secs())))
}

pub fn add_skill_github(url: String, tag: String) -> Result<SkillActionResult, String> {
    let gh = parse_git_url(&url, GitHost::Github)?;
    add_skill_from_git(gh, &tag)
}

pub fn add_skill_gitcode(url: String, tag: String) -> Result<SkillActionResult, String> {
    let gh = parse_git_url(&url, GitHost::Gitcode)?;
    add_skill_from_git(gh, &tag)
}

fn add_skill_from_git(gh: GitRepoRef, tag: &str) -> Result<SkillActionResult, String> {
    let source = gh.host.source();
    let host_label = gh.host.label();
    let clone_url = format!("{}.git", gh.repo_url);
    let tmp = temp_clone_dir(&gh.owner, &gh.repo)?;
    git_clone(&clone_url, &gh.branch, &tmp)?;

    let search_root = if gh.path.is_empty() {
        tmp.clone()
    } else {
        tmp.join(&gh.path)
    };
    if !search_root.exists() {
        let _ = fs::remove_dir_all(&tmp);
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: format!("仓库中不存在路径: {}", gh.path),
        });
    }

    let skill_dirs = find_skill_dirs(&search_root);
    if skill_dirs.is_empty() {
        let _ = fs::remove_dir_all(&tmp);
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "仓库中未找到包含 SKILL.md 的技能目录".into(),
        });
    }

    // Dedupe by skill identity (frontmatter `name`, else dir name) so a repo that
    // vendors the same SKILL.md under multiple paths imports it only once, and so a
    // re-import does not create `name-2` copies of skills already in the library.
    let mut seen_keys: BTreeSet<String> = list_skills()
        .map(|r| {
            r.skills
                .iter()
                .map(|s| s.title.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let dedupe_key = |dir: &Path| -> String {
        let name = read_skill_doc(dir).name.trim().to_lowercase();
        if !name.is_empty() {
            name
        } else {
            dir.file_name()
                .map(|s| s.to_string_lossy().trim().to_lowercase())
                .unwrap_or_default()
        }
    };

    let mut last: Option<SkillRecord> = None;
    let mut count = 0usize;
    let mut skipped_dupe = 0usize;
    for dir in &skill_dirs {
        let key = dedupe_key(dir);
        if !key.is_empty() && !seen_keys.insert(key) {
            skipped_dupe += 1;
            continue;
        }
        let sub_path = dir
            .strip_prefix(&tmp)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut gh_one = gh.clone();
        gh_one.path = sub_path;
        match import_skill_dir(dir, None, source.clone(), Some(gh_one), tag, None) {
            Ok(rec) => {
                // capture local git HEAD for update checks
                if let Ok(head) = git_rev_parse(dir.parent().unwrap_or(dir)) {
                    let _ = db::update_skill_refs(&rec.id, Some(&head), Some(&head), now_secs());
                }
                count += 1;
                last = Some(rec);
            }
            Err(e) => {
                eprintln!("[skills] import failed {}: {}", dir.display(), e);
            }
        }
    }

    // Prefer capturing commit from clone root
    if let Ok(head) = git_rev_parse(&tmp) {
        if let Some(ref rec) = last {
            let _ = db::update_skill_refs(&rec.id, Some(&head), Some(&head), now_secs());
        }
        // stamp all skills from this owner/repo that still lack local_ref
        if let Ok(map) = db::load_skill_meta_map() {
            for (id, entry) in map {
                if entry.source == source
                    && entry.github_owner == gh.owner
                    && entry.github_repo == gh.repo
                    && entry.local_ref.is_empty()
                {
                    let _ = db::update_skill_refs(&id, Some(&head), Some(&head), now_secs());
                }
            }
        }
    }

    let _ = fs::remove_dir_all(&tmp);

    if count == 0 {
        let message = if skipped_dupe > 0 {
            format!(
                "未导入新技能：{} 个技能已存在于技能库或在仓库中重复",
                skipped_dupe
            )
        } else {
            "导入失败，请检查仓库内容".into()
        };
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: skipped_dupe > 0,
            skill: None,
            message,
        });
    }

    let message = if skipped_dupe > 0 {
        format!(
            "已从 {} {}/{} 导入 {} 个技能，跳过 {} 个重复项",
            host_label, gh.owner, gh.repo, count, skipped_dupe
        )
    } else {
        format!(
            "已从 {} {}/{} 导入 {} 个技能",
            host_label, gh.owner, gh.repo, count
        )
    };

    Ok(SkillActionResult {
        errors: Vec::new(),
        ok: true,
        skill: last,
        message,
    })
}

fn git_rev_parse(repo: &Path) -> Result<String, String> {
    // walk up for .git
    let mut cur = repo.to_path_buf();
    let root = loop {
        if cur.join(".git").exists() {
            break cur;
        }
        if !cur.pop() {
            return Err("不是 git 仓库".into());
        }
    };
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("git rev-parse 失败: {}", e))?;
    if !output.status.success() {
        return Err("git rev-parse 失败".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/* ===== Sniff skills from agents ===== */

/// One aggregated skill discovered across agents' skills dirs.
struct SniffScanEntry {
    /// Lowercased identity: frontmatter `name` if present, else dir name.
    identity: String,
    /// Directory name (preferred library id on import).
    directory: String,
    source_path: PathBuf,
    title: String,
    description: String,
    /// Agents where this skill identity was found.
    found_agents: BTreeSet<String>,
}

/// Compute the lowercased identity of a skill dir (frontmatter `name`, else dir name).
fn skill_identity(dir_name: &str, doc: &SkillDoc) -> String {
    if !doc.name.trim().is_empty() {
        doc.name.trim().to_lowercase()
    } else {
        dir_name.trim().to_lowercase()
    }
}

/// Scan every supported agent's skills dir, aggregating by skill identity so a
/// skill installed in multiple agents becomes one entry with merged `found_agents`.
/// Returns (scanned_agents, entries). The first physical dir seen wins as the
/// import source. Mirrors `scan_applied_agents` for shared roots,
/// so both agents are credited.
fn collect_sniff_scan() -> (usize, Vec<SniffScanEntry>) {
    let mut scanned_agents = 0usize;
    let mut map: std::collections::BTreeMap<String, SniffScanEntry> =
        std::collections::BTreeMap::new();

    for target in agent_skills_targets() {
        if !target.supported {
            continue;
        }
        scanned_agents += 1;
        for root in &target.roots {
            if !root.is_dir() {
                continue;
            }
            let entries = match fs::read_dir(root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !is_skill_dir(&path) {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if dir_name.starts_with('.') || dir_name.starts_with("__octopus") {
                    continue;
                }
                let doc = read_skill_doc(&path);
                let identity = skill_identity(&dir_name, &doc);
                let slot = map
                    .entry(identity.clone())
                    .or_insert_with(|| SniffScanEntry {
                        identity,
                        directory: dir_name.clone(),
                        source_path: path.clone(),
                        title: if !doc.name.trim().is_empty() {
                            doc.name.trim().to_string()
                        } else {
                            dir_name.clone()
                        },
                        description: doc.description.clone(),
                        found_agents: BTreeSet::new(),
                    });
                slot.found_agents.insert(target.agent.to_string());
            }
        }
    }

    (scanned_agents, map.into_values().collect())
}

/// Identities already present in the library: lowercased dir id + frontmatter name.
fn existing_library_identities(lib: &Path) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let entries = match fs::read_dir(lib) {
        Ok(e) => e,
        Err(_) => return set,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_skill_dir(&path) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if id.starts_with('.') {
            continue;
        }
        set.insert(id.trim().to_lowercase());
        let doc = read_skill_doc(&path);
        if !doc.name.trim().is_empty() {
            set.insert(doc.name.trim().to_lowercase());
        }
    }
    set
}

/// True when a scanned skill already exists in the library (by identity or dir name).
fn sniff_entry_exists(entry: &SniffScanEntry, existing: &BTreeSet<String>) -> bool {
    existing.contains(&entry.identity) || existing.contains(&entry.directory.trim().to_lowercase())
}

/// Scan agents and build an import preview (no files copied). Each item is
/// tagged "import" or "skip_exists" and lists the agents it was found in.
pub fn preview_sniff_skills() -> Result<SniffPreviewResult, String> {
    let _ = db::migrate_skills_meta_json_if_present();
    let lib = skills_library_dir()?;
    let (scanned_agents, scan) = collect_sniff_scan();
    let existing = existing_library_identities(&lib);

    let mut items: Vec<SniffPreviewItem> = Vec::new();
    for entry in &scan {
        let exists = sniff_entry_exists(entry, &existing);
        let (status, status_label) = if exists {
            ("skip_exists", "技能库已存在，将跳过")
        } else {
            ("import", "将导入")
        };
        items.push(SniffPreviewItem {
            key: entry.source_path.to_string_lossy().to_string(),
            directory: entry.directory.clone(),
            title: entry.title.clone(),
            description: entry.description.clone(),
            source_path: entry.source_path.to_string_lossy().to_string(),
            found_agents: entry.found_agents.iter().cloned().collect(),
            status: status.to_string(),
            status_label: status_label.to_string(),
        });
    }

    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));

    let importable = items.iter().filter(|i| i.status == "import").count();
    let skip_exists = items.iter().filter(|i| i.status == "skip_exists").count();
    let total = items.len();

    let message = if total == 0 {
        format!("已扫描 {} 个 Agent，未发现任何 Skill", scanned_agents)
    } else {
        format!(
            "已扫描 {} 个 Agent，发现 {} 个技能：可导入 {}，已存在跳过 {}",
            scanned_agents, total, importable, skip_exists
        )
    };

    Ok(SniffPreviewResult {
        ok: true,
        items,
        scanned_agents,
        total,
        importable,
        skip_exists,
        message,
    })
}

/// Import the selected sniffed skills (by `key` = source path). Re-scans and
/// re-checks existence so a stale selection can't overwrite or duplicate.
pub fn import_sniffed_skills(keys: Vec<String>) -> Result<SniffImportResult, String> {
    if keys.is_empty() {
        return Ok(SniffImportResult {
            ok: true,
            imported: 0,
            skipped: 0,
            failed: 0,
            skills: list_skills()?.skills,
            message: "未选择任何技能".into(),
            errors: vec![],
        });
    }

    let lib = skills_library_dir()?;
    let (_scanned, scan) = collect_sniff_scan();
    let existing = existing_library_identities(&lib);
    let wanted: BTreeSet<&str> = keys.iter().map(|s| s.as_str()).collect();

    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for entry in &scan {
        let key = entry.source_path.to_string_lossy().to_string();
        if !wanted.contains(key.as_str()) {
            continue;
        }
        // Re-check: another import may have added it since preview.
        if sniff_entry_exists(entry, &existing) {
            skipped += 1;
            continue;
        }
        match import_skill_dir(
            &entry.source_path,
            Some(&entry.directory),
            SkillSource::Local,
            None,
            "",
            None,
        ) {
            Ok(_rec) => imported += 1,
            Err(e) => {
                failed += 1;
                errors.push(format!("{}: {}", entry.title, e));
            }
        }
    }

    let listed = list_skills()?;
    let message = if failed == 0 {
        format!("导入完成：新增 {} 个，跳过 {} 个", imported, skipped)
    } else {
        format!(
            "导入结束：新增 {}，跳过 {}，失败 {}",
            imported, skipped, failed
        )
    };

    Ok(SniffImportResult {
        ok: failed == 0,
        imported,
        skipped,
        failed,
        skills: listed.skills,
        message,
        errors,
    })
}

pub fn sniff_skills() -> Result<SkillSniffResult, String> {
    let _ = db::migrate_skills_meta_json_if_present();
    let lib = skills_library_dir()?;
    let mut meta_map = db::load_skill_meta_map().unwrap_or_default();
    let mut imported = 0usize;
    let mut found_entries = 0usize;
    let mut scanned_agents = 0usize;
    // Avoid double-importing shared roots (shared_root 标识相同者)
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();

    for target in agent_skills_targets() {
        if !target.supported {
            continue;
        }
        scanned_agents += 1;
        for root in &target.roots {
            let key = root.to_string_lossy().to_string();
            if !seen_paths.insert(key) {
                continue;
            }
            if !root.is_dir() {
                continue;
            }
            for entry in fs::read_dir(root).into_iter().flatten().flatten() {
                let path = entry.path();
                if !is_skill_dir(&path) {
                    continue;
                }
                let id_hint = entry.file_name().to_string_lossy().to_string();
                if id_hint.starts_with('.') || id_hint.starts_with("__octopus") {
                    continue;
                }
                found_entries += 1;

                // already in library by same dir name?
                if lib.join(&id_hint).exists() && is_skill_dir(&lib.join(&id_hint)) {
                    // ensure meta exists
                    if !meta_map.contains_key(&id_hint) {
                        let created = default_local_meta();
                        if let Ok(()) = db::upsert_skill_meta(&id_hint, &created) {
                            meta_map.insert(id_hint.clone(), created);
                        }
                    }
                    continue;
                }

                // match by frontmatter name against existing library
                let doc = read_skill_doc(&path);
                let already = meta_map.keys().any(|k| {
                    k == &id_hint || (!doc.name.is_empty() && k == &doc.name) || {
                        let p = lib.join(k);
                        let d = read_skill_doc(&p);
                        !doc.name.is_empty() && d.name == doc.name
                    }
                });
                if already {
                    continue;
                }

                match import_skill_dir(&path, Some(&id_hint), SkillSource::Local, None, "", None) {
                    Ok(rec) => {
                        imported += 1;
                        meta_map.insert(
                            rec.id.clone(),
                            SkillMetaEntry {
                                source: SkillSource::Local,
                                created_at: rec.created_at,
                                updated_at: rec.updated_at,
                                ..Default::default()
                            },
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "[skills] sniff import {} from {} failed: {}",
                            id_hint, target.agent, e
                        );
                    }
                }
            }
        }
    }

    let listed = list_skills()?;
    let message = if imported > 0 {
        format!(
            "嗅探完成：扫描 {} 个 Agent，发现 {} 个技能，新导入 {} 个",
            scanned_agents, found_entries, imported
        )
    } else {
        format!(
            "嗅探完成：扫描 {} 个 Agent，发现 {} 个技能，无新导入",
            scanned_agents, found_entries
        )
    };

    Ok(SkillSniffResult {
        skills: listed.skills,
        scanned_agents,
        found_entries,
        imported,
        message,
    })
}

/* ===== Check updates (github) ===== */

/// Stable content hash of a skill directory: sorted relative paths + raw file
/// bytes fed into SHA-256. Hidden entries (`.git`, `.DS_Store`, …) and
/// symlinks are skipped so clones and library dirs hash identically.
fn hash_skill_dir(dir: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut files: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    for path in &files {
        let rel = path
            .strip_prefix(dir)
            .map_err(|e| format!("路径计算失败: {}", e))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let bytes = fs::read(path).map_err(|e| format!("读取文件 {} 失败: {}", rel_str, e))?;
        hasher.update(rel_str.as_bytes());
        hasher.update(b"\0");
        hasher.update(&bytes);
        hasher.update(b"\0");
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Map `items` through `f` on at most `limit` scoped worker threads,
/// preserving input order. No extra deps needed for bounded concurrency.
fn map_bounded<T, R, F>(items: &[T], limit: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    use std::sync::Mutex;
    if items.is_empty() || limit == 0 {
        return Vec::new();
    }
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..limit.min(items.len()) {
            s.spawn(|| loop {
                let i = {
                    let mut g = next.lock().unwrap();
                    let i = *g;
                    *g += 1;
                    i
                };
                if i >= items.len() {
                    break;
                }
                let r = f(&items[i]);
                results.lock().unwrap().push((i, r));
            });
        }
    });
    let mut out = results.into_inner().unwrap();
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/* ===== Clone cache (content-addressed, shared by check & update) ===== */

fn clone_cache_root() -> PathBuf {
    std::env::temp_dir().join("octopus-skills")
}

fn clone_cache_dir(owner: &str, repo: &str, sha: &str) -> PathBuf {
    clone_cache_root().join(format!("{}-{}-{}", owner, repo, sha))
}

/// Drop superseded / stale clones: same owner-repo with a different sha is
/// obsolete immediately; anything else is cleaned once older than 24h.
fn cleanup_clone_cache(keep: Option<&str>) {
    let base = clone_cache_root();
    let Ok(rd) = fs::read_dir(&base) else {
        return;
    };
    let keep_prefix = keep
        .and_then(|k| k.rsplit_once('-'))
        .map(|(p, _)| p.to_string());
    let now = now_secs();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if Some(name.as_str()) == keep {
            continue;
        }
        let superseded = match (&keep_prefix, name.rsplit_once('-')) {
            (Some(p), Some((prefix, _))) => prefix == p,
            _ => false,
        };
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| now.saturating_sub(d.as_secs() as i64) > 24 * 3600)
            .unwrap_or(true);
        if superseded || stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Shallow-clone `repo_url` at its current HEAD `sha`, reusing the cache dir
/// when a valid clone for that exact commit already exists.
fn cached_clone(repo_url: &str, owner: &str, repo: &str, sha: &str) -> Result<PathBuf, String> {
    cleanup_clone_cache(Some(&format!("{}-{}-{}", owner, repo, sha)));
    let dest = clone_cache_dir(owner, repo, sha);
    let valid =
        dest.join(".git").exists() && git_rev_parse(&dest).map(|h| h == sha).unwrap_or(false);
    if valid {
        return Ok(dest);
    }
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    let clone_url = format!("{}.git", repo_url.trim_end_matches(".git"));
    if let Err(e) = git_clone(&clone_url, "", &dest) {
        let _ = fs::remove_dir_all(&dest);
        return Err(e);
    }
    Ok(dest)
}

/// HEAD 拉取失败（如 GitHub API 限流 403）时，回退到本地克隆缓存里该
/// owner/repo 最近一次有效克隆，做尽力而为的差异比对；无缓存则返回 None。
fn cached_clone_fallback(owner: &str, repo: &str) -> Option<PathBuf> {
    let root = clone_cache_root();
    let prefix = format!("{owner}-{repo}-");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let rd = fs::read_dir(&root).ok()?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let p = e.path();
        if !p.join(".git").exists() {
            continue;
        }
        let Ok(mt) = e.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        match &best {
            Some((bt, _)) if mt > *bt => best = Some((mt, p)),
            None => best = Some((mt, p)),
            _ => {}
        }
    }
    best.map(|(_, p)| p)
}

fn github_default_branch_sha(owner: &str, repo: &str) -> Result<String, String> {
    // Use GitHub API: GET /repos/{owner}/{repo}/commits/HEAD
    // Fallback: /repos/{owner}/{repo}
    let url = format!(
        "https://api.github.com/repos/{}/{}/commits/HEAD",
        owner, repo
    );
    let client = {
        let builder = reqwest::blocking::Client::builder()
            .user_agent("Octopus/0.1 (skills-update-check)")
            .timeout(std::time::Duration::from_secs(15));
        crate::http_client::apply_proxy(builder)?
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?
    };

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("请求 GitHub 失败: {}", e))?;

    if !resp.status().is_success() {
        // fallback repo info
        let repo_url = format!("https://api.github.com/repos/{}/{}", owner, repo);
        let resp2 = client
            .get(&repo_url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|e| format!("请求 GitHub 失败: {}", e))?;
        if !resp2.status().is_success() {
            return Err(format!("GitHub API 返回 {}", resp2.status()));
        }
        let v: serde_json::Value = resp2
            .json()
            .map_err(|e| format!("解析 GitHub 响应失败: {}", e))?;
        if let Some(sha) = v
            .pointer("/default_branch")
            .and_then(|b| b.as_str())
            .map(|branch| {
                let commits = format!(
                    "https://api.github.com/repos/{}/{}/commits/{}",
                    owner, repo, branch
                );
                client
                    .get(&commits)
                    .header("Accept", "application/vnd.github+json")
                    .send()
                    .ok()
                    .and_then(|r| r.json::<serde_json::Value>().ok())
                    .and_then(|j| j.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string()))
            })
            .flatten()
        {
            return Ok(sha);
        }
        return Err("无法获取远程提交信息".into());
    }

    let v: serde_json::Value = resp
        .json()
        .map_err(|e| format!("解析 GitHub 响应失败: {}", e))?;
    v.get("sha")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "GitHub 响应缺少 sha".into())
}

/// Resolve the HEAD commit of a repo's default branch via `git ls-remote`.
/// Host-agnostic and token-free — used for GitCode (and as GitHub fallback).
fn git_ls_remote_head(repo_url: &str) -> Result<String, String> {
    let clone_url = if repo_url.ends_with(".git") {
        repo_url.to_string()
    } else {
        format!("{}.git", repo_url)
    };
    let output = Command::new("git")
        .args(["ls-remote", &clone_url, "HEAD"])
        .output()
        .map_err(|e| format!("执行 git ls-remote 失败（请确认已安装 Git）: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-remote 失败: {}", err.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .map(|sha| sha.to_string())
        .filter(|sha| !sha.is_empty())
        .ok_or_else(|| "git ls-remote 未返回 HEAD".into())
}

/// Fetch the latest remote commit for a skill's repo, dispatching by source.
fn remote_head_for_source(
    source: &SkillSource,
    owner: &str,
    repo: &str,
    repo_url: &str,
) -> Result<String, String> {
    match source {
        SkillSource::Github => github_default_branch_sha(owner, repo),
        SkillSource::Gitcode => {
            let url = if repo_url.is_empty() {
                format!("https://gitcode.com/{}/{}", owner, repo)
            } else {
                repo_url.to_string()
            };
            git_ls_remote_head(&url)
        }
        SkillSource::Local => Err("本地技能无远程源".into()),
    }
}

fn host_domain_for(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::Github => GitHost::Github.domain(),
        SkillSource::Gitcode => GitHost::Gitcode.domain(),
        SkillSource::Local => "",
    }
}

pub fn check_skill_updates() -> Result<SkillUpdateCheckResult, String> {
    let listed = list_skills()?;
    let lib = skills_library_dir()?;
    let now = now_secs();

    #[derive(Clone)]
    struct Target {
        skill: SkillRecord,
        key: String,
    }
    struct RepoGroup {
        key: String,
        source: SkillSource,
        owner: String,
        repo: String,
        repo_url: String,
    }

    // Group remote skills by (source, owner, repo) to share one HEAD query
    // and one clone across all skills of the same repository.
    let mut groups: Vec<RepoGroup> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    let mut targets: Vec<Target> = Vec::new();
    let mut checked = 0usize;
    for skill in listed.skills {
        if !skill.source.is_remote()
            || skill.github_owner.is_empty()
            || skill.github_repo.is_empty()
        {
            continue;
        }
        checked += 1;
        let key = format!(
            "{:?}:{}/{}",
            skill.source, skill.github_owner, skill.github_repo
        );
        if !group_index.contains_key(&key) {
            group_index.insert(key.clone(), groups.len());
            groups.push(RepoGroup {
                key: key.clone(),
                source: skill.source.clone(),
                owner: skill.github_owner.clone(),
                repo: skill.github_repo.clone(),
                repo_url: skill.repo_url.clone(),
            });
        }
        targets.push(Target { skill, key });
    }

    // Stage 1 (lightweight, concurrent): fetch each repo's remote HEAD.
    let heads: Vec<Result<String, String>> = map_bounded(&groups, 6, |g| {
        remote_head_for_source(&g.source, &g.owner, &g.repo, &g.repo_url)
    });
    let mut head_map: HashMap<String, Result<String, String>> = HashMap::new();
    for (g, h) in groups.iter().zip(heads) {
        head_map.insert(g.key.clone(), h);
    }

    // Repos whose HEAD moved beyond at least one installed skill's baseline
    // need a stage-2 clone for content-hash comparison.
    let stale_keys: std::collections::HashSet<String> = targets
        .iter()
        .filter(|t| {
            let meta_local = db::get_skill_meta(&t.skill.id)
                .ok()
                .flatten()
                .map(|m| m.local_ref)
                .unwrap_or_default();
            matches!(head_map.get(&t.key), Some(Ok(head)) if !head.is_empty() && *head != meta_local)
        })
        .map(|t| t.key.clone())
        .collect();
    let to_clone: Vec<&RepoGroup> = groups
        .iter()
        .filter(|g| stale_keys.contains(&g.key))
        .collect();

    // Stage 2 (bounded clones): shallow-clone each stale repo once into the
    // content-addressed cache (reused later by updates).
    let clone_results: Vec<Result<PathBuf, String>> = map_bounded(&to_clone, 2, |g| {
        let head = match head_map.get(&g.key) {
            Some(Ok(h)) => h.clone(),
            _ => return Err("缺少远端 HEAD".into()),
        };
        let url = if g.repo_url.is_empty() {
            format!(
                "https://{}/{}/{}",
                host_domain_for(&g.source),
                g.owner,
                g.repo
            )
        } else {
            g.repo_url.clone()
        };
        cached_clone(&url, &g.owner, &g.repo, &head)
    });
    let mut clone_map: HashMap<String, Result<PathBuf, String>> = HashMap::new();
    for (g, r) in to_clone.iter().zip(clone_results) {
        clone_map.insert(g.key.clone(), r);
    }

    // Compare per-skill content hashes against the clone.
    let mut updates = 0usize;
    let mut update_ids: BTreeSet<String> = BTreeSet::new();
    let mut missing = 0usize;
    for t in &targets {
        let head = match head_map.get(&t.key) {
            Some(Ok(h)) if !h.is_empty() => h.clone(),
            _ => continue,
        };
        let mut entry = match db::get_skill_meta(&t.skill.id)? {
            Some(e) => e,
            None => continue,
        };
        entry.remote_ref = head.clone();
        entry.updated_at = now;
        if entry.local_ref == head {
            // Fast path: repo hasn't moved past this skill's baseline.
            db::upsert_skill_meta(&t.skill.id, &entry)?;
            continue;
        }
        let Some(Ok(clone_dir)) = clone_map.get(&t.key) else {
            // Clone unavailable: keep refs fresh but don't flag an update.
            db::upsert_skill_meta(&t.skill.id, &entry)?;
            continue;
        };
        let search_root = if t.skill.github_path.is_empty() {
            clone_dir.clone()
        } else {
            clone_dir.join(&t.skill.github_path)
        };
        let local_dir = lib.join(&t.skill.id);
        let local_doc = read_skill_doc(&local_dir);
        let local_identity = skill_identity(&t.skill.id, &local_doc);
        let remote_dir = if is_skill_dir(&search_root) {
            Some(search_root)
        } else {
            find_skill_dirs(&search_root).into_iter().next()
        }
        // 旧路径失效（仓库重排/搬家）时按身份整仓重定位，回写新路径
        .or_else(|| {
            relocate_skill_in_clone(clone_dir, &local_identity, &t.skill.id).map(|(d, rel)| {
                entry.github_path = rel;
                d
            })
        });
        let Some(remote_dir) = remote_dir else {
            missing += 1;
            db::upsert_skill_meta(&t.skill.id, &entry)?;
            continue;
        };
        let changed = match (
            hash_skill_dir(&remote_dir),
            hash_skill_dir(&lib.join(&t.skill.id)),
        ) {
            (Ok(remote_hash), Ok(local_hash)) => remote_hash != local_hash,
            _ => true, // unreadable: let the user decide via an update
        };
        if changed {
            updates += 1;
            update_ids.insert(t.skill.id.clone());
        } else {
            // Unrelated commit elsewhere in the repo: realign baseline so the
            // next check takes the cheap stage-1 path again.
            entry.local_ref = head;
        }
        db::upsert_skill_meta(&t.skill.id, &entry)?;
    }

    let mut skills = list_skills()?.skills;
    for s in &mut skills {
        if update_ids.contains(&s.id) {
            s.update_available = true;
        }
    }

    let message = if checked == 0 {
        "没有来自 GitHub / GitCode 的技能可检查".into()
    } else if updates > 0 {
        format!("已检查 {} 个远程技能，其中 {} 个有更新", checked, updates)
    } else {
        format!("已检查 {} 个远程技能，全部为最新", checked)
    };
    let message = if missing > 0 {
        format!("{}（{} 个技能在远端仓库中已不存在）", message, missing)
    } else {
        message
    };

    Ok(SkillUpdateCheckResult {
        skills,
        checked,
        updates,
        message,
    })
}

/* ===== Repo drift scan: upstream added / upstream removed ===== */

fn host_label_of(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::Github => "GitHub",
        SkillSource::Gitcode => "GitCode",
        SkillSource::Local => "本地",
    }
}

/// 枚举某技能在各 Agent skills 目录下的落盘副本（软链接 / 真实目录）。
/// `managed_names` 为库 id 与 frontmatter 名（可能不同），两者都查。
fn installations_for_names(managed_names: &[String]) -> Vec<RepoChangeInstall> {
    let mut out = Vec::new();
    for target in agent_skills_targets() {
        if !target.supported {
            continue;
        }
        for root in &target.roots {
            if !root.is_dir() {
                continue;
            }
            for name in managed_names {
                if !is_plain_entry_name(name) {
                    continue;
                }
                let p = root.join(name);
                match fs::symlink_metadata(&p) {
                    Ok(md) => out.push(RepoChangeInstall {
                        agent: target.agent.to_string(),
                        path: p.to_string_lossy().to_string(),
                        link: md.file_type().is_symlink(),
                    }),
                    Err(_) => {}
                }
            }
        }
    }
    out
}

/// 扫描所有已跟踪远程仓库：克隆最新内容，比对技能库，返回
/// 「远端新增候选」与「远端已移除」两组差异（不写盘，供前端勾选）。
pub fn scan_skill_repo_changes() -> Result<SkillRepoChangesResult, String> {
    let listed = list_skills()?;
    let lib = skills_library_dir()?;
    let existing = existing_library_identities(&lib);
    let applied_map = scan_applied_agents();

    #[derive(Clone)]
    struct TrackedGroup {
        source: SkillSource,
        owner: String,
        repo: String,
        repo_url: String,
        members: Vec<SkillRecord>,
    }

    // 按 (source, owner, repo) 归组库内远程技能；只有已导入过技能的仓库才跟踪。
    let mut groups: Vec<TrackedGroup> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    for skill in listed.skills {
        if !skill.source.is_remote()
            || skill.github_owner.is_empty()
            || skill.github_repo.is_empty()
        {
            continue;
        }
        let key = format!(
            "{:?}:{}/{}",
            skill.source, skill.github_owner, skill.github_repo
        );
        match group_index.get(&key) {
            Some(gi) => groups[*gi].members.push(skill),
            None => {
                group_index.insert(key.clone(), groups.len());
                groups.push(TrackedGroup {
                    source: skill.source.clone(),
                    owner: skill.github_owner.clone(),
                    repo: skill.github_repo.clone(),
                    repo_url: skill.repo_url.clone(),
                    members: vec![skill],
                });
            }
        }
    }

    // 并发克隆（上限 2，与检查更新一致；命中内容寻址缓存则直接复用）。
    // HEAD 拉取失败（如 API 限流）时回退本地缓存克隆，避免整仓被跳过。
    let cloned: Vec<(TrackedGroup, Result<PathBuf, String>)> = map_bounded(&groups, 2, |g| {
        let r = (|| -> Result<PathBuf, String> {
            let base_url = repo_base_url(&g.source, &g.repo_url, &g.owner, &g.repo);
            match remote_head_for_source(&g.source, &g.owner, &g.repo, &g.repo_url) {
                Ok(head) if !head.is_empty() => cached_clone(&base_url, &g.owner, &g.repo, &head),
                _ => cached_clone_fallback(&g.owner, &g.repo)
                    .ok_or_else(|| "无法获取远端 HEAD，且无本地缓存可比对".to_string()),
            }
        })();
        (g.clone(), r)
    });

    let mut result_groups: Vec<RepoChangeGroup> = Vec::new();
    let mut failed_repos: Vec<String> = Vec::new();
    let mut checked_repos = 0usize;
    let mut added_count = 0usize;
    let mut removed_count = 0usize;

    for (g, clone) in cloned {
        let clone_dir = match clone {
            Ok(d) => d,
            Err(e) => {
                failed_repos.push(format!("{}/{}：{}", g.owner, g.repo, e));
                continue;
            }
        };
        checked_repos += 1;

        // 远端技能目录集合：身份键 + 目录名都纳入匹配。
        let remote_dirs = find_skill_dirs(&clone_dir);
        let mut remote_identities: BTreeSet<String> = BTreeSet::new();
        let mut remote_added: Vec<RepoChangeAdded> = Vec::new();
        for dir in &remote_dirs {
            let doc = read_skill_doc(dir);
            let dir_name = dir
                .file_name()
                .map(|s| s.to_string_lossy().trim().to_lowercase())
                .unwrap_or_default();
            let identity = skill_identity(&dir_name, &doc);
            remote_identities.insert(identity.clone());
            remote_identities.insert(dir_name.clone());

            let globally_known = existing.contains(&identity) || existing.contains(&dir_name);
            if globally_known {
                continue;
            }
            let repo_path = dir
                .strip_prefix(&clone_dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let title = if !doc.name.is_empty() {
                doc.name.clone()
            } else {
                dir_name.clone()
            };
            remote_added.push(RepoChangeAdded {
                key: if !identity.is_empty() {
                    identity
                } else {
                    dir_name.clone()
                },
                title,
                description: doc.description,
                version: doc.version,
                repo_path,
                source: g.source.clone(),
                owner: g.owner.clone(),
                repo: g.repo.clone(),
                repo_url: repo_base_url(&g.source, &g.repo_url, &g.owner, &g.repo),
                host_label: host_label_of(&g.source).to_string(),
                exists_in_library: false,
            });
        }

        // 已移除：库内成员的身份 / 目录名都不在远端集合中。
        let mut removed: Vec<RepoChangeRemoved> = Vec::new();
        for member in &g.members {
            let local_dir = lib.join(&member.id);
            let local_doc = read_skill_doc(&local_dir);
            let local_identity = skill_identity(&member.id, &local_doc);
            let local_id = member.id.trim().to_lowercase();
            let still_present = remote_identities.contains(&local_identity)
                || remote_identities.contains(&local_id);
            if still_present {
                continue;
            }

            let mut applied: Vec<String> = applied_map
                .get(&member.id)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            if !local_doc.name.is_empty() && local_doc.name != member.id {
                if let Some(extra) = applied_map.get(&local_doc.name) {
                    for a in extra {
                        if !applied.contains(a) {
                            applied.push(a.clone());
                        }
                    }
                }
            }
            applied.sort_by_key(|a| crate::agents::display_order(a));

            let managed = managed_entry_names(&member.id, &local_dir);
            let installations = installations_for_names(&managed);

            removed.push(RepoChangeRemoved {
                skill_id: member.id.clone(),
                title: member.title.clone(),
                description: member.description.clone(),
                version: member.version.clone(),
                source: member.source.clone(),
                owner: member.github_owner.clone(),
                repo: member.github_repo.clone(),
                repo_url: member.repo_url.clone(),
                host_label: host_label_of(&member.source).to_string(),
                local_path: member.local_path.clone(),
                applied_agents: applied,
                installations,
            });
        }

        if !remote_added.is_empty() || !removed.is_empty() {
            added_count += remote_added.len();
            removed_count += removed.len();
            result_groups.push(RepoChangeGroup {
                source: g.source.clone(),
                owner: g.owner.clone(),
                repo: g.repo.clone(),
                repo_url: repo_base_url(&g.source, &g.repo_url, &g.owner, &g.repo),
                host_label: host_label_of(&g.source).to_string(),
                added: remote_added,
                removed,
            });
        }
    }

    let message = if checked_repos == 0 {
        "没有可扫描的远程仓库".into()
    } else if added_count == 0 && removed_count == 0 {
        format!("已检查 {} 个远程仓库，无新增或已移除技能", checked_repos)
    } else {
        format!(
            "发现 {} 个远端新增、{} 个远端已移除技能",
            added_count, removed_count
        )
    };

    Ok(SkillRepoChangesResult {
        ok: failed_repos.is_empty(),
        groups: result_groups,
        added_count,
        removed_count,
        checked_repos,
        failed_repos,
        message,
    })
}

/// 前端勾选的一个远端新增候选（重新克隆定位后导入）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCandidateInput {
    pub source: SkillSource,
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub repo_url: String,
    /// 技能目录在仓库内的相对路径。
    pub path: String,
}

/// 把勾选的远端新增候选导入技能库：按仓库分组共享一次克隆，逐项导入并对齐 refs。
pub fn import_repo_candidates(
    selections: Vec<RepoCandidateInput>,
    tag: String,
) -> Result<BatchSkillResult, String> {
    if selections.is_empty() {
        let list = list_skills()?;
        return Ok(BatchSkillResult {
            missing_upstream: Vec::new(),
            ok: false,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            skills: list.skills,
            message: "未选择要导入的技能".into(),
            errors: vec![],
        });
    }

    // 按 (source, owner, repo) 归组，同仓库共享一次克隆。
    #[derive(Clone)]
    struct Pick {
        source: SkillSource,
        owner: String,
        repo: String,
        repo_url: String,
        path: String,
    }
    let mut groups: Vec<Vec<Pick>> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    for sel in selections {
        if !sel.source.is_remote() || sel.owner.is_empty() || sel.repo.is_empty() {
            continue;
        }
        let key = format!("{:?}:{}/{}", sel.source, sel.owner, sel.repo);
        let pick = Pick {
            source: sel.source,
            owner: sel.owner,
            repo: sel.repo,
            repo_url: sel.repo_url,
            path: sel.path,
        };
        match group_index.get(&key) {
            Some(gi) => groups[*gi].push(pick),
            None => {
                group_index.insert(key.clone(), groups.len());
                groups.push(vec![pick]);
            }
        }
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for picks in &groups {
        let first = &picks[0];
        let clone = (|| -> Result<PathBuf, String> {
            let base_url = repo_base_url(&first.source, &first.repo_url, &first.owner, &first.repo);
            match remote_head_for_source(&first.source, &first.owner, &first.repo, &first.repo_url)
            {
                Ok(head) if !head.is_empty() => {
                    cached_clone(&base_url, &first.owner, &first.repo, &head)
                }
                _ => cached_clone_fallback(&first.owner, &first.repo)
                    .ok_or_else(|| "无法获取远端 HEAD，且无本地缓存可比对".to_string()),
            }
        })();
        let clone_dir = match clone {
            Ok(d) => d,
            Err(e) => {
                for p in picks {
                    failed += 1;
                    errors.push(format!("{}/{}{}: {}", p.owner, p.repo, p.path, e));
                }
                continue;
            }
        };
        let head = git_rev_parse(&clone_dir).ok();

        for pick in picks {
            let src = clone_dir.join(&pick.path);
            if !is_skill_dir(&src) {
                failed += 1;
                errors.push(format!(
                    "{}/{}：仓库路径 {} 不是有效技能目录",
                    pick.owner, pick.repo, pick.path
                ));
                continue;
            }
            let host = match pick.source {
                SkillSource::Github => GitHost::Github,
                SkillSource::Gitcode => GitHost::Gitcode,
                SkillSource::Local => {
                    failed += 1;
                    errors.push("仅支持 GitHub / GitCode 候选".into());
                    continue;
                }
            };
            let gh = GitRepoRef {
                host,
                owner: pick.owner.clone(),
                repo: pick.repo.clone(),
                path: pick.path.clone(),
                repo_url: repo_base_url(&pick.source, &pick.repo_url, &pick.owner, &pick.repo),
                branch: String::new(),
            };
            match import_skill_dir(&src, None, pick.source.clone(), Some(gh), &tag, None) {
                Ok(rec) => {
                    if let Some(h) = head.as_deref().filter(|h| !h.is_empty()) {
                        let _ = db::update_skill_refs(&rec.id, Some(h), Some(h), now_secs());
                    }
                    succeeded += 1;
                }
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{}：{}", pick.path, e));
                }
            }
        }
    }

    let list = list_skills()?;
    let message = if failed == 0 {
        format!("已导入 {} 个远端新增技能", succeeded)
    } else {
        format!("导入结束：成功 {}，失败 {}", succeeded, failed)
    };

    Ok(BatchSkillResult {
        missing_upstream: Vec::new(),
        ok: failed == 0,
        succeeded,
        failed,
        skipped: 0,
        skills: list.skills,
        message,
        errors,
    })
}

/// Open an external URL in the system default browser.
/// Only http/https URLs are allowed to avoid handing arbitrary schemes/args to the shell.
pub fn open_external_url(url: String) -> Result<(), String> {
    crate::platform::open_url(&url)
}

/* ===== Update: pull a remote skill to the repo's latest ===== */

/// 在克隆内定位技能目录：优先 github_path，其次浅搜第一个 skill 目录。
fn locate_skill_in_clone(clone_root: &Path, github_path: &str) -> Option<PathBuf> {
    let search_root = if github_path.is_empty() {
        clone_root.to_path_buf()
    } else {
        clone_root.join(github_path)
    };
    if is_skill_dir(&search_root) {
        Some(search_root)
    } else {
        find_skill_dirs(&search_root).into_iter().next()
    }
}

/// 已记录路径失效（仓库重排 / 技能搬家）时，按本地身份（frontmatter name，
/// 否则目录名）在整仓全局找回，返回 (克隆内目录, 相对克隆根的路径)。
fn relocate_skill_in_clone(
    clone_root: &Path,
    local_identity: &str,
    local_dir_name: &str,
) -> Option<(PathBuf, String)> {
    let candidates = find_skill_dirs(clone_root);
    let by_identity = |d: &Path| -> bool {
        if local_identity.is_empty() {
            return false;
        }
        let doc = read_skill_doc(d);
        let remote_ident = skill_identity(
            d.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
                .as_ref(),
            &doc,
        );
        remote_ident == local_identity
    };
    let found = candidates.iter().find(|d| by_identity(d)).or_else(|| {
        let want = local_dir_name.trim().to_lowercase();
        candidates.iter().find(|d| {
            d.file_name()
                .map(|n| n.to_string_lossy().trim().to_lowercase() == want)
                .unwrap_or(false)
        })
    })?;
    let rel = found
        .strip_prefix(clone_root)
        .unwrap_or(found.as_path())
        .to_string_lossy()
        .replace('\\', "/");
    Some((found.clone(), rel))
}

/// 在暂存目录构建新内容后原子替换库内技能目录，返回新内容的 hash。
/// 「先复制、成功后再替换」避免中途失败清空既有技能。
fn replace_skill_from_clone(lib: &Path, id: &str, src_dir: &Path) -> Result<String, String> {
    let dest = lib.join(id);
    let staging = lib.join(format!(".{}.update-{}", id, std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    if let Err(e) = copy_dir_recursive(src_dir, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = fs::remove_dir_all(&dest) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("清理旧技能目录失败: {}", e));
    }
    if let Err(e) = fs::rename(&staging, &dest) {
        return Err(format!(
            "替换技能目录失败: {}（新内容暂存于 {}，可手动恢复）",
            e,
            staging.display()
        ));
    }
    hash_skill_dir(&dest)
}

/// 更新后对齐 ref 到克隆 HEAD 并写入新内容 hash（拿不到 HEAD 不致命）。
fn finalize_skill_update(id: &str, clone_root: &Path, new_hash: String) {
    let ts = now_secs();
    let head = git_rev_parse(clone_root).ok().filter(|h| !h.is_empty());
    match &head {
        Some(h) => {
            let _ = db::update_skill_refs(id, Some(h), Some(h), ts);
        }
        None => {
            let _ = db::update_skill_refs(id, None, None, ts);
        }
    }
    if !new_hash.is_empty() {
        if let Ok(Some(mut entry)) = db::get_skill_meta(id) {
            entry.content_hash = new_hash;
            let _ = db::upsert_skill_meta(id, &entry);
        }
    }
}

/// 计算仓库的基础 URL（元数据缺失 repo_url 时按平台域名拼接）。
fn repo_base_url(source: &SkillSource, repo_url: &str, owner: &str, repo: &str) -> String {
    if repo_url.is_empty() {
        format!("https://{}/{}/{}", host_domain_for(source), owner, repo)
    } else {
        repo_url.to_string()
    }
}

/// 把一个来自 GitHub / GitCode 的技能更新到远端仓库最新内容：
/// 浅克隆（优先复用内容寻址缓存）→ 定位子目录 → 暂存目录构建新内容 →
/// 原子替换库目录 → 对齐 `local_ref` / `content_hash` 到远端 HEAD。
/// 本地技能不支持更新。
pub fn update_skill(skill_id: String) -> Result<SkillActionResult, String> {
    let id = skill_id.trim().to_string();
    // Guard against path traversal: id must be a plain directory name.
    if !is_plain_entry_name(&id) {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "非法的技能标识".into(),
        });
    }
    let lib = skills_library_dir()?;
    let dest = lib.join(&id);
    if !dest.starts_with(&lib) || !is_skill_dir(&dest) {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "技能不存在或缺少 SKILL.md".into(),
        });
    }
    let meta = db::get_skill_meta(&id)?.ok_or_else(|| format!("技能元数据缺失: {}", id))?;
    if !meta.source.is_remote() {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "仅支持更新来自 GitHub / GitCode 的技能".into(),
        });
    }
    if meta.github_owner.is_empty() || meta.github_repo.is_empty() {
        return Ok(SkillActionResult {
            errors: Vec::new(),
            ok: false,
            skill: None,
            message: "缺少仓库信息，无法更新".into(),
        });
    }

    let repo_url = repo_base_url(
        &meta.source,
        &meta.repo_url,
        &meta.github_owner,
        &meta.github_repo,
    );
    let clone_url = format!("{}.git", repo_url.trim_end_matches(".git"));

    // 优先内容寻址克隆缓存（与检查更新共享）；解析不到 HEAD 时退回一次性临时克隆。
    let (tmp, cached) = match remote_head_for_source(
        &meta.source,
        &meta.github_owner,
        &meta.github_repo,
        &repo_url,
    ) {
        Ok(head) if !head.is_empty() => (
            cached_clone(&repo_url, &meta.github_owner, &meta.github_repo, &head)?,
            true,
        ),
        _ => {
            let tmp = temp_clone_dir(&meta.github_owner, &meta.github_repo)?;
            if let Err(e) = git_clone(&clone_url, "", &tmp) {
                let _ = fs::remove_dir_all(&tmp);
                return Err(e);
            }
            (tmp, false)
        }
    };
    // 缓存目录保留供后续复用；仅临时克隆用后清理。
    let cleanup_tmp = |tmp: &Path| {
        if !cached {
            let _ = fs::remove_dir_all(tmp);
        }
    };

    let local_doc = read_skill_doc(&dest);
    let local_identity = skill_identity(&id, &local_doc);
    let src_dir = match locate_skill_in_clone(&tmp, &meta.github_path) {
        Some(d) => d,
        None => match relocate_skill_in_clone(&tmp, &local_identity, &id) {
            Some((d, rel)) => {
                // 仓库重排导致搬家：回写新的 github_path，下次直接命中
                let mut updated = meta.clone();
                updated.github_path = rel;
                let _ = db::upsert_skill_meta(&id, &updated);
                d
            }
            None => {
                cleanup_tmp(&tmp);
                return Ok(SkillActionResult {
                    errors: Vec::new(),
                    ok: false,
                    skill: None,
                    message: "远端仓库中未找到该技能目录（可能已被移动或删除）".into(),
                });
            }
        },
    };

    let new_hash = match replace_skill_from_clone(&lib, &id, &src_dir) {
        Ok(h) => h,
        Err(e) => {
            cleanup_tmp(&tmp);
            return Err(e);
        }
    };
    finalize_skill_update(&id, &tmp, new_hash);
    cleanup_tmp(&tmp);

    let meta2 = db::get_skill_meta(&id)?.unwrap_or(meta);
    let applied = applied_after_for(&id, &dest);
    let record = build_record(&id, &dest, &meta2, &applied, false);
    let title = record.title.clone();
    Ok(SkillActionResult {
        errors: Vec::new(),
        ok: true,
        skill: Some(record),
        message: format!("已将「{}」更新到远端最新", title),
    })
}

/// 批量把多个远程技能更新到远端最新：按 (source, owner, repo) 分组，
/// 每个仓库只克隆一次（组间并发上限 2，复用检查更新留下的缓存），
/// 组内逐个替换技能目录；单个技能失败不影响其余。
pub fn update_skills_batch(ids: Vec<String>) -> Result<BatchSkillResult, String> {
    let lib = skills_library_dir()?;

    struct Item {
        id: String,
        title: String,
        meta: SkillMetaEntry,
        repo_url: String,
    }

    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<Item>> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();

    for raw in ids {
        let id = raw.trim().to_string();
        let title_of = |lib: &Path, id: &str| {
            let name = read_skill_doc(&lib.join(id)).name;
            if name.trim().is_empty() {
                id.to_string()
            } else {
                name
            }
        };
        let invalid = |msg: &str| -> Result<Item, String> {
            Err(format!("{}: {}", title_of(&lib, &id), msg))
        };
        let item = (|| -> Result<Item, String> {
            if !is_plain_entry_name(&id) {
                return invalid("非法的技能标识");
            }
            let dir = lib.join(&id);
            if !is_skill_dir(&dir) {
                return invalid("技能不存在或缺少 SKILL.md");
            }
            let meta = db::get_skill_meta(&id)?
                .ok_or_else(|| format!("{}: 技能元数据缺失", title_of(&lib, &id)))?;
            if !meta.source.is_remote() {
                return invalid("仅支持更新来自 GitHub / GitCode 的技能");
            }
            if meta.github_owner.is_empty() || meta.github_repo.is_empty() {
                return invalid("缺少仓库信息，无法更新");
            }
            Ok(Item {
                repo_url: repo_base_url(
                    &meta.source,
                    &meta.repo_url,
                    &meta.github_owner,
                    &meta.github_repo,
                ),
                id: id.clone(),
                title: title_of(&lib, &id),
                meta,
            })
        })();
        match item {
            Ok(it) => {
                let key = format!(
                    "{:?}:{}/{}",
                    it.meta.source, it.meta.github_owner, it.meta.github_repo
                );
                match group_index.get(&key) {
                    Some(gi) => groups[*gi].push(it),
                    None => {
                        group_index.insert(key, groups.len());
                        groups.push(vec![it]);
                    }
                }
            }
            Err(e) => {
                failed += 1;
                errors.push(e);
            }
        }
    }

    // 每组：解析 HEAD → 取/建克隆缓存 → 组内逐个替换。
    // 克隆成功但仓库内定位不到的技能判定为「远端已移除」，单独结构化回传供弹窗勾选删除。
    let applied_map = scan_applied_agents();
    let group_results: Vec<
        Vec<(
            String,
            String,
            Result<(), String>,
            Option<RepoChangeRemoved>,
        )>,
    > = map_bounded(&groups, 2, |items| {
        let first = &items[0];
        let owner = &first.meta.github_owner;
        let repo = &first.meta.github_repo;
        let clone = remote_head_for_source(&first.meta.source, owner, repo, &first.repo_url)
            .and_then(|head| {
                if head.is_empty() {
                    return Err("无法获取远端 HEAD".into());
                }
                cached_clone(&first.repo_url, owner, repo, &head)
            });
        items
            .iter()
            .map(|item| {
                let (outcome, missing) = match &clone {
                    Err(e) => (Err(e.clone()), None),
                    Ok(clone_dir) => {
                        let local_dir = lib.join(&item.id);
                        let local_doc = read_skill_doc(&local_dir);
                        let local_identity = skill_identity(&item.id, &local_doc);
                        // 已记录路径优先；失效则按身份整仓重定位（仓库重排/搬家），并回写新路径
                        let src = locate_skill_in_clone(clone_dir, &item.meta.github_path).or_else(
                            || {
                                relocate_skill_in_clone(clone_dir, &local_identity, &item.id).map(
                                    |(d, rel)| {
                                        let mut updated = item.meta.clone();
                                        updated.github_path = rel;
                                        let _ = db::upsert_skill_meta(&item.id, &updated);
                                        d
                                    },
                                )
                            },
                        );
                        match src {
                            Some(src) => {
                                let r = (|| -> Result<(), String> {
                                    let new_hash = replace_skill_from_clone(&lib, &item.id, &src)?;
                                    finalize_skill_update(&item.id, clone_dir, new_hash);
                                    Ok(())
                                })();
                                (r, None)
                            }
                            None => {
                                let mut applied: Vec<String> = applied_map
                                    .get(&item.id)
                                    .map(|s| s.iter().cloned().collect())
                                    .unwrap_or_default();
                                if !local_doc.name.is_empty() && local_doc.name != item.id {
                                    if let Some(extra) = applied_map.get(&local_doc.name) {
                                        for a in extra {
                                            if !applied.contains(a) {
                                                applied.push(a.clone());
                                            }
                                        }
                                    }
                                }
                                applied.sort_by_key(|a| crate::agents::display_order(a));
                                let managed = managed_entry_names(&item.id, &local_dir);
                                let installations = installations_for_names(&managed);
                                let rec = RepoChangeRemoved {
                                    skill_id: item.id.clone(),
                                    title: item.title.clone(),
                                    description: local_doc.description,
                                    version: local_doc.version,
                                    source: item.meta.source.clone(),
                                    owner: item.meta.github_owner.clone(),
                                    repo: item.meta.github_repo.clone(),
                                    repo_url: repo_base_url(
                                        &item.meta.source,
                                        &item.meta.repo_url,
                                        &item.meta.github_owner,
                                        &item.meta.github_repo,
                                    ),
                                    host_label: host_label_of(&item.meta.source).to_string(),
                                    local_path: local_dir.to_string_lossy().replace('\\', "/"),
                                    applied_agents: applied,
                                    installations,
                                };
                                (
                                    Err("远端仓库中未找到该技能目录（可能已被移动或删除）"
                                        .to_string()),
                                    Some(rec),
                                )
                            }
                        }
                    }
                };
                (item.id.clone(), item.title.clone(), outcome, missing)
            })
            .collect()
    });

    let mut succeeded = 0usize;
    let mut missing_upstream: Vec<RepoChangeRemoved> = Vec::new();
    for results in group_results {
        for (_id, title, r, missing) in results {
            match r {
                Ok(()) => succeeded += 1,
                Err(e) => {
                    failed += 1;
                    match missing {
                        Some(rec) => missing_upstream.push(rec),
                        None => errors.push(format!("{}: {}", title, e)),
                    }
                }
            }
        }
    }

    let list = list_skills()?;
    let message = if succeeded == 0 && failed == 0 {
        "没有可更新的远程技能".to_string()
    } else if failed == 0 {
        format!("已更新 {} 个技能到远端最新", succeeded)
    } else {
        format!("批量更新结束：成功 {}，失败 {}", succeeded, failed)
    };

    Ok(BatchSkillResult {
        ok: failed == 0,
        succeeded,
        failed,
        skipped: 0,
        skills: list.skills,
        message,
        errors,
        missing_upstream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter() {
        let md = r#"---
name: demo-skill
description: A short demo skill for tests.
---

# Demo

body
"#;
        let doc = parse_skill_md(md);
        assert_eq!(doc.name, "demo-skill");
        assert_eq!(doc.description, "A short demo skill for tests.");
    }

    #[test]
    fn parse_github_simple() {
        let g = parse_git_url("https://github.com/foo/bar", GitHost::Github).unwrap();
        assert_eq!(g.owner, "foo");
        assert_eq!(g.repo, "bar");
        assert!(g.path.is_empty());
        assert_eq!(g.repo_url, "https://github.com/foo/bar");
    }

    #[test]
    fn parse_github_tree() {
        let g = parse_git_url(
            "https://github.com/foo/bar/tree/main/skills/baz",
            GitHost::Github,
        )
        .unwrap();
        assert_eq!(g.owner, "foo");
        assert_eq!(g.repo, "bar");
        assert_eq!(g.branch, "main");
        assert_eq!(g.path, "skills/baz");
    }

    #[test]
    fn hash_skill_dir_stable_and_ignores_hidden() {
        let base = std::env::temp_dir().join(format!(
            "octopus_hash_test_{}_{}",
            now_secs(),
            std::process::id()
        ));
        let write = |root: &Path, files: &[(&str, &str)]| {
            for (rel, content) in files {
                let p = root.join(rel);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, content).unwrap();
            }
        };
        let a = base.join("a");
        let b = base.join("b");
        write(&a, &[("SKILL.md", "# demo"), ("scripts/run.sh", "echo hi")]);
        // Same content plus hidden noise (.git / .DS_Store) must hash equal.
        write(
            &b,
            &[
                ("SKILL.md", "# demo"),
                ("scripts/run.sh", "echo hi"),
                (".git/HEAD", "ref: refs/heads/main"),
                (".DS_Store", "junk"),
            ],
        );
        let ha = hash_skill_dir(&a).unwrap();
        let hb = hash_skill_dir(&b).unwrap();
        assert_eq!(ha, hb);
        assert!(!ha.is_empty());

        // A content change must change the hash.
        fs::write(a.join("SKILL.md"), "# demo v2").unwrap();
        let ha2 = hash_skill_dir(&a).unwrap();
        assert_ne!(ha, ha2);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn parse_owner_repo() {
        let g = parse_git_url("anthropics/skills", GitHost::Github).unwrap();
        assert_eq!(g.owner, "anthropics");
        assert_eq!(g.repo, "skills");
    }

    #[test]
    fn parse_gitcode_full_url() {
        let g = parse_git_url(
            "https://gitcode.com/HarmonyOS_Skills/harmonyos-agent-skills",
            GitHost::Gitcode,
        )
        .unwrap();
        assert_eq!(g.host, GitHost::Gitcode);
        assert_eq!(g.owner, "HarmonyOS_Skills");
        assert_eq!(g.repo, "harmonyos-agent-skills");
        assert_eq!(
            g.repo_url,
            "https://gitcode.com/HarmonyOS_Skills/harmonyos-agent-skills"
        );
        assert_eq!(g.host.source(), SkillSource::Gitcode);
    }

    #[test]
    fn parse_gitcode_tree_and_shorthand() {
        let g = parse_git_url(
            "gitcode.com/owner/repo/tree/master/07-tools/foo",
            GitHost::Gitcode,
        )
        .unwrap();
        assert_eq!(g.owner, "owner");
        assert_eq!(g.repo, "repo");
        assert_eq!(g.branch, "master");
        assert_eq!(g.path, "07-tools/foo");

        let g2 = parse_git_url("owner/repo", GitHost::Gitcode).unwrap();
        assert_eq!(g2.owner, "owner");
        assert_eq!(g2.repo, "repo");
        assert_eq!(g2.repo_url, "https://gitcode.com/owner/repo");
    }

    #[test]
    fn parse_git_url_rejects_wrong_host() {
        // A github URL must not parse under the GitCode host.
        assert!(parse_git_url("https://github.com/foo/bar", GitHost::Gitcode).is_err());
    }

    #[test]
    fn parse_block_scalar_description() {
        let md = "---\nname: hmos-arkui\ndescription: |\n  第一段说明文本。\n\n  触发场景：xyz\n---\n\n# Body\n";
        let doc = parse_skill_md(md);
        assert_eq!(doc.name, "hmos-arkui");
        assert_eq!(doc.description, "第一段说明文本。");
    }

    /// Offline structural check against a local clone of the HarmonyOS GitCode
    /// repo (if present at /tmp/gitcode-probe). Validates that the shallow BFS
    /// finds deeply-nested standalone skills and does NOT split out the
    /// `references/*/SKILL.md` sub-skills bundled inside a parent skill.
    #[test]
    fn gitcode_find_skill_dirs_layout_if_present() {
        let root = PathBuf::from("/tmp/gitcode-probe");
        if !root.exists() {
            eprintln!("no local gitcode clone; skip layout smoke");
            return;
        }
        let found = find_skill_dirs(&root);
        // A deeply-nested standalone skill must be discovered.
        assert!(
            found.iter().any(|p| p.ends_with(
                "04-development/01-application-framework/ArkUI/hmos-arkui-develop-skill"
            )),
            "expected to find nested ArkUI skill; got {} dirs",
            found.len()
        );
        // Sub-skills under a parent skill's references/ must NOT be surfaced
        // as separate top-level skills.
        assert!(
            !found.iter().any(|p| p
                .to_string_lossy()
                .contains("deveco-native-flow/references")),
            "references/* sub-skills should stay bundled in parent skill"
        );

        // Block-scalar `description: |` must summarize to its first paragraph.
        let arkui =
            root.join("04-development/01-application-framework/ArkUI/hmos-arkui-develop-skill");
        let doc = read_skill_doc(&arkui);
        assert_eq!(doc.name, "hmos-arkui-develop-skill");
        assert!(
            !doc.description.is_empty() && !doc.description.contains("触发场景"),
            "block-scalar description should be summarized to first paragraph, got: {}",
            doc.description
        );
    }

    #[test]
    fn skill_identity_prefers_frontmatter_name() {
        let doc = SkillDoc {
            name: "My-Skill".into(),
            description: String::new(),
            version: String::new(),
        };
        // frontmatter name wins, lowercased
        assert_eq!(skill_identity("some-dir", &doc), "my-skill");
        // no name → dir name, lowercased
        let empty = SkillDoc::default();
        assert_eq!(skill_identity("Some-Dir", &empty), "some-dir");
    }

    #[test]
    fn sniff_entry_exists_matches_identity_or_dir() {
        let entry = SniffScanEntry {
            identity: "demo-skill".into(),
            directory: "demo-dir".into(),
            source_path: PathBuf::from("/x/demo-dir"),
            title: "Demo".into(),
            description: String::new(),
            found_agents: BTreeSet::new(),
        };
        // matches by identity
        let mut existing: BTreeSet<String> = BTreeSet::new();
        existing.insert("demo-skill".into());
        assert!(sniff_entry_exists(&entry, &existing));
        // matches by dir name
        let mut by_dir: BTreeSet<String> = BTreeSet::new();
        by_dir.insert("demo-dir".into());
        assert!(sniff_entry_exists(&entry, &by_dir));
        // no match
        let empty: BTreeSet<String> = BTreeSet::new();
        assert!(!sniff_entry_exists(&entry, &empty));
    }

    #[test]
    fn is_plain_entry_name_rejects_traversal() {
        assert!(is_plain_entry_name("my-skill"));
        assert!(is_plain_entry_name("skill_1.2"));
        assert!(!is_plain_entry_name(""));
        assert!(!is_plain_entry_name("."));
        assert!(!is_plain_entry_name(".."));
        assert!(!is_plain_entry_name("a/b"));
        assert!(!is_plain_entry_name("a\\b"));
    }

    /// A unique scratch dir under the system temp dir for symlink tests.
    fn scratch_dir(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("octopus-symlink-test-{}-{}", tag, now_secs()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("mk scratch");
        base
    }

    #[test]
    fn count_skills_in_root_includes_directory_symlinks() {
        let base = scratch_dir("count-links");
        let library_skill = base.join("library-skill");
        fs::create_dir_all(&library_skill).unwrap();
        fs::write(library_skill.join("SKILL.md"), "name: linked").unwrap();
        let root = base.join("agent-root");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&library_skill, root.join("linked")).unwrap();
        fs::create_dir_all(root.join("plain")).unwrap();
        fs::write(root.join("plain/SKILL.md"), "name: plain").unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden/SKILL.md"), "name: hidden").unwrap();

        assert_eq!(count_skills_in_root(&root), 2);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_link_install_creates_and_replaces_idempotently() {
        let base = scratch_dir("ensure");
        let target = base.join("lib-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "---\nname: t\n---\n").unwrap();
        let root = base.join("agent-root");

        ensure_skill_install(&root, "lib-skill", &target, SkillInstallMode::Link)
            .expect("first link");
        let link = root.join("lib-skill");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), target);

        // Idempotent: correct link stays a link, no error.
        ensure_skill_install(&root, "lib-skill", &target, SkillInstallMode::Link)
            .expect("second link");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_link_install_replaces_real_directory() {
        let base = scratch_dir("replace");
        let target = base.join("lib-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "x").unwrap();
        let root = base.join("agent-root");
        fs::create_dir_all(&root).unwrap();
        // Pre-existing REAL directory occupying the name.
        let occupied = root.join("lib-skill");
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("old.txt"), "old").unwrap();

        ensure_skill_install(&root, "lib-skill", &target, SkillInstallMode::Link).expect("replace");
        assert!(fs::symlink_metadata(&occupied)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&occupied).unwrap(), target);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_copy_install_creates_independent_directory() {
        let base = scratch_dir("copy");
        let target = base.join("lib-skill");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("SKILL.md"), "original").unwrap();
        fs::write(target.join("nested/file.txt"), "content").unwrap();
        let root = base.join("agent-root");

        ensure_skill_install(&root, "lib-skill", &target, SkillInstallMode::Copy)
            .expect("copy install");
        let installed = root.join("lib-skill");
        assert!(fs::symlink_metadata(&installed)
            .unwrap()
            .file_type()
            .is_dir());
        assert!(!fs::symlink_metadata(&installed)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(installed.join("nested/file.txt")).unwrap(),
            "content"
        );

        fs::write(target.join("nested/file.txt"), "updated").unwrap();
        assert_eq!(
            fs::read_to_string(installed.join("nested/file.txt")).unwrap(),
            "content"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_agent_skill_entry_handles_symlink_dir_and_missing() {
        let base = scratch_dir("remove");
        let target = base.join("lib-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "x").unwrap();
        let root = base.join("agent-root");
        fs::create_dir_all(&root).unwrap();

        // Missing → false, no error.
        assert!(!remove_agent_skill_entry(&root, "lib-skill").unwrap());

        // Symlink → removed, target survives.
        ensure_skill_install(&root, "lib-skill", &target, SkillInstallMode::Link).unwrap();
        assert!(remove_agent_skill_entry(&root, "lib-skill").unwrap());
        assert!(!root.join("lib-skill").exists());
        assert!(
            target.join("SKILL.md").exists(),
            "real target must survive unlink"
        );

        // Real directory → removed.
        let real = root.join("lib-skill");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("f.txt"), "y").unwrap();
        assert!(remove_agent_skill_entry(&root, "lib-skill").unwrap());
        assert!(!real.exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn relocate_finds_moved_skill_including_dot_dir() {
        let base = scratch_dir("relocate");
        // 仓库重排：旧路径 .hmos-skill-reviewer 已不存在，技能搬进 .aw-skills/
        let moved = base.join(".aw-skills/hmos-skill-reviewer");
        fs::create_dir_all(&moved).unwrap();
        fs::write(
            moved.join("SKILL.md"),
            "---\nname: hmos-skill-reviewer\n---\n# reviewer\n",
        )
        .unwrap();

        // 旧记录路径直接定位失败
        assert!(locate_skill_in_clone(&base, ".hmos-skill-reviewer").is_none());

        // 按身份整仓重定位成功，并能进入点目录
        let (dir, rel) =
            relocate_skill_in_clone(&base, "hmos-skill-reviewer", "hmos-skill-reviewer").unwrap();
        assert!(dir.join("SKILL.md").exists());
        assert!(rel.contains(".aw-skills/hmos-skill-reviewer"), "rel={rel}");

        // 找不到的身份返回 None（真删除）
        assert!(relocate_skill_in_clone(&base, "does-not-exist", "does-not-exist").is_none());

        let _ = fs::remove_dir_all(&base);
    }

    /// Test-only HOME swap. Must hold `TEST_HOME_LOCK` so parallel tests don't
    /// see a mutated HOME while deriving agent paths. Windows home resolution
    /// bypasses env vars, so write tests using this helper are Unix-only.
    #[cfg(not(windows))]
    struct TempHome {
        path: PathBuf,
        prev_home: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(not(windows))]
    impl TempHome {
        fn new(name: &str) -> Self {
            let guard = crate::config::lock_home_for_test();
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("octopus-{name}-{nonce}"));
            fs::create_dir_all(&path).unwrap();
            let prev_home = std::env::var_os("HOME");
            std::env::set_var("HOME", &path);
            Self {
                path,
                prev_home,
                _guard: guard,
            }
        }
    }

    #[cfg(not(windows))]
    impl Drop for TempHome {
        fn drop(&mut self) {
            match self.prev_home.take() {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn omo_skills_target_is_supported_under_agent_root() {
        let targets = agent_skills_targets();
        let target = targets
            .iter()
            .find(|target| target.agent == "omo")
            .expect("OmO skills target missing");
        assert!(target.supported);
        assert_eq!(
            target.roots,
            vec![crate::platform::expand_home("~/.omo/agent/skills").unwrap()]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn omo_skills_link_copy_scan_count_list() {
        let home = TempHome::new("omo-skills-install");
        let lib_skill = home.path.join(".octopus/skills/demo");
        fs::create_dir_all(&lib_skill).unwrap();
        fs::write(
            lib_skill.join("SKILL.md"),
            "---\nname: demo\ndescription: omo test\n---\n# Demo\n",
        )
        .unwrap();

        let desired: BTreeSet<String> = ["omo".to_string()].into_iter().collect();
        let link = sync_skill_installations(
            "demo",
            &lib_skill,
            &desired,
            &desired,
            SkillInstallMode::Link,
        );
        assert!(link.errors.is_empty(), "{:?}", link.errors);

        let dest = home.path.join(".omo/agent/skills/demo");
        assert!(
            fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink(),
            "Link mode must create a symlink"
        );
        assert_eq!(count_agent_skills("omo"), 1);
        assert!(list_agent_skills("omo")
            .iter()
            .any(|skill| skill.id == "demo"));
        assert!(scan_applied_agents()
            .get("demo")
            .is_some_and(|agents| agents.contains("omo")));

        let copy = sync_skill_installations(
            "demo",
            &lib_skill,
            &desired,
            &desired,
            SkillInstallMode::Copy,
        );
        assert!(copy.errors.is_empty(), "{:?}", copy.errors);
        let meta = fs::symlink_metadata(&dest).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert!(dest.join("SKILL.md").is_file());
        assert_eq!(count_agent_skills("omo"), 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_agent_behavior_is_unchanged() {
        let home = TempHome::new("omo-skills-unsupported");
        let lib_skill = home.path.join(".octopus/skills/demo");
        fs::create_dir_all(&lib_skill).unwrap();
        fs::write(lib_skill.join("SKILL.md"), "---\nname: demo\n---\n# Demo\n").unwrap();

        let desired: BTreeSet<String> = ["claude-desktop".to_string()].into_iter().collect();
        let outcome = sync_skill_installations(
            "demo",
            &lib_skill,
            &desired,
            &desired,
            SkillInstallMode::Copy,
        );
        assert!(
            outcome.errors.iter().any(|e| e.contains("暂不支持")),
            "{:?}",
            outcome.errors
        );
        assert!(!home.path.join(".omo/agent/skills/demo").exists());
    }

    #[cfg(not(windows))]
    #[test]
    fn local_import_skips_existing_without_overwrite() {
        let home = TempHome::new("local-import-skip");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("demo");
        fs::create_dir_all(&existing).unwrap();
        fs::write(
            existing.join("SKILL.md"),
            "---\nname: demo\n---\n# Demo v1\n",
        )
        .unwrap();

        // 待导入目录：一个与库内同名（未勾选覆盖），一个全新技能。
        let src = home.path.join("incoming");
        let dup = src.join("demo-copy");
        fs::create_dir_all(&dup).unwrap();
        fs::write(dup.join("SKILL.md"), "---\nname: demo\n---\n# Demo v2\n").unwrap();
        let fresh = src.join("fresh");
        fs::create_dir_all(&fresh).unwrap();
        fs::write(fresh.join("SKILL.md"), "# fresh\n").unwrap();

        let res = add_skill_local(src.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(res.message.contains("跳过 1"), "{}", res.message);
        assert!(lib.join("fresh").is_dir());
        // 未勾选覆盖的重名技能被跳过，不再生成 name-2 副本
        assert!(!lib.join("demo-2").exists());
        assert!(fs::read_to_string(existing.join("SKILL.md"))
            .unwrap()
            .contains("Demo v1"));
    }

    #[cfg(not(windows))]
    #[test]
    fn local_import_overwrites_selected_duplicate() {
        let home = TempHome::new("local-import-overwrite");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("demo");
        fs::create_dir_all(&existing).unwrap();
        fs::write(
            existing.join("SKILL.md"),
            "---\nname: demo\n---\n# Demo v1\n",
        )
        .unwrap();

        let src = home.path.join("incoming");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo\n---\n# Demo v2\n").unwrap();

        let res = add_skill_local(
            src.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["demo".to_string()]),
        )
        .unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(fs::read_to_string(existing.join("SKILL.md"))
            .unwrap()
            .contains("Demo v2"));
        assert!(!lib.join("demo-2").exists());
    }

    #[test]
    fn local_import_plan_skips_unchecked_duplicates() {
        let mut existing = HashSet::new();
        existing.insert("demo".to_string());
        let mut overwrite = HashMap::new();
        assert_eq!(
            local_import_plan("demo", &existing, &overwrite),
            LocalImportPlan::Skip
        );
        assert_eq!(
            local_import_plan("other", &existing, &overwrite),
            LocalImportPlan::Import
        );
        overwrite.insert("demo".to_string(), "demo".to_string());
        assert_eq!(
            local_import_plan("demo", &existing, &overwrite),
            LocalImportPlan::Overwrite("demo")
        );
    }

    /// D-002：覆盖导入失败时必须保留库里原有技能，且不留 staging/backup 残留。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_import_failure_keeps_original_skill() {
        let home = TempHome::new("skill-overwrite-import-fail");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join("demo")).unwrap();
        fs::write(lib.join("demo/SKILL.md"), "---\nname: demo\n---\n# v1\n").unwrap();
        fs::write(lib.join("demo/keep.txt"), "keep").unwrap();

        let src = home.path.join("incoming/demo");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo\n---\n# v2\n").unwrap();
        fs::write(src.join("locked.bin"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(src.join("locked.bin"), fs::Permissions::from_mode(0o000)).unwrap();
        }

        let err = add_skill_local(
            src.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["demo".to_string()]),
        )
        .unwrap_err();
        assert!(err.contains("复制文件失败"), "{}", err);

        assert!(fs::read_to_string(lib.join("demo/SKILL.md"))
            .unwrap()
            .contains("# v1"));
        assert!(lib.join("demo/keep.txt").is_file());
        assert!(
            fs::read_dir(&lib)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".octopus-")),
            "不应残留 staging/backup 目录"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(src.join("locked.bin"), fs::Permissions::from_mode(0o600));
        }
    }

    /// D-003：覆盖导出失败时必须保留目标目录里用户的原有内容。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_export_failure_keeps_target_dir() {
        let home = TempHome::new("skill-overwrite-export-fail");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join("exp")).unwrap();
        fs::write(lib.join("exp/SKILL.md"), "---\nname: exp\n---\n# lib\n").unwrap();
        fs::write(lib.join("exp/locked.bin"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                lib.join("exp/locked.bin"),
                fs::Permissions::from_mode(0o000),
            )
            .unwrap();
        }

        let target = home.path.join("target");
        fs::create_dir_all(target.join("exp")).unwrap();
        fs::write(target.join("exp/SKILL.md"), "# user version\n").unwrap();
        fs::write(target.join("exp/keep2.txt"), "keep2").unwrap();

        let res = export_skills_to_dir(
            vec!["exp".to_string()],
            SkillInstallMode::Copy,
            target.to_string_lossy().to_string(),
            vec!["exp".to_string()],
        )
        .unwrap();
        assert!(!res.ok, "{}", res.message);
        assert_eq!(res.failed, 1, "{:?}", res.errors);

        assert!(fs::read_to_string(target.join("exp/SKILL.md"))
            .unwrap()
            .contains("user version"));
        assert!(target.join("exp/keep2.txt").is_file());
        assert!(
            fs::read_dir(&target)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".octopus-")),
            "不应残留 staging/backup 目录"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                lib.join("exp/locked.bin"),
                fs::Permissions::from_mode(0o600),
            );
        }
    }

    /// D-007/D-008：同名条目按「跳过」计数，不拉低 `ok`，也不靠错误文案分类。
    #[cfg(not(windows))]
    #[test]
    fn export_skips_existing_without_failing() {
        let home = TempHome::new("skill-export-skip");
        let lib = home.path.join(".octopus/skills");
        for (id, body) in [("exp", "# lib exp\n"), ("fresh", "# lib fresh\n")] {
            fs::create_dir_all(lib.join(id)).unwrap();
            fs::write(
                lib.join(id).join("SKILL.md"),
                format!("---\nname: {}\nversion: 2.0.0\n---\n{}", id, body),
            )
            .unwrap();
        }

        let target = home.path.join("target");
        fs::create_dir_all(target.join("exp")).unwrap();
        fs::write(
            target.join("exp/SKILL.md"),
            "---\nname: exp\nversion: 1.0.0\n---\n# user version\n",
        )
        .unwrap();
        fs::write(target.join("exp/keep.txt"), "keep").unwrap();

        let res = export_skills_to_dir(
            vec!["exp".to_string(), "fresh".to_string()],
            SkillInstallMode::Copy,
            target.to_string_lossy().to_string(),
            vec![],
        )
        .unwrap();
        assert!(res.ok, "跳过不应把 ok 拉成 false：{}", res.message);
        assert_eq!((res.succeeded, res.skipped, res.failed), (1, 1, 0));
        assert!(res.message.contains("跳过 1 个"), "{}", res.message);
        assert_eq!(res.errors.len(), 1);
        assert!(
            res.errors[0].contains("existing_version=1.0.0")
                && res.errors[0].contains("import_version=2.0.0"),
            "{:?}",
            res.errors
        );

        // 跳过的目标保持用户原样，新技能正常落盘。
        assert!(fs::read_to_string(target.join("exp/SKILL.md"))
            .unwrap()
            .contains("user version"));
        assert!(target.join("exp/keep.txt").is_file());
        assert!(target.join("fresh/SKILL.md").is_file());
    }

    /// D-001：首部点号的技能名落库时必须归一，导入后可见且能被判重。
    #[cfg(not(windows))]
    #[test]
    fn dot_named_skill_import_is_visible_and_deduped() {
        let home = TempHome::new("skill-dot-visible");
        let lib = home.path.join(".octopus/skills");
        let src = home.path.join("incoming/.hidden-skill");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "no heading body\n").unwrap();

        let res = add_skill_local(src.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(lib.join("hidden-skill").is_dir());
        assert!(!lib.join(".hidden-skill").exists());
        assert!(list_skills()
            .unwrap()
            .skills
            .iter()
            .any(|s| s.id == "hidden-skill"));

        // 判重与写入同口径：再导入应被识别为重复并跳过。
        let check = check_skill_local_duplicate(src.to_string_lossy().to_string()).unwrap();
        assert_eq!(check.conflicts.len(), 1);
        assert_eq!(check.conflicts[0].existing_id, "hidden-skill");

        let again =
            add_skill_local(src.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(!again.ok, "{}", again.message);
        assert!(!lib.join("hidden-skill-2").exists());
    }

    /// D-004：导入按链接本体复制，不跟随软链，因此环不会递归、文件链不会实体化。
    #[cfg(all(not(windows), unix))]
    #[test]
    fn import_preserves_symlinks_without_following_cycles() {
        let home = TempHome::new("skill-symlink-import");
        let lib = home.path.join(".octopus/skills");
        let src = home.path.join("incoming/demo");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo\n---\n# demo\n").unwrap();
        fs::write(src.join("real.txt"), "real").unwrap();
        std::os::unix::fs::symlink("real.txt", src.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(".", src.join("loop")).unwrap();

        let res = add_skill_local(src.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(res.ok, "{}", res.message);

        let imported = lib.join("demo");
        let loop_meta = fs::symlink_metadata(imported.join("loop")).unwrap();
        assert!(loop_meta.file_type().is_symlink(), "目录软链应按链接保留");
        let link_meta = fs::symlink_metadata(imported.join("link.txt")).unwrap();
        assert!(link_meta.file_type().is_symlink(), "文件软链不应被实体化");
        assert_eq!(
            fs::read_link(imported.join("link.txt")).unwrap(),
            PathBuf::from("real.txt")
        );
        // 环没有被展开：顶层只有源目录的四个条目，没有多出来的副本。
        let mut names: Vec<String> = fs::read_dir(&imported)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["SKILL.md", "link.txt", "loop", "real.txt"]);
    }

    /// 覆盖目标 id 必须是库内单层目录名，`../` 不得触达库外目录。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_import_rejects_traversal_ids() {
        let home = TempHome::new("skill-overwrite-traversal");
        let outside = home.path.join("evil");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("SKILL.md"),
            "---\nname: evil\n---\n# outside\n",
        )
        .unwrap();

        let src = home.path.join("incoming/evil");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: evil\n---\n# incoming\n").unwrap();

        // 直接调用导入内核：穿越 id 被拒，库外目录不受影响。
        let err = import_skill_dir(&src, None, SkillSource::Local, None, "", Some("../../evil"))
            .unwrap_err();
        assert!(err.contains("非法的技能标识"), "{}", err);
        assert!(outside.join("SKILL.md").is_file());

        // 命令层：穿越 id 会被过滤掉，退化为普通导入，不触碰库外目录。
        let res = add_skill_local(
            src.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["../../evil".to_string()]),
        )
        .unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(fs::read_to_string(outside.join("SKILL.md"))
            .unwrap()
            .contains("outside"));
        assert!(home.path.join(".octopus/skills/evil").is_dir());
    }

    /// D-005：删除不存在的技能必须报告失败，而不是「已删除技能」。
    #[cfg(not(windows))]
    #[test]
    fn delete_missing_skill_reports_not_found() {
        let home = TempHome::new("skill-delete-missing");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(&lib).unwrap();

        let res = delete_skill("nope".to_string(), false).unwrap();
        assert!(!res.ok, "{}", res.message);
        assert_eq!(res.message, "技能不存在");
        assert!(res.errors.is_empty());

        // 批量入口把「不存在」计入 skipped，而不是成功或失败。
        let batch = batch_delete_skills(vec!["nope".to_string()], false).unwrap();
        assert!(batch.ok, "{}", batch.message);
        assert_eq!((batch.succeeded, batch.skipped, batch.failed), (0, 1, 0));
        assert!(
            batch.message.contains("跳过 1 个（不存在）"),
            "{}",
            batch.message
        );
        assert_eq!(batch.errors.len(), 1);
        assert!(batch.errors[0].contains("技能不存在"), "{:?}", batch.errors);
    }

    /// D-006：单条删除的副本清理失败必须进入结构化 errors，而不是只拼进 message。
    #[cfg(all(not(windows), unix))]
    #[test]
    fn delete_skill_surfaces_copy_cleanup_errors() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempHome::new("skill-delete-copy-errors");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join("demo")).unwrap();
        fs::write(lib.join("demo/SKILL.md"), "---\nname: demo\n---\n# demo\n").unwrap();

        // 让 codex 侧的副本无法删除：子目录去掉写权限后 remove_dir_all 会失败。
        let agent_copy = home.path.join(".codex/skills/demo");
        fs::create_dir_all(agent_copy.join("blocked")).unwrap();
        fs::write(agent_copy.join("blocked/file.txt"), "x").unwrap();
        fs::set_permissions(
            agent_copy.join("blocked"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();

        let res = delete_skill("demo".to_string(), true).unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(!lib.join("demo").exists());
        assert!(!res.errors.is_empty(), "副本清理失败应进入 errors");
        assert!(res.message.contains("副本清理未成功"), "{}", res.message);
        assert!(
            res.errors.iter().any(|e| e.contains("codex")),
            "{:?}",
            res.errors
        );

        // 恢复权限，避免临时 HOME 清理失败。
        fs::set_permissions(
            agent_copy.join("blocked"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let _ = fs::remove_dir_all(&agent_copy);
    }

    /// D-009：更深层级的技能不会被静默忽略，导入消息必须给出数量。
    #[cfg(not(windows))]
    #[test]
    fn nested_skill_dirs_are_reported() {
        let home = TempHome::new("skill-nested-report");
        let lib = home.path.join(".octopus/skills");
        let parent = home.path.join("incoming");
        fs::create_dir_all(parent.join("a")).unwrap();
        fs::write(parent.join("a/SKILL.md"), "---\nname: a\n---\n# a\n").unwrap();
        fs::create_dir_all(parent.join("group/c")).unwrap();
        fs::write(parent.join("group/c/SKILL.md"), "---\nname: c\n---\n# c\n").unwrap();

        let res =
            add_skill_local(parent.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(
            res.message.contains("另有 1 个技能在更深层级"),
            "{}",
            res.message
        );
        assert!(lib.join("a").is_dir());
        assert!(!lib.join("c").is_dir());

        // 只有深层候选、没有一层技能时，不能只说“未找到”。
        let only_nested = home.path.join("only-nested");
        fs::create_dir_all(only_nested.join("group/c")).unwrap();
        fs::write(
            only_nested.join("group/c/SKILL.md"),
            "---\nname: c\n---\n# c\n",
        )
        .unwrap();
        let res = add_skill_local(
            only_nested.to_string_lossy().to_string(),
            String::new(),
            None,
        )
        .unwrap();
        assert!(!res.ok, "{}", res.message);
        assert!(
            res.message.contains("发现 1 个技能在更深层级"),
            "{}",
            res.message
        );
    }

    /// D-010：同一批内的同名技能要在预览阶段标出，而不是导入后才发现少了一个。
    #[cfg(not(windows))]
    #[test]
    fn batch_duplicate_skills_are_previewed() {
        let home = TempHome::new("skill-batch-dup-preview");
        let lib = home.path.join(".octopus/skills");
        let parent = home.path.join("incoming");
        for dir in ["dup-one", "dup-two"] {
            fs::create_dir_all(parent.join(dir)).unwrap();
            fs::write(
                parent.join(dir).join("SKILL.md"),
                "---\nname: same\n---\n# same\n",
            )
            .unwrap();
        }

        let check = check_skill_local_duplicate(parent.to_string_lossy().to_string()).unwrap();
        assert!(check.ok);
        assert!(check.conflicts.is_empty());
        assert_eq!(check.batch_duplicates, vec!["same".to_string()]);
        assert_eq!(check.total_detected, 2);

        let res =
            add_skill_local(parent.to_string_lossy().to_string(), String::new(), None).unwrap();
        assert!(res.ok, "{}", res.message);
        assert!(lib.join("same").is_dir());
        assert!(!lib.join("same-2").exists(), "批内重复不应生成副本");
        assert!(res.message.contains("跳过 1 个重复技能"), "{}", res.message);

        // 库内同名时：第一个进 conflicts，第二个仍归为批内重复。
        let check = check_skill_local_duplicate(parent.to_string_lossy().to_string()).unwrap();
        assert_eq!(check.conflicts.len(), 1);
        assert_eq!(check.conflicts[0].existing_id, "same");
        assert_eq!(check.batch_duplicates, vec!["same".to_string()]);
    }

    /// D-011：安装失败时标签不得被写入，失败文案要说明这一点。
    #[cfg(not(windows))]
    #[test]
    fn apply_failure_keeps_tag_unchanged() {
        let home = TempHome::new("skill-apply-tag-order");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join("demo")).unwrap();
        fs::write(lib.join("demo/SKILL.md"), "---\nname: demo\n---\n# demo\n").unwrap();
        batch_set_skill_tag(vec!["demo".to_string()], "old".to_string()).unwrap();

        // claude-desktop 不支持本地 Skills，安装必然失败。
        let res = apply_skill_to_agents(
            "demo".to_string(),
            vec!["claude-desktop".to_string()],
            "new".to_string(),
            SkillInstallMode::Link,
        )
        .unwrap();
        assert!(!res.ok, "{}", res.message);
        assert!(!res.errors.is_empty());
        assert!(res.message.contains("标签未更新"), "{}", res.message);
        let tag = list_skills()
            .unwrap()
            .skills
            .into_iter()
            .find(|s| s.id == "demo")
            .map(|s| s.tag)
            .unwrap();
        assert_eq!(tag, "old", "失败时不应写入新标签");
        assert!(res.skill.unwrap().applied_agents.is_empty());

        // 成功路径仍然落标签。
        let ok = apply_skill_to_agents(
            "demo".to_string(),
            vec!["codex".to_string()],
            "new".to_string(),
            SkillInstallMode::Link,
        )
        .unwrap();
        assert!(ok.ok, "{}", ok.message);
        let tag = list_skills()
            .unwrap()
            .skills
            .into_iter()
            .find(|s| s.id == "demo")
            .map(|s| s.tag)
            .unwrap();
        assert_eq!(tag, "new");
    }

    /// D-012：覆盖模式清空 Agent 集合时，提示要说「解除」而不是「应用」。
    #[cfg(not(windows))]
    #[test]
    fn batch_apply_clear_reports_unlink() {
        let home = TempHome::new("skill-batch-clear-message");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join("m1")).unwrap();
        fs::write(lib.join("m1/SKILL.md"), "---\nname: m1\n---\n# m1\n").unwrap();

        let applied = batch_apply_skills_to_agents(
            vec!["m1".to_string()],
            vec!["codex".to_string()],
            BatchApplyMode::Replace,
            SkillInstallMode::Link,
        )
        .unwrap();
        assert!(applied.ok, "{}", applied.message);
        assert!(home.path.join(".codex/skills/m1").exists());

        let cleared = batch_apply_skills_to_agents(
            vec!["m1".to_string()],
            vec![],
            BatchApplyMode::Replace,
            SkillInstallMode::Link,
        )
        .unwrap();
        assert!(cleared.ok, "{}", cleared.message);
        assert!(
            cleared.message.contains("解除 1 个技能"),
            "{}",
            cleared.message
        );
        assert!(
            !cleared.message.contains("应用 1 个"),
            "{}",
            cleared.message
        );
        assert!(!home.path.join(".codex/skills/m1").exists());
    }

    /// D-015：修复前落库的点号目录要在启动迁移里改成归一 id，并同步 meta 与 Agent 副本。
    #[cfg(not(windows))]
    #[test]
    fn hidden_skill_dirs_migrate_on_upgrade() {
        let home = TempHome::new("skill-hidden-migrate");
        let lib = home.path.join(".octopus/skills");
        fs::create_dir_all(lib.join(".ghost")).unwrap();
        fs::write(
            lib.join(".ghost/SKILL.md"),
            "---\nname: ghost\n---\n# ghost\n",
        )
        .unwrap();

        let mut meta = default_local_meta();
        meta.tag = "legacy".to_string();
        meta.source = SkillSource::Github;
        db::upsert_skill_meta(".ghost", &meta).unwrap();

        let agent_copy = home.path.join(".codex/skills/.ghost");
        fs::create_dir_all(&agent_copy).unwrap();
        fs::write(
            agent_copy.join("SKILL.md"),
            "---\nname: ghost\n---\n# ghost\n",
        )
        .unwrap();

        // 修复前：列表里看不到这个技能。
        assert!(!list_skills()
            .unwrap()
            .skills
            .iter()
            .any(|s| s.id == ".ghost"));

        let report = migrate_hidden_skill_dirs().unwrap();
        assert_eq!(
            report.renamed,
            vec![(".ghost".to_string(), "ghost".to_string())]
        );
        assert_eq!(report.agent_entries, 1);
        assert!(lib.join("ghost/SKILL.md").is_file());
        assert!(!lib.join(".ghost").exists());
        assert!(home.path.join(".codex/skills/ghost").exists());
        assert!(!home.path.join(".codex/skills/.ghost").exists());

        let skill = list_skills()
            .unwrap()
            .skills
            .into_iter()
            .find(|s| s.id == "ghost")
            .unwrap();
        assert_eq!(skill.tag, "legacy");
        assert_eq!(skill.source, SkillSource::Github);
        assert_eq!(skill.applied_agents, vec!["codex".to_string()]);
        assert!(db::get_skill_meta(".ghost").unwrap().is_none());

        // 幂等：再跑一次不应有变化。
        let again = migrate_hidden_skill_dirs().unwrap();
        assert!(again.renamed.is_empty());
        assert!(again.skipped.is_empty());
    }

    /// D-015：目标 id 已被占用时不动数据，只记录冲突。
    #[cfg(not(windows))]
    #[test]
    fn hidden_skill_migration_keeps_conflicts() {
        let home = TempHome::new("skill-hidden-conflict");
        let lib = home.path.join(".octopus/skills");
        for id in [".dup", "dup"] {
            fs::create_dir_all(lib.join(id)).unwrap();
            fs::write(
                lib.join(id).join("SKILL.md"),
                format!("---\nname: {}\n---\n# {}\n", id.trim_start_matches('.'), id),
            )
            .unwrap();
        }

        let report = migrate_hidden_skill_dirs().unwrap();
        assert!(report.renamed.is_empty());
        assert_eq!(report.skipped, vec![".dup".to_string()]);
        assert!(lib.join(".dup").is_dir());
        assert!(lib.join("dup").is_dir());
    }

    /// D-015：内部 staging/backup 临时目录不能被当成遗留技能迁移。
    #[cfg(not(windows))]
    #[test]
    fn hidden_skill_migration_skips_internal_dirs() {
        let home = TempHome::new("skill-hidden-internal");
        let lib = home.path.join(".octopus/skills");
        let staging = lib.join(".octopus-demo-staging-1-2");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("SKILL.md"), "# staging\n").unwrap();

        let report = migrate_hidden_skill_dirs().unwrap();
        assert!(report.renamed.is_empty());
        assert!(report.skipped.is_empty());
        assert!(staging.is_dir());
    }

    /// D-017：库内目录 id 与 frontmatter name 不一致时，按 id 勾选的覆盖必须生效。
    /// 扫描导入以目录名落库，这类「id 与 name 不同」的技能是常态。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_import_matches_existing_dir_id_not_only_name() {
        let home = TempHome::new("overwrite-by-dir-id");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("demo-skill");
        fs::create_dir_all(&existing).unwrap();
        fs::write(
            existing.join("SKILL.md"),
            "---\nname: Demo Skill\n---\n# v1\n",
        )
        .unwrap();

        // 源技能的名字恰好等于库内目录 id，而不是库内技能的展示名。
        let src = home.path.join("incoming");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo-skill\n---\n# v2\n").unwrap();

        let res = add_skill_local(
            src.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["demo-skill".to_string()]),
        )
        .unwrap();

        assert!(res.ok, "勾选覆盖被跳过: {}", res.message);
        assert!(
            fs::read_to_string(existing.join("SKILL.md"))
                .unwrap()
                .contains("# v2"),
            "覆盖未生效，库内仍是旧内容"
        );
        assert!(!lib.join("demo-skill-2").exists());

        // 反向：源技能命中的是库内技能的展示名（目录 id 不同）时同样要覆盖，
        // 且不能因为 id/name 不同而新建一个同名目录。
        let odd = lib.join("odd-id");
        fs::create_dir_all(&odd).unwrap();
        fs::write(odd.join("SKILL.md"), "---\nname: Pretty Name\n---\n# v1\n").unwrap();
        let src2 = home.path.join("incoming2");
        fs::create_dir_all(&src2).unwrap();
        fs::write(src2.join("SKILL.md"), "---\nname: Pretty Name\n---\n# v2\n").unwrap();

        let res2 = add_skill_local(
            src2.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["odd-id".to_string()]),
        )
        .unwrap();
        assert!(res2.ok, "按展示名勾选覆盖被跳过: {}", res2.message);
        assert!(
            fs::read_to_string(odd.join("SKILL.md"))
                .unwrap()
                .contains("# v2"),
            "展示名命中时覆盖未生效"
        );
        assert!(!lib.join("Pretty-Name").exists());
    }

    /// D-018：批量导入里单项失败不能吞掉整批结果（不计失败数、只抛裸错误）。
    #[cfg(not(windows))]
    #[test]
    fn batch_import_reports_partial_failure() {
        let home = TempHome::new("batch-import-partial");
        let lib = home.path.join(".octopus/skills");
        let batch = home.path.join("batch");

        let good = batch.join("good");
        fs::create_dir_all(&good).unwrap();
        fs::write(good.join("SKILL.md"), "---\nname: good-skill\n---\n# a\n").unwrap();

        // 含不可读文件的技能：复制必然失败，用来制造单项失败。
        let bad = batch.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("SKILL.md"), "---\nname: bad-skill\n---\n# b\n").unwrap();
        let locked = bad.join("locked.bin");
        fs::write(&locked, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        }

        let res = add_skill_local(batch.to_string_lossy().to_string(), String::new(), None);

        // 先恢复权限，保证 TempHome 清理时不残留不可读文件。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o644));
        }

        let res = res.expect("单项失败不应让整批返回 Err");
        assert!(!res.ok, "有失败项时 ok 必须为 false: {}", res.message);
        assert!(res.message.contains("导入 1"), "{}", res.message);
        assert!(res.message.contains("失败 1"), "{}", res.message);
        assert_eq!(
            res.errors.len(),
            1,
            "逐项失败必须进 errors: {:?}",
            res.errors
        );
        assert!(res.errors[0].contains("bad-skill"), "{:?}", res.errors);
        assert!(lib.join("good-skill").is_dir());
        assert!(!lib.join("bad-skill").exists());
    }

    /// D-018 附带回归：整批都是重名时仍应只报跳过，不出现「失败」也不置空 errors。
    #[cfg(not(windows))]
    #[test]
    fn batch_import_all_skipped_reports_skip_only() {
        let home = TempHome::new("batch-import-all-skip");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("dup");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("SKILL.md"), "---\nname: dup\n---\n# v1\n").unwrap();

        let batch = home.path.join("batch");
        for name in ["a", "b"] {
            let dir = batch.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), "---\nname: dup\n---\n# v2\n").unwrap();
        }

        let res = add_skill_local(batch.to_string_lossy().to_string(), String::new(), None)
            .expect("全跳过不应返回 Err");
        assert!(!res.ok, "整批跳过时 ok 应为 false: {}", res.message);
        assert!(res.message.contains("均已存在"), "{}", res.message);
        assert!(!res.message.contains("失败"), "{}", res.message);
        assert!(res.errors.is_empty(), "{:?}", res.errors);
        assert!(fs::read_to_string(existing.join("SKILL.md"))
            .unwrap()
            .contains("# v1"));
    }

    /// D-019：覆盖导入只替换内容，不能丢掉标签与远端来源信息。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_import_keeps_tag_and_remote_meta() {
        let home = TempHome::new("overwrite-keeps-meta");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("demo");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("SKILL.md"), "---\nname: demo\n---\n# v1\n").unwrap();

        // 模拟一个从 GitHub 导入、带标签且有远端基线的技能。
        let mut meta = default_local_meta();
        meta.source = SkillSource::Github;
        meta.repo_url = "https://github.com/acme/demo".to_string();
        meta.github_owner = "acme".to_string();
        meta.github_repo = "demo".to_string();
        meta.github_path = "skills/demo".to_string();
        meta.tag = "legacy".to_string();
        meta.remote_ref = "abc123".to_string();
        meta.local_ref = "abc123".to_string();
        db::upsert_skill_meta("demo", &meta).unwrap();

        let src = home.path.join("v2");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo\n---\n# v2\n").unwrap();

        let res = add_skill_local(
            src.to_string_lossy().to_string(),
            String::new(),
            Some(vec!["demo".to_string()]),
        )
        .unwrap();
        assert!(res.ok, "{}", res.message);

        let after = db::get_skill_meta("demo").unwrap().unwrap();
        assert_eq!(after.tag, "legacy", "覆盖不应清空标签");
        assert_eq!(after.source, SkillSource::Github, "覆盖不应丢掉远端来源");
        assert_eq!(after.repo_url, "https://github.com/acme/demo");
        assert_eq!(after.github_owner, "acme");
        assert_eq!(after.github_repo, "demo");
        assert_eq!(after.github_path, "skills/demo");
        assert_eq!(after.remote_ref, "abc123");
        assert_eq!(after.local_ref, "abc123");
        // 内容基线必须按新内容重算，不能沿用旧 hash
        assert_eq!(after.content_hash, hash_skill_dir(&existing).unwrap());
        // 内容确实被覆盖
        assert!(fs::read_to_string(existing.join("SKILL.md"))
            .unwrap()
            .contains("# v2"));

        // 显式给新标签时以新标签为准。
        let src2 = home.path.join("v3");
        fs::create_dir_all(&src2).unwrap();
        fs::write(src2.join("SKILL.md"), "---\nname: demo\n---\n# v3\n").unwrap();
        add_skill_local(
            src2.to_string_lossy().to_string(),
            "fresh".to_string(),
            Some(vec!["demo".to_string()]),
        )
        .unwrap();
        assert_eq!(db::get_skill_meta("demo").unwrap().unwrap().tag, "fresh");
    }

    /// D-019 反向保护：远端导入覆盖本地技能时，应以新的仓库信息为准（不沿用旧值）。
    #[cfg(not(windows))]
    #[test]
    fn overwrite_from_remote_replaces_repo_meta() {
        let home = TempHome::new("overwrite-remote-meta");
        let lib = home.path.join(".octopus/skills");
        let existing = lib.join("demo");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("SKILL.md"), "---\nname: demo\n---\n# v1\n").unwrap();
        let mut meta = default_local_meta();
        meta.tag = "legacy".to_string();
        db::upsert_skill_meta("demo", &meta).unwrap();

        let src = home.path.join("incoming");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "---\nname: demo\n---\n# v2\n").unwrap();

        let gh = GitRepoRef {
            host: GitHost::Github,
            owner: "acme".to_string(),
            repo: "demo".to_string(),
            path: "skills/demo".to_string(),
            repo_url: "https://github.com/acme/demo".to_string(),
            branch: String::new(),
        };
        let rec =
            import_skill_dir(&src, None, SkillSource::Github, Some(gh), "", Some("demo")).unwrap();
        assert_eq!(rec.source, SkillSource::Github);

        let after = db::get_skill_meta("demo").unwrap().unwrap();
        assert_eq!(after.source, SkillSource::Github);
        assert_eq!(after.repo_url, "https://github.com/acme/demo");
        assert_eq!(after.github_owner, "acme");
        assert_eq!(after.github_path, "skills/demo");
        // 未给标签时仍沿用旧标签
        assert_eq!(after.tag, "legacy");
    }
}
