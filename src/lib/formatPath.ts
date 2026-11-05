/**
 * 展示用路径格式化：把真实文件系统路径转成不含真实用户名的可展示文本。
 *
 * 仅用于 UI 文本；传给后端做打开/读写/定位的路径必须继续使用原始真实路径。
 *
 * 规则：
 * - macOS `/Users/<name>/...`、Linux `/home/<name>/...` → `~/...`
 * - Windows `<drive>:\Users\<name>\AppData\Local\...` → `%LOCALAPPDATA%\...`
 * - Windows `<drive>:\Users\<name>\AppData\Roaming\...` → `%APPDATA%\...`
 * - Windows 其它用户目录 `<drive>:\Users\<name>\...` → `%USERPROFILE%\...`
 * - 不在用户目录下的路径（自定义目录、外接盘、网络盘、`/opt` 等）保持原样
 */
export function formatDisplayPath(raw: string | null | undefined): string {
  if (!raw) return "";
  const path = raw.trim();
  if (!path) return "";

  // 已是展示形态（~、$HOME、%LOCALAPPDATA% 等环境变量占位符），统一分隔符后原样返回
  const envToken = path.match(/^%[A-Za-z_][A-Za-z0-9_]*%/);
  if (envToken) {
    return envToken[0] + path.slice(envToken[0].length).replace(/\//g, "\\");
  }
  if (path.startsWith("~") || path.startsWith("$HOME")) {
    return path;
  }

  // Windows 盘符路径与 UNC 路径
  if (/^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\")) {
    return formatWindowsPath(path);
  }

  // POSIX 用户目录
  if (path.startsWith("/")) {
    const home = path.match(/^\/(?:Users|home)\/[^/]+(?=\/|$)/);
    if (home) return "~" + path.slice(home[0].length);
  }

  return path;
}

function formatWindowsPath(path: string): string {
  // 展示态统一使用反斜杠，避免出现 `%LOCALAPPDATA%\Octopus/xxx` 这类混用
  const normalized = path.replace(/\//g, "\\");
  const rules: Array<[RegExp, string]> = [
    [/^[A-Za-z]:\\Users\\[^\\]+\\AppData\\Local(?=\\|$)/i, "%LOCALAPPDATA%"],
    [/^[A-Za-z]:\\Users\\[^\\]+\\AppData\\Roaming(?=\\|$)/i, "%APPDATA%"],
    [/^[A-Za-z]:\\Users\\[^\\]+(?=\\|$)/i, "%USERPROFILE%"],
  ];
  for (const [re, token] of rules) {
    const matched = normalized.match(re);
    if (matched) return token + normalized.slice(matched[0].length);
  }
  return normalized;
}
