// Integration checks for the actual shell + vendored prelude + Myco supplement.
// node gamepad.mjs [path to playwright/index.mjs]
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
const { chromium } = await import(process.argv[2] ? pathToFileURL(process.argv[2]).href : 'playwright');
const assets = new URL('../../assets/', import.meta.url);
const shell = readFileSync(new URL('shell.html', assets), 'utf8');
const vendor = readFileSync(new URL('vendor/napplet-shim-prelude.global.js', assets), 'utf8');
const supplement = readFileSync(new URL('myco-prelude.js', assets), 'utf8');
const artifact = '<!doctype html><body>Gamepad test<script>' + vendor + '\n' + supplement + `
NappletShimPrelude.install({ domains: ['gamepad'] });
MycoPrelude.install({ domains: ['shell', 'gamepad'] });
window.states = [];
window.defaults = { available: napplet.gamepad.available, focused: napplet.gamepad.focused,
  pads: napplet.gamepad.getGamepads() };
window.subscription = napplet.gamepad.onChange(state => states.push(state));
parent.postMessage({ type: 'shell.ready' }, '*');
<` + '/script>';
const server = createServer((req, res) => {
  res.setHeader('Content-Type', 'text/html');
  res.end(shell);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(({ artifact }) => {
    if (window !== window.top) return;
    window.testPads = [];
    window.testCalls = [];
    window.testReadError = null;
    Object.defineProperty(navigator, 'getGamepads', { configurable: true, value() {
      if (testReadError) throw new DOMException('test failure', testReadError);
      return testPads;
    } });
    function deliver(message) { window.mycoNappletRuntime.onmessage({ data: JSON.stringify(message) }); }
    window.deliver = deliver;
    window.mycoNappletRuntime = {
      postMessage(raw) {
        const frame = JSON.parse(raw);
        if (frame.channel === 'shell' && frame.action === 'mounted') {
          const load = () => {
            deliver({ channel: 'shell', action: 'load', sandbox: 'allow-scripts', artifact });
            window.dispatchEvent(new CustomEvent('myco-host-focus', { detail: true }));
          };
          if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', load, { once: true });
          else load();
        }
        if (frame.channel !== 'napplet') return;
        testCalls.push(frame.message);
        // The native capability gate is covered by gamepad.rs tests. This
        // harness supplies its authorized control command, never input data.
        if (frame.message.type === 'shell.ready') deliver({ channel: 'napplet', message: {
          type: 'shell.init', capabilities: { domains: ['shell', 'gamepad'] }, services: []
        } });
        if (frame.message.type === 'gamepad.subscribe') deliver({ channel: 'gamepad', enabled: true });
        if (frame.message.type === 'gamepad.unsubscribe') deliver({ channel: 'gamepad', enabled: false });
      }
    };
  }, { artifact });
  await page.goto('http://127.0.0.1:' + server.address().port);
  await page.waitForFunction(() => testCalls.some(call => call.type === 'gamepad.subscribe'));
  const child = page.frames().find(frame => frame !== page.mainFrame());
  await child.waitForFunction(() => states.length > 0);
  assert.deepEqual(await child.evaluate(() => defaults), { available: true, focused: false, pads: [] });
  assert.equal(await page.evaluate(() => testCalls.filter(call => call.type === 'gamepad.subscribe').length), 1);
  assert.equal(await child.evaluate(() => {
    try { navigator.getGamepads(); return 'readable'; } catch (error) { return error.name; }
  }), 'SecurityError');
  const pad = { index: 1, id: 'Test controller', mapping: 'standard', connected: true,
    timestamp: 42, axes: [0.75, -2, 2], buttons: [{ value: 2, pressed: true }] };
  await page.evaluate(pad => { testPads = [null, pad]; }, pad);
  await child.waitForFunction(() => napplet.gamepad.focused && napplet.gamepad.getGamepads()[1]?.timestamp === 42);
  const pads = await child.evaluate(() => napplet.gamepad.getGamepads());
  assert.equal(pads[0], null);
  assert.equal(pads[1].index, 1);
  assert.deepEqual(pads[1].axes, [0.75, -1, 1]);
  assert.deepEqual(pads[1].buttons, [{ value: 1, pressed: true, touched: true }]);
  assert.equal(await child.evaluate(() => {
    const pads = napplet.gamepad.getGamepads(); pads[1].axes[0] = 123;
    return napplet.gamepad.getGamepads()[1].axes[0];
  }), 0.75);
  // Repeated subscription must force a fresh snapshot even with no input change.
  const beforeRepeat = await child.evaluate(() => states.length);
  await child.evaluate(() => parent.postMessage({ type: 'gamepad.subscribe' }, '*'));
  await child.waitForFunction(count => states.length > count, beforeRepeat);
  await page.evaluate(() => dispatchEvent(new CustomEvent('myco-host-focus', { detail: false })));
  await child.waitForFunction(() => !napplet.gamepad.focused);
  const neutral = await child.evaluate(() => napplet.gamepad.getGamepads()[1]);
  assert.equal(neutral.timestamp, 0);
  assert.deepEqual(neutral.axes, [0, 0, 0]);
  assert.deepEqual(neutral.buttons, [{ value: 0, pressed: false, touched: false }]);
  const beforeIdle = await child.evaluate(() => states.length);
  await page.evaluate(() => { testPads[1].timestamp = 99; testPads[1].axes[0] = -0.5; });
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await child.evaluate(() => states.length), beforeIdle);
  // Connection metadata still changes while focus is elsewhere.
  await page.evaluate(() => { testPads = [null, null]; });
  await child.waitForFunction(() => napplet.gamepad.getGamepads()[1] === null);
  await page.evaluate(pad => {
    testPads = [null, pad];
    dispatchEvent(new CustomEvent('myco-host-focus', { detail: true }));
  }, pad);
  await child.waitForFunction(() => napplet.gamepad.focused);
  // Moving browser focus out of the napplet neutralizes without an Android signal.
  await page.evaluate(() => {
    const button = document.createElement('button');
    document.body.appendChild(button);
    button.focus();
  });
  await child.waitForFunction(() => !napplet.gamepad.focused);
  assert.equal(await child.evaluate(() => napplet.gamepad.getGamepads()[1].timestamp), 0);
  await page.evaluate(() => document.querySelector('iframe').focus());
  await child.waitForFunction(() => napplet.gamepad.focused);
  await page.evaluate(() => { testReadError = 'SecurityError'; });
  await child.waitForFunction(() => !napplet.gamepad.available);
  assert.deepEqual(await child.evaluate(() => states.at(-1)), {
    available: false, reason: 'blocked', focused: false, pads: []
  });
  await page.evaluate(() => { testReadError = 'NotSupportedError'; });
  await child.waitForFunction(() => states.at(-1).reason === 'unavailable');
  await page.evaluate(() => { testReadError = null; });
  await child.waitForFunction(() => napplet.gamepad.available);
  // A closed handler stops callbacks; synchronous reads keep the subscription.
  await child.evaluate(() => subscription.close());
  const beforeClose = await child.evaluate(() => states.length);
  await page.evaluate(() => { testPads[1].timestamp = 100; });
  await child.waitForFunction(() => napplet.gamepad.getGamepads()[1]?.timestamp === 100);
  assert.equal(await child.evaluate(() => states.length), beforeClose);
  await child.evaluate(() => parent.postMessage({ type: 'gamepad.unsubscribe' }, '*'));
  await page.waitForFunction(() => testCalls.some(call => call.type === 'gamepad.unsubscribe'));
  // A napplet cannot turn its postMessage into a trusted shell control frame.
  await child.evaluate(() => parent.postMessage({ channel: 'gamepad', enabled: true }, '*'));
  await page.evaluate(() => { testPads[1].timestamp = 200; });
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await child.evaluate(() => napplet.gamepad.getGamepads()[1].timestamp), 100);
  assert.deepEqual(errors, []);
  console.log('PASS: gamepad binding, handshake, snapshots, clamping, slots, focus neutralization, deduplication, connection changes, failures, teardown and control isolation.');
} finally {
  await browser?.close();
  server.close();
}
