// Changed-text ranges for the editor's change highlight, as
// crates/draftmux/src/changes.rs computes them. Shared by the mock and the
// demo video scene.
(function () {
  // Matched index pairs of the longest common subsequence of A and B, ending
  // with [A.length, B.length]. The common ends are trimmed before the table.
  function lcsPairs(A, B) {
    let p = 0, q = 0;
    while (p < A.length && p < B.length && A[p] === B[p]) p++;
    while (q < A.length - p && q < B.length - p && A[A.length - 1 - q] === B[B.length - 1 - q]) q++;
    const a = A.slice(p, A.length - q), b = B.slice(p, B.length - q), n = a.length, m = b.length;
    const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    const pairs = [];
    for (let k = 0; k < p; k++) pairs.push([k, k]);
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (a[i] === b[j]) { pairs.push([p + i, p + j]); i++; j++; }
      else if (dp[i + 1][j] >= dp[i][j + 1]) i++;
      else j++;
    }
    for (let k = q; k > 0; k--) pairs.push([A.length - k, B.length - k]);
    pairs.push([A.length, B.length]);
    return pairs;
  }
  // [start, end] offsets of each piece of `pieces`, laid end to end.
  const offsets = (pieces) => pieces.reduce((acc, x) => (acc.push(acc[acc.length - 1] + x.length), acc), [0]);
  // Spans of B's pieces that are inserted or replaced, as [start, end) offsets,
  // with each gap's removed and added pieces handed to `refine` when both exist.
  function gaps(A, B, refine) {
    const offB = offsets(B), offA = offsets(A), out = [];
    let pi = 0, pj = 0;
    for (const [i, j] of lcsPairs(A, B)) {
      if (j > pj) {
        if (i > pi && refine) out.push(...refine(A.slice(pi, i).join(""), B.slice(pj, j).join("")).map(([s, e]) => [s + offB[pj], e + offB[pj]]));
        else out.push([offB[pj], offB[j]]);
      }
      pi = i + 1; pj = j + 1;
    }
    return out;
  }
  // The ranges of `b` inserted or replaced relative to `a`: lines first, then
  // the words within changed lines, without whitespace at either end and with
  // neighbours separated only by spaces joined. Deletions are left out.
  function changedRanges(a, b) {
    const lines = (t) => t.match(/[^\n]*\n|[^\n]+$/g) || [];
    const words = (t) => t.match(/\s+|\S+/g) || [];
    const raw = gaps(lines(a), lines(b), (x, y) => gaps(words(x), words(y)));
    const out = [];
    for (let [s, e] of raw) {
      while (s < e && /\s/.test(b[s])) s++;
      while (e > s && /\s/.test(b[e - 1])) e--;
      if (s >= e) continue;
      const last = out[out.length - 1];
      if (last && /^[ \t]*$/.test(b.slice(last[1], s))) last[1] = e;
      else out.push([s, e]);
    }
    return out;
  }

  window.changedRanges = changedRanges;

  // ---------- review comparison (crates/core/src/diff.rs) ----------
  // Hunks between two texts, line by line and then word by word inside
  // related lines, for review's columns and the chat's edit rows.
  // Edit script between token arrays, deletions before insertions.
  function seqOps(A, B) {
    const n = A.length, m = B.length;
    let p = 0, q = 0;
    while (p < n && p < m && A[p] === B[p]) p++;
    while (q < n - p && q < m - p && A[n - 1 - q] === B[m - 1 - q]) q++;
    const a = A.slice(p, n - q), b = B.slice(p, m - q), N = a.length, M = b.length;
    const dp = Array.from({ length: N + 1 }, () => new Int32Array(M + 1));
    for (let i = N - 1; i >= 0; i--) for (let j = M - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    const ops = [];
    for (let k = 0; k < p; k++) ops.push(["=", A[k]]);
    let i = 0, j = 0, del = [], add = [];
    const flush = () => { for (const x of del) ops.push(["-", x]); for (const x of add) ops.push(["+", x]); del = []; add = []; };
    while (i < N || j < M) {
      if (i < N && j < M && a[i] === b[j]) { flush(); ops.push(["=", a[i]]); i++; j++; }
      else if (i < N && (j >= M || dp[i + 1][j] >= dp[i][j + 1])) { del.push(a[i]); i++; }
      else { add.push(b[j]); j++; }
    }
    flush();
    for (let k = n - q; k < n; k++) ops.push(["=", A[k]]);
    return ops;
  }
  const lines = (t) => t.match(/[^\n]*\n|[^\n]+$/g) || [];
  const words = (t) => t.match(/\s+|\S+/g) || [];

  // Whether replaced text shares at least half its words with its
  // replacement, whatever their case or the punctuation around them.
  function related(a, b) {
    const norm = (t) => (t.match(/\S+/g) || []).map((w) => w.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, "").toLowerCase());
    const A = norm(a), B = norm(b);
    if (!A.length && !B.length) return false;
    const shared = seqOps(A, B).filter((op) => op[0] === "=").length;
    return 4 * shared >= A.length + B.length;
  }

  // Collapses each run of changes into one deletion then one insertion, and
  // joins neighbouring unchanged runs.
  function normalize(ops) {
    const out = [];
    for (let k = 0; k < ops.length;) {
      if (ops[k][0] === "=") {
        const last = out[out.length - 1];
        if (last && last[0] === "=") last[1] += ops[k][1]; else out.push([...ops[k]]);
        k++; continue;
      }
      let del = "", add = "";
      for (; k < ops.length && ops[k][0] !== "="; k++) ops[k][0] === "-" ? (del += ops[k][1]) : (add += ops[k][1]);
      if (del) out.push(["-", del]);
      if (add) out.push(["+", add]);
    }
    return out;
  }

  function diffOps(a, b) {
    const L = seqOps(lines(a), lines(b));
    let ops = [];
    for (let k = 0; k < L.length;) {
      if (L[k][0] === "=") { ops.push(L[k]); k++; continue; }
      let del = "", add = "";
      for (; k < L.length && L[k][0] !== "="; k++) L[k][0] === "-" ? (del += L[k][1]) : (add += L[k][1]);
      if (del && add && related(del, add)) ops.push(...seqOps(words(del), words(add)));
      // Unrelated lines stay a separate removal and insertion; the empty
      // unchanged run between them keeps them in separate hunks.
      else if (del && add) ops.push(["-", del], ["=", ""], ["+", add]);
      else ops.push(del ? ["-", del] : ["+", add]);
    }
    ops = normalize(ops);
    // Unchanged stretches of at most two words inside a line fold into the
    // change around them, so a rewritten clause reads as one replacement.
    const removesBefore = (k) => ops[k - 1][0] === "-" || (ops[k - 1][0] === "+" && k >= 2 && ops[k - 2][0] === "-");
    const folded = [];
    ops.forEach((op, k) => {
      const inner = k > 0 && k + 1 < ops.length && op[0] === "=" && removesBefore(k) && ops[k + 1][0] === "-";
      if (inner && !op[1].includes("\n") && (op[1].match(/\S+/g) || []).length <= 2) folded.push(["-", op[1]], ["+", op[1]]);
      else folded.push(op);
    });
    return normalize(folded);
  }

  // Hunks with their byte ranges in both texts; each op gets its hunk.
  function compare(a, b) {
    const ops = diffOps(a, b).map(([kind, text]) => ({ kind, text, hunk: null })), hunks = [];
    let ia = 0, ib = 0;
    ops.forEach((op, k) => {
      if (op.kind === "=") { ia += op.text.length; ib += op.text.length; return; }
      if (!(k > 0 && ops[k - 1].kind !== "=")) hunks.push({ old: [ia, ia], new: [ib, ib], removed: "", added: "" });
      const hk = hunks[hunks.length - 1];
      op.hunk = hunks.length - 1;
      if (op.kind === "-") { hk.removed += op.text; ia += op.text.length; hk.old[1] = ia; }
      else { hk.added += op.text; ib += op.text.length; hk.new[1] = ib; }
    });
    return { ops, hunks };
  }
  // ---------- one side's text with its marks ----------
  // Noncharacters survive the markdown renderer and become marks once it
  // has drawn the text; each opener and point is followed by a private use
  // character carrying the hunk.
  const OPEN_INS = "﷐", OPEN_DEL = "﷑", CLOSE = "﷒", POINT = "﷓", HELD = "﷔";
  const tag = (i) => String.fromCharCode(0xE000 + i);
  const TOKENS = /[﷐﷑﷓﷔][-]|﷒/g;
  const plain = (t) => t.replace(TOKENS, "");
  const reBlockPrefix = /^(?:\s*(?:[-*+]|\d{1,9}[.)])\s+(?:\[[ xX]\]\s+)?|\s{0,3}#{1,6}\s+|\s{0,3}>\s?(?:\[!\w+\]\s*)?)*/;
  const reRawLine = /^\s*(?:`{3,}|~{3,}|([-*_])(\s*\1){2,}\s*$|\|?\s*:?-+:?\s*\|)/;
  const atLineStart = (out) => { const t = out.replace(/(?:[﷐﷑﷓﷔][-]|﷒)+$/, ""); return !t || t.endsWith("\n"); };
  // Where a line's text starts, after any marks and block marker.
  const textStart = (line) => {
    const lead = line.match(/^(?:[﷐﷑﷓﷔][-]|﷒)*/)[0].length;
    return lead + line.slice(lead).match(reBlockPrefix)[0].length;
  };

  // Marks `s` line by line, after any list, heading or quote marker so the
  // block still parses. Fences, rules and blank lines stay bare.
  function wrap(out, s, open, hunk) {
    for (const seg of lines(s)) {
      const nl = seg.endsWith("\n") ? "\n" : "";
      const body = nl ? seg.slice(0, -1) : seg;
      const start = atLineStart(out);
      const inFrontmatter = out.startsWith("---\n") && out.indexOf("\n---\n", 3) < 0 && !/^---$/.test(body);
      if (!body.trim() || (start && reRawLine.test(body))) { out += seg; continue; }
      let prefix = "";
      if (start) prefix = inFrontmatter ? (body.match(/^[A-Za-z_][\w-]*:\s*/) || [""])[0] : body.match(reBlockPrefix)[0];
      const rest = body.slice(prefix.length), core = rest.trimEnd(), lead = core.length - core.trimStart().length;
      out += core.trim() ? prefix + core.slice(0, lead) + open + tag(hunk) + core.slice(lead) + CLOSE + rest.slice(core.length) + nl : seg;
    }
    return out;
  }

  // The side's text with its decorations: its own text of each change, and
  // a point where that text is empty or only whitespace. The before side's
  // point follows its text, the now side's precedes it.
  function marked(cmp, side) {
    const shown = side === "before" ? "-" : "+", open = side === "before" ? OPEN_DEL : OPEN_INS;
    const blank = (hk) => !(side === "before" ? hk.removed : hk.added).trim();
    const seen = new Set();
    let out = "";
    cmp.ops.forEach((op, k) => {
      if (op.kind === "=") { out += op.text; return; }
      const hk = cmp.hunks[op.hunk], first = !seen.has(op.hunk), next = cmp.ops[k + 1];
      const last = !(next && next.kind !== "=" && next.hunk === op.hunk);
      seen.add(op.hunk);
      if (side === "after" && first && blank(hk)) out += POINT + tag(op.hunk);
      if (op.kind === shown) out = wrap(out, op.text, open, op.hunk);
      if (side === "before" && last && blank(hk)) out += POINT + tag(op.hunk);
    });
    return placePoints(out);
  }

  // A point at the start of a line would open a block of its own, so it
  // waits for the next line with text and opens it, after its block marker;
  // with none, it goes after the last text before it.
  function placePoints(s) {
    for (let i = s.indexOf(POINT); i >= 0; i = s.indexOf(POINT)) {
      const token = s.slice(i, i + 2);
      const lineStart = s.lastIndexOf("\n", i - 1) + 1;
      if (plain(s.slice(lineStart, i)) !== "") { s = s.slice(0, i) + HELD + s.slice(i + 1); continue; }
      s = s.slice(0, i) + s.slice(i + 2);
      let at = -1;
      for (let p = i; p <= s.length;) {
        const e = s.indexOf("\n", p), end = e < 0 ? s.length : e, line = s.slice(p, end);
        if (plain(line).trim()) { if (!reRawLine.test(plain(line))) at = p + textStart(line); break; }
        if (e < 0) break;
        p = e + 1;
      }
      if (at < 0) { at = i; while (at > 0 && /\s/.test(s[at - 1])) at--; }
      s = s.slice(0, at) + HELD + token[1] + s.slice(at);
    }
    return s.split(HELD).join(POINT);
  }

  // Renders a side for `target` and turns its marks into elements. Marks
  // wrap each text node they cover, so one crossing elements stays valid.
  function renderSide(text, target) {
    const { html, empty } = window.MD.renderPreview(text, target);
    const box = document.createElement("div");
    if (empty) { box.innerHTML = '<div class="preview-empty">Nothing to preview</div>'; return box; }
    box.innerHTML = html.replace(/="[^"]*"/g, (m) => m.replace(TOKENS, ""));
    const walker = document.createTreeWalker(box, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    let open = null;
    for (const node of nodes) {
      const t = node.nodeValue;
      if (!open && !/[﷐-﷓]/.test(t)) continue;
      const frag = document.createDocumentFragment();
      let buf = "";
      const flush = () => {
        if (!buf) return;
        if (open && buf.trim()) {
          const el = document.createElement("span");
          el.className = "rl " + (open.del ? "del" : "ins");
          el.dataset.h = open.h;
          el.textContent = buf;
          frag.append(el);
        } else frag.append(buf);
        buf = "";
      };
      for (let k = 0; k < t.length; k++) {
        const c = t[k];
        if (c === OPEN_INS || c === OPEN_DEL) { flush(); open = { h: t.charCodeAt(k + 1) - 0xE000, del: c === OPEN_DEL }; k++; }
        else if (c === CLOSE) { flush(); open = null; }
        else if (c === POINT) {
          flush();
          const pt = document.createElement("span");
          pt.className = "rl pt";
          pt.dataset.h = t.charCodeAt(k + 1) - 0xE000;
          frag.append(pt);
          k++;
        } else buf += c;
      }
      flush();
      node.replaceWith(frag);
    }
    return box;
  }
  window.reviewDiff = { compare, lines, marked, renderSide };

  // Changed lines with one line of context, like the app's edit diff.
  function lineDiff(a, b) {
    const A = a.split("\n"), B = b.split("\n");
    const n = A.length, m = B.length;
    const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    const ops = [];
    let i = 0, j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && A[i] === B[j]) { ops.push([" ", A[i]]); i++; j++; }
      else if (i < n && (j >= m || dp[i + 1][j] >= dp[i][j + 1])) { ops.push(["-", A[i]]); i++; }
      else { ops.push(["+", B[j]]); j++; }
    }
    const keep = ops.map((o, k) => o[0] !== " " || (ops[k - 1] && ops[k - 1][0] !== " ") || (ops[k + 1] && ops[k + 1][0] !== " "));
    // A gap in the kept lines starts a new hunk, as "@@" does in a patch.
    const lines = [];
    ops.forEach((o, k) => { if (!keep[k]) return; if (lines.length && !keep[k - 1]) lines.push(["@", ""]); lines.push(o); });
    return { lines, added: ops.filter((o) => o[0] === "+").length, removed: ops.filter((o) => o[0] === "-").length };
  }

  // ---------- an edit's word changes (edit_changes.rs) ----------
  // Each hunk of an edit's diff compared word by word, with the line breaks
  // inside a paragraph read as spaces, so a rewrap alone isn't a change.
  // A change keeps up to four words of unchanged text on either side.
  const CONTEXT_WORDS = 4;
  const fenceLine = (l) => /^\s*(```|~~~)/.test(l);
  const ruleLine = (l) => { l = l.trim(); return l.length >= 3 && /^[-=*]+$/.test(l); };
  const endsBlock = (l) => { l = l.trimStart(); return /^[#|]/.test(l) || fenceLine(l) || ruleLine(l); };
  const startsBlockLine = (l) => endsBlock(l) || /^\s*(>|[-*+] |\d+[.)] )/.test(l);
  function joinWrapped(text) {
    const ls = text.split("\n");
    let out = "", fenced = false;
    ls.forEach((l, i) => {
      out += l;
      if (i + 1 === ls.length) return;
      if (fenceLine(l)) fenced = !fenced;
      const next = ls[i + 1];
      const joins = !fenced && l.trim() && next.trim() && !l.endsWith("  ") && !l.endsWith("\\") && !endsBlock(l) && !startsBlockLine(next);
      out += joins ? " " : "\n";
    });
    return out;
  }
  function leadingContext(text) {
    const line = text.split("\n").pop(), words = line.split(/\s+/).filter(Boolean);
    if (!words.length) return "";
    return (words.length > CONTEXT_WORDS ? "…" : "") + words.slice(-CONTEXT_WORDS).join(" ") + (/\s$/.test(line) ? " " : "");
  }
  function trailingContext(text) {
    const line = text.split("\n")[0], words = line.split(/\s+/).filter(Boolean);
    if (!words.length) return "";
    return (/^\s/.test(line) ? " " : "") + words.slice(0, CONTEXT_WORDS).join(" ") + (words.length > CONTEXT_WORDS ? "…" : "");
  }
  const dropBlankLines = (t) => t.split("\n").filter((l) => l.trim()).join("\n");
  // The word before `at`, else the one after: what a removal points at.
  function wordBeside(text, at) {
    const head = text.slice(0, at).trimEnd();
    if (head) { const start = Math.max(head.search(/\S+$/), 0); return [start, head.length]; }
    const start = at + (text.slice(at).length - text.slice(at).trimStart().length);
    const sp = text.slice(start).search(/\s/);
    return [start, sp < 0 ? text.length : start + sp];
  }
  function wordChanges(old, nu) {
    const hunks = [];
    // A removal and an insertion with only whitespace between them are one
    // replacement.
    for (const hk of window.reviewDiff.compare(old, nu).hunks) {
      const last = hunks[hunks.length - 1];
      if (last && !old.slice(last.old[1], hk.old[0]).trim() && !nu.slice(last.new[1], hk.new[0]).trim()) {
        last.removed += old.slice(last.old[1], hk.old[0]) + hk.removed;
        last.added += nu.slice(last.new[1], hk.new[0]) + hk.added;
        last.old = [last.old[0], hk.old[1]]; last.new = [last.new[0], hk.new[1]];
      } else hunks.push({ ...hk, old: [...hk.old], new: [...hk.new] });
    }
    return hunks.flatMap((hk) => {
      const removed = dropBlankLines(hk.removed.trim()), added = dropBlankLines(hk.added.trim());
      if (!removed && !added) return [];
      const lead = hk.added.length - hk.added.trimStart().length, trail = hk.added.length - hk.added.trimEnd().length;
      const start = hk.new[0] + lead, end = Math.max(hk.new[1] - trail, start);
      const side = added ? hk.added : hk.removed;
      let before = leadingContext(nu.slice(0, hk.new[0])), after = trailingContext(nu.slice(hk.new[1]));
      if (before && /^\s/.test(side) && !/\s$/.test(before)) before += " ";
      if (after && /\s$/.test(side) && !/^\s/.test(after)) after = " " + after;
      return [{ before, removed, added, after, target: start < end ? [start, end] : wordBeside(nu, start) }];
    });
  }
  // An edit's hunks from its diff lines: the text after the edit, as in the
  // file, and its changes.
  function editHunks(lines) {
    const sides = [];
    for (const [s, t] of lines) {
      if (s === "@" || !sides.length) sides.push({ old: "", nu: "" });
      if (s === "@") continue;
      const hk = sides[sides.length - 1];
      if (s !== "+") hk.old += t + "\n";
      if (s !== "-") hk.nu += t + "\n";
    }
    return sides.map(({ old, nu }) => ({ text: nu, changes: wordChanges(joinWrapped(old), joinWrapped(nu)) }));
  }
  window.editChanges = { lineDiff, editHunks };
})();
