// Aura Shield – läuft vor jedem Seitenskript (AddScriptToExecuteOnDocumentCreated).
//
// Zwei Aufgaben:
//  1. Ersatzobjekte für geblockte Werbe-SDKs, damit Seiten nicht in Fehler laufen
//     oder "Bitte Adblocker deaktivieren" anzeigen.
//  2. Popunder abfangen: window.open ohne echte Nutzerinteraktion wird verworfen.
//
// Bewusst konservativ: alles, was echte Klicks braucht (Zahlungs-Popups,
// OAuth-Fenster), läuft weiter. Der strenge Modus ist optional.
(() => {
  'use strict';
  try {
    if (location.hostname === 'aura.internal') return;
    const AURA = window.__auraShield || (window.__auraShield = {blocked: 0});
    const noop = function () {};

    // ---------------------------------------------------------------- Stubs
    // Google AdSense: viele Seiten prüfen, ob push() existiert.
    if (!window.adsbygoogle) {
      window.adsbygoogle = [];
      window.adsbygoogle.push = noop;
      window.adsbygoogle.loaded = true;
    }

    // Google Publisher Tag: cmd.push muss Callbacks sofort ausführen, sonst
    // bleibt Seitenlogik hängen, die darin auf Inhalte wartet.
    const gt = (window.googletag = window.googletag || {});
    if (!gt.__aura) {
      gt.__aura = true;
      const run = f => { try { if (typeof f === 'function') f(); } catch (e) {} };
      const queued = Array.isArray(gt.cmd) ? gt.cmd.slice() : [];
      gt.cmd = {push: run, length: 0};
      const slot = {
        addService() { return slot; },
        setTargeting() { return slot; },
        setCollapseEmptyDiv() { return slot; },
        defineSizeMapping() { return slot; },
        getSlotElementId() { return ''; },
      };
      gt.defineSlot = () => slot;
      gt.defineOutOfPageSlot = () => slot;
      gt.pubads = () => ({
        addEventListener: noop, removeEventListener: noop, refresh: noop,
        setTargeting: noop, enableSingleRequest: noop, collapseEmptyDivs: noop,
        disableInitialLoad: noop, setCentering: noop, getSlots: () => [],
        updateCorrelator: noop, clear: noop, isInitialLoadDisabled: () => false,
      });
      gt.enableServices = noop;
      gt.display = noop;
      gt.destroySlots = noop;
      gt.sizeMapping = () => {
        const b = {addSize() { return b; }, build() { return []; }};
        return b;
      };
      gt.apiReady = true;
      gt.pubadsReady = true;
      queued.forEach(run);
    }

    // Messpixel: nur anlegen, wenn die Seite sie noch nicht selbst definiert hat.
    if (typeof window.fbq === 'undefined') { window.fbq = noop; window.fbq.queue = []; }
    if (typeof window.gtag === 'undefined') window.gtag = noop;
    if (typeof window.ga === 'undefined') window.ga = noop;
    if (typeof window._gaq === 'undefined') window._gaq = {push: noop};
    // dataLayer bleibt unangetastet – Seiten nutzen ihn auch für eigene Logik.

    // ---------------------------------------------------------------- Popunder
    const realOpen = window.open;
    const dummy = () => ({
      closed: true, close: noop, focus: noop, blur: noop, postMessage: noop,
      opener: null, name: '',
      location: {href: 'about:blank', replace: noop, assign: noop, reload: noop},
      document: {write: noop, writeln: noop, close: noop, open: noop},
    });
    window.open = function (url, name, features) {
      const active = navigator.userActivation ? navigator.userActivation.isActive : true;
      if (!active) {
        AURA.blocked++;                       // rein programmatisch → Popunder
        return dummy();
      }
      if (window.__auraStrictPopups) {
        // Strenger Modus: nur Links dürfen Fenster öffnen.
        const ev = window.event;
        const fromLink = !!(ev && ev.target && ev.target.closest && ev.target.closest('a[href]'));
        if (!fromLink) {
          AURA.blocked++;
          return dummy();
        }
      }
      return realOpen.apply(window, arguments);
    };
    // Manche Seiten prüfen, ob window.open überschrieben wurde.
    try {
      window.open.toString = () => 'function open() { [native code] }';
    } catch (e) {}
  } catch (e) {}
})();
