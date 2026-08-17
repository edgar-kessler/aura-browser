// Ersetzt Chromiums Fehlerseite (mit „Microsoft Edge“ in der Fußzeile) durch
// eine eigene im Aura-Stil. Wird nur ausgeführt, wenn die Seite wirklich die
// eingebaute Fehlerseite ist – Seiten mit eigenem 404-Layout bleiben unberührt.
(() => {
  'use strict';
  try {
    // Chromiums Fehlerseite hat immer diesen Container.
    const marker = document.getElementById('main-frame-error')
      || document.querySelector('.neterror, .interstitial-wrapper');
    if (!marker) return;

    const accent = '__ACCENT__';
    const dark = __DARK__;
    const info = window.__auraError || {};
    const fg = dark ? '250,250,250' : '9,9,11';
    const dim = dark ? '161,161,170' : '113,113,122';
    const bg = dark ? '9,9,11' : '255,255,255';
    const line = dark ? '63,63,70' : '212,212,216';
    // Helle Akzente brauchen dunkle Schrift, sonst ist der Button unlesbar.
    const [ar, ag, ab] = accent.split(',').map(Number);
    const onAccent = (ar * .299 + ag * .587 + ab * .114) > 150 ? '9,9,11' : '255,255,255';

    // Ueberschrift und Erklaerung aus dem Fehlercode ableiten.
    const status = info.status || 0;
    const reason = info.reason || '';
    let title = 'Diese Seite lässt sich nicht öffnen';
    let text = 'Die Verbindung kam nicht zustande.';
    if (status === 403) {
      title = 'Zugriff verweigert';
      text = 'Der Server hat die Anfrage abgelehnt. Oft hilft eine Anmeldung — oder die Seite ist in deiner Region gesperrt.';
    } else if (status === 404) {
      title = 'Seite nicht gefunden';
      text = 'Unter dieser Adresse liegt nichts. Vielleicht wurde sie verschoben oder gelöscht.';
    } else if (status === 429) {
      title = 'Zu viele Anfragen';
      text = 'Der Server bremst dich aus. In ein paar Minuten noch einmal versuchen.';
    } else if (status >= 500) {
      title = 'Der Server hat ein Problem';
      text = 'Das liegt nicht an dir. Später noch einmal versuchen.';
    } else if (reason.includes('NAME_NOT_RESOLVED') || reason.includes('HostNameNotResolved')) {
      title = 'Adresse nicht gefunden';
      text = 'Diesen Servernamen kennt das Netz nicht. Tippfehler in der Adresse?';
    } else if (reason.includes('CONNECTION') || reason.includes('ConnectionAborted')) {
      title = 'Keine Verbindung';
      text = 'Der Server antwortet nicht. Prüfe deine Internetverbindung.';
    } else if (reason.includes('InvalidServerResponse')) {
      title = 'Der Server hat unerwartet geantwortet';
      text = 'Die Antwort war leer oder fehlerhaft — häufig steckt eine Sperre oder eine Wartung dahinter.';
    } else if (reason.includes('Timeout')) {
      title = 'Zeitüberschreitung';
      text = 'Der Server hat zu lange gebraucht. Später noch einmal versuchen.';
    } else if (reason.includes('CERT') || reason.includes('Certificate')) {
      title = 'Verbindung nicht sicher';
      text = 'Das Zertifikat der Seite ist ungültig. Aura hat die Verbindung deshalb abgebrochen.';
    }

    // location zeigt hier auf chromewebdata – die echte Adresse kommt vom Host.
    let host = info.url || location.href;
    try {
      host = new URL(host).hostname || host;
    } catch (e) {}
    document.documentElement.innerHTML = `
<head><meta charset="utf-8"><title>${title}</title></head>
<body>
  <div class="wrap">
    <svg viewBox="0 0 24 24" class="ico" aria-hidden="true">
      <circle cx="12" cy="12" r="9"/><path d="M12 8v5"/><path d="M12 16h.01"/>
    </svg>
    <h1>${title}</h1>
    <p class="lead">${text}</p>
    <p class="host">${host}${status ? ` · HTTP ${status}` : ''}${reason && !status ? ` · ${reason}` : ''}</p>
    <div class="row">
      <button id="retry">Neu laden</button>
      <button id="back" class="ghost">Zurück</button>
    </div>
  </div>
</body>`;

    const style = document.createElement('style');
    style.textContent = `
      *{margin:0;padding:0;box-sizing:border-box}
      body{min-height:100vh;display:grid;place-items:center;padding:40px;
        font-family:"Segoe UI Variable Text","Segoe UI",system-ui,sans-serif;
        background:rgb(${bg});color:rgb(${fg})}
      .wrap{max-width:480px;text-align:center;animation:rise .2s cubic-bezier(.4,0,.2,1) both}
      .ico{width:44px;height:44px;margin-bottom:20px;stroke:rgb(${dim});fill:none;stroke-width:1.5;
        stroke-linecap:round}
      h1{font-size:24px;font-weight:600;letter-spacing:-.4px;margin-bottom:8px}
      .lead{font-size:14px;line-height:1.6;color:rgb(${dim})}
      .host{margin-top:16px;font-size:12px;color:rgb(${dim});word-break:break-all}
      .row{display:flex;gap:8px;justify-content:center;margin-top:28px}
      button{display:inline-flex;align-items:center;justify-content:center;height:36px;padding:0 16px;
        border-radius:6px;border:1px solid transparent;
        background:rgb(${accent});color:rgb(${onAccent});font:inherit;font-size:14px;font-weight:500;cursor:pointer;
        transition:background .15s cubic-bezier(.4,0,.2,1),border-color .15s}
      button:hover{filter:brightness(1.06)}
      button.ghost{background:transparent;color:rgb(${fg});border-color:rgb(${line})}
      button.ghost:hover{background:rgb(${line});filter:none}
      @keyframes rise{from{opacity:0;transform:translateY(4px)}to{opacity:1;transform:none}}`;
    document.head.appendChild(style);

    document.getElementById('retry').addEventListener('click', () => {
      if (info.url) location.href = info.url;
      else location.reload();
    });
    document.getElementById('back').addEventListener('click', () => history.back());
  } catch (e) {}
})();
