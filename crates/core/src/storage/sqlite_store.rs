use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::models::{AppSettings, Cookie, HistoryEntry, HttpMethod};

const SETTINGS_KEY: &str = "app_settings";

/// SQLite-backed store for request History, Cookies, and AppSettings.
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Opens or creates a SQLite database at `path` and initializes the schema.
    pub fn open(path: impl AsRef<Path>) -> CoreResult<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Opens an in-memory SQLite database (ideal for tests).
    pub fn open_in_memory() -> CoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> CoreResult<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS history (
                id            TEXT PRIMARY KEY,
                timestamp     TEXT NOT NULL,
                method        TEXT NOT NULL,
                url           TEXT NOT NULL,
                status        INTEGER,
                duration_ms   REAL,
                request_json  TEXT NOT NULL,
                response_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_history_timestamp ON history (timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_history_url ON history (url);

            CREATE TABLE IF NOT EXISTS cookies (
                domain    TEXT NOT NULL,
                path      TEXT NOT NULL,
                name      TEXT NOT NULL,
                value     TEXT NOT NULL,
                expires   TEXT,
                http_only INTEGER NOT NULL,
                secure    INTEGER NOT NULL,
                same_site TEXT,
                PRIMARY KEY (domain, path, name)
            );
            CREATE INDEX IF NOT EXISTS idx_cookies_domain ON cookies (domain);

            CREATE TABLE IF NOT EXISTS settings (
                key        TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    // ---- History ----

    pub fn insert_history(&self, entry: &HistoryEntry) -> CoreResult<()> {
        let request_json = serde_json::to_string(&entry.request)?;
        let response_json = entry
            .response
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        self.conn.execute(
            "INSERT OR REPLACE INTO history
             (id, timestamp, method, url, status, duration_ms, request_json, response_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.id.to_string(),
                entry.timestamp.to_rfc3339(),
                entry.method.as_str(),
                entry.url,
                entry.status.map(i64::from),
                entry.duration_ms,
                request_json,
                response_json,
            ],
        )?;
        Ok(())
    }

    pub fn list_history(&self, limit: usize, offset: usize) -> CoreResult<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, method, url, status, duration_ms, request_json, response_json
             FROM history
             ORDER BY timestamp DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt.query_map(params![limit as i64, offset as i64], decode_history_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub fn search_history(&self, query: &str, limit: usize) -> CoreResult<Vec<HistoryEntry>> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, method, url, status, duration_ms, request_json, response_json
             FROM history
             WHERE url LIKE ?1 OR method LIKE ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![pattern, limit as i64], decode_history_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub fn delete_history(&self, id: Uuid) -> CoreResult<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }

    pub fn clear_history(&self) -> CoreResult<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    pub fn prune_history(&self, max_entries: usize) -> CoreResult<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM history WHERE id NOT IN (
                SELECT id FROM history ORDER BY timestamp DESC LIMIT ?1
            )",
            params![max_entries as i64],
        )?;
        Ok(deleted)
    }

    // ---- Cookies ----

    pub fn upsert_cookie(&self, cookie: &Cookie) -> CoreResult<()> {
        let expires = cookie.expires.map(|dt| dt.to_rfc3339());
        self.conn.execute(
            "INSERT OR REPLACE INTO cookies
             (domain, path, name, value, expires, http_only, secure, same_site)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                cookie.domain,
                cookie.path,
                cookie.name,
                cookie.value,
                expires,
                i64::from(cookie.http_only),
                i64::from(cookie.secure),
                cookie.same_site,
            ],
        )?;
        Ok(())
    }

    pub fn get_cookies_for_domain(&self, domain: &str) -> CoreResult<Vec<Cookie>> {
        let mut stmt = self.conn.prepare(
            "SELECT domain, path, name, value, expires, http_only, secure, same_site
             FROM cookies
             WHERE LTRIM(LOWER(domain), '.') = LOWER(?1)
                OR LOWER(?1) LIKE ('%.' || LTRIM(LOWER(domain), '.'))
             ORDER BY domain, path, name",
        )?;

        let rows = stmt.query_map(params![domain], decode_cookie_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub fn list_cookies(&self) -> CoreResult<Vec<Cookie>> {
        let mut stmt = self.conn.prepare(
            "SELECT domain, path, name, value, expires, http_only, secure, same_site
             FROM cookies
             ORDER BY domain, path, name",
        )?;

        let rows = stmt.query_map([], decode_cookie_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub fn delete_cookie(&self, domain: &str, path: &str, name: &str) -> CoreResult<()> {
        self.conn.execute(
            "DELETE FROM cookies WHERE domain = ?1 AND path = ?2 AND name = ?3",
            params![domain, path, name],
        )?;
        Ok(())
    }

    pub fn clear_cookies(&self) -> CoreResult<()> {
        self.conn.execute("DELETE FROM cookies", [])?;
        Ok(())
    }

    // ---- Settings ----

    pub fn save_settings(&self, settings: &AppSettings) -> CoreResult<()> {
        let json = serde_json::to_string(settings)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value_json) VALUES (?1, ?2)",
            params![SETTINGS_KEY, json],
        )?;
        Ok(())
    }

    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![SETTINGS_KEY],
                |row| row.get(0),
            )
            .optional()?;

        match raw {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(AppSettings::default()),
        }
    }
}

