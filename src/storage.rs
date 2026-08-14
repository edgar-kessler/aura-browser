// Persistent storage: SQLite database (history, bookmarks, settings, downloads, groups, permissions)
// plus a small JSON session file for restoring open tabs.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub struct Storage {
    pub conn: Connection,
    pub session_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub favicon: Option<Vec<u8>>,
    pub visit_count: i64,
    pub last_visit: i64,
}

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub id: i64,
    pub parent: i64,
    pub is_folder: bool,
    pub title: String,
    pub url: String,
    pub favicon: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub group: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

#[derive(Clone, Debug)]
pub struct DownloadEntry {
    pub id: i64,
    pub path: String,
    pub url: String,
    pub filename: String,
    pub total_bytes: i64,
    pub received_bytes: i64,
    pub finished: bool,
    pub started_at: i64,
}

#[derive(Clone, Debug)]
pub struct TabGroup {
    pub id: i64,
    pub name: String,
    pub color: String,
}

pub const GROUP_COLORS: &[(&str, &str)] = &[
    ("Violett", "#6E5BD0"),
    ("Blau", "#4A90D9"),
    ("Grün", "#3FA97A"),
    ("Orange", "#D98E4A"),
    ("Rosa", "#D95BA0"),
    ("Grau", "#8A8A9A"),
];

impl Storage {
    /// Root data dir: %LOCALAPPDATA%\AuraBrowser\<profile>\
    pub fn data_dir(profile: &str) -> PathBuf {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let dir = base.join("AuraBrowser").join(profile);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    pub fn open(profile: &str) -> std::result::Result<Storage, String> {
        let dir = Self::data_dir(profile);
        let db_path = dir.join("aura.db");
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        let conn = conn;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE IF NOT EXISTS history(
                url TEXT PRIMARY KEY,
                title TEXT,
                favicon BLOB,
                visit_count INTEGER DEFAULT 1,
                last_visit INTEGER);
            CREATE TABLE IF NOT EXISTS bookmarks(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent INTEGER DEFAULT 0,
                is_folder INTEGER DEFAULT 0,
                title TEXT,
                url TEXT,
                favicon BLOB,
                position INTEGER DEFAULT 0);
            CREATE TABLE IF NOT EXISTS closed_tabs(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT, title TEXT, closed_at INTEGER);
            CREATE TABLE IF NOT EXISTS downloads(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT, url TEXT, filename TEXT,
                total_bytes INTEGER DEFAULT 0, received_bytes INTEGER DEFAULT 0,
                finished INTEGER DEFAULT 0, started_at INTEGER);
            CREATE TABLE IF NOT EXISTS tab_groups(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT, color TEXT);
            CREATE TABLE IF NOT EXISTS permissions(
                origin TEXT, kind TEXT, allow INTEGER,
                PRIMARY KEY(origin, kind));
            CREATE TABLE IF NOT EXISTS passwords(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                origin TEXT, username TEXT, password BLOB);
            CREATE TABLE IF NOT EXISTS reading_list(
                url TEXT PRIMARY KEY,
                title TEXT, favicon BLOB,
                added_at INTEGER, read INTEGER DEFAULT 0);
            "#,
        )
        .map_err(|e| e.to_string())?;
        Ok(Storage {
            conn,
            session_path: dir.join("session.json"),
        })
    }

