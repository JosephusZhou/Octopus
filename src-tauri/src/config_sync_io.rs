//! Scoped optimistic file protection for provider synchronization.
//!
//! Only fingerprints and paths are kept in memory; contents/credentials are never
//! persisted. Other processes cannot share our mutex, so check the file again
//! after writing the temporary file and retry the entire merge on a conflict.
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const CONFLICT: &str = "配置同步冲突：";
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    digest: [u8; 32],
    len: u64,
    modified: Option<std::time::SystemTime>,
}
fn fingerprint(path: &Path) -> Result<Option<Fingerprint>, String> {
    let metadata = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(format!("无法检查配置文件：{}", path.display())),
    };
    let bytes = fs::read(path).map_err(|_| format!("无法检查配置文件：{}", path.display()))?;
    let after =
        fs::metadata(path).map_err(|_| format!("{CONFLICT}{} 在读取时被删除", path.display()))?;
    if metadata.len() != after.len() || metadata.modified().ok() != after.modified().ok() {
        return Err(format!("{CONFLICT}{} 在读取时发生变化", path.display()));
    }
    Ok(Some(Fingerprint {
        digest: Sha256::digest(&bytes).into(),
        len: after.len(),
        modified: after.modified().ok(),
    }))
}

#[derive(Default)]
struct Attempt {
    reads: HashMap<PathBuf, Option<Fingerprint>>,
    files: HashMap<PathBuf, Result<(), String>>,
}
thread_local! { static ATTEMPT: RefCell<Option<Attempt>> = const { RefCell::new(None) }; }
struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        ATTEMPT.with(|a| *a.borrow_mut() = None);
    }
}
/// Retry only genuine concurrent modifications. Never retry parse/permission errors
/// by replacing a broken file with an empty document.
pub(crate) fn reconcile(
    mut operation: impl FnMut() -> Result<(), String>,
) -> (Result<(), String>, Vec<(PathBuf, Result<(), String>)>) {
    let nested = ATTEMPT.with(|a| a.borrow().is_some());
    if nested {
        return (Err("不能嵌套执行配置同步".into()), Vec::new());
    }
    let _scope = Scope;
    let mut files = HashMap::new();
    for attempt in 0..3 {
        ATTEMPT.with(|a| *a.borrow_mut() = Some(Attempt::default()));
        let result = operation();
        let current = ATTEMPT.with(|a| a.borrow_mut().take().expect("sync scope exists"));
        files.extend(current.files);
        let retry = result
            .as_ref()
            .err()
            .is_some_and(|e| e.starts_with(CONFLICT));
        if !retry || attempt == 2 {
            return (result, files.into_iter().collect());
        }
    }
    unreachable!("bounded retry returns on third attempt")
}
pub(crate) fn active() -> bool {
    ATTEMPT.with(|a| a.borrow().is_some())
}
/// Remember the first state observed in this attempt, including a missing file.
pub(crate) fn observe(path: &Path) -> Result<(), String> {
    if !ATTEMPT.with(|a| a.borrow().is_some()) {
        return Ok(());
    }
    let state = match fingerprint(path) {
        Ok(state) => state,
        Err(error) => {
            record(path, Err(error.clone()));
            return Err(error);
        }
    };
    ATTEMPT.with(|a| {
        if let Some(attempt) = a.borrow_mut().as_mut() {
            attempt.reads.entry(path.to_path_buf()).or_insert(state);
        }
    });
    Ok(())
}
pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, std::io::Error> {
    let path = path.as_ref();
    observe(path).map_err(std::io::Error::other)?;
    let result = fs::read_to_string(path);
    if result.is_err() {
        record(path, Err(format!("读取配置文件失败：{}", path.display())));
    }
    result
}
/// Avoid rewriting completed steps on recovery. Still verify the baseline before
/// accepting an unchanged document, so external changes are not silently ignored.
pub(crate) fn unchanged(path: &Path, content: &str) -> Result<bool, String> {
    if !active() {
        return Ok(false);
    }
    before_replace(path)?;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(format!("无法检查配置文件：{}", path.display())),
    };
    if bytes == content.as_bytes() {
        replaced(path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
/// Called after the temporary file is complete, immediately before rename.
pub(crate) fn before_replace(path: &Path) -> Result<(), String> {
    if !ATTEMPT.with(|a| a.borrow().is_some()) {
        return Ok(());
    }
    let current = fingerprint(path)?;
    // `reads` 区分三种状态：本次尝试没读过、读到过「不存在」、读到过具体内容的指纹。
    let expected = ATTEMPT.with(|a| a.borrow().as_ref().and_then(|a| a.reads.get(path).copied()));
    let result = match expected {
        // 没读过（例如只用 is_file() 探过存在性的新建凭据文件）：没有基线可比，放行。
        None => Ok(()),
        Some(None) if current.is_none() => Ok(()),
        Some(fingerprint) if fingerprint == current => Ok(()),
        // 观察到缺失却出现内容、读到后被改或被删：外部改动，重新读取合并。
        _ => Err(format!(
            "{CONFLICT}{} 在读取后发生变化，重新读取合并",
            path.display()
        )),
    };
    if result.is_err() {
        record(path, result.clone());
    }
    result
}
pub(crate) fn record(path: &Path, result: Result<(), String>) {
    ATTEMPT.with(|a| {
        if let Some(attempt) = a.borrow_mut().as_mut() {
            attempt.files.insert(path.to_path_buf(), result);
        }
    });
}
/// Our own completed writes become the baseline for subsequent steps in the same attempt.
pub(crate) fn replaced(path: &Path) -> Result<(), String> {
    if !ATTEMPT.with(|a| a.borrow().is_some()) {
        return Ok(());
    }
    let state = fingerprint(path)?;
    ATTEMPT.with(|a| {
        if let Some(attempt) = a.borrow_mut().as_mut() {
            attempt.reads.insert(path.to_path_buf(), state);
            attempt.files.insert(path.to_path_buf(), Ok(()));
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path() -> PathBuf {
        std::env::temp_dir().join(format!("octopus-conflict-{}", uuid::Uuid::new_v4()))
    }
    #[test]
    fn external_changes_retry_from_latest_document() {
        let path = path();
        fs::write(&path, r#"{"other":1}"#).unwrap();
        let mut runs = 0;
        let (result, _) = reconcile(|| {
            runs += 1;
            let mut root: serde_json::Value =
                serde_json::from_str(&read_to_string(&path).unwrap()).unwrap();
            if runs == 1 {
                fs::write(&path, r#"{"other":2,"external":true}"#).unwrap();
            }
            root["managed"] = serde_json::json!(true);
            crate::opencode_config::atomic_write(&path, &root.to_string())
        });
        assert!(result.is_ok());
        assert_eq!(runs, 2);
        let root: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["other"], 2);
        assert_eq!(root["external"], true);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn repeated_conflicts_leave_external_file_intact() {
        let path = path();
        fs::write(&path, "initial").unwrap();
        let mut runs = 0;
        let (result, _) = reconcile(|| {
            observe(&path)?;
            runs += 1;
            fs::write(&path, format!("external-{runs}")).unwrap();
            crate::opencode_config::atomic_write(&path, "must-not-overwrite")
        });
        assert!(result.is_err());
        assert_eq!(runs, 3);
        assert_eq!(fs::read_to_string(&path).unwrap(), "external-3");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn missing_file_created_externally_is_not_overwritten() {
        let path = path();
        let (result, _) = reconcile(|| {
            observe(&path)?;
            if !path.exists() {
                fs::write(&path, "external").unwrap();
            }
            before_replace(&path)?;
            Err("stop after confirming retry".into())
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        fs::remove_file(path).unwrap();
    }
}
