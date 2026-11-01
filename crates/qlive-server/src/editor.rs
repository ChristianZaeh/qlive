//! Authenticated, local-only editor buffer input. Source files are never written.
use axum::{extract::State, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::{mpsc, Arc, atomic::{AtomicU64, Ordering}}};
use tokio::sync::oneshot;
use crate::Preview;

pub struct BufferUpdate {
    pub text: String,
    pub revision: u64,
    pub reply: oneshot::Sender<Result<String, String>>,
}

#[derive(Clone)]
pub(crate) struct Editor {
    token: String,
    target: PathBuf,
    tx: mpsc::Sender<BufferUpdate>,
    revision: Arc<AtomicU64>,
}

impl Preview {
    pub fn enable_editor(&mut self, token: String, target: PathBuf) -> mpsc::Receiver<BufferUpdate> {
        let (tx, rx) = mpsc::channel();
        self.editor = Some(Editor { token, target, tx, revision: Arc::new(AtomicU64::new(0)) });
        rx
    }

    pub fn is_current_buffer(&self, revision: u64) -> bool {
        self.editor.as_ref().is_some_and(|e| e.revision.load(Ordering::SeqCst) == revision)
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
    if std::fs::canonicalize(path).ok() != std::fs::canonicalize(&editor.target).ok() || !editor.target.is_file() {
        return (StatusCode::FORBIDDEN, "buffer does not belong to this preview").into_response();
    }
    let revision = editor.revision.fetch_add(1, Ordering::SeqCst) + 1;
    let (reply, response) = oneshot::channel();
    if editor.tx.send(BufferUpdate { text: text.to_owned(), revision, reply }).is_err() {
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
