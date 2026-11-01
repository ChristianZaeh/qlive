//! Authenticated, local-only editor buffer input. Source files are never written.
use axum::{extract::State, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::{mpsc, Arc, Mutex, RwLock, atomic::{AtomicU64, Ordering}}};
use tokio::sync::oneshot;
use crate::Preview;

pub struct BufferUpdate {
    pub snapshot: BufferSnapshot,
    pub reply: oneshot::Sender<Result<String, String>>,
}

#[derive(Clone)]
pub struct BufferSnapshot {
    pub revision: u64,
    pub buffers: BTreeMap<PathBuf, String>,
}

impl BufferSnapshot {
    pub fn read(&self, path: &Path) -> std::io::Result<String> {
        let path = std::fs::canonicalize(path)?;
        self.buffers.get(&path).cloned().map(Ok).unwrap_or_else(|| std::fs::read_to_string(path))
    }
}

#[derive(Clone)]
pub(crate) struct Editor {
    token: String,
    files: Arc<RwLock<Vec<PathBuf>>>,
    buffers: Arc<Mutex<BTreeMap<PathBuf, String>>>,
    tx: mpsc::Sender<BufferUpdate>,
    revision: Arc<AtomicU64>,
}

impl Preview {
    pub(crate) fn editor_authorized(&self, headers: &HeaderMap) -> bool {
        self.editor.as_ref().is_some_and(|editor| headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(format!("Bearer {}", editor.token).as_str()))
    }
    pub fn enable_editor(&mut self, token: String, target: PathBuf) -> mpsc::Receiver<BufferUpdate> {
        let (tx, rx) = mpsc::channel();
        self.editor = Some(Editor { token, files: Arc::new(RwLock::new(std::fs::canonicalize(target).ok().into_iter().collect())),
            buffers: Arc::new(Mutex::new(BTreeMap::new())), tx, revision: Arc::new(AtomicU64::new(0)) });
        rx
    }

    pub fn is_current_buffer(&self, revision: u64) -> bool {
        self.editor.as_ref().is_some_and(|e| e.revision.load(Ordering::SeqCst) == revision)
    }

    pub fn buffer_snapshot(&self) -> Option<BufferSnapshot> {
        let editor = self.editor.as_ref()?;
        let buffers = editor.buffers.lock().ok()?;
        Some(BufferSnapshot { revision: editor.revision.load(Ordering::SeqCst), buffers: buffers.clone() })
    }

    pub fn set_editor_files(&self, files: &[PathBuf], revision: Option<u64>) -> bool {
        let Some(editor) = &self.editor else { return true };
        let files: Vec<_> = files.iter().filter_map(|path| std::fs::canonicalize(path).ok()).collect();
        // Use the same lock as accepting buffers: an older render may not prune
        // a newly accepted chapter overlay or revoke its source scope.
        if let Ok(mut buffers) = editor.buffers.lock() {
            if editor.revision.load(Ordering::SeqCst) != revision.unwrap_or(0) { return false; }
            let Ok(mut allowed) = editor.files.write() else { return false; };
            *allowed = files.clone();
            buffers.retain(|path, _| files.contains(path));
            return true;
        }
        false
    }
}

pub(crate) async fn update(State(state): State<Arc<Preview>>, headers: HeaderMap, Json(value): Json<Value>) -> Response {
    let Some(editor) = &state.editor else { return StatusCode::NOT_FOUND.into_response() };
    let expected = format!("Bearer {}", editor.token);
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(path) = value["path"].as_str() else { return StatusCode::BAD_REQUEST.into_response() };
    let Some(text) = value["text"].as_str() else { return StatusCode::BAD_REQUEST.into_response() };
    let path = std::fs::canonicalize(path).ok();
    if !path.as_ref().is_some_and(|path| editor.files.read().is_ok_and(|files| files.contains(path))) {
        return (StatusCode::FORBIDDEN, "buffer does not belong to this preview").into_response();
    }
    let snapshot = {
        let Ok(mut buffers) = editor.buffers.lock() else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
        if !editor.files.read().is_ok_and(|files| files.contains(path.as_ref().unwrap())) {
            return StatusCode::FORBIDDEN.into_response();
        }
        if value["dirty"].as_bool().unwrap_or(true) { buffers.insert(path.unwrap(), text.to_owned()); }
        else { buffers.remove(&path.unwrap()); }
        let revision = editor.revision.fetch_add(1, Ordering::SeqCst) + 1;
        BufferSnapshot { revision, buffers: buffers.clone() }
    };
    let revision = snapshot.revision;
    let (reply, response) = oneshot::channel();
    if editor.tx.send(BufferUpdate { snapshot, reply }).is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match tokio::time::timeout(std::time::Duration::from_secs(30), response).await {
        Ok(Ok(Ok(status))) => Json(json!({"revision": revision, "status": status})).into_response(),
        Ok(Ok(Err(message))) => (StatusCode::CONFLICT, Json(json!({"revision": revision, "message": message}))).into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

pub(crate) async fn reveal(State(state): State<Arc<Preview>>, Json(value): Json<Value>) -> Response {
    if state.editor.is_none() { return StatusCode::NOT_FOUND.into_response(); }
    let Some(id) = value["id"].as_u64().and_then(|id| usize::try_from(id).ok()) else { return StatusCode::BAD_REQUEST.into_response() };
    let source = state.sources.read().ok().and_then(|sources| sources.get(id).cloned());
    let Some(source) = source else { return StatusCode::NOT_FOUND.into_response() };
    println!("QLIVE_REVEAL {source}");
    Json(json!({"status": "revealed"})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_scope_cannot_remove_a_newer_chapter_buffer() {
        let root = std::env::temp_dir().join(format!("qlive-buffer-scope-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let master = root.join("master.qmd");
        let child = root.join("child.qmd");
        std::fs::write(&master, "Master").unwrap();
        std::fs::write(&child, "Child").unwrap();
        let mut preview = Preview::new(root, "master.html".into());
        let _rx = preview.enable_editor("token".into(), master.clone());
        assert!(preview.set_editor_files(&[master.clone(), child.clone()], Some(0)));
        let editor = preview.editor.as_ref().unwrap();
        editor.buffers.lock().unwrap().insert(std::fs::canonicalize(&child).unwrap(), "New unsaved chapter".into());
        editor.revision.store(2, Ordering::SeqCst);
        assert!(!preview.set_editor_files(&[master.clone()], Some(1)));
        assert_eq!(preview.buffer_snapshot().unwrap().read(&child).unwrap(), "New unsaved chapter");
        assert!(preview.set_editor_files(&[master], Some(2)));
        assert!(preview.buffer_snapshot().unwrap().buffers.is_empty());
    }
}
