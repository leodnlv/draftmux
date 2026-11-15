// Renders index.html to website/img/social.png at 2400×1260 (1200×630 at 2x).
// Needs Playwright; set PLAYWRIGHT to its module path if it isn't installed here.
const path = require('path');
const { chromium } = require(process.env.PLAYWRIGHT || 'playwright-core');
(async () => {
  const b = await chromium.launch({ channel: 'chrome' });
  const p = await b.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 2 });
  await p.goto('file://' + path.join(__dirname, 'index.html'));
  await p.evaluate(() => Promise.all([...document.images].map((i) => i.decode())));
  await p.screenshot({ path: path.join(__dirname, '../../website/img/social.png') });
  await b.close();
})();
