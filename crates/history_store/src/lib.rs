use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS read_histories (
    id TEXT PRIMARY KEY,
    source_path TEXT NOT NULL,
    source_file_name TEXT NOT NULL,
    source_file_size INTEGER NOT NULL,
    source_modified_at INTEGER,
    detected_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_read_histories_detected_at
ON read_histories (detected_at DESC);

CREATE INDEX IF NOT EXISTS idx_read_histories_source_path
ON read_histories (source_path);

CREATE TABLE IF NOT EXISTS read_results (
    id TEXT PRIMARY KEY,
    history_id TEXT NOT NULL,
    result_index INTEGER NOT NULL,
    code_type TEXT NOT NULL,
    decoded_text TEXT NOT NULL,
    decoded_kind TEXT NOT NULL,
    FOREIGN KEY (history_id)
        REFERENCES read_histories (id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_read_results_history_id
ON read_results (history_id);
"#;

pub const ENABLE_FOREIGN_KEYS_SQL: &str = "PRAGMA foreign_keys = ON;";

#[derive(Debug, Clone)]
pub struct HistoryStore {
    conn: Arc<Mutex<Connection>>,
    // 0 means unlimited; otherwise the maximum number of read_histories rows to retain.
    history_limit: Arc<AtomicU32>,
}

impl HistoryStore {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create history directory: {}", parent.display())
            })?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open history database: {}", path.display()))?;

        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory().context("failed to open in-memory database")?;

        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(ENABLE_FOREIGN_KEYS_SQL)
            .context("failed to enable SQLite foreign keys")?;
        // WAL mode is silently ignored on in-memory databases, which is fine for tests.
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("failed to enable WAL journal mode")?;
        conn.execute_batch(SCHEMA_SQL)
            .context("failed to initialize history schema")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            history_limit: Arc::new(AtomicU32::new(0)),
        })
    }

    fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Set the maximum number of `read_histories` rows to retain in the database.
    /// `None` disables the cap (unlimited). The new limit is enforced lazily on the next
    /// insert; call [`Self::enforce_history_limit`] to apply it immediately.
    pub fn set_history_limit(&self, limit: Option<u32>) {
        // 0 represents "unlimited"; clamp values >= 1 to avoid an empty table.
        let value = match limit {
            None => 0,
            Some(n) => n.max(1),
        };
        self.history_limit.store(value, Ordering::Relaxed);
    }

    pub fn current_history_limit(&self) -> Option<u32> {
        let value = self.history_limit.load(Ordering::Relaxed);
        if value == 0 {
            None
        } else {
            Some(value)
        }
    }

    /// Delete oldest `read_histories` rows that exceed the configured limit.
    /// Returns the number of rows deleted (0 if unlimited or no excess).
    pub fn enforce_history_limit(&self) -> anyhow::Result<usize> {
        let Some(limit) = self.current_history_limit() else {
            return Ok(0);
        };

        let conn = self.lock_conn();
        let deleted = conn
            .execute(
                "DELETE FROM read_histories WHERE id IN (\
                    SELECT id FROM read_histories \
                    ORDER BY detected_at DESC, id DESC \
                    LIMIT -1 OFFSET ?1\
                )",
                [limit as i64],
            )
            .context("failed to enforce history limit")?;
        Ok(deleted)
    }

    pub fn insert_read(
        &self,
        history: NewReadHistory,
        results: &[NewReadResult],
    ) -> anyhow::Result<String> {
        let history_id = Uuid::new_v4().to_string();
        let mut conn = self.lock_conn();
        let tx = conn
            .transaction()
            .context("failed to start history insert transaction")?;

        tx.execute(
            r#"
            INSERT INTO read_histories (
                id,
                source_path,
                source_file_name,
                source_file_size,
                source_modified_at,
                detected_at,
                status,
                error_message
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                history_id,
                history.source_path.to_string_lossy(),
                history.source_file_name,
                history.source_file_size,
                history.source_modified_at,
                history.detected_at,
                history.status.as_str(),
                history.error_message,
            ],
        )
        .context("failed to insert read history")?;

        for (result_index, result) in results.iter().enumerate() {
            tx.execute(
                r#"
                INSERT INTO read_results (
                    id,
                    history_id,
                    result_index,
                    code_type,
                    decoded_text,
                    decoded_kind
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    Uuid::new_v4().to_string(),
                    history_id,
                    result_index as i64,
                    result.code_type.as_str(),
                    result.decoded_text,
                    result.decoded_kind.as_str(),
                ],
            )
            .context("failed to insert read result")?;
        }

        tx.commit()
            .context("failed to commit history insert transaction")?;

        drop(conn);
        // Best-effort retention enforcement; logged via context but not fatal.
        let _ = self.enforce_history_limit();

        Ok(history_id)
    }

    pub fn list_histories(&self, limit: u32) -> anyhow::Result<Vec<ReadHistory>> {
        let conn = self.lock_conn();
        let mut statement = conn
            .prepare(
                r#"
                SELECT
                    id,
                    source_path,
                    source_file_name,
                    source_file_size,
                    source_modified_at,
                    detected_at,
                    status,
                    error_message
                FROM read_histories
                ORDER BY detected_at DESC, id DESC
                LIMIT ?1
                "#,
            )
            .context("failed to prepare read history list query")?;

        let histories = statement
            .query_map([limit as i64], read_history_from_row)
            .context("failed to query read histories")?
            .collect::<Result<Vec<_>, _>>()
            .context("failed to read history row")?;

        Ok(histories)
    }

    pub fn get_history(&self, id: &str) -> anyhow::Result<Option<ReadHistory>> {
        let conn = self.lock_conn();
        conn.query_row(
            r#"
                SELECT
                    id,
                    source_path,
                    source_file_name,
                    source_file_size,
                    source_modified_at,
                    detected_at,
                    status,
                    error_message
                FROM read_histories
                WHERE id = ?1
                "#,
            [id],
            read_history_from_row,
        )
        .optional()
        .context("failed to fetch read history")
    }

    pub fn list_results(&self, history_id: &str) -> anyhow::Result<Vec<ReadResult>> {
        let conn = self.lock_conn();
        let mut statement = conn
            .prepare(
                r#"
                SELECT
                    id,
                    history_id,
                    result_index,
                    code_type,
                    decoded_text,
                    decoded_kind
                FROM read_results
                WHERE history_id = ?1
                ORDER BY result_index ASC, id ASC
                "#,
            )
            .context("failed to prepare read result list query")?;

        let results = statement
            .query_map([history_id], read_result_from_row)
            .context("failed to query read results")?
            .collect::<Result<Vec<_>, _>>()
            .context("failed to read result row")?;

        Ok(results)
    }

    pub fn delete_history(&self, id: &str) -> anyhow::Result<usize> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM read_histories WHERE id = ?1", [id])
            .context("failed to delete read history")
    }

    pub fn delete_result(&self, id: &str) -> anyhow::Result<usize> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM read_results WHERE id = ?1", [id])
            .context("failed to delete read result")
    }

    #[cfg(test)]
    fn foreign_keys_enabled(&self) -> anyhow::Result<bool> {
        let conn = self.lock_conn();
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys;", [], |row| row.get(0))
            .context("failed to query SQLite foreign key setting")?;

        Ok(enabled == 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReadHistory {
    pub source_path: PathBuf,
    pub source_file_name: String,
    pub source_file_size: i64,
    pub source_modified_at: Option<i64>,
    pub detected_at: i64,
    pub status: ReadStatus,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadHistory {
    pub id: String,
    pub source_path: PathBuf,
    pub source_file_name: String,
    pub source_file_size: i64,
    pub source_modified_at: Option<i64>,
    pub detected_at: i64,
    pub status: ReadStatus,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReadResult {
    pub code_type: CodeType,
    pub decoded_text: String,
    pub decoded_kind: DecodedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub id: String,
    pub history_id: String,
    pub result_index: i64,
    pub code_type: CodeType,
    pub decoded_text: String,
    pub decoded_kind: DecodedKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    Decoded,
    NoCode,
    Failed,
}

impl ReadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadStatus::Decoded => "decoded",
            ReadStatus::NoCode => "no_code",
            ReadStatus::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ParseHistoryValueError> {
        match value {
            "decoded" => Ok(ReadStatus::Decoded),
            "no_code" => Ok(ReadStatus::NoCode),
            "failed" => Ok(ReadStatus::Failed),
            _ => Err(ParseHistoryValueError::new("read status", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeType {
    Qr,
}

impl CodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeType::Qr => "qr",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ParseHistoryValueError> {
        match value {
            "qr" => Ok(CodeType::Qr),
            _ => Err(ParseHistoryValueError::new("code type", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedKind {
    Url,
    Text,
}

impl DecodedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecodedKind::Url => "url",
            DecodedKind::Text => "text",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ParseHistoryValueError> {
        match value {
            "url" => Ok(DecodedKind::Url),
            "text" => Ok(DecodedKind::Text),
            _ => Err(ParseHistoryValueError::new("decoded kind", value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHistoryValueError {
    field_name: &'static str,
    value: String,
}

impl ParseHistoryValueError {
    fn new(field_name: &'static str, value: &str) -> Self {
        Self {
            field_name,
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for ParseHistoryValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field_name, self.value)
    }
}

impl std::error::Error for ParseHistoryValueError {}

fn read_history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadHistory> {
    let status: String = row.get(6)?;

    Ok(ReadHistory {
        id: row.get(0)?,
        source_path: PathBuf::from(row.get::<_, String>(1)?),
        source_file_name: row.get(2)?,
        source_file_size: row.get(3)?,
        source_modified_at: row.get(4)?,
        detected_at: row.get(5)?,
        status: ReadStatus::parse(&status).map_err(to_sql_conversion_error)?,
        error_message: row.get(7)?,
    })
}

fn read_result_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadResult> {
    let code_type: String = row.get(3)?;
    let decoded_kind: String = row.get(5)?;

    Ok(ReadResult {
        id: row.get(0)?,
        history_id: row.get(1)?,
        result_index: row.get(2)?,
        code_type: CodeType::parse(&code_type).map_err(to_sql_conversion_error)?,
        decoded_text: row.get(4)?,
        decoded_kind: DecodedKind::parse(&decoded_kind).map_err(to_sql_conversion_error)?,
    })
}

fn to_sql_conversion_error(error: ParseHistoryValueError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_initializes_schema_and_enables_foreign_keys() {
        let store = HistoryStore::open_in_memory().expect("open in-memory history store");

        assert!(store.foreign_keys_enabled().expect("query foreign keys"));
        assert_eq!(store.list_histories(10).expect("list histories"), []);
    }

    #[test]
    fn open_creates_parent_directory_for_database_path() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let db_path = temp_dir.path().join("nested").join("history.sqlite3");

        let store = HistoryStore::open(&db_path).expect("open history store");

        assert!(db_path.exists());
        assert!(store.foreign_keys_enabled().expect("query foreign keys"));
    }

    #[test]
    fn open_can_reopen_existing_database() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let db_path = temp_dir.path().join("history.sqlite3");
        let history_id = {
            let store = HistoryStore::open(&db_path).expect("open history store");
            store
                .insert_read(decoded_history(), &[url_result()])
                .expect("insert history")
        };

        let store = HistoryStore::open(&db_path).expect("reopen history store");
        let histories = store.list_histories(10).expect("list histories");

        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].id, history_id);
        assert_eq!(
            store.list_results(&history_id).expect("list results").len(),
            1
        );
    }

    #[test]
    fn insert_read_persists_history_and_multiple_results() {
        let store = HistoryStore::open_in_memory().expect("open history store");

        let history_id = store
            .insert_read(decoded_history(), &[url_result(), text_result()])
            .expect("insert decoded history");

        let histories = store.list_histories(10).expect("list histories");
        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].id, history_id);
        assert_eq!(histories[0].status, ReadStatus::Decoded);

        let results = store.list_results(&history_id).expect("list results");
        assert_eq!(
            results
                .iter()
                .map(|result| (
                    result.result_index,
                    result.code_type,
                    result.decoded_text.as_str(),
                    result.decoded_kind,
                ))
                .collect::<Vec<_>>(),
            [
                (0, CodeType::Qr, "https://example.com", DecodedKind::Url),
                (1, CodeType::Qr, "plain text", DecodedKind::Text),
            ]
        );
    }

    #[test]
    fn list_histories_orders_by_detected_at_descending() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        let mut older = decoded_history();
        older.detected_at = 100;
        let mut newer = decoded_history();
        newer.detected_at = 200;

        let older_id = store.insert_read(older, &[]).expect("insert older history");
        let newer_id = store.insert_read(newer, &[]).expect("insert newer history");

        let histories = store.list_histories(10).expect("list histories");

        assert_eq!(
            histories
                .iter()
                .map(|history| history.id.as_str())
                .collect::<Vec<_>>(),
            [newer_id.as_str(), older_id.as_str()]
        );
    }

    #[test]
    fn insert_read_can_store_no_code_and_failed_histories_without_results() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        let mut no_code = decoded_history();
        no_code.status = ReadStatus::NoCode;
        let mut failed = decoded_history();
        failed.status = ReadStatus::Failed;
        failed.error_message = Some("decode timeout".to_owned());

        let no_code_id = store.insert_read(no_code, &[]).expect("insert no_code");
        let failed_id = store.insert_read(failed, &[]).expect("insert failed");

        assert!(store
            .list_results(&no_code_id)
            .expect("list results")
            .is_empty());
        assert!(store
            .list_results(&failed_id)
            .expect("list results")
            .is_empty());
        assert_eq!(
            store
                .get_history(&failed_id)
                .expect("get failed history")
                .expect("failed history exists")
                .error_message,
            Some("decode timeout".to_owned())
        );
    }

    #[test]
    fn insert_read_evicts_oldest_when_history_limit_is_reached() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        store.set_history_limit(Some(2));

        let mut ids = Vec::new();
        for detected_at in [100, 200, 300, 400] {
            let mut history = decoded_history();
            history.detected_at = detected_at;
            let id = store.insert_read(history, &[]).expect("insert history");
            ids.push((detected_at, id));
        }

        let remaining = store.list_histories(10).expect("list histories");
        let kept_ids: Vec<&str> = remaining.iter().map(|h| h.id.as_str()).collect();
        // Only the two most recent rows should remain.
        assert_eq!(kept_ids.len(), 2);
        assert!(kept_ids.contains(&ids[3].1.as_str()), "newest must be kept");
        assert!(
            kept_ids.contains(&ids[2].1.as_str()),
            "second newest must be kept"
        );
    }

    #[test]
    fn enforce_history_limit_trims_existing_excess() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        for detected_at in [100, 200, 300, 400, 500] {
            let mut history = decoded_history();
            history.detected_at = detected_at;
            store.insert_read(history, &[]).expect("insert history");
        }

        // 5 rows inserted with unlimited cap, now apply limit retroactively
        store.set_history_limit(Some(3));
        let deleted = store
            .enforce_history_limit()
            .expect("enforce history limit");
        assert_eq!(deleted, 2);

        let remaining = store.list_histories(10).expect("list histories");
        assert_eq!(remaining.len(), 3);
    }

    #[test]
    fn insert_read_does_not_evict_when_history_limit_is_unlimited() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        // Default is unlimited (0)
        for detected_at in [100, 200, 300, 400, 500] {
            let mut history = decoded_history();
            history.detected_at = detected_at;
            store.insert_read(history, &[]).expect("insert history");
        }

        let remaining = store.list_histories(10).expect("list histories");
        assert_eq!(remaining.len(), 5);
    }

    #[test]
    fn delete_history_cascades_to_results() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        let history_id = store
            .insert_read(decoded_history(), &[url_result()])
            .expect("insert history");

        assert_eq!(
            store.delete_history(&history_id).expect("delete history"),
            1
        );

        assert!(store
            .get_history(&history_id)
            .expect("get history")
            .is_none());
        assert!(store
            .list_results(&history_id)
            .expect("list results")
            .is_empty());
    }

    #[test]
    fn delete_result_does_not_change_history_status() {
        let store = HistoryStore::open_in_memory().expect("open history store");
        let history_id = store
            .insert_read(decoded_history(), &[url_result()])
            .expect("insert history");
        let result_id = store.list_results(&history_id).expect("list results")[0]
            .id
            .clone();

        assert_eq!(store.delete_result(&result_id).expect("delete result"), 1);

        let history = store
            .get_history(&history_id)
            .expect("get history")
            .expect("history exists");
        assert_eq!(history.status, ReadStatus::Decoded);
        assert!(store
            .list_results(&history_id)
            .expect("list results")
            .is_empty());
    }

    fn decoded_history() -> NewReadHistory {
        NewReadHistory {
            source_path: PathBuf::from(r"C:\Users\example\Pictures\Screenshots\image.png"),
            source_file_name: "image.png".to_owned(),
            source_file_size: 1024,
            source_modified_at: Some(1_700_000_000),
            detected_at: 1_700_000_001,
            status: ReadStatus::Decoded,
            error_message: None,
        }
    }

    fn url_result() -> NewReadResult {
        NewReadResult {
            code_type: CodeType::Qr,
            decoded_text: "https://example.com".to_owned(),
            decoded_kind: DecodedKind::Url,
        }
    }

    fn text_result() -> NewReadResult {
        NewReadResult {
            code_type: CodeType::Qr,
            decoded_text: "plain text".to_owned(),
            decoded_kind: DecodedKind::Text,
        }
    }
}
