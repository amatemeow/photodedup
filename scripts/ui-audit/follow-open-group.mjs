// Checks that a group open in the detail view stays open while a Fine-tune
// slider moves (regrouping gives every group a new id).
// usage: node follow-open-group.mjs <url-with-token> <library-path>
import { chromium } from 'playwright-core';

const [url, lib] = process.argv.slice(2);
const base = url.split('?')[0];
const token = url.split('token=')[1];

const browser = await chromium.launch({ channel: 'chrome', headless: true });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
await page.goto(`${base}/?token=${token}`);
await page.waitForSelector('#root');
await page.fill('#root', lib);
await page.getByRole('button', { name: 'Scan' }).click();
await page.waitForSelector('.tile', { timeout: 180000 });

// Open the first group of the first category that has one.
const tabs = page.locator('.sidebar .item, .sidebar button.item');
let opened = false;
for (let i = 0; i < (await tabs.count()) && !opened; i++) {
  await tabs.nth(i).click();
  await page.waitForSelector('.tile, .empty');
  if ((await page.locator('.tile').count()) > 0) {
    await page.locator('.tile').first().click();
    await page.waitForSelector('article');
    opened = true;
  }
}
if (!opened) { console.log('FAIL: no group to open'); process.exit(2); }
const names = async () => (await page.locator('article img').evaluateAll((els) => els.map((e) => e.getAttribute('alt') ?? e.getAttribute('src') ?? ''))).filter(Boolean);
const before = await names();
let regroups = 0;
page.on('response', (r) => { if (r.request().method() === 'POST' && r.url().includes('/regroup')) regroups++; });

await page.getByRole('button', { name: /fine-tune/i }).click();
const slider = page.locator('aside.tuning input[type=range]').first();
await slider.waitFor();
await slider.focus();
const value = () => slider.evaluate((e) => e.value);
const v0 = await value();
for (let i = 0; i < 3; i++) {
  await slider.press('ArrowRight');
  await page.waitForTimeout(900); // each move saves, regroups and reloads
}
await page.waitForTimeout(1500);

const v1 = await value();
const stillOpen = (await page.locator('article').count()) > 0;
const after = stillOpen ? await names() : [];
const shared = after.filter((n) => before.includes(n)).length;
console.log(`slider ${v0} -> ${v1}, ${regroups} regroup request(s)`);
console.log(`before: ${before.length} photos; after the slider moved: detail ${stillOpen ? 'still open' : 'CLOSED'}, ${shared} of the same photos shown`);
await browser.close();
if (regroups === 0) { console.log('INCONCLUSIVE: the slider did not trigger a regroup'); process.exit(3); }
process.exit(stillOpen && shared > 0 ? 0 : 1);
