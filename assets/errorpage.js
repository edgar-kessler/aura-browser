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
    const fg = dark ? '236,237,238' : '17,24,28';
    const dim = dark ? '150,150,162' : '113,113,122';
    const bg = dark ? '9,9,11' : '255,255,255';
    const line = dark ? '255,255,255' : '24,24,27';

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
      .wrap{max-width:520px;text-align:center;animation:rise .45s cubic-bezier(.32,.72,0,1) both}
      .ico{width:56px;height:56px;margin-bottom:22px;stroke:rgb(${accent});fill:none;stroke-width:1.5;
        stroke-linecap:round}
      h1{font-size:26px;font-weight:600;letter-spacing:-.5px;margin-bottom:10px}
      .lead{font-size:15px;line-height:1.6;color:rgb(${dim})}
      .host{margin-top:18px;font-size:12px;color:rgb(${dim});opacity:.75;word-break:break-all}
      .row{display:flex;gap:10px;justify-content:center;margin-top:28px}
      button{padding:10px 20px;border-radius:12px;border:1px solid transparent;
        background:rgb(${accent});color:#fff;font:inherit;font-size:14px;cursor:pointer;
        transition:transform .2s cubic-bezier(.32,.72,0,1),filter .2s}
      button:hover{transform:translateY(-1px);filter:brightness(1.08)}
      button.ghost{background:transparent;color:rgb(${fg});border-color:rgba(${line},.22)}
      button.ghost:hover{border-color:rgb(${accent});color:rgb(${accent});filter:none}
      @keyframes rise{from{opacity:0;transform:translateY(10px)}to{opacity:1;transform:none}}`;
    document.head.appendChild(style);

    document.getElementById('retry').addEventListener('click', () => {
      if (info.url) location.href = info.url;
      else location.reload();
    });
    document.getElementById('back').addEventListener('click', () => history.back());
  } catch (e) {}
})();
