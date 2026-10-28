// Drives the real UI in headless Chrome and saves screenshots of every screen.
// usage: node shots.mjs <url-with-token> <library-path> <outdir> [tag]
import { chromium } from 'playwright-core';
import fs from 'node:fs';

const [url, lib, out, onlyScheme, onlySize, tag = ''] = process.argv.slice(2);
const sizes = { wide: [1440, 900], medium: [1024, 720], narrow: [720, 560] };
const base = url.split('?')[0];
const token = url.split('token=')[1];

const browser = await chromium.launch({ channel: 'chrome', headless: true });
const log = (m) => console.log(m);

async function run(scheme, sizeName) {
  const [w, h] = sizes[sizeName];
  const dir = `${out}/${scheme}-${sizeName}${tag}`;
  fs.mkdirSync(dir, { recursive: true });
  const ctx = await browser.newContext({ viewport: { width: w, height: h }, colorScheme: scheme, deviceScaleFactor: 1 });
  const page = await ctx.newPage();
  const shot = async (name) => { await page.waitForTimeout(350); await page.screenshot({ path: `${dir}/${name}.png` }); };
  const t0 = Date.now();
  await page.goto(`${base}/?token=${token}`);
  await page.waitForSelector('#root');
  await shot('01-start');

  await page.getByRole('button', { name: 'Options' }).click();
  await page.waitForSelector('text=Scan speed');
  await shot('02-options');
  await page.locator('summary').click();
  await page.waitForTimeout(200);
  await shot('03-options-advanced');
  await page.getByRole('button', { name: 'Back' }).click();

  await page.fill('#root', lib);
  await page.getByRole('button', { name: 'Scan' }).click();
  await page.waitForSelector('.tile', { timeout: 180000 });
  log(`${scheme}/${sizeName}: scan to review in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  await page.waitForSelector('.tile');
  await shot('04-review-overview');

  const tab = (i) => page.locator('.sidebar .item, .sidebar button.item').nth(i);
  const names = ['exact', 'true', 'sequences', 'lookalikes', 'orphans'];
  for (let i = 0; i < names.length; i++) {
    await tab(i).click();
    await page.waitForSelector('.tile, .empty');
    await shot(`05-tab-${names[i]}`);
  }

  await tab(1).click();
  await page.locator('.tile').first().click();
  await page.waitForSelector('article');
  await shot('06-detail-duplicates');
  await page.keyboard.press('Escape');

  await tab(2).click();
  if (await page.locator('.tile').count()) {
    await page.locator('.tile').first().click();
    await page.waitForSelector('article');
    await shot('07-detail-sequence');
    await page.keyboard.press(' '); // lightbox
    await shot('08-lightbox');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
  }

  await page.keyboard.press('?');
  await shot('09-help');
  await page.keyboard.press('Escape');

  // mark everything suggested in the duplicates tab, then open Apply
  await tab(1).click();
  await page.getByRole('button', { name: 'Accept all suggestions' }).click();
  await shot('10-accept-all-confirm');
  await page.getByRole('button', { name: 'Accept', exact: true }).click();
  await page.waitForTimeout(500);
  await shot('11-after-accept');
  await page.getByRole('button', { name: /^Apply/ }).click();
  await page.waitForSelector('text=Apply decisions');
  await shot('12-apply-preview');
  await ctx.close();
}

await run(onlyScheme, onlySize);
await browser.close();
log('done');
