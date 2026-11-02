// Continues a scanned library, shows the Possible defects tab and saves a screenshot.
// usage: node defects-tab.mjs <url-with-token> <out.png>
import { chromium } from 'playwright-core';

const [url, out] = process.argv.slice(2);
const base = url.split('?')[0];
const token = url.split('token=')[1];

const browser = await chromium.launch({ channel: 'chrome', headless: true });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
await page.goto(`${base}/?token=${token}`);
await page.getByRole('button', { name: 'Continue', exact: true }).click();
await page.getByRole('button', { name: /Possible defects/ }).click();
await page.waitForSelector('.tile');
console.log('tiles:', await page.locator('.tile').count());
console.log(await page.locator('.tile').allInnerTexts());
await page.waitForTimeout(800);
await page.screenshot({ path: out });
await browser.close();
