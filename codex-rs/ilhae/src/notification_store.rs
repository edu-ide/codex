//! Notification store backed by SQLite.
//!
//! Stores notifications from agent tools (ui_notify), system events,
//! and Telegram messages at `~/.ilhae/notifications.db`.

use chrono::Utc;
use rusqlite::{Connection, Result as SqlResult, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

// ─── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub message: String,
    pub level: String,
    pub source: String,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationStats {
    pub total: i64,
    pub unread: i64,
}

// ─── Store ───────────────────────────────────────────────────────────────

pub struct NotificationStore {
    pub conn: Mutex<Connection>,
}

impl NotificationStore {
    pub fn open(db_path: &Path) -> SqlResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS notifications (
                id TEXT PRIMARY KEY,
                message TEXT NOT NULL,
                level TEXT NOT NULL DEFAULT 'info',
                source TEXT NOT NULL DEFAULT 'agent',
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_notif_created ON notifications(created_at);
            CREATE INDEX IF NOT EXISTS idx_notif_read ON notifications(read);
            ",
        )?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Add a new notification. Returns the generated ID.
    pub fn add(&self, message: &str, level: &str, source: &str) -> SqlResult<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO notifications (id, message, level, source, read, created_at) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![id, message, level, source, now],
        )?;
        Ok(id)
    }

    /// List notifications with pagination, newest first.
    pub fn list(&self, offset: usize, limit: usize) -> SqlResult<Vec<Notification>> {
        let conn = self.conn.lock().unwrap();
        let safe_limit = limit.clamp(1, 200) as i64;
        let mut stmt = conn.prepare(
            "SELECT id, message, level, source, read, created_at
             FROM notifications ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![safe_limit, offset as i64], |row| {
                Ok(Notification {
                    id: row.get(0)?,
                    message: row.get(1)?,
                    level: row.get(2)?,
                    source: row.get(3)?,
                    read: row.get::<_, i64>(4)? != 0,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark a single notification as read.
    pub fn mark_read(&self, id: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE notifications SET read = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// Mark all notifications as read.
    pub fn mark_all_read(&self) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE notifications SET read = 1 WHERE read = 0", [])?;
        Ok(n)
    }

    /// Get stats.
    pub fn stats(&self) -> SqlResult<NotificationStats> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))?;
        let unread: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(NotificationStats { total, unread })
    }

    /// Delete old notifications, keeping the most recent `keep` items.
    #[allow(dead_code)]
    pub fn prune(&self, keep: usize) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM notifications WHERE id NOT IN (SELECT id FROM notifications ORDER BY created_at DESC LIMIT ?1)",
            params![keep as i64],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_store(name: &str) -> NotificationStore {
        let path = PathBuf::from(format!(
            "/tmp/ilhae-notif-test-{}-{}.db",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        NotificationStore::open(&path).unwrap()
    }

    #[test]
    fn test_add_and_list() {
        let store = temp_store("add_list");
        store.add("Hello", "info", "agent").unwrap();
        store.add("Warning!", "warning", "system").unwrap();
        let items = store.list(0, 10).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].level, "warning"); // newest first
    }

    #[test]
    fn test_mark_read() {
        let store = temp_store("mark_read");
        let id = store.add("Test", "info", "agent").unwrap();
        assert!(!store.list(0, 1).unwrap()[0].read);
        store.mark_read(&id).unwrap();
        assert!(store.list(0, 1).unwrap()[0].read);
    }

    #[test]
    fn test_stats() {
        let store = temp_store("stats");
        store.add("A", "info", "agent").unwrap();
        store.add("B", "info", "agent").unwrap();
        let s = store.stats().unwrap();
        assert_eq!(s.total, 2);
        assert_eq!(s.unread, 2);
        store.mark_all_read().unwrap();
        let s2 = store.stats().unwrap();
        assert_eq!(s2.unread, 0);
    }
}
