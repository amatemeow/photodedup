// Runs axe-core (WCAG 2.0/2.1/2.2 A and AA) on every screen and state.
// usage: node axe.mjs <url> <lib> <scheme> <w> <h>
import { chromium } from 'playwright-core';
import fs from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const axeSource = fs.readFileSync(require.resolve('axe-core/axe.min.js'), 'utf8');

const [url, lib, scheme, w, h] = process.argv.slice(2);
const base = url.split('?')[0], token = url.split('token=')[1];
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const ctx = await browser.newContext({ viewport: { width: +w, height: +h }, colorScheme: scheme });
const page = await ctx.newPage();
const seen = new Map();

async function audit(name) {
  await page.waitForTimeout(400);
  await page.evaluate(axeSource);
  const r = await page.evaluate(() => axe.run(document, { runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'] } }));
  for (const v of r.violations) {
    const key = `${v.id}`;
    const entry = seen.get(key) ?? { impact: v.impact, help: v.help, where: new Set(), sample: '' };
    entry.where.add(name);
    if (!entry.sample) entry.sample = v.nodes[0].html.slice(0, 140) + '  ::  ' + (v.nodes[0].any[0]?.message ?? v.nodes[0].failureSummary ?? '').slice(0, 160);
    seen.set(key, entry);
  }
  console.log(`${name.padEnd(28)} ${r.violations.length ? r.violations.map((v) => `${v.id}×${v.nodes.length}`).join(', ') : 'no violations'}  (${r.passes.length} rules passed)`);
}

await page.goto(`${base}/?token=${token}`);
await page.waitForSelector('#root');
await audit('start');
await page.getByRole('button', { name: 'Settings' }).click();
await page.waitForSelector('text=Scan speed');
await audit('options');
await page.locator('summary').click();
await audit('options (advanced open)');
await page.getByRole('button', { name: 'Back' }).click();
await page.fill('#root', lib);
await page.getByRole('button', { name: 'Scan', exact: true }).click();
await page.waitForSelector('.tile', { timeout: 180000 });
const tab = (i) => page.locator('.sidebar .item').nth(i);
const names = ['exact', 'true', 'sequences', 'look-alikes', 'orphans'];
for (let i = 0; i < 5; i++) { await tab(i).click(); await page.waitForSelector('.tile, .empty'); await audit(`overview: ${names[i]}`); }
await tab(1).click();
await page.locator('.tile').first().click(); await page.waitForSelector('article'); await audit('detail: duplicates');
await page.keyboard.press('Escape');
await tab(2).click(); await page.locator('.tile').first().click(); await page.waitForSelector('article'); await audit('detail: sequence');
await page.keyboard.press(' '); await audit('lightbox');
await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
await page.keyboard.press('?'); await audit('help sheet'); await page.keyboard.press('Escape');
await tab(1).click();
await page.getByRole('button', { name: 'Accept all suggestions' }).click(); await audit('accept-all sheet');
await page.getByRole('button', { name: 'Accept', exact: true }).click(); await page.waitForTimeout(400);
await audit('overview: accepted');
await page.getByRole('button', { name: /^Apply/ }).click(); await page.waitForSelector('text=Apply decisions'); await audit('apply sheet');
await browser.close();

console.log('\n=== distinct violations ===');
for (const [id, e] of seen) console.log(`\n[${e.impact}] ${id}: ${e.help}\n  on: ${[...e.where].join('; ')}\n  e.g. ${e.sample}`);
if (!seen.size) console.log('none');
