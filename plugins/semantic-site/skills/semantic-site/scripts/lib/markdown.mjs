// A small Markdown renderer for content pages: headings, paragraphs, lists,
// fenced code, block quotes, rules, links, images, emphasis and inline code.
// Raw HTML is escaped. Use an .html fragment as a page source for anything more.

export const esc = (s) => String(s ?? '')
  .replace(/&/g, '&amp;')
  .replace(/</g, '&lt;')
  .replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;');

const safeUrl = (u) => (/^\s*(javascript|vbscript|data):/i.test(u) ? '#' : u);

export function inline(s) {
  const codes = [];
  let t = String(s).replace(/`([^`]+)`/g, (_, c) => { codes.push(c); return `\u0000${codes.length - 1}\u0000`; });
  t = esc(t);
  t = t.replace(/!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;([^&]*)&quot;)?\)/g, (_, alt, src, title) =>
    `<img src="${safeUrl(src)}" alt="${alt}"${title ? ` title="${title}"` : ''}>`);
  t = t.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, txt, href) => `<a href="${safeUrl(href)}">${txt}</a>`);
  t = t.replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, (_, pre, u) => `${pre}<a href="${u}">${u}</a>`);
  t = t.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  t = t.replace(/(^|[^*\w])\*([^*\s][^*]*)\*/g, '$1<em>$2</em>');
  t = t.replace(/(^|[^_\w])_([^_\s][^_]*)_(?!\w)/g, '$1<em>$2</em>');
  return t.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${esc(codes[Number(i)])}</code>`);
}

export function markdown(src) {
  const lines = String(src).replace(/\r\n?/g, '\n').split('\n');
  const out = [];
  let i = 0;
  const isBlockStart = (l) => /^(#{1,6}\s|```|>\s?|[-*+]\s|\d+[.)]\s|(-{3,}|\*{3,}|_{3,})\s*$)/.test(l);
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    let m;
    if ((m = /^```\s*([\w-]*)/.exec(line))) {
      const body = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) body.push(lines[i++]);
      i++;
      out.push(`<pre><code${m[1] ? ` class="language-${esc(m[1])}"` : ''}>${esc(body.join('\n'))}</code></pre>`);
      continue;
    }
    if ((m = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line))) {
      const n = m[1].length;
      const id = m[2].toLowerCase().replace(/<[^>]+>/g, '').replace(/[^\p{L}\p{N}]+/gu, '-').replace(/^-|-$/g, '');
      out.push(`<h${n} id="${esc(id)}">${inline(m[2])}</h${n}>`);
      i++;
      continue;
    }
    if (/^(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { out.push('<hr>'); i++; continue; }
    if (/^>\s?/.test(line)) {
      const body = [];
      while (i < lines.length && /^>\s?/.test(lines[i])) body.push(lines[i++].replace(/^>\s?/, ''));
      out.push(`<blockquote>${markdown(body.join('\n'))}</blockquote>`);
      continue;
    }
    if ((m = /^([-*+]|\d+[.)])\s+/.exec(line))) {
      const ordered = /\d/.test(m[1]);
      const re = ordered ? /^\d+[.)]\s+/ : /^[-*+]\s+/;
      const items = [];
      while (i < lines.length && re.test(lines[i])) {
        let item = lines[i++].replace(re, '');
        while (i < lines.length && /^\s{2,}\S/.test(lines[i]) && !re.test(lines[i])) item += ` ${lines[i++].trim()}`;
        items.push(`<li>${inline(item)}</li>`);
      }
      out.push(ordered ? `<ol>${items.join('')}</ol>` : `<ul>${items.join('')}</ul>`);
      continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !isBlockStart(lines[i])) para.push(lines[i++].trim());
    if (!para.length) { para.push(lines[i++].trim()); }
    out.push(`<p>${inline(para.join(' '))}</p>`);
  }
  return out.join('\n');
}
