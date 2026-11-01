const vscode = require('vscode');
const http = require('node:http');
const net = require('node:net');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
let session;
let sourcesView;
let extensionState;
const fileKey = file => process.platform === 'win32' ? file.toLowerCase().replace(/\\/g, '/') : file;

class SourcesView {
  constructor() { this.changed = new vscode.EventEmitter(); this.onDidChangeTreeData = this.changed.event; }
  update(s, manifest) { this.s = s; this.manifest = manifest; this.changed.fire(); }
  getChildren(item) {
    if (!this.s || session !== this.s || !this.manifest?.root) return [];
    if (!item) return [{ file: this.manifest.root }];
    return (this.manifest.edges || []).filter(edge => fileKey(edge.parent) === fileKey(item.file)).map(edge => ({ file: edge.file }));
  }
  getTreeItem(item) {
    const children = this.getChildren(item);
    const result = new vscode.TreeItem(vscode.Uri.file(item.file), children.length ? vscode.TreeItemCollapsibleState.Expanded : vscode.TreeItemCollapsibleState.None);
    result.command = { command: 'qlive.revealSource', title: 'Open source', arguments: [{ file: item.file, line: 0 }] };
    result.tooltip = item.file;
    return result;
  }
}

async function revealSource(s, source) {
  if (session !== s || typeof source.file !== 'string' || !s.files.has(fileKey(source.file))) return;
  const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(source.file));
  const editor = await vscode.window.showTextDocument(doc, { viewColumn: vscode.ViewColumn.One, preserveFocus: false });
  const line = Math.max(0, Math.min(Number(source.line) || 0, doc.lineCount - 1));
  editor.selection = new vscode.Selection(line, 0, line, 0);
  editor.revealRange(new vscode.Range(line, 0, line, 0), vscode.TextEditorRevealType.InCenterIfOutsideViewport);
}

function native(request, uri) {
  const executable = vscode.workspace.getConfiguration('qlive', uri).get('executable', 'qlive');
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['editor'], { windowsHide: true });
    let output = '', errors = '';
    const timer = setTimeout(() => { child.kill(); reject(new Error('Native editor operation timed out')); }, 10000);
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', code => {
      clearTimeout(timer);
      try { if (code !== 0) throw new Error(errors || `qlive exited (${code})`); resolve(JSON.parse(output)); }
      catch (error) { reject(error); }
    });
    child.stdin.on('error', () => {});
    child.stdin.end(JSON.stringify(request));
  });
}

async function navigate(direction) {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  const version = editor.document.version;
  const result = await native({ op: 'navigate', text: editor.document.getText(), line: editor.selection.active.line, direction }, editor.document.uri);
  if (result.line === null || editor.document.version !== version) return;
  editor.selection = new vscode.Selection(result.line, 0, result.line, 0);
  editor.revealRange(new vscode.Range(result.line, 0, result.line, 0));
}

