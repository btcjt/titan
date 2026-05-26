//! Download manager — tracks file downloads from nsite content pages.
//!
//! Architecture mirrors `bookmarks.rs`: Mutex-wrapped in-memory state with
//! JSON persistence to `downloads.json` in the data directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: u64,
    pub url: String,
    pub display_url: String,
    pub filename: String,
    pub destination: PathBuf,
    pub status: DownloadStatus,
    pub bytes_total: Option<u64>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Pending,
    Downloading,
    Complete,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedDownloads {
    version: u32,
    downloads: Vec<Download>,
}

pub struct DownloadManager {
    downloads: Mutex<Vec<Download>>,
    next_id: AtomicU64,
    data_dir: PathBuf,
}

impl DownloadManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let (downloads, max_id) = load_from_disk(&data_dir);
        Self {
            downloads: Mutex::new(downloads),
            next_id: AtomicU64::new(max_id + 1),
            data_dir,
        }
    }

    pub fn start_download(
        &self,
        url: String,
        display_url: String,
        filename: String,
        destination: PathBuf,
    ) -> Download {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let download = Download {
            id,
            url,
            display_url,
            filename,
            destination,
            status: DownloadStatus::Pending,
            bytes_total: None,
            started_at: now,
            finished_at: None,
            error: None,
        };
        let mut list = self.downloads.lock().unwrap();
        list.push(download.clone());
        drop(list);
        self.save();
        download
    }

    pub fn mark_downloading(&self, id: u64) {
        let mut list = self.downloads.lock().unwrap();
        if let Some(dl) = list.iter_mut().find(|d| d.id == id) {
            dl.status = DownloadStatus::Downloading;
        }
    }

    pub fn mark_complete(&self, id: u64, bytes_total: Option<u64>) {
        let mut list = self.downloads.lock().unwrap();
        if let Some(dl) = list.iter_mut().find(|d| d.id == id) {
            dl.status = DownloadStatus::Complete;
            dl.bytes_total = bytes_total;
            dl.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
        drop(list);
        self.save();
    }

    pub fn mark_complete_by_url(&self, url: &str) {
        let mut list = self.downloads.lock().unwrap();
        if let Some(dl) = list.iter_mut().rev().find(|d| d.url == url && d.status != DownloadStatus::Complete) {
            dl.status = DownloadStatus::Complete;
            dl.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
        drop(list);
        self.save();
    }

    pub fn mark_failed(&self, id: u64, error: String) {
        let mut list = self.downloads.lock().unwrap();
        if let Some(dl) = list.iter_mut().find(|d| d.id == id) {
            dl.status = DownloadStatus::Failed;
            dl.error = Some(error);
            dl.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
        drop(list);
        self.save();
    }

    pub fn mark_failed_by_url(&self, url: &str, error: String) {
        let mut list = self.downloads.lock().unwrap();
        if let Some(dl) = list.iter_mut().rev().find(|d| d.url == url && d.status != DownloadStatus::Complete) {
            dl.status = DownloadStatus::Failed;
            dl.error = Some(error);
            dl.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
        drop(list);
        self.save();
    }

    pub fn list(&self) -> Vec<Download> {
        let list = self.downloads.lock().unwrap();
        list.iter().rev().cloned().collect()
    }

    pub fn get(&self, id: u64) -> Option<Download> {
        let list = self.downloads.lock().unwrap();
        list.iter().find(|d| d.id == id).cloned()
    }

    pub fn remove(&self, id: u64) {
        let mut list = self.downloads.lock().unwrap();
        list.retain(|d| d.id != id);
        drop(list);
        self.save();
    }

    pub fn clear_completed(&self) {
        let mut list = self.downloads.lock().unwrap();
        list.retain(|d| d.status != DownloadStatus::Complete && d.status != DownloadStatus::Failed);
        drop(list);
        self.save();
    }

    fn save(&self) {
        let list = self.downloads.lock().unwrap();
        let persisted = PersistedDownloads {
            version: 1,
            downloads: list.clone(),
        };
        drop(list);
        let path = self.data_dir.join("downloads.json");
        match serde_json::to_string_pretty(&persisted) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("failed to save downloads: {e}");
                }
            }
            Err(e) => warn!("failed to serialize downloads: {e}"),
        }
    }
}

