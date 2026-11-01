// Buffer-to-patch timings, excluding editor debounce and browser paint.
const { spawn } = require('node:child_process');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function main() {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'qlive-paragraph-bench-'));
  const doc = path.join(root, 'large.qmd');
  let source = '---\ntitle: Paragraph benchmark\nformat: html\n---\n\n';
  for (let i = 0; i < 300; i++) source += `## Section ${i}\n\nParagraph ${i} with **bold** words and ordinary text.\n\n`;
  await fs.writeFile(doc, source);
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  const token = crypto.randomBytes(32).toString('hex');
  const url = `http://127.0.0.1:${port}`;
  const child = spawn(path.resolve(process.argv[2]), ['live', doc, '--port', String(port)], {
    env: { ...process.env, QLIVE_EDITOR_TOKEN: token }, windowsHide: true
  });
  let logs = '';
  child.stdout.on('data', data => { logs += data; });
  child.stderr.on('data', data => { logs += data; });
  let ws;
  try {
    for (let i = 0; !logs.includes(url); i++) {
      if (i > 450 || child.exitCode !== null) throw new Error(logs || 'Daemon failed to start');
      await delay(100);
    }
    ws = new WebSocket(`${url.replace('http:', 'ws:')}/qlive/ws`);
    await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }); });
    const messages = [];
    ws.addEventListener('message', event => messages.push(JSON.parse(event.data)));
    const request = async text => {
      const response = await fetch(`${url}/qlive/buffer`, { method: 'POST',
        headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
        body: JSON.stringify({ path: doc, text }), signal: AbortSignal.timeout(35000) });
      assert.equal(response.status, 200);
      return response.json();
    };
    const times = [];
    for (let i = 0; i < 10; i++) {
      const start = performance.now();
      const result = await request(source.replace('Paragraph 150 with', `Edited paragraph ${i} with`));
      times.push(performance.now() - start);
      assert.equal(result.status, 'updated paragraph');
    }
    for (let i = 0; messages.length < 10 && i < 100; i++) await delay(10);
    assert.equal(messages.length, 10);
    assert.ok(messages.every(message => message.ops?.length === 1 && message.ops[0].op === 'replace'));
    const start = performance.now();
    const fallback = await request(source.replace('Section 150', 'Renamed section 150'));
    const fullMs = performance.now() - start;
    assert.equal(fallback.status, 'updated');
    for (let i = 0; messages.length < 11 && i < 100; i++) await delay(10);
    assert.equal(messages[10].op, 'body');
    assert.equal(await fs.readFile(doc, 'utf8'), source);
    times.sort((a, b) => a - b);
    const report = `# Paragraph preview benchmark\n\nDebug build; authored 300-section document; 10 unsaved edits.\n\n` +
      `- Buffer request median: ${times[5].toFixed(1)} ms\n- Buffer request p95 (10 samples): ${times[9].toFixed(1)} ms\n` +
      `- Structural edit/full-body request: ${fullMs.toFixed(1)} ms\n` +
      `- All 10 paragraph edits delivered one WebSocket block replacement each.\n` +
      `- The heading edit delivered a full body; the source file stayed unchanged.\n\n` +
      `These timings exclude the configurable 100 ms editor debounce and browser paint.\n` +
      `Run: node editors/vscode/benchmark.js target/debug/qlive.exe\n`;
    if (process.argv[3]) await fs.writeFile(process.argv[3], report);
    console.log(report);
  } catch (error) { console.error(logs); throw error; }
  finally {
    ws?.close();
    if (child.exitCode === null) {
      const done = new Promise(resolve => child.once('exit', resolve));
      if (process.platform === 'win32') spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true });
      else child.kill();
      await done;
    }
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
