// Real daemon + Quarto + kernel checks. Run: node smoke.js <qlive.exe>
const { spawn } = require('node:child_process');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor(fn) {
  const until = Date.now() + 45000;
  while (Date.now() < until) {
    if (await fn()) return;
    await sleep(100);
  }
  throw new Error('Timed out waiting for preview');
}

async function main() {
  const binary = path.resolve(process.argv[2]);
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'qlive-buffer-smoke-'));
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  const token = crypto.randomBytes(32).toString('hex');
  const url = `http://127.0.0.1:${port}`;
  const doc = path.join(root, 'test.qmd');
  const original = '---\ntitle: Buffer test\n---\n\n## Heading\n\nSAVED MARKER\n';
  await fs.writeFile(doc, original);
  let child;
  let logs = '';
  const launch = () => {
    logs = '';
    child = spawn(binary, ['live', doc, '--port', String(port)], { env: { ...process.env, QLIVE_EDITOR_TOKEN: token }, windowsHide: true });
    child.stdout.on('data', data => { logs += data; });
    child.stderr.on('data', data => { logs += data; });
    child.on('error', error => { logs += error.message; });
  };
  const stop = async () => {
    if (!child || child.exitCode !== null) return;
    const done = new Promise(resolve => child.once('exit', resolve));
    if (process.platform === 'win32') {
      const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true });
      killer.on('error', () => child.kill());
    } else child.kill();
    await done;
  };
  const post = (text, auth = token, file = doc) => fetch(`${url}/qlive/buffer`, {
    method: 'POST', headers: { authorization: `Bearer ${auth}`, 'content-type': 'application/json' },
    body: JSON.stringify({ path: file, text }), signal: AbortSignal.timeout(35000)
  });
  try {
    launch();
    await waitFor(() => logs.includes(url));
    assert.equal((await post(original, 'wrong')).status, 401);
    const other = path.join(root, 'other.qmd');
    await fs.writeFile(other, original);
    assert.equal((await post(original, token, other)).status, 403);
    const edit = original.replace('SAVED MARKER', 'UNSAVED MARKER');
    assert.equal((await post(edit)).status, 200);
    assert.match(await (await fetch(url)).text(), /UNSAVED MARKER/);
    assert.equal(await fs.readFile(doc, 'utf8'), original);
    assert.equal((await post(edit.replace('Buffer test', 'Changed title'))).status, 409);
    const bursts = await Promise.all(Array.from({ length: 8 }, (_, i) => post(edit + `\nREVISION ${i}\n`)));
    assert.ok(bursts.every(response => response.status === 200));
    const revisions = await Promise.all(bursts.map(response => response.json()));
    const latest = revisions.reduce((best, response, i) => response.revision > revisions[best].revision ? i : best, 0);
    assert.ok((await (await fetch(url)).text()).includes(`REVISION ${latest}`));
    await stop();
    const source = '---\ntitle: Cells\n---\n\nSAVED PROSE\n\n```{python}\nx = 2\n```\n\n```{python}\nprint("dependent", x + 1)\n```\n\n```{python}\nprint("unrelated working")\n```\n';
    await fs.writeFile(doc, source);
    launch();
    await waitFor(() => logs.includes(url));
    assert.equal((await post(source.replace('SAVED PROSE', 'UNSAVED PROSE'))).status, 200);
    assert.match(await (await fetch(url)).text(), /UNSAVED PROSE/);
    assert.equal(await fs.readFile(doc, 'utf8'), source);
    assert.equal((await post(source.replace('x = 2', 'x = 3'))).status, 409);
    const broken = source.replace('x = 2', 'x = 1 / 0').replace('unrelated working', 'unrelated still working');
    await fs.writeFile(doc, broken);
    await waitFor(async () => (await (await fetch(url)).text()).includes('unrelated still working'));
    const html = await (await fetch(url)).text();
    assert.match(html, /ZeroDivisionError/);
    assert.match(html, /DependencyError/);
    assert.doesNotMatch(html, /dependent 3/);
    await fs.writeFile(doc, broken.replace('x = 1 / 0', 'x = 10'));
    await waitFor(async () => (await (await fetch(url)).text()).includes('dependent 11'));
    await stop();
    const chapters = path.join(root, 'chapters');
    await fs.mkdir(chapters);
    const one = path.join(chapters, 'one.qmd');
    const two = path.join(chapters, 'two.qmd');
    await fs.writeFile(one, '## Chapter\n\n```{python}\nx = 42\n```\n\n{{< include chapters/two.qmd >}}\n');
    await fs.writeFile(two, '## Nested\n\n```{python}\nprint("nested result", x)\n```\n');
    await fs.writeFile(doc, '---\ntitle: Master\n---\n\n{{< include chapters/one.qmd >}}\n');
    launch();
    await waitFor(() => logs.includes(url));
    const nestedHtml = await (await fetch(url)).text();
    assert.match(nestedHtml, /nested result 42/);
    assert.match(nestedHtml, /data-qlive-source="1"/);
    const manifest = JSON.parse(logs.split('\n').find(line => line.startsWith('QLIVE_SOURCES ')).slice('QLIVE_SOURCES '.length));
    assert.equal(manifest.files.length, 3);
    assert.equal(manifest.edges[1].parent, one);
    assert.equal(manifest.edges[1].file, two);
    assert.equal((await fetch(`${url}/qlive/reveal`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: 1 }) })).status, 200);
    await waitFor(() => logs.includes('QLIVE_REVEAL '));
    const location = JSON.parse(logs.split('\n').find(line => line.startsWith('QLIVE_REVEAL ')).slice('QLIVE_REVEAL '.length));
    assert.equal(location.file, two);
    assert.equal(location.line, 3);
    assert.equal((await fetch(`${url}/qlive/reveal`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: 99 }) })).status, 404);
    console.log('PASS: unsaved buffers, disk preservation, auth, target restriction, coalescing, cached cell outputs, isolated failure, blocked dependents, recovery');
    console.log('PASS: nested include tree and source IDs resolve to the producing chapter and line');
  } catch (error) { console.error(logs); throw error; }
  finally { await stop(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