async function ready(s) {
  if (s.connecting) return;
  s.connecting = true;
  for (let attempt = 0; attempt < 50 && session === s; attempt++) {
    const listening = await new Promise(resolve => {
      const socket = net.createConnection({ host: '127.0.0.1', port: s.port });
      socket.once('connect', () => { socket.destroy(); resolve(true); });
      socket.once('error', () => resolve(false));
      socket.setTimeout(500, () => { socket.destroy(); resolve(false); });
    });
    if (listening && session === s) {
      s.ready = true;
      void flush(s);
      showPreview(s);
      return;
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (session === s) {
    s.status.text = '$(warning) qlive: Could not connect';
    s.status.tooltip = 'The preview server did not become ready. See the qlive output channel.';
  }
}

function showPreview(s) {
  if (!s.ready || session !== s) return;
  if (s.panel) {
    s.panel.reveal(vscode.ViewColumn.Beside, true);
    return;
  }
  const panel = s.panel = vscode.window.createWebviewPanel('qlive.preview',
    `Preview — ${require('node:path').basename(s.doc.uri.fsPath)}`,
    { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
    { enableScripts: true, retainContextWhenHidden: true,
      portMapping: [{ webviewPort: s.port, extensionHostPort: s.port }] });
  const nonce = crypto.randomBytes(16).toString('hex');
  panel.webview.html = `<!doctype html><html><head>
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-src http://127.0.0.1:${s.port}; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'">
    <style>html,body,iframe{width:100%;height:100%;margin:0;padding:0;border:0;overflow:hidden}body{background:white}</style>
    </head><body><iframe id="preview" title="Quarto live preview" src="http://127.0.0.1:${s.port}/?qlive-view=${s.viewMode}"></iframe>
    <script nonce="${nonce}">
      const vscode = acquireVsCodeApi();
      const frame = document.getElementById('preview');
      let mode = '${s.viewMode}';
      frame.addEventListener('load', () => frame.contentWindow.postMessage({type:'qlive-view',mode}, '*'));
      window.addEventListener('message', event => {
        if (event.source === frame.contentWindow && event.data?.type === 'qlive-view-changed') {
          if (event.data.mode === 'paper' || event.data.mode === 'web') { mode = event.data.mode; vscode.postMessage({type:'view-mode',mode}); }
        } else if (event.data?.type === 'view-mode' && (event.data.mode === 'paper' || event.data.mode === 'web')) {
          mode = event.data.mode; frame.contentWindow.postMessage({type:'qlive-view',mode}, '*');
        }
      });
    </script></body></html>`;
  panel.webview.onDidReceiveMessage(message => {
    if (message.type === 'view-mode' && (message.mode === 'web' || message.mode === 'paper')) {
      s.viewMode = message.mode;
      void extensionState.update('qlive.viewMode', s.viewMode);
    }
  });
  panel.onDidDispose(() => {
    s.panel = undefined;
    if (session === s) stop();
  });
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });
}

function sendBuffer(s, buffer) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify(buffer);
    const request = http.request({ hostname: '127.0.0.1', port: s.port, path: '/qlive/buffer', method: 'POST',
      headers: { authorization: `Bearer ${s.token}`, 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) } }, response => {
      let data = '';
      response.setEncoding('utf8');
      response.on('data', chunk => { data += chunk; });
      response.on('end', () => {
        try { resolve({ code: response.statusCode, ...JSON.parse(data) }); }
        catch { reject(new Error(`qlive returned HTTP ${response.statusCode}`)); }
      });
    });
    request.setTimeout(35000, () => request.destroy(new Error('qlive buffer update timed out')));
    request.on('error', reject);
    request.end(body);
  });
}

function stop() {
  if (!session) return;
  const s = session;
  session = undefined;
  sourcesView?.changed.fire();
  void vscode.commands.executeCommand('setContext', 'qlive.previewRunning', false);
  clearTimeout(s.timer);
  if (s.panel) s.panel.dispose();
  if (s.child.pid && s.child.exitCode === null) {
    if (process.platform === 'win32') {
      // The daemon owns pandoc and (for executable documents) a kernel worker.
      const killer = spawn('taskkill.exe', ['/PID', String(s.child.pid), '/T', '/F'], { windowsHide: true });
      killer.on('error', () => s.child.kill());
    } else s.child.kill();
  }
  s.status.dispose();
}

async function flush(s) {
  if (session !== s || !s.ready || s.sending || s.pending.size === 0) return;
  const [key, buffer] = s.pending.entries().next().value;
  s.pending.delete(key);
  if (!s.files.has(key)) { if (s.pending.size) void flush(s); return; }
  s.sending = true;
  s.sent.set(key, buffer.version + ':' + buffer.dirty);
  try {
    const result = await sendBuffer(s, buffer);
    if (session !== s) return;
    if (result.code === 200 || result.revision > (s.savedRevision || 0)) {
      s.status.text = result.code === 200 ? '$(check) qlive: Live' : '$(info) qlive: Save needed';
      s.status.tooltip = result.message || 'Unsaved prose is live. Save to execute changed code or rebuild metadata.';
    }
    if (result.code !== 200) s.output.appendLine(result.message || `HTTP ${result.code}`);
  } catch (error) {
    if (session === s) {
      s.status.text = '$(warning) qlive: Update failed';
      s.status.tooltip = error.message;
      s.output.appendLine(error.message);
    }
  } finally {
    s.sending = false;
    if (session === s && s.pending.size) void flush(s);
  }
}

