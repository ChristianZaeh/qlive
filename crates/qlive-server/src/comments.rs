//! Project-local preview annotations and an agent-readable inbox.
use anyhow::{anyhow, bail, Context, Result};
use axum::{extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs::{File, OpenOptions}, io::Write, path::{Path, PathBuf}, sync::{mpsc, Arc}};
use crate::Preview;

mod runner;

#[derive(Clone)]
pub struct Comments {
    pub token: String,
    pub root: PathBuf,
    pub master: PathBuf,
    jobs: mpsc::Sender<runner::Job>,
    broadcast: tokio::sync::broadcast::Sender<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Inbox { #[serde(default = "version")] version: u32, #[serde(default)] comments: Vec<Comment> }
fn version() -> u32 { 1 }

#[derive(Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub client_id: String,
    pub master: PathBuf,
    pub created: u64,
    pub message: String,
    pub anchor: Value,
    pub resolved: bool,
    pub agent: Option<String>,
    pub run_status: String,
    pub reply: String,
}

pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow!("random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn valid_id(id: &str) -> bool { id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) }

impl Preview {
    pub fn enable_comments(&mut self, master: &Path) -> Result<()> {
        let master = std::path::absolute(master)?;
        let root = master.ancestors().skip(1).find(|dir| ["_quarto.yml", "_quarto.yaml", ".git"].iter().any(|name| dir.join(name).exists()))
            .unwrap_or(master.parent().unwrap()).to_path_buf();
        let (jobs, rx) = mpsc::channel();
        let comments = Comments { token: random_id()?, root, master, jobs, broadcast: self.tx.clone() };
        comments.access(|_| Ok(()))?;
        let worker = comments.clone();
        std::thread::spawn(move || { for job in rx { runner::work(&worker, job); } });
        println!("QLIVE_INBOX {}", json!({"path": comments.root.join(".qlive/agent-inbox.md")}));
        self.comments = Some(comments);
        Ok(())
    }
}

impl Comments {
    pub(super) fn folder(&self) -> PathBuf { self.root.join(".qlive") }
    fn notify(&self) { let _ = self.broadcast.send(json!({"op":"comments"}).to_string()); }

    fn access<T>(&self, action: impl FnOnce(&mut Inbox) -> Result<T>) -> Result<T> {
        let folder = self.folder();
        std::fs::create_dir_all(&folder)?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(folder.join("inbox.lock"))?;
        lock.lock()?;
        let path = folder.join("agent-inbox.json");
        let before = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes), Err(e) if e.kind() == std::io::ErrorKind::NotFound => None, Err(e) => return Err(e.into()),
        };
        let mut inbox: Inbox = match &before { Some(bytes) => serde_json::from_slice(bytes).context("Cannot parse agent inbox; existing file was preserved")?, None => Inbox { version: 1, comments: Vec::new() } };
        if inbox.version != 1 { bail!("Unsupported inbox version") }
        let result = action(&mut inbox)?;
        let bytes = serde_json::to_vec_pretty(&inbox)?;
        if before.as_deref() != Some(&bytes) { atomic_write(&path, &bytes)?; }
        let markdown = markdown(&inbox);
        let md = folder.join("agent-inbox.md");
        if std::fs::read_to_string(&md).ok().as_deref() != Some(&markdown) { atomic_write(&md, markdown.as_bytes())?; }
        Ok(result)
    }

    pub(super) fn change(&self, id: &str, status: &str, reply: &str) -> Result<()> {
        self.access(|inbox| {
            let comment = inbox.comments.iter_mut().find(|c| c.id == id).ok_or_else(|| anyhow!("Comment not found"))?;
            if !comment.resolved { comment.run_status = status.into(); comment.reply = reply.into(); }
            Ok(())
        })?;
        self.notify();
        Ok(())
    }

