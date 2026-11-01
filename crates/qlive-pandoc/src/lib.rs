//! Warm pandoc: the whole point is to never pay process startup again.
//!
//! Measured on the development machine (pandoc 3.6.3, Windows 11):
//!
//! | path                                   | cost        |
//! |----------------------------------------|-------------|
//! | one-shot `pandoc.exe` (process start)  | ~550 ms     |
//! | `pandoc server` HTTP, one block        | 6–9 ms      |
//! | `pandoc lua` in-process, one block     | ~3.9 ms     |
//!
//! M0 uses [`PandocServer`], which is simple and already 60× better than a cold
//! process. It is sandboxed, though, so it cannot run Quarto's Lua filters — that is
//! why [`lua`] exists as the M1 target.

pub mod lua;

use anyhow::{anyhow, Result};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A long-lived `pandoc server` process.
pub struct PandocServer {
    child: Child,
    port: u16,
}

impl PandocServer {
    /// Spawn the server and wait until the port accepts connections.
    pub fn start(pandoc: &Path, port: u16) -> Result<Self> {
        let child = Command::new(pandoc)
            .arg("server")
            .arg("--port")
            .arg(port.to_string())
            .arg("--timeout")
            .arg("3600")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("could not start `pandoc server`: {e}"))?;

        let server = Self { child, port };
        server.wait_ready(Duration::from_secs(20))?;
        Ok(server)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn wait_ready(&self, budget: Duration) -> Result<()> {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(anyhow!(
            "pandoc server did not come up on port {} within {:?}",
            self.port,
            budget
        ))
    }

    /// Convert a fragment. `from`/`to` are pandoc format names.
    pub fn convert(&self, text: &str, from: &str, to: &str) -> Result<String> {
        let body = serde_json::json!({
            "text": text,
            "from": from,
            "to": to,
            "standalone": false,
        })
        .to_string();

        let response = ureq::post(&format!("http://127.0.0.1:{}/", self.port))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| anyhow!("pandoc server request failed: {e}"))?;

        Ok(response.into_string()?)
    }
}

impl Drop for PandocServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One-shot conversion through a fresh process. Only used by `qlive bench`, as the
/// baseline that everything else is compared against.
pub fn one_shot(pandoc: &Path, input: &Path, from: &str, to: &str) -> Result<String> {
    let out = Command::new(pandoc)
        .arg(input)
        .arg("-f")
        .arg(from)
        .arg("-t")
        .arg(to)
        .output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "pandoc failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
