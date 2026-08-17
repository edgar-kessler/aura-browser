// Texte in Deutsch und Englisch. Gewaehlt wird nach der Anzeigesprache von
// Windows; `--lang=de|en` erzwingt eine.
//
// Platzhalter in geschweiften Klammern fuellt `fill` – bewusst kein Format-
// Makro, damit die Texte hier als Daten stehen und nicht als Code.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    De,
    En,
}

pub struct Strings {
    pub app_name: &'static str,
    pub version_line: &'static str,
    pub lead: &'static str,

    pub opt_desktop: &'static str,
    pub opt_register: &'static str,
    pub folder_label: &'static str,
    pub change: &'static str,
    pub folder_dialog_title: &'static str,

    pub notice_update: &'static str,
    pub notice_same: &'static str,
    pub notice_webview2: &'static str,
    pub notice_running: &'static str,

    pub btn_install: &'static str,
    pub btn_update: &'static str,
    pub btn_reinstall: &'static str,
    pub btn_launch: &'static str,
    pub btn_close: &'static str,
    pub btn_cancel: &'static str,
    pub btn_remove: &'static str,
    pub btn_retry: &'static str,

    pub footer_license: &'static str,
    pub footer_source: &'static str,

    pub status_closing: &'static str,
    pub status_files: &'static str,
    pub status_uninstaller: &'static str,
    pub status_shortcuts: &'static str,
    pub status_registry: &'static str,
    pub status_webview2: &'static str,
    pub status_removing: &'static str,
    pub status_data: &'static str,

    pub done_title: &'static str,
    pub done_sub: &'static str,
    pub done_webview2_failed: &'static str,
    pub link_webview2: &'static str,
    pub link_default: &'static str,

    pub fail_title: &'static str,
    pub link_log: &'static str,

    pub un_title: &'static str,
    pub un_lead: &'static str,
    pub un_opt_data: &'static str,
    pub un_done: &'static str,

    pub err_no_payload: &'static str,
    pub err_space: &'static str,
    pub err_dir: &'static str,
    pub err_not_installed: &'static str,
}

pub const DE: Strings = Strings {
    app_name: "Aura Browser",
    version_line: "Version {v}",
    lead: "Nativer Windows-Browser mit eingebautem Werbeblocker.\nOhne Electron, ohne Hintergrunddienst.",

    opt_desktop: "Verknüpfung auf dem Desktop",
    opt_register: "Bei Windows als Browser anmelden",
    folder_label: "Ordner",
    change: "Ändern",
    folder_dialog_title: "Ordner für Aura Browser wählen",

    notice_update: "Version {old} ist installiert und wird aktualisiert.",
    notice_same: "Version {v} ist bereits installiert.",
    notice_webview2: "Die WebView2-Laufzeit fehlt und wird mitinstalliert.",
    notice_running: "Aura läuft gerade und wird dafür beendet.",

    btn_install: "Installieren",
    btn_update: "Aktualisieren",
    btn_reinstall: "Erneut installieren",
    btn_launch: "Aura starten",
    btn_close: "Schließen",
    btn_cancel: "Abbrechen",
    btn_remove: "Entfernen",
    btn_retry: "Erneut versuchen",

    footer_license: "MIT-Lizenz",
    footer_source: "Quelltext auf GitHub",

    status_closing: "Aura wird beendet …",
    status_files: "Dateien werden entpackt …",
    status_uninstaller: "Deinstallation wird vorbereitet …",
    status_shortcuts: "Verknüpfungen werden angelegt …",
    status_registry: "Aura wird bei Windows angemeldet …",
    status_webview2: "WebView2-Laufzeit wird geladen und installiert – das kann eine Minute dauern …",
    status_removing: "Dateien werden entfernt …",
    status_data: "Browserdaten werden gelöscht …",

    done_title: "Aura Browser ist installiert.",
    done_sub: "Version {v} · {dir}",
    done_webview2_failed: "Die WebView2-Laufzeit konnte nicht geladen werden. Ohne sie zeigt Aura keine Seiten – bitte von Microsoft nachinstallieren.",
    link_webview2: "WebView2 herunterladen",
    link_default: "Als Standardbrowser festlegen",

    fail_title: "Das hat nicht geklappt.",
    link_log: "Protokoll öffnen",

    un_title: "Aura Browser entfernen",
    un_lead: "Aura wird von diesem Computer entfernt.\nVerlauf, Lesezeichen und Passwörter bleiben erhalten, wenn du sie nicht mit entfernst.",
    un_opt_data: "Auch Browserdaten löschen (Verlauf, Lesezeichen, Passwörter)",
    un_done: "Aura Browser wurde entfernt.",

    err_no_payload: "Diese Datei enthält keine Programmdateien. Bitte das fertige Setup aus den Releases laden.",
    err_space: "Zu wenig freier Speicherplatz auf dem Ziellaufwerk.",
    err_dir: "Der Ordner {dir} lässt sich nicht anlegen.",
    err_not_installed: "Aura Browser ist auf diesem Computer nicht installiert.",
};

pub const EN: Strings = Strings {
    app_name: "Aura Browser",
    version_line: "Version {v}",
    lead: "A native Windows browser with a built-in ad blocker.\nNo Electron, no background service.",

    opt_desktop: "Desktop shortcut",
    opt_register: "Register as a browser with Windows",
    folder_label: "Folder",
    change: "Change",
    folder_dialog_title: "Choose a folder for Aura Browser",

    notice_update: "Version {old} is installed and will be updated.",
    notice_same: "Version {v} is already installed.",
    notice_webview2: "The WebView2 runtime is missing and will be installed as well.",
    notice_running: "Aura is running and will be closed for this.",

    btn_install: "Install",
    btn_update: "Update",
    btn_reinstall: "Reinstall",
    btn_launch: "Launch Aura",
    btn_close: "Close",
    btn_cancel: "Cancel",
    btn_remove: "Remove",
    btn_retry: "Try again",

    footer_license: "MIT license",
    footer_source: "Source on GitHub",

    status_closing: "Closing Aura …",
    status_files: "Unpacking files …",
    status_uninstaller: "Preparing the uninstaller …",
    status_shortcuts: "Creating shortcuts …",
    status_registry: "Registering with Windows …",
    status_webview2: "Downloading and installing the WebView2 runtime – this can take a minute …",
    status_removing: "Removing files …",
    status_data: "Deleting browser data …",

    done_title: "Aura Browser is installed.",
    done_sub: "Version {v} · {dir}",
    done_webview2_failed: "The WebView2 runtime could not be downloaded. Aura cannot show pages without it – please install it from Microsoft.",
    link_webview2: "Download WebView2",
    link_default: "Make it the default browser",

    fail_title: "That didn't work.",
    link_log: "Open log",

    un_title: "Remove Aura Browser",
    un_lead: "Aura will be removed from this computer.\nHistory, bookmarks and passwords stay unless you remove them too.",
    un_opt_data: "Also delete browser data (history, bookmarks, passwords)",
    un_done: "Aura Browser has been removed.",

    err_no_payload: "This file contains no program files. Please download the finished setup from the releases.",
    err_space: "Not enough free space on the target drive.",
    err_dir: "The folder {dir} cannot be created.",
    err_not_installed: "Aura Browser is not installed on this computer.",
};

pub fn strings(lang: Lang) -> &'static Strings {
    match lang {
        Lang::De => &DE,
        Lang::En => &EN,
    }
}

/// Ersetzt `{name}` durch den Wert.
pub fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in pairs {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}