function schedule(doc, dirty = doc.isDirty) {
  const s = session;
  if (!s || doc.uri.scheme !== 'file' || !s.files.has(fileKey(doc.uri.fsPath))) return;
  s.pending.set(fileKey(doc.uri.fsPath), { path: doc.uri.fsPath, text: dirty ? doc.getText() : '', dirty, version: doc.version });
  clearTimeout(s.timer);
  const delay = vscode.workspace.getConfiguration('qlive', doc.uri).get('debounceMs', 100);
  s.timer = setTimeout(() => void flush(s), delay);
}

function activate(context) {
  extensionState = context.globalState;
  sourcesView = new SourcesView();
  context.subscriptions.push(sourcesView.changed, vscode.window.registerTreeDataProvider('qlive.includes', sourcesView));
  const quarto = vscode.extensions.getExtension('quarto.quarto');
  const integrated = Object.values(quarto?.packageJSON?.contributes?.menus || {}).some(entries => entries.some(entry => entry.command === 'qlive.start'));
  void vscode.commands.executeCommand('setContext', 'qlive.quartoPreviewIntegrated', integrated);
  const output = vscode.window.createOutputChannel('qlive');
  context.subscriptions.push(output);
  context.subscriptions.push(vscode.commands.registerCommand('qlive.start', async resource => {
    const doc = resource?.scheme === 'file' ? await vscode.workspace.openTextDocument(resource) : vscode.window.activeTextEditor?.document;
    if (!doc || doc.uri.scheme !== 'file' || !/\.(qmd|md)$/i.test(doc.uri.fsPath)) {
      return vscode.window.showErrorMessage('Open a saved .qmd or .md file first.');
    }
    if (!vscode.workspace.isTrusted) return;
    if (session?.files.has(fileKey(doc.uri.fsPath))) {
      schedule(doc);
      showPreview(session);
      return;
    }
    stop();
    try {
      await vscode.workspace.fs.stat(doc.uri);
      const port = await freePort();
      const token = crypto.randomBytes(32).toString('hex');
      const executable = vscode.workspace.getConfiguration('qlive', doc.uri).get('executable', 'qlive');
      const child = spawn(executable, ['live', doc.uri.fsPath, '--port', String(port)], {
        cwd: require('node:path').dirname(doc.uri.fsPath), env: { ...process.env, QLIVE_EDITOR_TOKEN: token }, windowsHide: true
      });
      const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 50);
      status.command = 'qlive.open';
      status.text = '$(sync~spin) qlive: Starting';
      status.show();
      const s = session = { doc, child, port, token, status, output, pending: new Map(), sent: new Map(), ready: false,
        files: new Set([fileKey(doc.uri.fsPath)]), lines: '', viewMode: extensionState.get('qlive.viewMode', 'web') === 'paper' ? 'paper' : 'web' };
      schedule(doc);
      void vscode.commands.executeCommand('setContext', 'qlive.previewRunning', true);
      let startup = '';
      child.stdout.on('data', chunk => {
        output.append(chunk.toString());
        s.lines += chunk.toString();
        let end;
        while ((end = s.lines.indexOf('\n')) >= 0) {
          const line = s.lines.slice(0, end).trim();
          s.lines = s.lines.slice(end + 1);
          try {
            if (line.startsWith('QLIVE_SOURCES ')) {
              const manifest = JSON.parse(line.slice('QLIVE_SOURCES '.length));
              s.files = new Set(manifest.files.map(fileKey));
              for (const key of s.sent.keys()) if (!s.files.has(key)) s.sent.delete(key);
              for (const key of s.pending.keys()) if (!s.files.has(key)) s.pending.delete(key);
              if (session === s) {
                sourcesView.update(s, manifest);
                for (const open of vscode.workspace.textDocuments) {
                  if (open.uri.scheme === 'file' && open.isDirty && s.files.has(fileKey(open.uri.fsPath))
                    && s.sent.get(fileKey(open.uri.fsPath)) !== open.version + ':true') schedule(open);
                }
              }
            } else if (line.startsWith('QLIVE_REVEAL ')) {
              void revealSource(s, JSON.parse(line.slice('QLIVE_REVEAL '.length))).catch(error => output.appendLine(error.message));
            } else if (line.startsWith('QLIVE_EDITOR_STATUS ') && session === s) {
              const saved = JSON.parse(line.slice('QLIVE_EDITOR_STATUS '.length));
              s.savedRevision = Math.max(s.savedRevision || 0, saved.revision);
              if (!s.pending.size) { s.status.text = '$(check) qlive: Live'; s.status.tooltip = saved.status; }
            }
          } catch (error) { output.appendLine(`Source mapping: ${error.message}`); }
        }
        startup = (startup + chunk.toString()).slice(-2000);
        if (!s.ready && startup.includes(`http://127.0.0.1:${port}/`)) {
          void ready(s);
        }
      });
      child.stderr.on('data', chunk => output.append(chunk.toString()));
      child.on('error', error => {
        output.appendLine(error.message);
        if (session === s) {
          stop();
          void vscode.window.showErrorMessage(`Cannot start qlive: ${error.message}. Set qlive.executable to the built executable.`);
        }
      });
      child.on('exit', code => {
        output.appendLine(`qlive exited (${code})`);
        if (session === s) stop();
      });
    } catch (error) { void vscode.window.showErrorMessage(`Cannot start qlive: ${error.message}`); }
  }));
  context.subscriptions.push(vscode.commands.registerCommand('qlive.stop', stop));
  context.subscriptions.push(vscode.commands.registerCommand('qlive.togglePaperView', () => {
    if (!session) return;
    session.viewMode = session.viewMode === 'paper' ? 'web' : 'paper';
    void extensionState.update('qlive.viewMode', session.viewMode);
    session.panel?.webview.postMessage({type: 'view-mode', mode: session.viewMode});
  }));
  context.subscriptions.push(vscode.commands.registerCommand('qlive.revealSource', source => session && revealSource(session, source)));
  const command = (name, action) => context.subscriptions.push(vscode.commands.registerCommand(name, async () => {
    try { await action(); } catch (error) { void vscode.window.showErrorMessage(`qlive: ${error.message}`); }
  }));
  command('qlive.goToNextCell', () => navigate('next'));
  command('qlive.goToPreviousCell', () => navigate('previous'));
  command('qlive.insertCodeCell', async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor) return;
    const version = editor.document.version;
    const edit = await native({ op: 'insert-cell', text: editor.document.getText(), line: editor.selection.active.line }, editor.document.uri);
    if (editor.document.version !== version) return;
    if (await editor.edit(builder => builder.insert(editor.document.positionAt(edit.offset), edit.text))) {
      const cursor = editor.document.positionAt(edit.cursorOffset);
      editor.selection = new vscode.Selection(cursor, cursor);
      editor.revealRange(new vscode.Range(cursor, cursor));
    }
  });
  command('qlive.newDocument', async () => {
    const result = await native({ op: 'new-document' });
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument({ language: 'quarto', content: result.text }));
  });
  context.subscriptions.push(vscode.commands.registerCommand('qlive.open', () => {
    if (session) showPreview(session);
  }));
  context.subscriptions.push(vscode.commands.registerCommand('qlive.openExternal', () => {
    if (session?.ready) return vscode.env.openExternal(vscode.Uri.parse(`http://127.0.0.1:${session.port}/?qlive-view=${session.viewMode}`));
  }));
  context.subscriptions.push(vscode.commands.registerCommand('qlive.refresh', () => {
    if (session) schedule(session.doc);
  }));
  context.subscriptions.push(vscode.workspace.onDidChangeTextDocument(event => schedule(event.document)));
  context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(doc => schedule(doc, false)));
  context.subscriptions.push(vscode.workspace.onDidCloseTextDocument(doc => {
    if (session?.doc.uri.toString() === doc.uri.toString()) stop();
    else schedule(doc, false);
  }));
  context.subscriptions.push({ dispose: stop });
}
module.exports = { activate, deactivate: stop };
