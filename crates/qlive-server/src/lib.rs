//! The preview server: static files from the cold render, plus a socket.
//!
//! M0 sends a bare `reload` over the socket and the client reloads while keeping its
//! scroll position. M2 replaces that message with block patches
//! (`{"op":"replace","id":"b17","html":"…"}`), at which point nothing reloads at all.
//! The transport does not change — only the payload.

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;

/// The client script, injected into every HTML response.
pub const CLIENT_JS: &str = include_str!("../assets/client.js");

/// Shared state: where the rendered output lives, and who to tell when it changes.
#[derive(Clone)]
pub struct Preview {
    root: PathBuf,
    entry: String,
    tx: broadcast::Sender<String>,
}

impl Preview {
    /// `root` is the directory holding the cold render, `entry` its file name.
    pub fn new(root: PathBuf, entry: String) -> Self {
        let (tx, _rx) = broadcast::channel(16);
        Self { root, entry, tx }
    }

    /// Tell every connected client to reload. Cheap and non-blocking; an error only
    /// means nobody is listening.
    pub fn notify_reload(&self) {
        let _ = self.tx.send("reload".to_string());
    }

    pub fn entry(&self) -> &str {
        &self.entry
    }
}

/// Serve until the process ends.
pub async fn serve(preview: Preview, port: u16) -> Result<()> {
    let state = Arc::new(preview);
    let app = Router::new()
        .route("/", get(index))
        .route("/qlive/client.js", get(client_js))
        .route("/qlive/ws", get(ws_upgrade))
        .route("/*path", get(file))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index(State(state): State<Arc<Preview>>) -> Response {
    let path = state.root.join(&state.entry);
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => Html(inject(&html)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot read {}: {e}", path.display()),
        )
            .into_response(),
    }
}

async fn client_js() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        CLIENT_JS,
    )
        .into_response()
}

async fn file(State(state): State<Arc<Preview>>, UrlPath(rel): UrlPath<String>) -> Response {
    let full = state.root.join(rel.trim_start_matches('/'));
    // Never serve outside the render directory.
    if !full.starts_with(&state.root) {
        return (StatusCode::FORBIDDEN, "outside the preview root").into_response();
    }
    match tokio::fs::read(&full).await {
        Ok(bytes) => {
            let mime = mime_of(&full);
            if mime == "text/html" {
                let html = String::from_utf8_lossy(&bytes).to_string();
                Html(inject(&html)).into_response()
            } else {
                ([(header::CONTENT_TYPE, mime)], bytes).into_response()
            }
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn ws_upgrade(State(state): State<Arc<Preview>>, upgrade: WebSocketUpgrade) -> Response {
    let rx = state.tx.subscribe();
    upgrade.on_upgrade(move |socket| pump(socket, rx))
}

async fn pump(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg)).await.is_err() {
            break;
        }
    }
}

/// Put the client script just before `</body>`.
fn inject(html: &str) -> String {
    const TAG: &str = "<script src=\"/qlive/client.js\" defer></script>";
    match html.rfind("</body>") {
        Some(at) => {
            let mut out = String::with_capacity(html.len() + TAG.len());
            out.push_str(&html[..at]);
            out.push_str(TAG);
            out.push_str(&html[at..]);
            out
        }
        None => format!("{html}{TAG}"),
    }
}

fn mime_of(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_before_body_close() {
        let out = inject("<html><body><p>hi</p></body></html>");
        assert!(out.contains("client.js"));
        assert!(out.find("client.js").unwrap() < out.find("</body>").unwrap());
    }

    #[test]
    fn injects_into_fragments_without_body() {
        assert!(inject("<p>hi</p>").contains("client.js"));
    }

    #[test]
    fn html_gets_html_mime() {
        assert_eq!(mime_of(Path::new("a/b.html")), "text/html");
        assert_eq!(mime_of(Path::new("a/b.woff2")), "font/woff2");
    }
}
