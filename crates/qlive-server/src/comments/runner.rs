use super::*;
#[cfg(windows)] use std::os::windows::process::CommandExt;
use std::{io::Read, process::{Command, Stdio}, time::{Duration, Instant}};

pub(super) struct Job { comment: Comment, folder: PathBuf, sources: Vec<Value>, _lease: File }

pub(super) fn mention(message: &str) -> Result<Option<String>> {
    let mut agents = Vec::new();
    for word in message.split_whitespace() {
        let word = word.trim_end_matches(|c: char| c.is_ascii_punctuation()).to_ascii_lowercase();
        let provider = match word.as_str() {
            "@agent" => std::env::var("QLIVE_DEFAULT_AGENT").unwrap_or_else(|_| "codex".into()),
            "@codex" => "codex".into(), "@claude" => "claude".into(), _ => continue,
        };
        if !["codex", "claude"].contains(&provider.as_str()) { bail!("Default agent must be codex or claude") }
        if !agents.contains(&provider) { agents.push(provider); }
    }
    if agents.len() > 1 { bail!("Mention one agent per comment") }
    Ok(agents.pop())
}

pub(super) fn prepare(state: &Preview, store: &Comments, comment: &Comment) -> Result<Job> {
    let folder = store.folder().join("requests").join(&comment.id);
    std::fs::create_dir_all(&folder)?;
    let lease = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(folder.join("active.lock"))?;
    lease.lock()?;
    let files: Vec<PathBuf> = state.source_manifest.read().map_err(|_| anyhow!("Source manifest unavailable"))?["files"]
        .as_array().ok_or_else(|| anyhow!("Preview sources unavailable"))?.iter().filter_map(|v| v.as_str().map(PathBuf::from)).collect();
    let snapshot = state.buffer_snapshot();
    let mut sources = Vec::new();
    let mut total = 0;
    for (i, file) in files.iter().enumerate() {
        let text = if let Some(snapshot) = &snapshot { snapshot.read(file)? } else { std::fs::read_to_string(file)? };
        total += text.len();
        if text.len() > 2 * 1024 * 1024 || total > 8 * 1024 * 1024 { bail!("Source snapshot is too large for a live agent request") }
        let draft = folder.join(format!("source-{i}.qmd"));
        atomic_write(&draft, text.as_bytes())?;
        sources.push(json!({"file":file,"draft":draft,"before":text,
            "unsaved":snapshot.as_ref().is_some_and(|s| std::fs::canonicalize(file).is_ok_and(|p| s.buffers.contains_key(&p)))}));
    }
    atomic_write(&folder.join("sources.json"), &serde_json::to_vec_pretty(&sources)?)?;
    atomic_write(&folder.join("request.json"), &serde_json::to_vec_pretty(comment)?)?;
    Ok(Job { comment: comment.clone(), folder, sources, _lease: lease })
}

fn schema() -> Value {
    json!({"type":"object","properties":{
        "summary":{"type":"string"},
        "edit":{"anyOf":[{"type":"null"},{"type":"object","properties":{
            "file":{"type":"string"},"new_text":{"type":"string"}},"required":["file","new_text"],"additionalProperties":false}]}},
        "required":["summary","edit"],"additionalProperties":false})
}

fn command(provider: &str, folder: &Path, root: &Path) -> Result<Command> {
    let variable = format!("QLIVE_{}_EXECUTABLE", provider.to_ascii_uppercase());
    let mut command = Command::new(std::env::var(variable).unwrap_or_else(|_| provider.into()));
    // Prefix arguments support CLI wrappers and deterministic local integration fixtures.
    if let Ok(prefix) = std::env::var(format!("QLIVE_{}_PREFIX_ARGS", provider.to_ascii_uppercase())) {
        command.args(serde_json::from_str::<Vec<String>>(&prefix).context("Invalid agent wrapper arguments")?);
    }
    if provider == "codex" {
        let schema_file = folder.join("schema.json");
        atomic_write(&schema_file, &serde_json::to_vec(&schema())?)?;
        command.args(["exec", "--json", "--sandbox", "read-only", "--skip-git-repo-check", "--output-schema"])
            .arg(schema_file).arg("--output-last-message").arg(folder.join("answer.json")).arg("-C").arg(root).arg("-");
    } else {
        command.args(["-p", "--output-format", "json", "--restricted", "--tools", "Read,Glob,Grep", "--no-session-persistence", "--json-schema"])
            .arg(schema().to_string());
    }
    command.current_dir(root).env_remove("QLIVE_EDITOR_TOKEN").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    Ok(command)
}

fn tail(mut input: impl Read) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(n) = input.read(&mut chunk) { if n == 0 { break; } out.extend_from_slice(&chunk[..n]); if out.len() > 4 * 1024 * 1024 { out.drain(..out.len() - 4 * 1024 * 1024); } }
    out
}

