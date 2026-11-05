//! “关于”页后端：应用元信息、配置目录 reveal 与 GitHub 版本检查。
//!
//! 所有外发 HTTP 都复用 `http_client`，从而遵循 `config.json` 的网络代理配置；
//! 前端不再直接访问 GitHub API。

use crate::http_client;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::Duration;

const REPO_OWNER: &str = "JosephusZhou";
const REPO_NAME: &str = "Octopus";

/// 应用元信息。`config_dir` 为真实绝对路径（仅用于后端逻辑，前端展示时须脱敏）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutInfo {
    pub version: String,
    pub platform: String,
    pub arch: String,
    /// 真实配置目录绝对路径（Windows 可能是 `%LOCALAPPDATA%\Octopus` 或兼容的 `~/.octopus`）。
    pub config_dir: String,
}

/// 与既有 reveal 类 command 同构的结果：用 `ok` + `message` 给前端可展示反馈。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutActionResult {
    pub ok: bool,
    pub message: String,
    /// 实际打开的真实配置目录。
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: String,
    pub published_at: String,
    pub html_url: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub has_update: bool,
    pub latest: Option<ReleaseInfo>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// 版本号与平台信息统一从编译期常量读取，避免前端回退硬编码版本号。
pub fn app_info() -> Result<AboutInfo, String> {
    let dir = crate::platform::app_data_dir()?;
    Ok(AboutInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: platform_label(),
        arch: arch_label(),
        config_dir: dir.to_string_lossy().into_owned(),
    })
}

fn platform_label() -> String {
    match std::env::consts::OS {
        "macos" => "macOS".to_string(),
        "windows" => "Windows".to_string(),
        "linux" => "Linux".to_string(),
        other => other.to_string(),
    }
}

fn arch_label() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        "x86_64" => "x64".to_string(),
        other => other.to_string(),
    }
}

/// 在系统文件管理器中 reveal 真实配置目录，并把失败信息交给前端展示。
pub fn reveal_config_dir() -> AboutActionResult {
    let path = match crate::platform::app_data_dir() {
        Ok(p) => p,
        Err(e) => {
            return AboutActionResult {
                ok: false,
                message: e,
                path: String::new(),
            }
        }
    };
    let display = path.to_string_lossy().into_owned();
    match crate::platform::reveal_path(&path) {
        Ok(()) => AboutActionResult {
            ok: true,
            message: "已在文件管理器中打开配置目录".to_string(),
            path: display,
        },
        Err(e) => AboutActionResult {
            ok: false,
            message: format!("打开配置目录失败: {e}"),
            path: display,
        },
    }
}

/// 请求 GitHub `releases/latest` 并与编译期版本比较。
///
/// 草稿/预发布版本沿用旧行为：直接返回错误文案。
pub fn check_latest_release() -> Result<UpdateCheckResult, String> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    let client = {
        let builder = reqwest::blocking::Client::builder()
            .user_agent("Octopus (update-check)")
            .timeout(Duration::from_secs(15));
        http_client::apply_proxy(builder)?
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?
    };

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("检查更新失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回 {}", resp.status()));
    }
    let release: GithubRelease = resp
        .json()
        .map_err(|e| format!("解析 GitHub 响应失败: {e}"))?;
    if release.draft || release.prerelease {
        return Err("当前发布版本为草稿或预发布，跳过".to_string());
    }

    let current = env!("CARGO_PKG_VERSION").to_string();
    let has_update = compare_versions(&release.tag_name, &current) == Ordering::Greater;
    Ok(UpdateCheckResult {
        current_version: current,
        has_update,
        latest: has_update.then(|| ReleaseInfo {
            tag_name: release.tag_name,
            name: release.name,
            published_at: release.published_at,
            html_url: release.html_url,
            body: release.body,
        }),
    })
}

/// 点分版本比较：非数字段与缺失段一律按 0 处理，避免 `NaN` 式误判。
fn compare_versions(a: &str, b: &str) -> Ordering {
    let aa = version_segments(a);
    let bb = version_segments(b);
    let n = aa.len().max(bb.len());
    for i in 0..n {
        let x = aa.get(i).copied().unwrap_or(0);
        let y = bb.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

fn version_segments(raw: &str) -> Vec<u64> {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    trimmed
        .split('.')
        .map(|seg| {
            seg.split(['-', '+'])
                .next()
                .unwrap_or("")
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_handles_v_prefix_and_prerelease() {
        assert_eq!(compare_versions("v0.6.0", "0.5.0"), Ordering::Greater);
        assert_eq!(compare_versions("v0.5.0-beta", "0.5.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.5.0", "0.5"), Ordering::Equal);
    }

    #[test]
    fn compare_versions_treats_unparseable_segments_as_zero() {
        assert_eq!(compare_versions("latest", "0.5.0"), Ordering::Less);
        assert_eq!(compare_versions("v0.5.0", "1.0"), Ordering::Less);
    }
}