fn load_from_disk(data_dir: &Path) -> (Vec<Download>, u64) {
    let path = data_dir.join("downloads.json");
    let json = match std::fs::read_to_string(&path) {
        Ok(j) => j,
        Err(_) => return (vec![], 0),
    };
    let persisted: PersistedDownloads = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(e) => {
            warn!("failed to parse downloads.json: {e}");
            return (vec![], 0);
        }
    };
    if persisted.version != 1 {
        warn!("unknown downloads.json version {}", persisted.version);
        return (vec![], 0);
    }
    let max_id = persisted.downloads.iter().map(|d| d.id).max().unwrap_or(0);
    // Discard pending/downloading entries from previous sessions
    let downloads: Vec<Download> = persisted
        .downloads
        .into_iter()
        .filter(|d| d.status == DownloadStatus::Complete || d.status == DownloadStatus::Failed)
        .collect();
    debug!("loaded {} downloads from disk", downloads.len());
    (downloads, max_id)
}

/// Generate a unique filename in `dir`, appending ` (1)`, ` (2)`, etc. if
/// the base name already exists. Sanitizes the filename to remove path
/// traversal attempts.
pub fn unique_filename(dir: &Path, raw_name: &str) -> PathBuf {
    let sanitized = sanitize_filename(raw_name);
    let name = if sanitized.is_empty() { "download" } else { &sanitized };

    let target = dir.join(name);
    if !target.exists() {
        return target;
    }

    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    for i in 1..1000 {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    dir.join(format!("{stem} ({}){ext}", chrono_fallback()))
}

fn sanitize_filename(name: &str) -> String {
    name.replace('/', "")
        .replace('\\', "")
        .replace('\0', "")
        .replace("..", "")
        .trim()
        .to_string()
}

fn chrono_fallback() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Extract a filename from a URL path. Falls back to "download" if the
/// path has no usable filename component.
pub fn extract_filename_from_url(url: &str) -> String {
    // Find the path portion: skip past "scheme://host" then take up to "?" or "#"
    let path = if let Some(after_scheme) = url.find("://") {
        let rest = &url[after_scheme + 3..];
        let path_start = rest.find('/').unwrap_or(rest.len());
        let path = &rest[path_start..];
        let end = path.find('?').unwrap_or(path.len()).min(path.find('#').unwrap_or(path.len()));
        &path[..end]
    } else {
        url
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(last) = segments.last() {
        let decoded = percent_decode(last);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    "download".to_string()
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn start_and_list() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl = mgr.start_download(
            "nsite-content://host/file.zip".into(),
            "nsite://site/file.zip".into(),
            "file.zip".into(),
            dir.path().join("file.zip"),
        );
        assert_eq!(dl.status, DownloadStatus::Pending);
        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].filename, "file.zip");
    }

    #[test]
    fn status_transitions_to_complete() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl = mgr.start_download(
            "url".into(), "display".into(), "f.txt".into(), dir.path().join("f.txt"),
        );
        mgr.mark_downloading(dl.id);
        assert_eq!(mgr.get(dl.id).unwrap().status, DownloadStatus::Downloading);
        mgr.mark_complete(dl.id, Some(1024));
        let completed = mgr.get(dl.id).unwrap();
        assert_eq!(completed.status, DownloadStatus::Complete);
        assert_eq!(completed.bytes_total, Some(1024));
        assert!(completed.finished_at.is_some());
    }

    #[test]
    fn status_transitions_to_failed() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl = mgr.start_download(
            "url".into(), "display".into(), "f.txt".into(), dir.path().join("f.txt"),
        );
        mgr.mark_failed(dl.id, "network error".into());
        let failed = mgr.get(dl.id).unwrap();
        assert_eq!(failed.status, DownloadStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("network error"));
    }

    #[test]
    fn persistence_round_trip() {
        let dir = test_dir();
        {
            let mgr = DownloadManager::new(dir.path().to_path_buf());
            let dl = mgr.start_download(
                "url".into(), "display".into(), "saved.zip".into(), dir.path().join("saved.zip"),
            );
            mgr.mark_complete(dl.id, Some(2048));
        }
        // Reload from disk
        let mgr2 = DownloadManager::new(dir.path().to_path_buf());
        let list = mgr2.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].filename, "saved.zip");
        assert_eq!(list[0].status, DownloadStatus::Complete);
    }

    #[test]
    fn pending_not_persisted() {
        let dir = test_dir();
        {
            let mgr = DownloadManager::new(dir.path().to_path_buf());
            mgr.start_download(
                "url".into(), "display".into(), "pending.zip".into(), dir.path().join("pending.zip"),
            );
        }
        let mgr2 = DownloadManager::new(dir.path().to_path_buf());
        assert!(mgr2.list().is_empty());
    }

    #[test]
    fn clear_completed_keeps_pending() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl1 = mgr.start_download(
            "a".into(), "a".into(), "done.zip".into(), dir.path().join("done.zip"),
        );
        mgr.mark_complete(dl1.id, Some(100));
        let _dl2 = mgr.start_download(
            "b".into(), "b".into(), "pending.zip".into(), dir.path().join("pending.zip"),
        );
        mgr.clear_completed();
        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].filename, "pending.zip");
    }

    #[test]
    fn remove_by_id() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl = mgr.start_download(
            "url".into(), "display".into(), "f.txt".into(), dir.path().join("f.txt"),
        );
        assert_eq!(mgr.list().len(), 1);
        mgr.remove(dl.id);
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn unique_filename_no_collision() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "file.zip");
        assert_eq!(path, dir.path().join("file.zip"));
    }

    #[test]
    fn unique_filename_with_collision() {
        let dir = test_dir();
        fs::write(dir.path().join("file.zip"), b"existing").unwrap();
        let path = unique_filename(dir.path(), "file.zip");
        assert_eq!(path, dir.path().join("file (1).zip"));
    }

    #[test]
    fn unique_filename_multiple_collisions() {
        let dir = test_dir();
        fs::write(dir.path().join("file.zip"), b"a").unwrap();
        fs::write(dir.path().join("file (1).zip"), b"b").unwrap();
        fs::write(dir.path().join("file (2).zip"), b"c").unwrap();
        let path = unique_filename(dir.path(), "file.zip");
        assert_eq!(path, dir.path().join("file (3).zip"));
    }

    #[test]
    fn sanitize_path_traversal() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "../../../etc/passwd");
        assert!(!path.to_string_lossy().contains(".."));
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn sanitize_slashes() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "path/to/file.zip");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains('/'));
    }

    #[test]
    fn sanitize_empty_name() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "download");
    }

    #[test]
    fn sanitize_null_bytes() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "file\0.zip");
        assert!(!path.to_string_lossy().contains('\0'));
    }

    #[test]
    fn unicode_filename() {
        let dir = test_dir();
        let path = unique_filename(dir.path(), "日本語ファイル.pdf");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "日本語ファイル.pdf");
    }

    #[test]
    fn very_long_filename() {
        let dir = test_dir();
        let long_name = format!("{}.zip", "a".repeat(300));
        let path = unique_filename(dir.path(), &long_name);
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn extract_filename_basic() {
        assert_eq!(extract_filename_from_url("nsite-content://host/path/file.zip"), "file.zip");
    }

    #[test]
    fn extract_filename_no_path() {
        assert_eq!(extract_filename_from_url("nsite-content://host/"), "download");
    }

    #[test]
    fn extract_filename_encoded() {
        assert_eq!(
            extract_filename_from_url("nsite-content://host/my%20file.zip"),
            "my file.zip"
        );
    }

    #[test]
    fn extract_filename_deep_path() {
        assert_eq!(
            extract_filename_from_url("nsite-content://host/a/b/c/deep.tar.gz"),
            "deep.tar.gz"
        );
    }

    #[test]
    fn list_returns_newest_first() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let dl1 = mgr.start_download("a".into(), "a".into(), "first.zip".into(), dir.path().join("first.zip"));
        let dl2 = mgr.start_download("b".into(), "b".into(), "second.zip".into(), dir.path().join("second.zip"));
        let list = mgr.list();
        assert_eq!(list[0].id, dl2.id);
        assert_eq!(list[1].id, dl1.id);
    }

    #[test]
    fn mark_complete_by_url_finds_latest() {
        let dir = test_dir();
        let mgr = DownloadManager::new(dir.path().to_path_buf());
        let _dl1 = mgr.start_download("same".into(), "d".into(), "a.zip".into(), dir.path().join("a.zip"));
        let dl2 = mgr.start_download("same".into(), "d".into(), "b.zip".into(), dir.path().join("b.zip"));
        mgr.mark_complete_by_url("same");
        assert_eq!(mgr.get(dl2.id).unwrap().status, DownloadStatus::Complete);
    }

    #[test]
    fn concurrent_id_generation() {
        let dir = test_dir();
        let mgr = std::sync::Arc::new(DownloadManager::new(dir.path().to_path_buf()));
        let mut handles = vec![];
        for i in 0..10 {
            let m = mgr.clone();
            let d = dir.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                m.start_download(
                    format!("url{i}"),
                    format!("d{i}"),
                    format!("f{i}.zip"),
                    d.join(format!("f{i}.zip")),
                )
            }));
        }
        let ids: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap().id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "all IDs should be unique");
    }
}
