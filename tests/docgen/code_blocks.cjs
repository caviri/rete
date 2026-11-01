// Run in the Playwright image with the repository mounted at /work:
// node tests/docgen/code_blocks.cjs
const { chromium } = require('../gate/node_modules/playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');

(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
      writeText: async text => { window.copied = text; }
    }});
  });
  const visit = name => page.goto('file://' + path.join(root, 'docs', name));
  await visit('testing.html');
  const first = page.locator('.code-copy').first();
  assert.ok(await first.count(), 'Every code block needs a copy button');
  await first.focus();
  await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(() => window.copied), await page.locator('pre code').first().textContent());
  assert.equal(await first.textContent(), 'Copied');

  // All syntax colors must stay legible against the code background in both themes.
  for (const theme of ['light', 'dark']) {
    const contrasts = await page.evaluate(theme => {
      document.documentElement.dataset.theme = theme;
      const code = document.querySelector('pre code');
      const luminance = color => {
        const rgb = color.match(/[\d.]+/g).slice(0, 3).map(Number).map(value => {
          value /= 255; return value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4;
        });
        return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
      };
      const background = luminance(getComputedStyle(code.parentElement).backgroundColor);
      return ['com', 'str', 'kw', 'num', 'fn', 'iri', 'var', 'flag'].map(token => {
        const span = document.createElement('span'); span.className = 'tok-' + token;
        code.append(span);
        const foreground = luminance(getComputedStyle(span).color); span.remove();
        return [token, (Math.max(foreground, background) + .05) / (Math.min(foreground, background) + .05)];
      });
    }, theme);
    for (const [token, ratio] of contrasts) assert.ok(ratio >= 4.5, `${theme}/${token}: contrast ${ratio}`);
  }

  // Each existing fenced language gets tokens; highlighting must preserve bytes.
  const fixtures = {
    rust: 'let answer = 42; // hello', python: 'import rete_graph\nprint("hello")',
    sh: 'rete build --card "$PWD/input.nt"', js: 'const answer = await open("a.rete");',
    json: '{"answer": 42}', sparql: 'SELECT ?s WHERE { ?s a <https://example.org/T> }',
    turtle: '@prefix ex: <https://example.org/> .', powershell: 'function rete { docker run -v "${PWD}:/data" @args }',
    r: 'library(rete)\nx <- TRUE', toml: '[dependencies]\nrete-graph = "0.4.2"',
    yaml: 'jobs:\n  test: true # comment', html: '<div class="test">hello & goodbye</div>',
    xml: '<node id="1"/>', dockerfile: 'FROM node:22\nRUN npm install',
    mermaid: 'graph TD\n  A --> B', text: 'plain output\n<unchanged> & spaced  text',
    '': 'directory/\n  file.txt'
  };
  for (const [language, source] of Object.entries(fixtures)) {
    const result = await page.evaluate(({ language, source }) => {
      const pre = document.createElement('pre'), code = document.createElement('code');
      code.className = language ? 'language-' + language : '';
      code.textContent = source;
      pre.append(code); document.querySelector('.content').append(pre);
      document.dispatchEvent(new CustomEvent('rete:code-updated', { detail: code }));
      return { text: code.textContent, tokens: code.querySelectorAll('span').length,
        copies: pre.parentElement.querySelectorAll('.code-copy').length };
    }, { language, source });
    assert.equal(result.text, source, language + ' must preserve code');
    assert.equal(result.copies, 1, language + ' must have exactly one copy button');
    if (language && language !== 'text') assert.ok(result.tokens > 0, language + ' needs highlighting');
  }

  await visit('install.html');
  for (const system of ['linux', 'macos', 'windows']) {
    await page.locator('[name="install-system"][value="' + system + '"]').check();
    for (const product of ['cli', 'python', 'javascript', 'rust']) {
      await page.locator('[name="install-product"][value="' + product + '"]').check();
      const code = page.locator('.install-command code');
      assert.ok(await code.locator('span').count(), 'Dynamic commands need highlighting');
      assert.equal(await page.locator('.install-command .code-copy').count(), 1);
      await page.locator('.install-command .code-copy').click();
      assert.equal(await page.evaluate(() => window.copied), await code.textContent());
    }
  }
  await page.evaluate(() => { navigator.clipboard.writeText = async () => { throw Error('Denied'); }; });
  await page.locator('.code-copy').click();
  assert.equal(await page.evaluate(() => getSelection().toString()), await page.locator('.install-command code').textContent());
  assert.match(await page.locator('.code-status').textContent(), /selected/);
  await page.evaluate(() => { Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined }); });
  await page.locator('.code-copy').click();
  assert.equal(await page.evaluate(() => getSelection().toString()), await page.locator('.install-command code').textContent());
  // A delayed clipboard promise must not mark a newly selected recipe as copied.
  await page.evaluate(() => { Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
    writeText: () => new Promise(resolve => { window.finishCopy = resolve; })
  }}); });
  await page.locator('.code-copy').click();
  await page.locator('[name="install-system"][value="linux"]').check();
  await page.evaluate(() => window.finishCopy());
  assert.equal(await page.locator('.code-copy').textContent(), 'Copy');
  assert.equal(await page.locator('.code-status').textContent(), '');

  // Every generated documentation page: unchanged content and one button per block.
  let pages = 0, blocks = 0;
  for (const name of fs.readdirSync(path.join(root, 'docs')).filter(name => name.endsWith('.html'))) {
    const html = fs.readFileSync(path.join(root, 'docs', name), 'utf8');
    if (!html.includes('data-nav-page=')) continue;
    await visit(name);
    const result = await page.evaluate(html => {
      const original = new DOMParser().parseFromString(html, 'text/html');
      const before = [...original.querySelectorAll('.content pre code')].map(el => el.textContent);
      const after = [...document.querySelectorAll('.content pre code')].map(el => el.textContent);
      return { before, after, buttons: document.querySelectorAll('.code-copy').length };
    }, html);
    assert.deepEqual(result.after, result.before, name + ' changed code');
    assert.equal(result.buttons, result.after.length, name + ' missing/duplicate button');
    pages++; blocks += result.after.length;
  }
  await visit('install.html');
  await page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  const screenshots = process.env.CODE_SCREENSHOTS || '/tmp';
  await page.locator('.install-command').screenshot({ path: path.join(screenshots, 'code-mobile.png') });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.evaluate(() => document.documentElement.dataset.theme = 'dark');
  await page.locator('.install-command').screenshot({ path: path.join(screenshots, 'code-dark.png') });
  const offline = await browser.newContext({ javaScriptEnabled: false });
  const plain = await offline.newPage();
  await plain.goto('file://' + path.join(root, 'docs/install.html'));
  assert.match(await plain.locator('.install-command code').textContent(), /docker run/);
  assert.equal(await plain.locator('.code-copy').count(), 0);
  assert.deepEqual(errors, []);
  await browser.close();
  console.log(`PASS: ${pages} pages, ${blocks} code blocks, language fixtures, keyboard/copy/fallback, installer updates, mobile and no-JS.`);
})().catch(error => { console.error(error); process.exit(1); });