pub(super) fn work(store: &Comments, job: Job) {
    let result = execute(store, &job);
    if let Err(error) = result { let _ = store.change(&job.comment.id, "failed", &error.to_string()); }
}

fn execute(store: &Comments, job: &Job) -> Result<()> {
    let serial = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(store.folder().join("agent-run.lock"))?;
    serial.lock()?;
    if !store.active(&job.comment.id)? { return Ok(()); }
    let provider = job.comment.agent.as_deref().unwrap();
    store.change(&job.comment.id, "working", &format!("{provider} is inspecting the preview request"))?;
    let prompt = format!(
        "You are handling a user request attached to a live Quarto preview.\n\nRequest:\n{}\n\nSelected preview context:\n{}\n\n\
        The authoritative live source snapshots are listed in {}. Read those draft copies, which may contain unsaved editor text. \
        The on-disk project files may be older. Follow relevant project guidance.\n\n\
        Do not edit source files or run execution cells. Return the requested structured response. \
        For a question, answer in summary with edit=null. For a requested change, propose a minimal change to ONE listed source file, \
        returning its original file path and the complete new file text. Preserve all unrelated content. \
        The host will apply it to the live editor only if the original snapshot still matches.\n",
        job.comment.message, job.comment.anchor, job.folder.join("sources.json").display());
    let mut child = command(provider, &job.folder, &store.root)?.spawn().with_context(|| format!("Cannot start {provider}. Configure its executable and sign in using its CLI"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || tail(stdout));
    let err = std::thread::spawn(move || tail(stderr));
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? { break status; }
        if start.elapsed() > Duration::from_secs(900) || !store.active(&job.comment.id)? {
            #[cfg(windows)] {
                let _ = Command::new("taskkill.exe").args(["/PID", &child.id().to_string(), "/T", "/F"]).creation_flags(0x08000000).output();
            }
            let _ = child.kill(); let _ = child.wait();
            bail!("Agent run stopped or timed out; no source edit was applied");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = writer.join();
    let output = out.join().unwrap_or_default();
    let errors = err.join().unwrap_or_default();
    if !status.success() { bail!("{provider} failed: {}", String::from_utf8_lossy(&errors).chars().take(1800).collect::<String>()); }
    let answer: Value = if provider == "codex" {
        let file = job.folder.join("answer.json");
        if std::fs::metadata(&file)?.len() > 3 * 1024 * 1024 { bail!("Agent response was too large") }
        serde_json::from_slice(&std::fs::read(file)?)?
    } else {
        let envelope: Value = serde_json::from_slice(&output).context("Claude did not return JSON")?;
        if envelope["is_error"].as_bool() == Some(true) { bail!("Claude: {}", envelope["result"]); }
        envelope.get("structured_output").filter(|v| v.is_object()).cloned()
            .or_else(|| envelope["result"].as_str().and_then(|text| serde_json::from_str(text).ok()))
            .ok_or_else(|| anyhow!("Claude returned no structured response"))?
    };
    if !store.active(&job.comment.id)? { return Ok(()); }
    let summary = answer["summary"].as_str().ok_or_else(|| anyhow!("Agent response has no summary"))?;
    if answer["edit"].is_null() { store.change(&job.comment.id, "answered", summary)?; return Ok(()); }
    let file = answer["edit"]["file"].as_str().ok_or_else(|| anyhow!("Agent edit has no file"))?;
    let canonical = std::fs::canonicalize(file)?;
    let source = job.sources.iter().find(|s| s["file"].as_str().is_some_and(|path| std::fs::canonicalize(path).ok().as_ref() == Some(&canonical)))
        .ok_or_else(|| anyhow!("Agent proposed a file outside the preview sources"))?;
    let after = answer["edit"]["new_text"].as_str().ok_or_else(|| anyhow!("Agent edit has no new text"))?;
    if after.len() > 2 * 1024 * 1024 { bail!("Agent edit is too large") }
    let proposal = json!({"id":job.comment.id,"summary":summary,"file":source["file"],"before":source["before"],"after":after});
    atomic_write(&job.folder.join("proposal.json"), &serde_json::to_vec(&proposal)?)?;
    store.change(&job.comment.id, "ready", summary)?;
    println!("QLIVE_AGENT_EDITS {}", json!({"id":job.comment.id}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn tags_are_explicit_and_one_agent_per_request() {
        assert_eq!(mention("Explain this").unwrap(), None);
        assert_eq!(mention("Please @claude: fix the paragraph").unwrap().as_deref(), Some("claude"));
        assert_eq!(mention("@codex improve this").unwrap().as_deref(), Some("codex"));
        assert_eq!(mention("mail@claude.com").unwrap(), None);
        assert!(mention("@codex @claude change this").is_err());
    }
}
