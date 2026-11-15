// Markdown parsing and rendering for the mock: the source editor's highlighting
// and the preview for each target, following crates/markup and markup_view.rs.
(function () {
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

  const EMOJI = {
    construction: "🚧", pray: "🙏", tada: "🎉", rocket: "🚀", white_check_mark: "✅", warning: "⚠️", eyes: "👀",
    wave: "👋", thumbsup: "👍", "+1": "👍", fire: "🔥", bug: "🐛", sparkles: "✨", memo: "📝", calendar: "📅",
    heart: "❤️", smile: "😄", slightly_smiling_face: "🙂", point_right: "👉", rotating_light: "🚨", x: "❌",
    hourglass: "⌛", lock: "🔒", key: "🔑", package: "📦", ship: "🚢", mega: "📣", loudspeaker: "📢", thread: "🧵",
    raised_hands: "🙌", muscle: "💪", coffee: "☕", zap: "⚡", chart_with_upwards_trend: "📈", bulb: "💡",
  };
  const emojify = (s) => s.replace(/:([a-z0-9_+-]+):/g, (m, k) => EMOJI[k] || m);
  const EMOJI_RE = /(\p{Extended_Pictographic}(️|‍\p{Extended_Pictographic})*)/gu;

  // ---------- frontmatter ----------
  function splitFrontmatter(text) {
    const m = text.match(/^---\n([\s\S]*?)\n---(\n|$)/);
    if (!m) return { fields: null, body: text, fmLines: 0 };
    const fields = [];
    for (const line of m[1].split("\n")) {
      const kv = line.match(/^([A-Za-z_][\w-]*):\s*(.*)$/);
      if (!kv) return { fields: null, body: text, fmLines: 0 };
      fields.push([kv[1], kv[2].replace(/^["']|["']$/g, "")]);
    }
    return { fields, body: text.slice(m[0].length), fmLines: m[0].split("\n").length - 1 };
  }

  // ---------- block parser (GFM subset) ----------
  const reFence = /^(\s{0,3})(`{3,}|~{3,})\s*([^`\s]*)\s*$/;
  const reHeading = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
  const reHr = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
  const reQuote = /^\s{0,3}>\s?/;
  const reItem = /^(\s*)([-*+•]|\d{1,9}[.)])(\s+|$)(.*)$/;
  const reTableSep = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;

  function startsBlock(line) {
    return reFence.test(line) || reHeading.test(line) || reHr.test(line) || reQuote.test(line) || reItem.test(line);
  }

  function parseBlocks(lines, opts) {
    const blocks = [];
    let i = 0;
    let blank = false;
    while (i < lines.length) {
      const line = lines[i];
      if (!line.trim()) { i++; blank = blocks.length > 0; continue; }
      const n0 = blocks.length;
      i = parseOne(lines, i, blocks, opts);
      if (blank && blocks[n0]) blocks[n0].blankBefore = true;
      blank = false;
    }
    return blocks;
  }

  function parseOne(lines, i, blocks, opts) {
    {
      const line = lines[i];
      let m;
      if ((m = line.match(reFence))) {
        const fence = m[2], body = [];
        i++;
        while (i < lines.length && !lines[i].trim().startsWith(fence[0].repeat(fence.length))) body.push(lines[i++]);
        i++;
        blocks.push({ t: "code", lang: m[3], text: body.join("\n") });
        return i;
      }
      if ((m = line.match(reHeading))) { blocks.push({ t: "heading", level: m[1].length, text: m[2] }); i++; return i; }
      if (reHr.test(line) && !(opts.slack)) { blocks.push({ t: "hr" }); i++; return i; }
      if (reQuote.test(line)) {
        const body = [];
        while (i < lines.length && lines[i].trim() && (reQuote.test(lines[i]) || (!startsBlock(lines[i]) && !opts.slack))) {
          body.push(lines[i].replace(reQuote, "")); i++;
        }
        const am = !opts.slack && body[0] && body[0].match(/^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$/i);
        if (am) blocks.push({ t: "alert", kind: am[1].toLowerCase(), children: parseBlocks(body.slice(1), opts) });
        else blocks.push({ t: "quote", children: opts.slack ? [{ t: "para", text: body.join("\n") }] : parseBlocks(body, opts) });
        return i;
      }
      if ((m = line.match(reItem))) { i = parseList(lines, i, blocks, opts); return i; }
      if (!opts.slack && line.includes("|") && lines[i + 1] && reTableSep.test(lines[i + 1]) && lines[i + 1].includes("-")) {
        const cells = (l) => l.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
        const head = cells(line);
        const align = cells(lines[i + 1]).map((c) => (c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : c.startsWith(":") ? "left" : ""));
        i += 2;
        const rows = [];
        while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(cells(lines[i++]));
        blocks.push({ t: "table", head, align, rows });
        return i;
      }
      const para = [];
      while (i < lines.length && (opts.slack ? true : lines[i].trim()) && !startsBlock(lines[i])) {
        if (opts.slack && !lines[i].trim()) {
          // Slack: blank lines stay inside the paragraph unless a block follows.
          let j = i;
          while (j < lines.length && !lines[j].trim()) j++;
          if (j >= lines.length || startsBlock(lines[j])) break;
        }
        para.push(lines[i++]);
      }
      blocks.push({ t: "para", text: para.join("\n") });
      return i;
    }
  }

  function parseList(lines, i, blocks, opts) {
    const first = lines[i].match(reItem);
    const indent = first[1].length;
    const ordered = /\d/.test(first[2]);
    const list = { t: "list", ordered, start: ordered ? parseInt(first[2], 10) : 1, tight: true, items: [] };
    let sawBlank = false;
    while (i < lines.length) {
      const m = lines[i].match(reItem);
      if (!m || m[1].length !== indent || /\d/.test(m[2]) !== ordered) break;
      if (sawBlank) list.tight = false;
      sawBlank = false;
      const contentIndent = m[1].length + m[2].length + Math.max(1, m[3].length);
      const body = [m[4]];
      i++;
      while (i < lines.length) {
        const l = lines[i];
        if (!l.trim()) {
          let j = i;
          while (j < lines.length && !lines[j].trim()) j++;
          if (j < lines.length && lines[j].search(/\S/) >= contentIndent) { body.push(""); i++; continue; }
          sawBlank = true; i++;
          break;
        }
        const ind = l.search(/\S/);
        if (ind >= Math.min(contentIndent, indent + 2)) { body.push(l.slice(Math.min(ind, contentIndent))); i++; continue; }
        if (reItem.test(l) || startsBlock(l)) break;
        body.push(l.trim()); i++;
      }
      let task = null;
      const tm = body[0].match(/^\[( |x|X)\]\s+/);
      if (tm && !opts.slack) { task = tm[1] !== " "; body[0] = body[0].slice(tm[0].length); }
      list.items.push({ task, children: parseBlocks(body, opts) });
      if (sawBlank) {
        const n = lines[i] && lines[i].match(reItem);
        // The list ends at the blank line, which stays for the next block.
        if (!n || n[1].length !== indent) { i--; break; }
      }
    }
    if (list.items.some((it) => it.children.length > 1 && it.children.some((c) => c.t === "para") && it.children.filter((c) => c.t === "para").length > 1)) list.tight = false;
    blocks.push(list);
    return i;
  }

  // ---------- inline rendering ----------
  function smartQuotes(s) {
    return s.replace(/(^|[\s(\[{"“])'/g, "$1‘").replace(/'/g, "’").replace(/(^|[\s(\[{])"/g, "$1“").replace(/"/g, "”");
  }

  function wrapEmoji(html) {
    return html.replace(EMOJI_RE, '<span class="emoji">$1</span>');
  }

  // GitHub-flavoured inline markdown.
  function inlineGithub(src) {
    const stash = [];
    const keep = (h) => `\u0000${stash.push(h) - 1}\u0000`;
    let s = src.replace(/`([^`]+)`/g, (m, c) => keep(`<code class="ic">${esc(c)}</code>`));
    s = esc(s);
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (m, t, u) => keep(`<a href="${u}">${inlineGithubLite(t)}</a>`));
    s = s.replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, (m, p, u) => p + keep(`<a href="${u}">${u}</a>`));
    s = inlineGithubLite(s);
    s = s.replace(/\u0000(\d+)\u0000/g, (m, n) => stash[+n]);
    s = s.replace(/\u0000(\d+)\u0000/g, (m, n) => stash[+n]);
    return wrapEmoji(s);
  }
  function inlineGithubLite(s) {
    return s
      .replace(/(\*\*|__)(?=\S)([\s\S]+?\S)\1/g, "<strong>$2</strong>")
      .replace(/(^|[^\w*])\*(?=\S)([^*]+?\S)\*(?!\*)/g, "$1<em>$2</em>")
      .replace(/(^|[^\w])_(?=\S)([^_]+?\S)_(?!\w)/g, "$1<em>$2</em>")
      .replace(/~~(?=\S)([\s\S]+?\S)~~/g, "<del>$1</del>");
  }

  // Slack inline: Markdown emphasis and links on Slack's blocks. <url|label>
  // still parses, :shortcode: emoji and @mentions work.
  function slackSlug(u) {
    let s = u.replace(/^[a-z]+:\/\//i, "").replace(/^www\./, "").replace(/:\d+/, "").replace(/\/$/, "");
    s = s.replace(/#.*$/, "#…").replace(/\?.*$/, "?…");
    return s;
  }
  function inlineSlack(src) {
    const stash = [];
    const keep = (h) => `\u0000${stash.push(h) - 1}\u0000`;
    let s = src.replace(/`([^`\n]+)`/g, (m, c) => keep(`<code class="ic">${esc(c)}</code>`));
    // Backslash escapes before ASCII punctuation.
    s = s.replace(/\\([!-\/:-@\[-`{-~])/g, (m, c) => keep(esc(c)));
    s = s.replace(/<(https?:[^|>]+)\|([^>]+)>/g, (m, u, t) => keep(`<a href="${esc(u)}">${esc(t)}</a>`));
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (m, t, u) => keep(`<a href="${esc(u)}">${esc(smartQuotes(t))}</a>`));
    s = s.replace(/<?(https?:\/\/[^\s<>|]+)>?/g, (m, u) =>
      keep(`<a class="slug" href="${esc(u)}">${window.ICONS.link}${esc(slackSlug(u))}</a>`));
    s = s.replace(/(^|\s)(@[A-Za-z][\w.-]*)/g, (m, p, n) => p + keep(`<span class="mention">${esc(n)}</span>`));
    s = esc(emojify(smartQuotes(s)));
    // A closer matches its opener's run, and word boundaries keep snake_case
    // and 2*3*4 literal.
    s = s
      .replace(/(^|[^\w*])\*\*(?=\S)([^\n]+?\S)\*\*(?![\w*])/g, "$1<strong>$2</strong>")
      .replace(/(^|[^\w_])__(?=\S)([^\n]+?\S)__(?![\w_])/g, "$1<strong>$2</strong>")
      .replace(/(^|[^\w*])\*(?=\S)([^*\n]+?\S)\*(?![\w*])/g, "$1<em>$2</em>")
      .replace(/(^|[^\w_])_(?=\S)([^_\n]+?\S)_(?![\w_])/g, "$1<em>$2</em>")
      .replace(/(^|[^\w~])~~(?=\S)([^\n]+?\S)~~(?![\w~])/g, "$1<del>$2</del>")
      .replace(/(^|[^\w~])~(?=\S)([^~\n]+?\S)~(?![\w~])/g, "$1<del>$2</del>");
    s = s.replace(/\u0000(\d+)\u0000/g, (m, n) => stash[+n]);
    return wrapEmoji(s);
  }

  // ---------- block rendering ----------
  const STYLES = {
    github: { size: 14, gap: 16, heading: 24, textGap: 16, tight: 3.5, scale: [2, 1.5, 1.25, 1, 0.875, 0.85], rule: 2 },
    plain: { size: 14, gap: 16, heading: 24, textGap: 16, tight: 3.5, scale: [2, 1.5, 1.25, 1, 0.875, 0.85], rule: 2 },
    chat: { size: 13, gap: 8, heading: 12, textGap: 16, tight: 3.5, scale: [2, 1.5, 1.25, 1, 0.875, 0.85], rule: 2 },
    slack: { size: 15, gap: 4, heading: 4, textGap: 0, tight: 0, scale: [1, 1, 1, 1, 1, 1], rule: 0 },
    rich: { size: 13, margins: true, tight: 0, scale: [2, 1.5, 1.17, 1, 0.83, 0.67], hmargin: [0.67, 0.83, 1, 1.33, 1.67, 2.33], rule: 0 },
  };
  const isText = (b) => b.t === "para" || b.t === "heading" || b.t === "list";

  function gapBetween(prev, b, st, inner) {
    if (!prev) return 0;
    if (st.margins) {
      const m = (x) => x.t === "heading" ? st.size * st.scale[x.level - 1] * st.hmargin[x.level - 1]
        : x.t === "code" ? 11.7 : x.t === "table" ? 0 : x.t === "hr" ? 6.5 : x.t === "list" && inner ? 0 : st.size;
      return Math.max(m(prev), m(b));
    }
    if (b.t === "heading") return st.heading;
    if (b.codecard && (prev.t === "quote" || prev.t === "code")) return 2 * st.gap;
    if (isText(prev) && isText(b)) return Math.min(st.textGap, inner == null ? st.gap : inner);
    return inner == null ? st.gap : inner;
  }

  function renderBlocks(blocks, target, st, depth, innerGap) {
    let out = "", prev = null;
    for (const b of blocks) {
      let g = gapBetween(prev, b, st, innerGap);
      if (target === "slack" && b.codecard && !prev) g = st.gap;
      if (target === "slack" && prev && b.blankBefore && !b.codecard) g += 8;
      out += renderBlock(b, target, st, depth, g);
      prev = b;
    }
    return out;
  }

  function renderBlock(b, target, st, depth, gap) {
    const pad = gap ? ` style="margin-top:${gap}px"` : "";
    const inl = target === "slack" ? inlineSlack : inlineGithub;
    switch (b.t) {
      case "para": {
        if (target === "slack") {
          const html = b.text.split("\n").map((l) => (l.trim() ? inl(l) : '<span class="blank"></span>')).join("\n")
            .replace(/<span class="blank"><\/span>\n/g, '<span class="blank"></span>');
          return `<p${pad}>${html}</p>`;
        }
        return `<p${pad}>${inl(b.text)}</p>`;
      }
      case "heading": {
        if (target === "slack") return `<p${pad}><strong>${inl(b.text)}</strong></p>`;
        const size = st.size * st.scale[b.level - 1];
        const cls = [b.level <= st.rule ? "h-rule" : "", b.level === 6 && target !== "rich" ? "muted" : ""].join(" ").trim();
        const style = `font-size:${size}px;--hs:${size}px;${gap ? `margin-top:${gap}px;` : ""}`;
        return `<h${b.level} class="${cls}" style="${style}">${inl(b.text)}</h${b.level}>`;
      }
      case "code": {
        if (target === "slack" && b.lang) {
          const lines = b.text.split("\n");
          const label = LANGS[b.lang] || b.lang;
          const nums = LANGS[b.lang] || /^(text|plaintext|txt|plain)$/.test(b.lang);
          return `<div class="codecard-wrap"${pad}><div class="codecard"><div class="cc-head"><span>${esc(label)}</span>` +
            `<span class="cc-copy">${window.ICONS["copy-slack"]}</span></div><div class="cc-body">` +
            (nums ? `<div class="cc-nums">${lines.map((_, i) => `<div class="cc-line">${i + 1}</div>`).join("")}</div>` : "") +
            `<div class="cc-code">${lines.map((l) => `<div class="cc-line">${esc(l) || " "}</div>`).join("")}</div></div></div></div>`;
        }
        // GitHub colors code in a language it has a grammar for; chat
        // messages share the style.
        const code = (target === "github" || target === "chat") && b.lang ? highlightCode(b.text, b.lang) : null;
        return `<pre${pad}>${code == null ? esc(b.text) : code}</pre>`;
      }
      case "quote":
        return `<blockquote${pad}>${renderBlocks(b.children, target, st, depth, st.margins ? null : st.gap)}</blockquote>`;
      case "alert": {
        if (target === "rich") {
          const label = b.kind[0].toUpperCase() + b.kind.slice(1);
          return renderBlock({ t: "quote", children: [{ t: "para", text: `**${label}**` }, ...b.children] }, target, st, depth, gap);
        }
        const label = b.kind[0].toUpperCase() + b.kind.slice(1);
        return `<div class="alert" style="--ac:var(--a-${b.kind});${gap ? `margin-top:${gap}px` : ""}"><div class="alert-title">` +
          `${window.ICONS["alert-" + b.kind]}${label}</div>${renderBlocks(b.children, target, st, depth, st.gap)}</div>`;
      }
      case "hr":
        return `<hr${pad}>`;
      case "table": {
        const al = (i) => (b.align[i] ? ` style="text-align:${b.align[i]}"` : "");
        return `<div class="table"${pad}><table><thead><tr>${b.head.map((c, i) => `<th${al(i) || ""}>${inl(c)}</th>`).join("")}</tr></thead>` +
          `<tbody>${b.rows.map((r) => `<tr>${b.head.map((_, i) => `<td${al(i)}>${inl(r[i] || "")}</td>`).join("")}</tr>`).join("")}</tbody></table></div>`;
      }
      case "list": {
        const tag = b.ordered ? "ol" : "ul";
        const itemGap = b.tight ? st.tight : st.margins ? 13 : st.gap;
        const kind = ["", "ring", "square"][depth % 3];
        const items = b.items.map((it, n) => {
          let mk;
          if (it.task != null && target !== "rich") mk = `<span class="chk${it.task ? " on" : ""}" style="top:${(st.size * (target === "slack" ? 22 / 15 : target === "rich" ? 15 / 13 : 1.5) - 13) / 2}px">${it.task ? "✓" : ""}</span>`;
          else if (b.ordered) mk = numbering(b.start + n, depth, target) + ".";
          else mk = `<span class="dot ${kind}"></span>`;
          let children = it.children;
          if (it.task != null && target === "rich" && children[0] && children[0].t === "para") {
            children = [{ ...children[0], text: (it.task ? "☑ " : "☐ ") + children[0].text }, ...children.slice(1)];
          }
          const ig = n ? itemGap : 0;
          return `<li${ig ? ` style="padding-top:${ig}px"` : ""}><span class="mk${b.ordered && it.task == null ? " num" : ""}">${mk}</span>` +
            `<div class="lc">${renderBlocks(children, target, st, depth + 1, b.tight ? 0 : st.margins ? 13 : st.gap)}</div></li>`;
        }).join("");
        return `<${tag}${pad}>${items}</${tag}>`;
      }
    }
    return "";
  }

  function numbering(n, depth, target) {
    const order = target === "github" || target === "chat" ? ["decimal", "roman", "alpha"] : ["decimal", "alpha", "roman"];
    const kind = order[depth % 3];
    if (kind === "alpha") return String.fromCharCode(96 + ((n - 1) % 26) + 1);
    if (kind === "roman") {
      const r = [["x", 10], ["ix", 9], ["v", 5], ["iv", 4], ["i", 1]];
      let s = "";
      for (const [k, v] of r) while (n >= v) { s += k; n -= v; }
      return s;
    }
    return String(n);
  }

  const LANGS = {
    js: "JavaScript", javascript: "JavaScript", cjs: "JavaScript", ts: "TypeScript", typescript: "TypeScript", rs: "Rust", rust: "Rust",
    sh: "Shell", bash: "Shell", zsh: "Shell", shell: "Shell", console: "Shell Session", diff: "Diff", py: "Python", python: "Python",
    json: "JSON", yml: "YAML", yaml: "YAML", html: "HTML", css: "CSS", go: "Go", sql: "SQL", ruby: "Ruby", rb: "Ruby",
  };

  // ---------- code highlighting (crates/markup/src/highlight.rs) ----------
  // A small tokenizer standing in for syntect: comments, strings, numbers
  // and constants, keywords, function and macro names, and shell variables,
  // as classes colored with GitHub's prettylights palette (style.css).
  const KW = {
    rust: "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while",
    js: "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield interface type enum implements readonly as",
    python: "and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield",
    go: "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
    sh: "if then else elif fi for while until do done case esac in function return export local readonly set unset",
    json: "",
  };
  const LANG_ALIAS = { rs: "rust", rust: "rust", js: "js", javascript: "js", jsx: "js", ts: "js", typescript: "js", tsx: "js", mjs: "js", cjs: "js",
    py: "python", python: "python", go: "go", golang: "go", sh: "sh", bash: "sh", zsh: "sh", shell: "sh", console: "sh", json: "json", jsonc: "json" };
  const CONSTANTS = /^(true|false|null|undefined|None|True|False|nil|NaN|Infinity)$/;
  function highlightCode(text, lang) {
    const kind = LANG_ALIAS[String(lang).toLowerCase()];
    if (!kind) return null;
    const kw = new Set(KW[kind].split(" ").filter(Boolean));
    const span = (cls, t) => `<span class="pl-${cls}">${esc(t)}</span>`;
    const comment = kind === "python" || kind === "sh" ? /^#[^\n]*/ : kind === "json" ? null : /^(\/\/[^\n]*|\/\*[\s\S]*?\*\/)/;
    let out = "", i = 0;
    while (i < text.length) {
      const rest = text.slice(i);
      let m;
      if (comment && (m = rest.match(comment)) && (kind !== "sh" || i === 0 || /\s/.test(text[i - 1]))) { out += span("c", m[0]); i += m[0].length; continue; }
      if ((m = rest.match(/^("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`)/)) && !(kind === "rust" && m[0][0] === "'" && m[0].length > 3)) {
        // A JSON string followed by a colon is a key.
        const key = kind === "json" && /^\s*:/.test(text.slice(i + m[0].length));
        out += span(key ? "c1" : "s", m[0]); i += m[0].length; continue;
      }
      if ((m = rest.match(/^\$(\{[^}]*\}|[A-Za-z_][\w]*|[0-9@#?$!*-])/)) && kind === "sh") { out += span("v", m[0]); i += m[0].length; continue; }
      if ((m = rest.match(/^\d[\d_]*(\.\d+)?([eE][+-]?\d+)?[a-z0-9]*/)) && !/[\w]/.test(text[i - 1] || "")) { out += span("c1", m[0]); i += m[0].length; continue; }
      if ((m = rest.match(/^[A-Za-z_][\w]*/))) {
        const w = m[0], after = text.slice(i + w.length);
        if (kw.has(w)) out += span("k", w);
        else if (CONSTANTS.test(w) || (/^[A-Z][A-Z0-9_]{1,}$/.test(w) && kind !== "sh")) out += span("c1", w);
        else if (kind === "rust" && after[0] === "!") { out += span("en", w + "!"); i += 1; }
        else if (/^\s*\(/.test(after) && kind !== "sh" && kind !== "json") out += span("en", w);
        else if (kind === "sh" && /(^|\n|[;|&]\s*)\s*$/.test(text.slice(0, i))) out += span("en", w);
        else out += esc(w);
        i += w.length; continue;
      }
      out += esc(text[i]); i++;
    }
    return out;
  }

  // ---------- GitHub repository files ----------
  // A file in a repository joins a paragraph's lines with spaces; only a
  // hard break (two trailing spaces or a backslash) breaks the line.
  function softBreaks(blocks) {
    return blocks.map((b) => {
      if (b.t === "para") return { ...b, text: b.text.replace(/( {2,}|\\)\n/g, "\u0001").replace(/\n/g, " ").replace(/\u0001/g, "\n") };
      if (b.children) return { ...b, children: softBreaks(b.children) };
      if (b.items) return { ...b, items: b.items.map((it) => ({ ...it, children: softBreaks(it.children) })) };
      return b;
    });
  }
  // Whether a GitHub comment's paragraphs look hard-wrapped: two or more
  // lines of 50 characters or more that end inside a paragraph
  // (markup_view.rs looks_wrapped).
  function looksWrapped(text) {
    const body = splitFrontmatter(text).body.replace(/^\n+/, "");
    const count = (blocks) => blocks.reduce((n, b) => {
      if (b.t === "para") { const ls = b.text.split("\n"); ls.pop(); return n + ls.filter((l) => [...l].length >= 50).length; }
      if (b.t === "list") return n + b.items.reduce((k, it) => k + count(it.children), 0);
      if (b.children) return n + count(b.children);
      return n;
    }, 0);
    return count(parseBlocks(body.split("\n"), {})) >= 2;
  }

  // Slack's paste transforms: tables and multi-line list items become code / text.
  function slackPosted(blocks) {
    return blocks.map((b) => {
      if (b.t === "code" && b.lang) return { ...b, codecard: true };
      return b;
    });
  }

  function renderPreview(text, target) {
    const { fields, body } = splitFrontmatter(text);
    let html = "";
    if (fields && fields.length) {
      const solo = fields.length === 1 && /^(title|subject)$/i.test(fields[0][0]);
      html += `<div class="fields${solo ? " solo" : ""}">` + fields.map(([k, v]) =>
        `<div class="field"><span class="field-key">${esc(k[0].toUpperCase() + k.slice(1).toLowerCase())}</span>` +
        `<span class="field-val${/^(title|subject)$/i.test(k) ? " strong" : ""}">${esc(v)}</span>` +
        `<span class="field-copy" data-copy-field="${esc(k)}" data-value="${esc(v)}">${window.ICONS.copy}</span></div>`).join("") + "</div>";
    }
    const trimmed = body.replace(/^\n+/, "").replace(/\s+$/, "");
    if (!trimmed && !html) return { html: "", empty: true };
    // A GitHub file draws as GitHub does, but with soft breaks as spaces.
    const file = target === "github-file";
    if (file) target = "github";
    const st = STYLES[target];
    if (target === "plain") {
      html += `<div class="md t-plain"><p>${esc(trimmed)}</p></div>`;
    } else {
      let blocks = parseBlocks(trimmed.split("\n"), { slack: target === "slack" });
      if (target === "slack") blocks = slackPosted(blocks);
      if (file) blocks = softBreaks(blocks);
      html += `<div class="md t-${target}">${renderBlocks(blocks, target, st, 0)}</div>`;
    }
    return { html, empty: false };
  }

  function renderChat(text) {
    const blocks = parseBlocks(text.trim().split("\n"), {});
    return `<div class="md chatmd">${renderBlocks(blocks, "chat", STYLES.chat, 0)}</div>`;
  }

  // ---------- source editor highlighting (crates/editor/src/highlight.rs) ----------
  function highlightSource(text) {
    const lines = text.split("\n");
    const out = [];
    let fmEnd = -1;
    if (lines[0] === "---") {
      for (let i = 1; i < lines.length; i++) if (lines[i] === "---") { fmEnd = i; break; }
      if (fmEnd > 0 && !lines.slice(1, fmEnd).every((l) => /^[A-Za-z_][\w-]*:/.test(l) || /^\s/.test(l) || !l.trim())) fmEnd = -1;
    }
    let fence = null;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (i <= fmEnd) {
        const kv = line.match(/^([^:\s][^:]*):([\s\S]*)$/);
        out.push(kv && line !== "---" ? `<span class="hl-fmkey">${esc(kv[1])}</span><span class="hl-fm">:${esc(kv[2])}</span>` : `<span class="hl-fm">${esc(line)}</span>`);
        continue;
      }
      const fm = line.match(/^\s{0,3}(`{3,}|~{3,})/);
      if (fence) {
        out.push(`<span class="hl-code">${esc(line)}</span>`);
        if (fm && fm[1][0] === fence[0] && fm[1].length >= fence.length && !line.trim().slice(fm[1].length).trim()) fence = null;
        continue;
      }
      if (fm) { fence = fm[1]; out.push(`<span class="hl-code">${esc(line)}</span>`); continue; }
      if (/^\s{0,3}#{1,6}(\s|$)/.test(line)) { out.push(`<span class="hl-h">${hlInline(line)}</span>`); continue; }
      out.push(hlInline(line));
    }
    return out.join("\n");
  }

  function hlInline(line) {
    const n = line.length;
    const f = Array.from({ length: n }, () => ({ code: 0, link: 0, b: 0, i: 0, s: 0 }));
    const mark = (re, key, guard = "code") => {
      re.lastIndex = 0;
      let m;
      while ((m = re.exec(line))) {
        const start = m.index + (m[1] && key !== "code" && m[0].startsWith(m[1]) && /^[^*_~`\[]$/.test(m[1]) ? m[1].length : 0);
        const end = m.index + m[0].length;
        if (guard && f[start] && f[start][guard]) continue;
        for (let k = start; k < end; k++) f[k][key] = 1;
      }
    };
    mark(/`[^`]+`/g, "code", null);
    mark(/!?\[[^\]]*\]\([^)]*\)|https?:\/\/[^\s)>]+/g, "link");
    mark(/(\*\*|__)(?=\S)(.+?\S)\1/g, "b");
    mark(/(^|[^\w*])(\*(?=\S)[^*]+?\S\*)(?!\*)/g, "i");
    mark(/(^|[^\w_])(_(?=\S)[^_]+?\S_)(?!\w)/g, "i");
    mark(/~~(?=\S).+?\S~~/g, "s");
    let out = "", cur = null, buf = "";
    const cls = (x) => [x.code ? "hl-code" : x.link ? "hl-link" : x.s ? "hl-strike" : "", x.b ? "hl-b" : "", x.i ? "hl-i" : ""].filter(Boolean).join(" ");
    const flush = () => { if (buf) out += cur ? `<span class="${cur}">${esc(buf)}</span>` : esc(buf); buf = ""; };
    for (let k = 0; k < n; k++) {
      const c = cls(f[k]);
      if (c !== cur) { flush(); cur = c; }
      buf += line[k];
    }
    flush();
    return out;
  }

  // Sidebar / header title: stored title, frontmatter title/subject, first body line.
  function draftTitle(text, { emoji = true } = {}) {
    const { fields, body } = splitFrontmatter(text);
    if (fields) {
      for (const k of ["title", "subject"]) {
        const f = fields.find(([key]) => key.toLowerCase() === k);
        if (f && f[1].trim()) return f[1].trim();
      }
    }
    const line = body.split("\n").find((l) => l.trim());
    if (!line) return "Untitled draft";
    const title = (line.replace(/^\s*(#{1,6}\s+|>\s*|[-*+]\s+|\d+[.)]\s+)/, "").replace(/[*_`~]/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").trim());
    return emoji ? emojify(title) : title;
  }

  window.MD = { renderPreview, renderChat, highlightSource, splitFrontmatter, draftTitle, esc, emojify, looksWrapped, highlightCode };
})();
