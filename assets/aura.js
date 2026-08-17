// Gemeinsame Bausteine der internen Seiten.
//
// Dropdown: ersetzt jedes <select> durch ein eigenes Menü. Das native Feld
// zeichnet Chromium in seinem eigenen Stil - ein hellgrauer Kasten mit
// System-Pfeil, der zu nichts sonst auf der Seite passt. Das native Element
// bleibt im Dokument (unsichtbar) und trägt den Wert; das Skript, das die
// Seite ohnehin daran hängt (change-Ereignis), läuft unverändert weiter.
//
// Öffnen mit Ein-Zoomen von 96 auf 100 Prozent und Fade, wie die Menüs der
// Oberfläche selbst. Tastatur: Pfeile, Enter, Escape, Anfangsbuchstabe.
(() => {
  'use strict';
  const build = sel => {
    if (sel.__aura) return;
    sel.__aura = true;
    const wrap = document.createElement('div');
    wrap.className = 'dd';
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dd-btn';
    btn.setAttribute('aria-haspopup', 'listbox');
    const label = document.createElement('span');
    btn.appendChild(label);
    btn.insertAdjacentHTML('beforeend',
      '<svg class="dd-chev" viewBox="0 0 24 24"><path d="M6 9l6 6 6-6"/></svg>');
    const menu = document.createElement('div');
    menu.className = 'dd-menu';
    menu.setAttribute('role', 'listbox');
    wrap.append(btn, menu);
    sel.parentNode.insertBefore(wrap, sel);
    wrap.appendChild(sel);
    sel.classList.add('dd-native');

    const sync = () => {
      const o = sel.options[sel.selectedIndex];
      label.textContent = o ? o.textContent : '';
      [...menu.children].forEach((c, i) => c.classList.toggle('sel', i === sel.selectedIndex));
    };
    const rebuild = () => {
      menu.innerHTML = '';
      [...sel.options].forEach((o, i) => {
        const it = document.createElement('div');
        it.className = 'dd-item';
        it.setAttribute('role', 'option');
        it.textContent = o.textContent;
        it.addEventListener('click', () => { choose(i); });
        menu.appendChild(it);
      });
      sync();
    };
    let open = false, hi = -1;
    const show = () => {
      if (open) return;
      document.querySelectorAll('.dd.open').forEach(d => d.__close && d.__close());
      open = true; hi = sel.selectedIndex;
      wrap.classList.add('open');
      // Nach unten, wenn Platz ist - sonst nach oben.
      const r = btn.getBoundingClientRect();
      const need = menu.scrollHeight + 8;
      wrap.classList.toggle('up', r.bottom + need > innerHeight && r.top > need);
      requestAnimationFrame(() => menu.classList.add('in'));
      [...menu.children].forEach((c, i) => c.classList.toggle('hi', i === hi));
      setTimeout(() => addEventListener('pointerdown', onOutside, {capture: true}), 0);
    };
    const hide = () => {
      if (!open) return;
      open = false;
      menu.classList.remove('in');
      wrap.classList.remove('open');
      removeEventListener('pointerdown', onOutside, {capture: true});
    };
    wrap.__close = hide;
    const onOutside = e => { if (!wrap.contains(e.target)) hide(); };
    const choose = i => {
      if (i < 0 || i >= sel.options.length) return;
      if (sel.selectedIndex !== i) {
        sel.selectedIndex = i;
        sel.dispatchEvent(new Event('change', {bubbles: true}));
      }
      sync(); hide(); btn.focus();
    };
    btn.addEventListener('click', () => open ? hide() : show());
    btn.addEventListener('keydown', e => {
      const n = sel.options.length;
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (!open) { show(); return; }
        hi = (hi + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
        [...menu.children].forEach((c, i) => c.classList.toggle('hi', i === hi));
      } else if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        if (open) choose(hi); else show();
      } else if (e.key === 'Escape') { hide(); }
      else if (e.key.length === 1) {
        const k = e.key.toLowerCase();
        const idx = [...sel.options].findIndex((o, i) => i > sel.selectedIndex && o.textContent.toLowerCase().startsWith(k));
        const j = idx >= 0 ? idx : [...sel.options].findIndex(o => o.textContent.toLowerCase().startsWith(k));
        if (j >= 0) choose(j);
      }
    });
    // Wenn die Seite den Wert per Skript setzt (init), nachziehen.
    new MutationObserver(rebuild).observe(sel, {childList: true});
    const desc = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value');
    Object.defineProperty(sel, 'value', {
      get() { return desc.get.call(this); },
      set(v) { desc.set.call(this, v); sync(); },
    });
    const descI = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'selectedIndex');
    Object.defineProperty(sel, 'selectedIndex', {
      get() { return descI.get.call(this); },
      set(v) { descI.set.call(this, v); sync(); },
    });
    rebuild();
  };
  const scan = () => document.querySelectorAll('select:not(.dd-native)').forEach(build);
  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', scan); else scan();
  new MutationObserver(scan).observe(document.documentElement, {childList: true, subtree: true});
})();
