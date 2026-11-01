const vscode = require('vscode');
const http = require('node:http');
const net = require('node:net');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
let session;

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

function sendBuffer(s, text) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({ path: s.doc.uri.fsPath, text });
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
  clearTimeout(s.timer);
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
  if (session !== s || !s.ready || s.sending || s.pending === undefined) return;
  const text = s.pending;
  s.pending = undefined;
  s.sending = true;
  try {
    const result = await sendBuffer(s, text);
    if (session !== s) return;
    s.status.text = result.code === 200 ? '$(check) qlive: Live' : '$(info) qlive: Save needed';
    s.status.tooltip = result.message || 'Unsaved prose is live. Save to execute changed code or rebuild metadata.';
    if (result.code !== 200) s.output.appendLine(result.message || `HTTP ${result.code}`);
  } catch (error) {
    if (session === s) {
      s.status.text = '$(warning) qlive: Update failed';
      s.status.tooltip = error.message;
      s.output.appendLine(error.message);
    }
  } finally {
    s.sending = false;
    if (session === s && s.pending !== undefined) void flush(s);
  }
}

function schedule(doc) {
  const s = session;
  if (!s || doc.uri.toString() !== s.doc.uri.toString()) return;
  s.pending = doc.getText();
  clearTimeout(s.timer);
  const delay = vscode.workspace.getConfiguration('qlive', doc.uri).get('debounceMs', 100);
  s.timer = setTimeout(() => void flush(s), delay);
}

function activate(context) {
  const output = vscode.window.createOutputChannel('qlive');
  context.subscriptions.push(output);
  context.subscriptions.push(vscode.commands.registerCommand('qlive.start', async () => {
    const doc = vscode.window.activeTextEditor?.document;
    if (!doc || doc.uri.scheme !== 'file' || !/\.(qmd|md)$/i.test(doc.uri.fsPath)) {
      return vscode.window.showErrorMessage('Open a saved .qmd or .md file first.');
    }
    if (!vscode.workspace.isTrusted) return;
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
      const s = session = { doc, child, port, token, status, output, pending: doc.getText(), ready: false };
      let startup = '';
      child.stdout.on('data', chunk => {
        output.append(chunk.toString());
        startup = (startup + chunk.toString()).slice(-2000);
        if (!s.ready && startup.includes(`http://127.0.0.1:${port}/`)) {
          s.ready = true;
          if (session === s) {
            void flush(s);
            void vscode.env.openExternal(vscode.Uri.parse(`http://127.0.0.1:${port}/`));
          }
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
  context.subscriptions.push(vscode.commands.registerCommand('qlive.open', () => {
    if (session?.ready) return vscode.env.openExternal(vscode.Uri.parse(`http://127.0.0.1:${session.port}/`));
  }));
  context.subscriptions.push(vscode.workspace.onDidChangeTextDocument(event => schedule(event.document)));
  context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(schedule));
  context.subscriptions.push(vscode.workspace.onDidCloseTextDocument(doc => {
    if (session?.doc.uri.toString() === doc.uri.toString()) stop();
  }));
  context.subscriptions.push({ dispose: stop });
}
module.exports = { activate, deactivate: stop };
