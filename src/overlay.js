// Injected into every document before the page's own scripts run.
// `__OXDEMO_THEME__` is replaced with the theme as JSON.
(() => {
  if (window.__oxdemo) return;
  const T = __OXDEMO_THEME__;
  const ours = new Set();
  const state = { last: null, dragStarted: false };

  const store = (k, v) => { try { sessionStorage.setItem('oxdemo-' + k, JSON.stringify(v)); } catch (_) {} };
  const load = (k) => { try { return JSON.parse(sessionStorage.getItem('oxdemo-' + k)); } catch (_) { return null; } };

  let cursor, ring, caption, badge;
  const place = (x, y) => {
    if (!cursor) return;
    cursor.style.left = x + 'px'; cursor.style.top = y + 'px';
    ring.style.left = x + 'px'; ring.style.top = y + 'px';
    store('pos', [x, y]);
  };

  const install = () => {
    if (document.getElementById('oxdemo-cursor') || !document.body) return;
    if (T.css) { const st = document.createElement('style'); st.textContent = T.css; document.head.append(st); }
    cursor = document.createElement('div');
    cursor.id = 'oxdemo-cursor';
    cursor.innerHTML = '<svg width="22" height="22" viewBox="0 0 22 22"><path d="M3 2 L3 18 L7.5 13.8 L10.6 20.2 L13.4 18.9 L10.4 12.6 L16.5 12.4 Z" fill="' + T.cursor + '" stroke="' + T.cursor_stroke + '" stroke-width="1.3" stroke-linejoin="round"/></svg>';
    Object.assign(cursor.style, { position: 'fixed', left: '-40px', top: '-40px', zIndex: 2147483647, pointerEvents: 'none', transition: 'transform .08s', transformOrigin: '3px 2px' });
    ring = document.createElement('div');
    Object.assign(ring.style, { position: 'fixed', left: '-80px', top: '-80px', width: '34px', height: '34px', marginLeft: '-14px', marginTop: '-14px', borderRadius: '50%', border: '2px solid ' + T.ring, opacity: '0', zIndex: 2147483646, pointerEvents: 'none', transition: 'opacity .35s, transform .35s', transform: 'scale(.4)', boxSizing: 'border-box' });
    caption = document.createElement('div');
    caption.id = 'oxdemo-caption';
    const edge = T.position === 'top' ? { top: '26px' } : { bottom: '26px' };
    Object.assign(caption.style, edge, { position: 'fixed', left: '50%', transform: 'translateX(-50%)', maxWidth: '980px', width: 'max-content', padding: '11px 20px', background: T.caption_bg, color: T.caption_fg, font: '500 ' + T.size + 'px/1.35 ' + T.font, borderRadius: '6px', zIndex: 2147483645, pointerEvents: 'none', opacity: '0', transition: 'opacity .3s', textAlign: 'center', boxShadow: '0 6px 24px rgba(0,0,0,.18)' });
    badge = document.createElement('div');
    Object.assign(badge.style, { position: 'fixed', left: '50%', bottom: T.position === 'top' ? '26px' : '92px', transform: 'translateX(-50%) scale(.9)', padding: '8px 16px', background: T.caption_bg, color: T.caption_fg, font: '600 ' + (T.size + 5) + 'px/1 ' + T.font, borderRadius: '8px', zIndex: 2147483645, pointerEvents: 'none', opacity: '0', transition: 'opacity .2s, transform .2s', letterSpacing: '.04em' });
    for (const el of [ring, cursor, caption, badge]) { ours.add(el); el.setAttribute('data-oxdemo', ''); }
    document.body.append(ring, cursor, caption, badge);
    const pos = load('pos');
    if (pos) place(pos[0], pos[1]);
    const text = load('caption');
    if (text) { caption.textContent = text; caption.style.transition = 'none'; caption.style.opacity = '1'; requestAnimationFrame(() => { caption.style.transition = 'opacity .3s'; }); }
  };

  window.addEventListener('mousemove', (e) => place(e.clientX, e.clientY), true);
  window.addEventListener('dragover', (e) => place(e.clientX, e.clientY), true);
  window.addEventListener('dragstart', () => { state.dragStarted = true; }, true);
  window.addEventListener('mousedown', () => {
    if (!ring) return;
    ring.style.opacity = '.9'; ring.style.transform = 'scale(1)'; cursor.style.transform = 'scale(.9)';
    setTimeout(() => { ring.style.opacity = '0'; ring.style.transform = 'scale(.4)'; cursor.style.transform = ''; }, 260);
  }, true);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', install); else install();

  // ---- target resolution -------------------------------------------------

  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const shown = (el) => {
    if (ours.has(el)) return false;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    return el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : true;
  };
  const INTERACTIVE = 'button,a,[role=button],[role=tab],[role=menuitem],[role=option],[role=link],summary,label,select,input,textarea,[draggable=true],[contenteditable=""],[contenteditable=true]';
  const all = (root) => Array.from(root.querySelectorAll('*'));
  // Keep the innermost matches: drop any element that contains another match.
  const innermost = (els) => els.filter((el) => !els.some((o) => o !== el && el.contains(o)));
  // Lift a text match to the control it belongs to, when the control says the same thing.
  const lift = (els, q, same) => Array.from(new Set(els.map((el) => {
    const c = el.closest(INTERACTIVE);
    return c && same(c, q) ? c : el;
  })));
  const nameOf = (el) => {
    const bits = [el.getAttribute('aria-label'), el.getAttribute('title'), el.getAttribute('placeholder'), el.getAttribute('alt')];
    const by = el.getAttribute('aria-labelledby');
    if (by) bits.push(by.split(/\s+/).map((id) => document.getElementById(id)).filter(Boolean).map((e) => e.textContent).join(' '));
    if (el.id) { const l = document.querySelector('label[for="' + CSS.escape(el.id) + '"]'); if (l) bits.push(l.textContent); }
    return bits.filter(Boolean).map(norm);
  };

  const tiers = (root, q, kind) => {
    const out = [];
    if (kind === 'css') { out.push(['css', () => Array.from(root.querySelectorAll(q))]); return out; }
    if (kind !== 'text') {
      out.push(['aria-label', () => all(root).filter((el) => norm(el.getAttribute('aria-label')) === q)]);
      out.push(['name', () => all(root).filter((el) => nameOf(el).includes(q))]);
    }
    if (kind !== 'label') {
      const exact = (el, s) => norm(el.innerText) === s;
      out.push(['text', () => lift(innermost(all(root).filter((el) => exact(el, q))), q, exact)]);
      out.push(['text contains', () => innermost(all(root).filter((el) => norm(el.innerText).includes(q)))]);
    }
    if (kind === 'any' && /^[.#\[a-z]/i.test(q)) {
      out.push(['css', () => { try { return Array.from(root.querySelectorAll(q)); } catch (_) { return []; } }]);
    }
    return out;
  };

  const find = (roots, part) => {
    let kind = 'any', q = part;
    const m = /^(css|text|label):(.*)$/s.exec(part);
    if (m) { kind = m[1]; q = m[2]; }
    q = kind === 'css' ? q.trim() : norm(q);
    for (const root of roots) {
      for (const [tier, run] of tiers(root, q, kind)) {
        const hits = run().filter(shown);
        if (hits.length) return { tier, hits };
      }
    }
    return { tier: null, hits: [] };
  };

  const describe = (el) => {
    let s = el.tagName.toLowerCase();
    const label = el.getAttribute('aria-label');
    if (el.id) s += '#' + el.id;
    if (label) s += '[aria-label="' + label + '"]';
    const text = norm(el.innerText).slice(0, 40);
    if (text) s += ' "' + text + '"';
    return s;
  };
  // A selector that would pick out exactly this element, if one is easy to find.
  const suggest = (el, q) => {
    const label = el.getAttribute('aria-label');
    if (label && document.querySelectorAll('[aria-label="' + CSS.escape(label) + '"]').length === 1) return label;
    if (el.id) return 'css:#' + CSS.escape(el.id);
    for (let a = el.parentElement; a && a !== document.body; a = a.parentElement) {
      const al = a.getAttribute('aria-label');
      if (al) return 'css:' + a.tagName.toLowerCase() + "[aria-label='" + al + "'] >> " + q;
      if (a.id) return 'css:#' + CSS.escape(a.id) + ' >> ' + q;
    }
    return null;
  };

  const box = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, inView: r.top >= 0 && r.left >= 0 && r.bottom <= innerHeight && r.right <= innerWidth };
  };

  const resolve = (q) => {
    const parts = q.split(/\s+>>\s+/);
    let roots = [document];
    let found = { tier: null, hits: [] };
    for (let i = 0; i < parts.length; i++) {
      found = find(roots, parts[i]);
      if (!found.hits.length) {
        const where = i ? ' inside "' + parts.slice(0, i).join(' >> ') + '"' : '';
        return { ok: false, error: 'nothing matches "' + parts[i] + '"' + where, near: nearMisses(parts[i]) };
      }
      roots = found.hits;
    }
    const hits = found.hits;
    if (hits.length > 1) {
      return { ok: false, error: '"' + q + '" matches ' + hits.length + ' elements (by ' + found.tier + ')',
        candidates: hits.slice(0, 6).map((el) => ({ desc: describe(el), use: suggest(el, parts[parts.length - 1]) })) };
    }
    state.last = hits[0];
    return { ok: true, tier: found.tier, desc: describe(hits[0]), box: box(hits[0]) };
  };

  // For "nothing matches": a few labels that do exist, closest first.
  const nearMisses = (q) => {
    const low = norm(q).toLowerCase();
    const seen = new Set();
    const names = [];
    for (const el of all(document)) {
      if (!shown(el) || !el.matches(INTERACTIVE)) continue;
      for (const n of [el.getAttribute('aria-label'), norm(el.innerText)]) {
        if (n && n.length < 60 && !seen.has(n)) { seen.add(n); names.push(n); }
      }
    }
    const score = (n) => { const l = n.toLowerCase(); return l.includes(low) || low.includes(l) ? 0 : 1 + Math.abs(l.length - low.length) / 10; };
    return names.sort((a, b) => score(a) - score(b)).slice(0, 5);
  };

  // ---- settling ------------------------------------------------------------

  const running = () => document.getAnimations().filter((a) => {
    if (a.playState !== 'running') return false;
    const t = a.effect && a.effect.getComputedTiming ? a.effect.getComputedTiming() : null;
    const target = a.effect && a.effect.target;
    if (target && (ours.has(target) || [...ours].some((o) => o.contains(target)))) return false;
    return t && Number.isFinite(t.endTime);
  }).length;
  const loadingImages = () => Array.from(document.images).filter((i) => !i.complete && i.loading !== 'lazy').length;

  // Resolves once nothing has changed for `quiet` ms, or after `cap` ms.
  const settle = (quiet, cap) => new Promise((done) => {
    let mutations = 0;
    let last = performance.now();
    const start = last;
    const obs = new MutationObserver((records) => {
      for (const r of records) {
        if (ours.has(r.target) || [...ours].some((o) => o.contains(r.target))) continue;
        mutations++;
        last = performance.now();
      }
    });
    obs.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    const tick = () => {
      const now = performance.now();
      if (running() || loadingImages()) last = Math.max(last, now - quiet / 2);
      if (now - last >= quiet || now - start >= cap) { obs.disconnect(); done({ mutations, ms: Math.round(now - start) }); return; }
      setTimeout(tick, 30);
    };
    (document.fonts ? document.fonts.ready : Promise.resolve()).then(() => setTimeout(tick, 30));
  });

  window.__oxdemo = {
    resolve,
    box: () => state.last ? box(state.last) : null,
    scroll: () => state.last && state.last.scrollIntoView({ block: 'center', inline: 'center', behavior: 'smooth' }),
    // What a click at (x, y) would land on, if not the resolved element.
    blocker: (x, y) => {
      const hit = document.elementFromPoint(x, y);
      if (!hit || !state.last || state.last.contains(hit) || hit.contains(state.last)) return null;
      return describe(hit);
    },
    focus: () => { state.last.focus(); return document.activeElement === state.last; },
    clear: () => { const el = state.last; if (el.select) el.select(); else { const r = document.createRange(); r.selectNodeContents(el); const s = getSelection(); s.removeAllRanges(); s.addRange(r); } },
    pick: (value) => {
      const el = state.last;
      if (!el || el.tagName !== 'SELECT') return { ok: false, error: 'not a <select>: ' + (el ? describe(el) : 'nothing') };
      const opt = Array.from(el.options).find((o) => o.value === value) || Array.from(el.options).find((o) => norm(o.textContent) === value);
      if (!opt) return { ok: false, error: 'no option "' + value + '"; options are ' + Array.from(el.options).map((o) => '"' + o.value + '"').join(', ') };
      el.value = opt.value;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return { ok: true };
    },
    settle,
    caption: (text) => {
      store('caption', text);
      if (!caption) install();
      if (!caption) return;
      if (text) caption.textContent = text;
      caption.style.opacity = text ? '1' : '0';
    },
    badge: (text) => {
      if (!badge) install();
      if (!badge) return;
      badge.textContent = text;
      badge.style.opacity = '1'; badge.style.transform = 'translateX(-50%) scale(1)';
      clearTimeout(badge.__t);
      badge.__t = setTimeout(() => { badge.style.opacity = '0'; badge.style.transform = 'translateX(-50%) scale(.9)'; }, 900);
    },
    // A hash of the page without the overlay, to tell whether an action changed anything.
    signature: () => {
      const c = document.body.cloneNode(true);
      c.querySelectorAll('[data-oxdemo]').forEach((e) => e.remove());
      const s = c.innerHTML;
      let h = 0;
      for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
      return s.length + ':' + h;
    },
    // Where the caption and key badge sit, so zoom can leave them in place.
    boxes: () => {
      const r = (el) => { if (!el) return null; const b = el.getBoundingClientRect(); return b.width ? [b.x, b.y, b.width, b.height] : null; };
      return { caption: caption && caption.textContent ? r(caption) : null, badge: r(badge) };
    },
    dragStarted: () => { const s = state.dragStarted; state.dragStarted = false; return s; },
    place,
  };
})();
