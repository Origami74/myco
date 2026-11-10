// Run with node gamepad-probe.mjs [path to playwright/index.mjs].
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
const { chromium } = await import(process.argv[2] ? pathToFileURL(process.argv[2]).href : 'playwright');
const root = new URL('../../../', import.meta.url);
const bootstrap = readFileSync(new URL('android/app/src/debug/assets/gamepad-probe.js', root), 'utf8');
const shell = readFileSync(new URL('myco-napplet-runtime/assets/shell.html', root), 'utf8');
const server = createServer((req, res) => {
  res.setHeader('Content-Type', 'text/html');
  res.end('<script>' + bootstrap + '</script>' + shell);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:' + server.address().port);
  await page.waitForFunction(() => document.querySelector('pre')?.textContent.includes('"isolated": true'), null, { timeout: 5000 }).catch(async error => {
    console.error({ errors, report: await page.locator('body').innerText() });
    throw error;
  });
  const report = JSON.parse(await page.locator('pre').textContent());
  assert.equal(report.secureContext, true);
  assert.equal(report.shell.status, 'readable');
  assert.equal(report.child.native, 'SecurityError');
  assert.equal(report.child.nested, 'SecurityError');
  assert.equal(report.frameFocus, true);
  assert.deepEqual(errors, []);
  console.log('PASS: actual shell reads native API; sandboxed napplet receives SecurityError; frame has focus.');
} finally {
  await browser?.close();
  server.close();
}