    // ---------- settings ----------
    pub fn get_setting(&self, key: &str, default: &str) -> String {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_else(|| default.to_string())
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        );
    }

    // ---------- history ----------
    pub fn add_history(&self, url: &str, title: &str, favicon: Option<&[u8]>) {
        let _ = self.conn.execute(
            "INSERT INTO history(url,title,favicon,visit_count,last_visit) VALUES(?1,?2,?3,1,?4)
             ON CONFLICT(url) DO UPDATE SET title=excluded.title,
                favicon=COALESCE(excluded.favicon, history.favicon),
                visit_count=history.visit_count+1, last_visit=excluded.last_visit",
            params![url, title, favicon, crate::util::now_unix()],
        );
    }

    pub fn search_history(&self, needle: &str, limit: usize) -> Vec<HistoryEntry> {
        let like = format!("%{needle}%");
        let mut st = match self.conn.prepare(
            "SELECT url,title,favicon,visit_count,last_visit FROM history
             WHERE title LIKE ?1 OR url LIKE ?1
             ORDER BY visit_count DESC, last_visit DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map(params![like, limit as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get(1)?,
                favicon: r.get(2)?,
                visit_count: r.get(3)?,
                last_visit: r.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    pub fn top_history(&self, limit: usize) -> Vec<HistoryEntry> {
        let mut st = match self.conn.prepare(
            "SELECT url,title,favicon,visit_count,last_visit FROM history
             ORDER BY visit_count DESC, last_visit DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map(params![limit as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get(1)?,
                favicon: r.get(2)?,
                visit_count: r.get(3)?,
                last_visit: r.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    pub fn clear_history(&self) {
        let _ = self.conn.execute("DELETE FROM history", []);
    }

    // ---------- bookmarks ----------
    pub fn add_bookmark(&self, title: &str, url: &str, favicon: Option<&[u8]>, parent: i64) -> i64 {
        let _ = self.conn.execute(
            "INSERT INTO bookmarks(parent,is_folder,title,url,favicon) VALUES(?1,0,?2,?3,?4)",
            params![parent, title, url, favicon],
        );
        self.conn.last_insert_rowid()
    }

    pub fn add_bookmark_folder(&self, title: &str, parent: i64) -> i64 {
        let _ = self.conn.execute(
            "INSERT INTO bookmarks(parent,is_folder,title,url) VALUES(?1,1,?2,'')",
            params![parent, title],
        );
        self.conn.last_insert_rowid()
    }

    pub fn remove_bookmark(&self, id: i64) {
        let _ = self
            .conn
            .execute("DELETE FROM bookmarks WHERE id=?1 OR parent=?1", params![id]);
    }

    /// History with server-side search and time range, newest visit first.
    pub fn history_page(&self, query: &str, since: i64, limit: usize) -> Vec<HistoryEntry> {
        let like = format!("%{}%", query.trim().to_lowercase());
        let sql = if query.trim().is_empty() {
            "SELECT url,title,favicon,visit_count,last_visit FROM history
             WHERE last_visit >= ?2 ORDER BY last_visit DESC LIMIT ?3"
        } else {
            "SELECT url,title,favicon,visit_count,last_visit FROM history
             WHERE last_visit >= ?2 AND (lower(url) LIKE ?1 OR lower(title) LIKE ?1)
             ORDER BY last_visit DESC LIMIT ?3"
        };
        let mut st = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map(params![like, since, limit as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get(1)?,
                favicon: r.get(2)?,
                visit_count: r.get(3)?,
                last_visit: r.get(4)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn remove_history_url(&self, url: &str) {
        let _ = self.conn.execute("DELETE FROM history WHERE url=?1", params![url]);
    }

    /// Deletes everything visited at or after `since` (0 = the whole history).
    pub fn remove_history_since(&self, since: i64) {
        let _ = self
            .conn
            .execute("DELETE FROM history WHERE last_visit >= ?1", params![since]);
    }

    pub fn history_count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // ---------- reading list ----------
    pub fn reading_add(&self, url: &str, title: &str, favicon: Option<&[u8]>) {
        let _ = self.conn.execute(
            "INSERT INTO reading_list(url,title,favicon,added_at,read) VALUES(?1,?2,?3,?4,0)
             ON CONFLICT(url) DO UPDATE SET title=excluded.title, favicon=coalesce(excluded.favicon, favicon)",
            params![url, title, favicon, crate::util::now_unix()],
        );
    }

    pub fn reading_remove(&self, url: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM reading_list WHERE url=?1", params![url]);
    }

    pub fn reading_set_read(&self, url: &str, read: bool) {
        let _ = self.conn.execute(
            "UPDATE reading_list SET read=?2 WHERE url=?1",
            params![url, read as i64],
        );
    }

    pub fn reading_has(&self, url: &str) -> bool {
        self.conn
            .query_row("SELECT 1 FROM reading_list WHERE url=?1", params![url], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// (url, title, favicon, added_at, read), unread first.
    pub fn reading_list(&self) -> Vec<(String, String, Option<Vec<u8>>, i64, bool)> {
        let mut st = match self.conn.prepare(
            "SELECT url,title,favicon,added_at,read FROM reading_list
             ORDER BY read ASC, added_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)? != 0,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    // ---------- passwords ----------
    /// (id, origin, username) — the secret stays encrypted until asked for.
    pub fn password_list(&self) -> Vec<(i64, String, String)> {
        let mut st = match self
            .conn
            .prepare("SELECT id,origin,username FROM passwords ORDER BY origin, username")
        {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn password_blob(&self, id: i64) -> Option<Vec<u8>> {
        self.conn
            .query_row("SELECT password FROM passwords WHERE id=?1", params![id], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn password_remove(&self, id: i64) {
        let _ = self
            .conn
            .execute("DELETE FROM passwords WHERE id=?1", params![id]);
    }

    /// Cached favicon for a URL (history first, then bookmarks).
    pub fn favicon_for(&self, url: &str) -> Option<Vec<u8>> {
        self.conn
            .query_row(
                "SELECT favicon FROM history WHERE url=?1 AND favicon IS NOT NULL",
                params![url],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .or_else(|| {
                self.conn
                    .query_row(
                        "SELECT favicon FROM bookmarks WHERE url=?1 AND favicon IS NOT NULL",
                        params![url],
                        |r| r.get(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            })
    }

    pub fn is_bookmarked(&self, url: &str) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT id FROM bookmarks WHERE url=?1 AND is_folder=0",
                params![url],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn all_bookmarks(&self) -> Vec<Bookmark> {
        let mut st = match self
            .conn
            .prepare("SELECT id,parent,is_folder,title,url,favicon FROM bookmarks ORDER BY is_folder DESC, position, title")
        {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map([], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                parent: r.get(1)?,
                is_folder: r.get::<_, i64>(2)? != 0,
                title: r.get(3)?,
                url: r.get(4)?,
                favicon: r.get(5)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    pub fn search_bookmarks(&self, needle: &str, limit: usize) -> Vec<Bookmark> {
        let like = format!("%{needle}%");
        let mut st = match self.conn.prepare(
            "SELECT id,parent,is_folder,title,url,favicon FROM bookmarks
             WHERE is_folder=0 AND (title LIKE ?1 OR url LIKE ?1) ORDER BY title LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map(params![like, limit as i64], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                parent: r.get(1)?,
                is_folder: false,
                title: r.get(3)?,
                url: r.get(4)?,
                favicon: r.get(5)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    // ---------- recently closed ----------
    pub fn push_closed_tab(&self, url: &str, title: &str) {
        if url.starts_with("aura://") || url.is_empty() {
            return;
        }
        let _ = self.conn.execute(
            "INSERT INTO closed_tabs(url,title,closed_at) VALUES(?1,?2,?3)",
            params![url, title, crate::util::now_unix()],
        );
        let _ = self.conn.execute(
            "DELETE FROM closed_tabs WHERE id NOT IN (SELECT id FROM closed_tabs ORDER BY id DESC LIMIT 25)",
            [],
        );
    }

    pub fn pop_closed_tab(&self) -> Option<(String, String)> {
        let row = self
            .conn
            .query_row(
                "SELECT id,url,title FROM closed_tabs ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()
            .ok()
            .flatten()?;
        let _ = self
            .conn
            .execute("DELETE FROM closed_tabs WHERE id=?1", params![row.0]);
        Some((row.1, row.2))
    }

    // ---------- downloads ----------
    pub fn add_download(&self, path: &str, url: &str, filename: &str) -> i64 {
        let _ = self.conn.execute(
            "INSERT INTO downloads(path,url,filename,started_at) VALUES(?1,?2,?3,?4)",
            params![path, url, filename, crate::util::now_unix()],
        );
        self.conn.last_insert_rowid()
    }

    pub fn update_download(&self, id: i64, received: i64, total: i64, finished: bool) {
        let _ = self.conn.execute(
            "UPDATE downloads SET received_bytes=?1,total_bytes=?2,finished=?3 WHERE id=?4",
            params![received, total, finished as i64, id],
        );
    }

    pub fn list_downloads(&self) -> Vec<DownloadEntry> {
        let mut st = match self.conn.prepare(
            "SELECT id,path,url,filename,total_bytes,received_bytes,finished,started_at FROM downloads ORDER BY id DESC LIMIT 100",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map([], |r| {
            Ok(DownloadEntry {
                id: r.get(0)?,
                path: r.get(1)?,
                url: r.get(2)?,
                filename: r.get(3)?,
                total_bytes: r.get(4)?,
                received_bytes: r.get(5)?,
                finished: r.get::<_, i64>(6)? != 0,
                started_at: r.get(7)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    // ---------- tab groups ----------
    pub fn list_groups(&self) -> Vec<TabGroup> {
        let mut st = match self
            .conn
            .prepare("SELECT id,name,color FROM tab_groups ORDER BY id")
        {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        st.query_map([], |r| {
            Ok(TabGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
            })
        })
        .map(|rows| rows.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }

    pub fn add_group(&self, name: &str, color: &str) -> i64 {
        let _ = self.conn.execute(
            "INSERT INTO tab_groups(name,color) VALUES(?1,?2)",
            params![name, color],
        );
        self.conn.last_insert_rowid()
    }

    // ---------- permissions ----------
    pub fn permission(&self, origin: &str, kind: &str) -> Option<bool> {
        self.conn
            .query_row(
                "SELECT allow FROM permissions WHERE origin=?1 AND kind=?2",
                params![origin, kind],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten()
            .map(|v| v != 0)
    }

    pub fn set_permission(&self, origin: &str, kind: &str, allow: bool) {
        let _ = self.conn.execute(
            "INSERT INTO permissions(origin,kind,allow) VALUES(?1,?2,?3) ON CONFLICT(origin,kind) DO UPDATE SET allow=excluded.allow",
            params![origin, kind, allow as i64],
        );
    }

    // ---------- session ----------
    pub fn save_session(&self, session: &Session) {
        if self.get_setting("restore_session", "1") != "1" {
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(session) {
            let _ = std::fs::write(&self.session_path, json);
        }
    }

    pub fn load_session(&self) -> Option<Session> {
        if self.get_setting("restore_session", "1") != "1" {
            return None;
        }
        let data = std::fs::read_to_string(&self.session_path).ok()?;
        serde_json::from_str(&data).ok()
    }
}

pub fn dir_webview(profile: &str) -> PathBuf {
    Storage::data_dir(profile).join("webview2")
}

pub fn assets_dir() -> PathBuf {
    // Assets live next to the exe (portable) or in the dev tree.
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.with_file_name("assets");
        if p.exists() {
            return p;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}
