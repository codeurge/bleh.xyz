// Minimal Chrome DevTools Protocol driver. No npm dependencies (Node 22+ has WebSocket built in).
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CANDIDATES = [
  process.env.CHROME_PATH,
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser',
].filter(Boolean);

export async function launch() {
  const bin = CANDIDATES.find(existsSync);
  if (!bin) throw new Error('No Chrome or Chromium found. Set CHROME_PATH.');
  const dir = mkdtempSync(join(tmpdir(), 'bleh-chrome-'));
  const proc = spawn(bin, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${dir}`,
    '--hide-scrollbars', '--no-first-run', '--no-default-browser-check', '--force-color-profile=srgb',
    '--disable-background-timer-throttling', '--allow-file-access-from-files', 'about:blank'], { stdio: 'ignore' });
  const portFile = join(dir, 'DevToolsActivePort');
  for (let i = 0; i < 200 && !existsSync(portFile); i++) await new Promise(r => setTimeout(r, 50));
  const port = readFileSync(portFile, 'utf8').split('\n')[0];
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const ws = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let id = 0; const pending = new Map(); const waiters = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) { const p = pending.get(d.id); pending.delete(d.id); d.error ? p.rej(new Error(d.error.message)) : p.res(d.result); }
    else if (d.method) for (let i = waiters.length - 1; i >= 0; i--) if (waiters[i].method === d.method) waiters.splice(i, 1)[0].res(d.params);
  };
  const send = (method, params = {}) => new Promise((res, rej) => { pending.set(++id, { res, rej }); ws.send(JSON.stringify({ id, method, params })); });
  const once = (method) => new Promise(res => waiters.push({ method, res }));
  await send('Page.enable'); await send('Runtime.enable');
  const page = {
    send,
    async viewport(width, height, scale = 1) { await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: scale, mobile: false }); },
    async scheme(v) { await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: v }] }); },
    async goto(url) { const l = once('Page.loadEventFired'); await send('Page.navigate', { url }); await l; await page.eval('document.fonts.ready.then(()=>1)'); },
    async eval(expression) {
      const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      return r.result.value;
    },
    async png(opts = {}) { return Buffer.from((await send('Page.captureScreenshot', { format: 'png', ...opts })).data, 'base64'); },
    async close() { try { ws.close(); } catch {} proc.kill(); await new Promise(r => setTimeout(r, 200)); try { rmSync(dir, { recursive: true, force: true }); } catch {} },
  };
  return page;
}
