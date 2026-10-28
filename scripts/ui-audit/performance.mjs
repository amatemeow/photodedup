// Times the interactions a person feels, against a big review. usage: node perf.mjs <url> <scheme>
import { chromium } from 'playwright-core';
const [url, scheme = 'light'] = process.argv.slice(2);
const base = url.split('?')[0], token = url.split('token=')[1];
const b = await chromium.launch({ channel: 'chrome', headless: true });
const ctx = await b.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: scheme });
const page = await ctx.newPage();
const ms = (t0) => `${(performance.now() - t0).toFixed(0)} ms`;
const row = (k, v) => console.log(`${k.padEnd(52)} ${v}`);

let t = performance.now();
await page.goto(`${base}/?token=${token}`);
await page.waitForSelector('text=Continue where you left off');
row('start screen ready (incl. last-scan lookup)', ms(t));

t = performance.now();
await page.getByRole('button', { name: /^Continue/ }).click();
await page.waitForSelector('.tile');
row('open the review: first tile in the DOM', ms(t));
await page.waitForFunction(() => { const im = [...document.querySelectorAll('.tile img')]; return im.length > 0 && im.every((i) => i.complete && i.naturalWidth); }, null, { timeout: 120000 });
row('  ... and every visible photo loaded (thumbnails made on first sight)', ms(t));

const counts = {};
const items = page.locator('.sidebar .item');
const n = await items.count();
let big = 0, bigN = 0;
for (let i = 0; i < n; i++) {
  const label = (await items.nth(i).locator('.label').textContent()).trim();
  const c = parseInt(await items.nth(i).locator('.count').textContent(), 10);
  counts[label] = c;
  if (c > bigN) { bigN = c; big = i; }
}
console.log('groups per category:', JSON.stringify(counts));

t = performance.now();
await items.nth(big).click();
await page.waitForFunction((k) => document.querySelectorAll('.tile').length >= Math.min(k, 8), bigN);
row(`switch to the biggest category (${bigN} groups): tiles in the DOM`, ms(t));
await page.waitForFunction(() => { const im = [...document.querySelectorAll('.tile img')].filter((i) => { const r = i.getBoundingClientRect(); return r.top < innerHeight && r.bottom > 0; }); return im.length > 0 && im.every((i) => i.complete && i.naturalWidth); }, null, { timeout: 180000 });
row('  ... and every photo on screen loaded', ms(t));
await page.waitForTimeout(500);
const dom = await page.evaluate(() => ({ nodes: document.querySelectorAll('*').length, tiles: document.querySelectorAll('.tile').length, heapMB: Math.round((performance.memory?.usedJSHeapSize ?? 0) / 1048576) }));
row('  DOM at that point', `${dom.nodes} nodes, ${dom.tiles} tiles, JS heap ${dom.heapMB} MB`);

// scroll the content area and watch frame times
const frames = await page.evaluate(async () => {
  const el = document.querySelector('.content');
  const times = [];
  let last = performance.now();
  await new Promise((res) => {
    let y = 0;
    const tick = (now) => {
      times.push(now - last); last = now;
      y += 60; el.scrollTop = y;
      if (y < el.scrollHeight - el.clientHeight && times.length < 400) requestAnimationFrame(tick); else res();
    };
    requestAnimationFrame(tick);
  });
  return times.slice(2);
});
frames.sort((a, b) => a - b);
const avg = frames.reduce((a, c) => a + c, 0) / frames.length;
row('scroll the whole grid (frames)', `${frames.length} frames, avg ${avg.toFixed(1)} ms, p95 ${frames[Math.floor(frames.length * 0.95)].toFixed(1)} ms, worst ${frames.at(-1).toFixed(0)} ms, >33ms: ${frames.filter((f) => f > 33).length}`);
await page.evaluate(() => (document.querySelector('.content').scrollTop = 0));

t = performance.now();
await page.locator('.tile').first().click();
await page.waitForSelector('article');
row('open a group: detail on screen', ms(t));
await page.waitForSelector('article .photo img.high.in', { timeout: 60000 });
row('  ... sharp preview loaded (first time: generated)', ms(t));

await page.waitForTimeout(3500); // time spent looking at the first group
t = performance.now();
await page.keyboard.press('ArrowRight');
await page.waitForSelector('article .photo img.high.in');
row('next group, after looking at the first for 3.5 s', ms(t));

// a decision: flip a quarantined photo to Keep (always allowed)
const quarantined = page.locator('article:has(button.aside[aria-pressed="true"])').first();
if (await quarantined.count()) {
  t = performance.now();
  await quarantined.locator('button.keep').click();
  await page.waitForSelector('article button.keep[aria-pressed="true"] >> nth=1');
  await page.waitForFunction(() => document.querySelectorAll('article button.keep[aria-pressed="true"]').length >= 2);
  row('click Keep -> screen updated', ms(t));
}

t = performance.now();
const before = await page.locator('.pos').textContent();
await page.keyboard.press('Enter');
await page.waitForFunction((b) => document.querySelector('.pos')?.textContent !== b, before);
row('Enter (accept + next) -> next group shown', ms(t));
await b.close();