fn decode_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoreResult<HistoryEntry>> {
    let id_str: String = row.get(0)?;
    let ts_str: String = row.get(1)?;
    let method_str: String = row.get(2)?;
    let url: String = row.get(3)?;
    let status: Option<i64> = row.get(4)?;
    let duration_ms: Option<f64> = row.get(5)?;
    let request_json: String = row.get(6)?;
    let response_json: Option<String> = row.get(7)?;

    let parsed = (|| -> CoreResult<HistoryEntry> {
        let id = Uuid::parse_str(&id_str)
            .map_err(|e| CoreError::InvalidData(format!("invalid uuid: {e}")))?;
        let timestamp = DateTime::parse_from_rfc3339(&ts_str)
            .map_err(|e| CoreError::InvalidData(format!("invalid timestamp: {e}")))?
            .with_timezone(&Utc);
        let method = method_str.parse::<HttpMethod>().unwrap();
        let request = serde_json::from_str(&request_json)?;
        let response = response_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;

        Ok(HistoryEntry {
            id,
            timestamp,
            method,
            url,
            status: status.map(|s| s as u16),
            duration_ms,
            request,
            response,
        })
    })();

    Ok(parsed)
}

fn decode_cookie_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoreResult<Cookie>> {
    let domain: String = row.get(0)?;
    let path: String = row.get(1)?;
    let name: String = row.get(2)?;
    let value: String = row.get(3)?;
    let expires_str: Option<String> = row.get(4)?;
    let http_only: i64 = row.get(5)?;
    let secure: i64 = row.get(6)?;
    let same_site: Option<String> = row.get(7)?;

    let parsed = (|| -> CoreResult<Cookie> {
        let expires = expires_str
            .as_deref()
            .map(|s| {
                DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| CoreError::InvalidData(format!("invalid cookie expires: {e}")))
            })
            .transpose()?;

        Ok(Cookie {
            name,
            value,
            domain,
            path,
            expires,
            http_only: http_only != 0,
            secure: secure != 0,
            same_site,
        })
    })();

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RequestItem, ResponseData, ThemeMode};

    #[test]
    fn history_insert_list_search_and_prune() {
        let store = SqliteStore::open_in_memory().expect("open");

        let e1 = HistoryEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now() - chrono::Duration::seconds(10),
            method: HttpMethod::Get,
            url: "https://api.example.com/users".into(),
            status: Some(200),
            duration_ms: Some(42.5),
            request: RequestItem::new("Users", HttpMethod::Get, "https://api.example.com/users"),
            response: Some(ResponseData {
                status: 200,
                status_text: "OK".into(),
                ..ResponseData::default()
            }),
        };
        let e2 = HistoryEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            method: HttpMethod::Post,
            url: "https://api.example.com/orders".into(),
            status: Some(201),
            duration_ms: Some(88.0),
            request: RequestItem::new(
                "Create Order",
                HttpMethod::Post,
                "https://api.example.com/orders",
            ),
            response: None,
        };

        store.insert_history(&e1).expect("insert e1");
        store.insert_history(&e2).expect("insert e2");

        let list = store.list_history(10, 0).expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, e2.id);

        let search = store.search_history("orders", 10).expect("search");
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].id, e2.id);

        let pruned = store.prune_history(1).expect("prune");
        assert_eq!(pruned, 1);
        assert_eq!(store.list_history(10, 0).expect("list").len(), 1);
    }

    #[test]
    fn cookie_upsert_domain_match_and_delete() {
        let store = SqliteStore::open_in_memory().expect("open");
        let cookie = Cookie {
            name: "session".into(),
            value: "abc123".into(),
            domain: "example.com".into(),
            path: "/".into(),
            expires: None,
            http_only: true,
            secure: true,
            same_site: Some("Lax".into()),
        };

        store.upsert_cookie(&cookie).expect("upsert");

        let exact = store
            .get_cookies_for_domain("example.com")
            .expect("exact match");
        assert_eq!(exact, vec![cookie.clone()]);

        let sub = store
            .get_cookies_for_domain("api.example.com")
            .expect("subdomain match");
        assert_eq!(sub, vec![cookie.clone()]);

        store
            .delete_cookie("example.com", "/", "session")
            .expect("delete");
        assert!(store.list_cookies().expect("list").is_empty());
    }

    #[test]
    fn settings_default_and_round_trip() {
        let store = SqliteStore::open_in_memory().expect("open");
        assert_eq!(store.load_settings().expect("default"), AppSettings::default());

        let custom = AppSettings {
            theme: ThemeMode::Light,
            verify_ssl: false,
            timeout_ms: 15_000,
            ..AppSettings::default()
        };
        store.save_settings(&custom).expect("save");
        assert_eq!(store.load_settings().expect("load"), custom);
    }
}