    pub(super) fn active(&self, id: &str) -> Result<bool> {
        self.access(|inbox| Ok(inbox.comments.iter().any(|c| c.id == id && !c.resolved && ["queued", "working", "ready"].contains(&c.run_status.as_str()))))
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", random_id()?));
    let mut file = OpenOptions::new().create_new(true).write(true).open(&temporary)?;
    let result = (|| -> Result<()> { file.write_all(bytes)?; file.sync_all()?; drop(file); std::fs::rename(&temporary, path)?; Ok(()) })();
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result
}

fn markdown(inbox: &Inbox) -> String {
    let mut out = "# qlive agent inbox\n\nCanonical data: `agent-inbox.json`. Requests originate in the preview.\n\n".to_owned();
    for comment in &inbox.comments {
        out.push_str(&format!("## {} — {}\n\nDocument: {}\n\n", comment.id, if comment.resolved { "resolved" } else { &comment.run_status }, comment.master.display()));
        if let Some(source) = comment.anchor.get("source") {
            out.push_str(&format!("Source: {}:{}{}\n\n", source["file"].as_str().unwrap_or(""), source["line"].as_u64().unwrap_or(0) + 1,
                if source["unsaved"].as_bool() == Some(true) { " (unsaved editor text)" } else { "" }));
        }
        out.push_str("Preview selection:\n\n");
        for line in comment.anchor["quote"].as_str().unwrap_or("").lines() { out.push_str(&format!("> {line}\n")); }
        out.push_str("\nRequest:\n\n");
        for line in comment.message.lines() { out.push_str(&format!("> {line}\n")); }
        if !comment.reply.is_empty() { out.push_str("\nAgent response:\n\n"); for line in comment.reply.lines() { out.push_str(&format!("> {line}\n")); } }
        out.push('\n');
    }
    out
}

fn authorized(headers: &HeaderMap, comments: &Comments) -> bool {
    headers.get("x-qlive-comment-token").and_then(|v| v.to_str().ok()) == Some(&comments.token)
}
fn failure(error: impl std::fmt::Display) -> Response { (StatusCode::BAD_REQUEST, Json(json!({"error": error.to_string()}))).into_response() }

pub(crate) async fn list(State(state): State<Arc<Preview>>, headers: HeaderMap) -> Response {
    let Some(comments) = state.comments.clone() else { return StatusCode::NOT_FOUND.into_response() };
    if !authorized(&headers, &comments) { return StatusCode::UNAUTHORIZED.into_response(); }
    let sources = state.sources.read().map(|s| s.clone()).unwrap_or_default();
    let master = comments.master.clone();
    let result = tokio::task::spawn_blocking(move || comments.access(|inbox| {
        for comment in inbox.comments.iter_mut().filter(|c| c.master == master && !c.resolved) {
            if ["queued", "working"].contains(&comment.run_status.as_str()) {
                let path = comments.folder().join("requests").join(&comment.id).join("active.lock");
                if let Ok(lease) = OpenOptions::new().read(true).write(true).open(path) {
                    if lease.try_lock().is_ok() { comment.run_status = "interrupted".into(); comment.reply = "Preview stopped before this agent finished. Submit a new request to retry.".into(); }
                }
            }
            if comment.run_status == "ready" { println!("QLIVE_AGENT_EDITS {}", json!({"id":comment.id})); }
        }
        Ok(json!({"comments": inbox.comments.iter().filter(|c| c.master == master).collect::<Vec<_>>(), "sources": sources}))
    })).await;
    match result { Ok(Ok(value)) => Json(value).into_response(), Ok(Err(e)) => failure(e), Err(e) => failure(e) }
}

pub(crate) async fn submit(State(state): State<Arc<Preview>>, headers: HeaderMap, Json(value): Json<Value>) -> Response {
    let Some(comments) = state.comments.clone() else { return StatusCode::NOT_FOUND.into_response() };
    if !authorized(&headers, &comments) { return StatusCode::UNAUTHORIZED.into_response(); }
    let result = tokio::task::spawn_blocking(move || create(&state, &comments, value)).await;
    match result { Ok(Ok(value)) => Json(value).into_response(), Ok(Err(e)) => failure(e), Err(e) => failure(e) }
}

fn create(state: &Preview, comments: &Comments, value: Value) -> Result<Value> {
    if value["action"].as_str() == Some("resolve") {
        let id = value["id"].as_str().ok_or_else(|| anyhow!("Missing comment id"))?;
        comments.access(|inbox| {
            let comment = inbox.comments.iter_mut().find(|c| c.id == id && c.master == comments.master).ok_or_else(|| anyhow!("Comment not found"))?;
            comment.resolved = value["resolved"].as_bool().unwrap_or(true);
            if comment.resolved && ["queued", "working", "ready"].contains(&comment.run_status.as_str()) { comment.run_status = "cancelled".into(); }
            Ok(())
        })?;
        comments.notify();
        return Ok(json!({"status":"updated"}));
    }
    let message = value["message"].as_str().unwrap_or("").trim();
    if message.is_empty() || message.len() > 16000 { bail!("Enter a comment of at most 16,000 bytes") }
    let quote = value["anchor"]["quote"].as_str().unwrap_or("").trim();
    if quote.is_empty() || quote.len() > 4000 { bail!("Choose a preview location") }
    let tag = value["anchor"]["tag"].as_str().unwrap_or("p");
    if !["p","pre","h1","h2","h3","h4","h5","h6","table","figure","img","svg","li","blockquote","section"].contains(&tag) { bail!("Unsupported preview location") }
    let mut anchor = json!({"quote":quote,"tag":tag,"occurrence":value["anchor"]["occurrence"].as_u64().unwrap_or(0)});
    if let Some(id) = value["anchor"]["source_id"].as_u64() {
        let source = state.sources.read().map_err(|_| anyhow!("Source map unavailable"))?.get(id as usize).cloned().ok_or_else(|| anyhow!("Preview changed; click the location again"))?;
        if value["anchor"]["source_key"].as_str() != source["key"].as_str() { bail!("Preview changed; click the location again") }
        let path = PathBuf::from(source["file"].as_str().ok_or_else(|| anyhow!("Source path missing"))?);
        let snapshot = state.buffer_snapshot();
        let text = if let Some(snapshot) = &snapshot { snapshot.read(&path)? } else { std::fs::read_to_string(&path)? };
        let line = source["line"].as_u64().unwrap_or(0) as usize;
        let mut source = source;
        source["context"] = text.lines().skip(line.saturating_sub(3)).take(12).collect::<Vec<_>>().join("\n").chars().take(8000).collect::<String>().into();
        source["unsaved"] = snapshot.as_ref().is_some_and(|s| std::fs::canonicalize(path).is_ok_and(|p| s.buffers.contains_key(&p))).into();
        anchor["source"] = source;
    }
    let client_id = value["client_id"].as_str().filter(|s| s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).ok_or_else(|| anyhow!("Missing request identity"))?;
    if let Some(existing) = comments.access(|inbox| Ok(inbox.comments.iter().find(|c| c.master == comments.master && c.client_id == client_id).cloned()))? { return Ok(json!({"comment":existing})); }
    let agent = runner::mention(message)?;
    if agent.is_some() && state.editor.is_none() { bail!("Open this preview through VS Code to apply live agent edits") }
    let comment = Comment { id: random_id()?, client_id: client_id.into(), master: comments.master.clone(), created: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        message: message.into(), anchor, resolved: false, agent: agent.clone(), run_status: if agent.is_some() { "queued" } else { "note" }.into(), reply: String::new() };
    let job = if agent.is_some() { Some(runner::prepare(state, comments, &comment)?) } else { None };
    let inserted = comments.access(|inbox| {
        if let Some(existing) = inbox.comments.iter().find(|c| c.client_id == comment.client_id && c.master == comment.master) { return Ok(Some(existing.clone())); }
        inbox.comments.push(comment.clone()); Ok(None)
    })?;
    if let Some(existing) = inserted { return Ok(json!({"comment":existing})); }
    if let Some(job) = job { comments.jobs.send(job).map_err(|_| anyhow!("Agent worker unavailable"))?; }
    comments.notify();
    Ok(json!({"comment":comment}))
}

pub(crate) async fn proposal(State(state): State<Arc<Preview>>, headers: HeaderMap, Query(query): Query<std::collections::HashMap<String,String>>) -> Response {
    if !state.editor_authorized(&headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    let Some(store) = &state.comments else { return StatusCode::NOT_FOUND.into_response() };
    let id = query.get("id").map(String::as_str).unwrap_or("");
    if !valid_id(id) { return StatusCode::BAD_REQUEST.into_response(); }
    let result = (|| -> Result<Value> {
        if !store.active(id)? { bail!("Request is resolved") }
        let comment = store.access(|inbox| Ok(inbox.comments.iter().find(|c| c.id == id && c.master == store.master).cloned()))?.ok_or_else(|| anyhow!("Request missing"))?;
        if comment.run_status != "ready" { bail!("No live edit ready") }
        Ok(serde_json::from_slice(&std::fs::read(store.folder().join("requests").join(id).join("proposal.json"))?)?)
    })();
    match result { Ok(value) => Json(value).into_response(), Err(e) => failure(e) }
}

pub(crate) async fn acknowledge(State(state): State<Arc<Preview>>, headers: HeaderMap, Json(value): Json<Value>) -> Response {
    if !state.editor_authorized(&headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    let Some(store) = &state.comments else { return StatusCode::NOT_FOUND.into_response() };
    let id = value["id"].as_str().unwrap_or("");
    let status = value["status"].as_str().unwrap_or("");
    if !valid_id(id) || !["applied","conflict","failed"].contains(&status) { return StatusCode::BAD_REQUEST.into_response(); }
    match store.change(id, status, value["message"].as_str().unwrap_or("")) { Ok(()) => Json(json!({"status":status})).into_response(), Err(e) => failure(e) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notes_persist_deduplicate_resolve_and_preserve_invalid_inbox() {
        let root = std::env::temp_dir().join(format!("qlive-comments-{}", random_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let master = root.join("test.qmd"); std::fs::write(&master, "A paragraph").unwrap();
        let mut preview = Preview::new(root.clone(), "test.html".into());
        preview.enable_comments(&master).unwrap();
        let store = preview.comments.as_ref().unwrap();
        let request = json!({"client_id":"fixture","message":"A note <script>","anchor":{"quote":"A paragraph","tag":"p"}});
        let first = create(&preview, store, request.clone()).unwrap();
        assert_eq!(create(&preview, store, request).unwrap()["comment"]["id"], first["comment"]["id"]);
        assert_eq!(store.access(|inbox| Ok(inbox.comments.len())).unwrap(), 1);
        create(&preview, store, json!({"action":"resolve","id":first["comment"]["id"],"resolved":true})).unwrap();
        assert!(store.access(|inbox| Ok(inbox.comments[0].resolved)).unwrap());
        assert!(std::fs::read_to_string(store.folder().join("agent-inbox.md")).unwrap().contains("A note <script>"));
        let invalid = b"invalid inbox";
        std::fs::write(store.folder().join("agent-inbox.json"), invalid).unwrap();
        assert!(store.access(|_| Ok(())).is_err());
        assert_eq!(std::fs::read(store.folder().join("agent-inbox.json")).unwrap(), invalid);
        std::fs::remove_dir_all(root).unwrap();
    }
}
