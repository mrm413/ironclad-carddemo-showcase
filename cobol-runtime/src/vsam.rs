// VSAM Storage Engine backed by SQLite.
// Replaces flat-file I/O with B-tree keyed access (KSDS),
// relative record (RRDS), and entry-sequenced (ESDS) storage.
// Also manages TSQ persistence, TDQ intrapartition, and LUW transactions.

use std::collections::HashMap;
use rusqlite::{Connection, params, OptionalExtension};

// ── Error codes (maps to CicsResp at boundary) ──────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VsamError {
    Normal,
    NotFound,
    DuplicateKey,
    NotOpen,
    EndData,
    IoErr,
    QIdErr,
    ItemErr,
    InvalidReq,
}

// ── VSAM organization types ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VsamOrganization {
    /// Key-Sequenced Data Set — primary key, keyed + sequential access
    Ksds,
    /// Relative Record Data Set — fixed slots by record number
    Rrds,
    /// Entry-Sequenced Data Set — append-only, sequential
    Esds,
}

// ── File definition ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VsamFileDef {
    pub name: String,
    pub table_name: String,
    pub organization: VsamOrganization,
}

// ── Browse cursor ───────────────────────────────────────────────────

pub struct VsamBrowseCursor {
    file_name: String,
    start_key: String,
    last_key: Option<String>,
}

// ── TDQ trigger ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TdqTrigger {
    pub queue: String,
    pub level: usize,
    pub program: String,
}

// ── VSAM Store ──────────────────────────────────────────────────────

pub struct VsamStore {
    conn: Connection,
    files: HashMap<String, VsamFileDef>,
    browse_cursors: HashMap<u32, VsamBrowseCursor>,
    next_token: u32,
    in_transaction: bool,
    tdq_triggers: Vec<TdqTrigger>,
    triggered_starts: Vec<(String, Vec<u8>)>,
}

impl VsamStore {
    /// Create store backed by a SQLite database file.
    pub fn new(db_path: &str) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| e.to_string())?;
        let mut store = Self {
            conn, files: HashMap::new(), browse_cursors: HashMap::new(),
            next_token: 1, in_transaction: false,
            tdq_triggers: Vec::new(), triggered_starts: Vec::new(),
        };
        store.init_system_tables()?;
        Ok(store)
    }

    /// Create store with in-memory SQLite (for testing).
    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory SQLite");
        let mut store = Self {
            conn, files: HashMap::new(), browse_cursors: HashMap::new(),
            next_token: 1, in_transaction: false,
            tdq_triggers: Vec::new(), triggered_starts: Vec::new(),
        };
        store.init_system_tables().expect("init system tables");
        store
    }

    fn init_system_tables(&mut self) -> Result<(), String> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _tsq (
                queue TEXT NOT NULL, item INTEGER NOT NULL,
                data BLOB NOT NULL, PRIMARY KEY (queue, item));
             CREATE TABLE IF NOT EXISTS _tdq (
                queue TEXT NOT NULL, seq INTEGER NOT NULL,
                data BLOB NOT NULL, processed INTEGER DEFAULT 0,
                PRIMARY KEY (queue, seq));"
        ).map_err(|e| e.to_string())
    }

    /// Expose connection for SQL layer.
    pub fn connection(&self) -> &Connection { &self.conn }

    // ── File Registration ────────────────────────────────────────────

    pub fn register_file(&mut self, name: &str, org: VsamOrganization) -> Result<(), String> {
        let uname = name.to_uppercase();
        let table = format!("vsam_{}", uname.to_lowercase());
        let ddl = match org {
            VsamOrganization::Ksds => format!(
                "CREATE TABLE IF NOT EXISTS {t} (key TEXT PRIMARY KEY, data TEXT NOT NULL)", t = table),
            VsamOrganization::Rrds => format!(
                "CREATE TABLE IF NOT EXISTS {t} (rrn INTEGER PRIMARY KEY, data TEXT NOT NULL)", t = table),
            VsamOrganization::Esds => format!(
                "CREATE TABLE IF NOT EXISTS {t} (seq INTEGER PRIMARY KEY AUTOINCREMENT, data TEXT NOT NULL)", t = table),
        };
        self.conn.execute(&ddl, []).map_err(|e| e.to_string())?;
        self.files.insert(uname.clone(), VsamFileDef {
            name: uname, table_name: table, organization: org,
        });
        Ok(())
    }

    pub fn is_registered(&self, name: &str) -> bool {
        self.files.contains_key(&name.to_uppercase())
    }

    fn get_file(&self, name: &str) -> Result<&VsamFileDef, VsamError> {
        self.files.get(&name.to_uppercase()).ok_or(VsamError::NotOpen)
    }

    // ── READ ─────────────────────────────────────────────────────────

    pub fn read(&self, file: &str, key: &str) -> Result<String, VsamError> {
        let f = self.get_file(file)?;
        let sql = match f.organization {
            VsamOrganization::Ksds => format!("SELECT data FROM {} WHERE key = ?1", f.table_name),
            VsamOrganization::Rrds => format!("SELECT data FROM {} WHERE rrn = ?1", f.table_name),
            VsamOrganization::Esds => format!("SELECT data FROM {} WHERE seq = ?1", f.table_name),
        };
        self.conn.query_row(&sql, params![key], |row| row.get::<_, String>(0))
            .optional().map_err(|_| VsamError::IoErr)?
            .ok_or(VsamError::NotFound)
    }

    // ── WRITE ────────────────────────────────────────────────────────

    pub fn write(&self, file: &str, key: &str, data: &str) -> Result<(), VsamError> {
        let f = self.get_file(file)?;
        let sql = match f.organization {
            VsamOrganization::Ksds => format!(
                "INSERT INTO {} (key, data) VALUES (?1, ?2)", f.table_name),
            VsamOrganization::Rrds => format!(
                "INSERT INTO {} (rrn, data) VALUES (?1, ?2)", f.table_name),
            VsamOrganization::Esds => format!(
                "INSERT INTO {} (data) VALUES (?2)", f.table_name),
        };
        self.conn.execute(&sql, params![key, data]).map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                VsamError::DuplicateKey
            } else {
                VsamError::IoErr
            }
        })?;
        Ok(())
    }

    // ── REWRITE ──────────────────────────────────────────────────────

    pub fn rewrite(&self, file: &str, key: &str, data: &str) -> Result<(), VsamError> {
        let f = self.get_file(file)?;
        let sql = match f.organization {
            VsamOrganization::Ksds => format!(
                "UPDATE {} SET data = ?2 WHERE key = ?1", f.table_name),
            VsamOrganization::Rrds => format!(
                "UPDATE {} SET data = ?2 WHERE rrn = ?1", f.table_name),
            VsamOrganization::Esds => return Err(VsamError::InvalidReq),
        };
        let rows = self.conn.execute(&sql, params![key, data]).map_err(|_| VsamError::IoErr)?;
        if rows == 0 { Err(VsamError::NotFound) } else { Ok(()) }
    }

    // ── DELETE ────────────────────────────────────────────────────────

    pub fn delete(&self, file: &str, key: &str) -> Result<(), VsamError> {
        let f = self.get_file(file)?;
        let sql = match f.organization {
            VsamOrganization::Ksds => format!("DELETE FROM {} WHERE key = ?1", f.table_name),
            VsamOrganization::Rrds => format!("DELETE FROM {} WHERE rrn = ?1", f.table_name),
            VsamOrganization::Esds => return Err(VsamError::InvalidReq),
        };
        let rows = self.conn.execute(&sql, params![key]).map_err(|_| VsamError::IoErr)?;
        if rows == 0 { Err(VsamError::NotFound) } else { Ok(()) }
    }

    // ── BROWSE ───────────────────────────────────────────────────────

    pub fn start_browse(&mut self, file: &str, key: &str) -> Result<u32, VsamError> {
        let _ = self.get_file(file)?;
        let token = self.next_token;
        self.next_token += 1;
        self.browse_cursors.insert(token, VsamBrowseCursor {
            file_name: file.to_uppercase(),
            start_key: key.to_string(),
            last_key: None,
        });
        Ok(token)
    }

    pub fn read_next(&mut self, token: u32) -> Result<(String, String), VsamError> {
        let cursor = self.browse_cursors.get(&token).ok_or(VsamError::NotOpen)?;
        let f = self.get_file(&cursor.file_name)?;

        let (sql, param) = match &cursor.last_key {
            None => (
                format!("SELECT key, data FROM {} WHERE key >= ?1 ORDER BY key ASC LIMIT 1", f.table_name),
                cursor.start_key.clone(),
            ),
            Some(k) => (
                format!("SELECT key, data FROM {} WHERE key > ?1 ORDER BY key ASC LIMIT 1", f.table_name),
                k.clone(),
            ),
        };

        let result = self.conn.query_row(&sql, params![param], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).optional().map_err(|_| VsamError::IoErr)?;

        match result {
            Some((key, data)) => {
                self.browse_cursors.get_mut(&token).unwrap().last_key = Some(key.clone());
                Ok((key, data))
            }
            None => Err(VsamError::EndData),
        }
    }

    pub fn read_prev(&mut self, token: u32) -> Result<(String, String), VsamError> {
        let cursor = self.browse_cursors.get(&token).ok_or(VsamError::NotOpen)?;
        let f = self.get_file(&cursor.file_name)?;
        let param = cursor.last_key.as_ref().unwrap_or(&cursor.start_key).clone();

        let sql = format!(
            "SELECT key, data FROM {} WHERE key < ?1 ORDER BY key DESC LIMIT 1", f.table_name);

        let result = self.conn.query_row(&sql, params![param], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).optional().map_err(|_| VsamError::IoErr)?;

        match result {
            Some((key, data)) => {
                self.browse_cursors.get_mut(&token).unwrap().last_key = Some(key.clone());
                Ok((key, data))
            }
            None => Err(VsamError::EndData),
        }
    }

    pub fn end_browse(&mut self, token: u32) -> Result<(), VsamError> {
        self.browse_cursors.remove(&token).map(|_| ()).ok_or(VsamError::NotOpen)
    }

    // ── TSQ Operations ──────────────────────────────────────────────

    /// Write to TSQ. item=Some rewrites at position; item=None appends. Returns item number.
    pub fn tsq_write(&self, queue: &str, data: &[u8], item: Option<usize>) -> Result<usize, VsamError> {
        let q = queue.to_uppercase();
        match item {
            Some(n) => {
                let rows = self.conn.execute(
                    "UPDATE _tsq SET data = ?3 WHERE queue = ?1 AND item = ?2",
                    params![q, n as i64, data],
                ).map_err(|_| VsamError::IoErr)?;
                if rows == 0 { Err(VsamError::ItemErr) } else { Ok(n) }
            }
            None => {
                let next: i64 = self.conn.query_row(
                    "SELECT COALESCE(MAX(item), 0) + 1 FROM _tsq WHERE queue = ?1",
                    params![q], |row| row.get(0),
                ).map_err(|_| VsamError::IoErr)?;
                self.conn.execute(
                    "INSERT INTO _tsq (queue, item, data) VALUES (?1, ?2, ?3)",
                    params![q, next, data],
                ).map_err(|_| VsamError::IoErr)?;
                Ok(next as usize)
            }
        }
    }

    /// Read TSQ item by number (1-based).
    pub fn tsq_read(&self, queue: &str, item: usize) -> Result<Vec<u8>, VsamError> {
        let q = queue.to_uppercase();
        self.conn.query_row(
            "SELECT data FROM _tsq WHERE queue = ?1 AND item = ?2",
            params![q, item as i64], |row| row.get::<_, Vec<u8>>(0),
        ).optional().map_err(|_| VsamError::IoErr)?.ok_or(VsamError::ItemErr)
    }

    /// Read next TSQ item after given item number. Returns (data, item_number).
    pub fn tsq_read_next(&self, queue: &str, after: usize) -> Result<(Vec<u8>, usize), VsamError> {
        let q = queue.to_uppercase();
        self.conn.query_row(
            "SELECT item, data FROM _tsq WHERE queue = ?1 AND item > ?2 ORDER BY item ASC LIMIT 1",
            params![q, after as i64], |row| {
                Ok((row.get::<_, i64>(0)? as usize, row.get::<_, Vec<u8>>(1)?))
            },
        ).optional().map_err(|_| VsamError::IoErr)?
            .map(|(item, data)| (data, item))
            .ok_or(VsamError::ItemErr)
    }

    pub fn tsq_numitems(&self, queue: &str) -> usize {
        let q = queue.to_uppercase();
        self.conn.query_row(
            "SELECT COUNT(*) FROM _tsq WHERE queue = ?1",
            params![q], |row| row.get::<_, i64>(0),
        ).unwrap_or(0) as usize
    }

    pub fn tsq_delete(&self, queue: &str) -> Result<(), VsamError> {
        let q = queue.to_uppercase();
        let rows = self.conn.execute("DELETE FROM _tsq WHERE queue = ?1", params![q])
            .map_err(|_| VsamError::IoErr)?;
        if rows == 0 { Err(VsamError::QIdErr) } else { Ok(()) }
    }

    pub fn tsq_exists(&self, queue: &str) -> bool {
        self.tsq_numitems(queue) > 0
    }

    // ── TDQ Operations ──────────────────────────────────────────────

    pub fn tdq_write(&mut self, queue: &str, data: &[u8]) -> Result<(), VsamError> {
        let q = queue.to_uppercase();
        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM _tdq WHERE queue = ?1",
            params![q], |row| row.get(0),
        ).map_err(|_| VsamError::IoErr)?;
        self.conn.execute(
            "INSERT INTO _tdq (queue, seq, data) VALUES (?1, ?2, ?3)",
            params![q, next, data],
        ).map_err(|_| VsamError::IoErr)?;
        self.check_tdq_triggers(&q);
        Ok(())
    }

    pub fn tdq_read(&self, queue: &str) -> Result<Vec<u8>, VsamError> {
        let q = queue.to_uppercase();
        let result = self.conn.query_row(
            "SELECT seq, data FROM _tdq WHERE queue = ?1 AND processed = 0 ORDER BY seq ASC LIMIT 1",
            params![q], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        ).optional().map_err(|_| VsamError::IoErr)?;
        match result {
            Some((seq, data)) => {
                self.conn.execute(
                    "UPDATE _tdq SET processed = 1 WHERE queue = ?1 AND seq = ?2",
                    params![q, seq],
                ).ok();
                Ok(data)
            }
            None => Err(VsamError::QIdErr),
        }
    }

    pub fn register_tdq_trigger(&mut self, queue: &str, level: usize, program: &str) {
        self.tdq_triggers.push(TdqTrigger {
            queue: queue.to_uppercase(), level, program: program.to_uppercase(),
        });
    }

    fn check_tdq_triggers(&mut self, queue: &str) {
        let depth: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM _tdq WHERE queue = ?1 AND processed = 0",
            params![queue], |row| row.get(0),
        ).unwrap_or(0);
        for trigger in &self.tdq_triggers {
            if trigger.queue == queue && depth as usize >= trigger.level {
                self.triggered_starts.push((trigger.program.clone(), Vec::new()));
            }
        }
    }

    pub fn drain_triggered_starts(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.triggered_starts)
    }

    // ── Transaction Management ──────────────────────────────────────

    pub fn begin_transaction(&mut self) -> Result<(), String> {
        if !self.in_transaction {
            self.conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
            self.in_transaction = true;
        }
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), String> {
        if self.in_transaction {
            self.conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            self.in_transaction = false;
        }
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<(), String> {
        if self.in_transaction {
            self.conn.execute_batch("ROLLBACK").map_err(|e| e.to_string())?;
            self.in_transaction = false;
        }
        Ok(())
    }

    pub fn is_in_transaction(&self) -> bool { self.in_transaction }

    // ── Raw SQL (for SqlContext integration) ─────────────────────────

    pub fn execute_raw_sql(&self, sql: &str) -> Result<usize, String> {
        self.conn.execute(sql, []).map_err(|e| e.to_string())
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> VsamStore {
        VsamStore::new_in_memory()
    }

    // -- KSDS CRUD --

    #[test]
    fn ksds_write_and_read() {
        let mut s = store();
        s.register_file("ACCTDAT", VsamOrganization::Ksds).unwrap();
        s.write("ACCTDAT", "001", "John Doe,100").unwrap();
        assert_eq!(s.read("ACCTDAT", "001").unwrap(), "John Doe,100");
    }

    #[test]
    fn ksds_duplicate_key() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "K1", "data1").unwrap();
        assert_eq!(s.write("F1", "K1", "data2"), Err(VsamError::DuplicateKey));
    }

    #[test]
    fn ksds_read_not_found() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        assert_eq!(s.read("F1", "NOKEY"), Err(VsamError::NotFound));
    }

    #[test]
    fn ksds_rewrite() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "K1", "old").unwrap();
        s.rewrite("F1", "K1", "new").unwrap();
        assert_eq!(s.read("F1", "K1").unwrap(), "new");
    }

    #[test]
    fn ksds_rewrite_not_found() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        assert_eq!(s.rewrite("F1", "NOKEY", "data"), Err(VsamError::NotFound));
    }

    #[test]
    fn ksds_delete() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "K1", "data").unwrap();
        s.delete("F1", "K1").unwrap();
        assert_eq!(s.read("F1", "K1"), Err(VsamError::NotFound));
    }

    #[test]
    fn ksds_delete_not_found() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        assert_eq!(s.delete("F1", "NOKEY"), Err(VsamError::NotFound));
    }

    // -- Browse --

    #[test]
    fn browse_forward() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "A", "1").unwrap();
        s.write("F1", "B", "2").unwrap();
        s.write("F1", "C", "3").unwrap();

        let tok = s.start_browse("F1", "A").unwrap();
        assert_eq!(s.read_next(tok).unwrap(), ("A".into(), "1".into()));
        assert_eq!(s.read_next(tok).unwrap(), ("B".into(), "2".into()));
        assert_eq!(s.read_next(tok).unwrap(), ("C".into(), "3".into()));
        assert_eq!(s.read_next(tok), Err(VsamError::EndData));
        s.end_browse(tok).unwrap();
    }

    #[test]
    fn browse_backward() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "A", "1").unwrap();
        s.write("F1", "B", "2").unwrap();
        s.write("F1", "C", "3").unwrap();

        let tok = s.start_browse("F1", "C").unwrap();
        // Read C first (forward from start key)
        assert_eq!(s.read_next(tok).unwrap(), ("C".into(), "3".into()));
        // Now go backward
        assert_eq!(s.read_prev(tok).unwrap(), ("B".into(), "2".into()));
        assert_eq!(s.read_prev(tok).unwrap(), ("A".into(), "1".into()));
        assert_eq!(s.read_prev(tok), Err(VsamError::EndData));
        s.end_browse(tok).unwrap();
    }

    #[test]
    fn browse_start_midpoint() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "A", "1").unwrap();
        s.write("F1", "C", "3").unwrap();
        s.write("F1", "E", "5").unwrap();

        let tok = s.start_browse("F1", "B").unwrap(); // B doesn't exist, should start at C
        assert_eq!(s.read_next(tok).unwrap(), ("C".into(), "3".into()));
        s.end_browse(tok).unwrap();
    }

    #[test]
    fn browse_mixed_direction() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        for c in ['A', 'B', 'C', 'D'] {
            s.write("F1", &c.to_string(), &c.to_string()).unwrap();
        }
        let tok = s.start_browse("F1", "A").unwrap();
        assert_eq!(s.read_next(tok).unwrap().0, "A");
        assert_eq!(s.read_next(tok).unwrap().0, "B");
        assert_eq!(s.read_next(tok).unwrap().0, "C");
        // Reverse
        assert_eq!(s.read_prev(tok).unwrap().0, "B");
        // Forward again
        assert_eq!(s.read_next(tok).unwrap().0, "C");
        s.end_browse(tok).unwrap();
    }

    #[test]
    fn endbr_invalid_token() {
        let mut s = store();
        assert_eq!(s.end_browse(999), Err(VsamError::NotOpen));
    }

    // -- RRDS --

    #[test]
    fn rrds_write_and_read() {
        let mut s = store();
        s.register_file("RR", VsamOrganization::Rrds).unwrap();
        s.write("RR", "1", "slot1").unwrap();
        s.write("RR", "5", "slot5").unwrap();
        assert_eq!(s.read("RR", "1").unwrap(), "slot1");
        assert_eq!(s.read("RR", "5").unwrap(), "slot5");
    }

    #[test]
    fn rrds_duplicate_slot() {
        let mut s = store();
        s.register_file("RR", VsamOrganization::Rrds).unwrap();
        s.write("RR", "1", "a").unwrap();
        assert_eq!(s.write("RR", "1", "b"), Err(VsamError::DuplicateKey));
    }

    #[test]
    fn rrds_read_empty_slot() {
        let mut s = store();
        s.register_file("RR", VsamOrganization::Rrds).unwrap();
        assert_eq!(s.read("RR", "99"), Err(VsamError::NotFound));
    }

    // -- ESDS --

    #[test]
    fn esds_append_and_read() {
        let mut s = store();
        s.register_file("ES", VsamOrganization::Esds).unwrap();
        s.write("ES", "", "rec1").unwrap();
        s.write("ES", "", "rec2").unwrap();
        assert_eq!(s.read("ES", "1").unwrap(), "rec1");
        assert_eq!(s.read("ES", "2").unwrap(), "rec2");
    }

    #[test]
    fn esds_rewrite_rejected() {
        let mut s = store();
        s.register_file("ES", VsamOrganization::Esds).unwrap();
        assert_eq!(s.rewrite("ES", "1", "x"), Err(VsamError::InvalidReq));
    }

    // -- TSQ --

    #[test]
    fn tsq_write_and_read() {
        let s = store();
        let item1 = s.tsq_write("Q1", b"rec1", None).unwrap();
        let item2 = s.tsq_write("Q1", b"rec2", None).unwrap();
        assert_eq!(item1, 1);
        assert_eq!(item2, 2);
        assert_eq!(s.tsq_read("Q1", 1).unwrap(), b"rec1");
        assert_eq!(s.tsq_read("Q1", 2).unwrap(), b"rec2");
    }

    #[test]
    fn tsq_random_access_rewrite() {
        let s = store();
        s.tsq_write("Q1", b"aaa", None).unwrap();
        s.tsq_write("Q1", b"bbb", None).unwrap();
        // Rewrite item 1
        s.tsq_write("Q1", b"AAA", Some(1)).unwrap();
        assert_eq!(s.tsq_read("Q1", 1).unwrap(), b"AAA");
        assert_eq!(s.tsq_read("Q1", 2).unwrap(), b"bbb");
    }

    #[test]
    fn tsq_numitems() {
        let s = store();
        assert_eq!(s.tsq_numitems("Q1"), 0);
        s.tsq_write("Q1", b"a", None).unwrap();
        s.tsq_write("Q1", b"b", None).unwrap();
        assert_eq!(s.tsq_numitems("Q1"), 2);
    }

    #[test]
    fn tsq_read_next() {
        let s = store();
        s.tsq_write("Q1", b"a", None).unwrap();
        s.tsq_write("Q1", b"b", None).unwrap();
        s.tsq_write("Q1", b"c", None).unwrap();
        let (data, item) = s.tsq_read_next("Q1", 0).unwrap();
        assert_eq!(data, b"a");
        assert_eq!(item, 1);
        let (data, item) = s.tsq_read_next("Q1", item).unwrap();
        assert_eq!(data, b"b");
        assert_eq!(item, 2);
    }

    #[test]
    fn tsq_delete_queue() {
        let s = store();
        s.tsq_write("Q1", b"data", None).unwrap();
        s.tsq_delete("Q1").unwrap();
        assert_eq!(s.tsq_numitems("Q1"), 0);
        assert_eq!(s.tsq_delete("Q1"), Err(VsamError::QIdErr));
    }

    #[test]
    fn tsq_item_err() {
        let s = store();
        assert_eq!(s.tsq_read("Q1", 1), Err(VsamError::ItemErr));
        assert_eq!(s.tsq_write("Q1", b"x", Some(99)), Err(VsamError::ItemErr));
    }

    #[test]
    fn tsq_session_isolation() {
        let s = store();
        s.tsq_write("SESS_A_Q", b"for_a", None).unwrap();
        s.tsq_write("SESS_B_Q", b"for_b", None).unwrap();
        assert_eq!(s.tsq_numitems("SESS_A_Q"), 1);
        assert_eq!(s.tsq_numitems("SESS_B_Q"), 1);
    }

    // -- TDQ --

    #[test]
    fn tdq_write_and_read() {
        let mut s = store();
        s.tdq_write("TDQ1", b"msg1").unwrap();
        s.tdq_write("TDQ1", b"msg2").unwrap();
        assert_eq!(s.tdq_read("TDQ1").unwrap(), b"msg1");
        assert_eq!(s.tdq_read("TDQ1").unwrap(), b"msg2");
        assert_eq!(s.tdq_read("TDQ1"), Err(VsamError::QIdErr));
    }

    #[test]
    fn tdq_trigger() {
        let mut s = store();
        s.register_tdq_trigger("TQ", 2, "TRIG_PGM");
        s.tdq_write("TQ", b"a").unwrap();
        assert!(s.triggered_starts.is_empty());
        s.tdq_write("TQ", b"b").unwrap();
        assert_eq!(s.triggered_starts.len(), 1);
        assert_eq!(s.triggered_starts[0].0, "TRIG_PGM");
    }

    // -- Transactions --

    #[test]
    fn transaction_commit() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.begin_transaction().unwrap();
        s.write("F1", "K1", "data").unwrap();
        s.commit().unwrap();
        assert_eq!(s.read("F1", "K1").unwrap(), "data");
    }

    #[test]
    fn transaction_rollback() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "K1", "original").unwrap();
        s.begin_transaction().unwrap();
        s.rewrite("F1", "K1", "modified").unwrap();
        s.rollback_transaction().unwrap();
        assert_eq!(s.read("F1", "K1").unwrap(), "original");
    }

    #[test]
    fn transaction_rollback_delete() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.write("F1", "K1", "keep").unwrap();
        s.begin_transaction().unwrap();
        s.delete("F1", "K1").unwrap();
        s.rollback_transaction().unwrap();
        assert_eq!(s.read("F1", "K1").unwrap(), "keep");
    }

    #[test]
    fn transaction_rollback_write() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.begin_transaction().unwrap();
        s.write("F1", "K1", "temp").unwrap();
        s.rollback_transaction().unwrap();
        assert_eq!(s.read("F1", "K1"), Err(VsamError::NotFound));
    }

    // -- Registry --

    #[test]
    fn unregistered_file() {
        let s = store();
        assert_eq!(s.read("NOFILE", "K1"), Err(VsamError::NotOpen));
    }

    #[test]
    fn register_idempotent() {
        let mut s = store();
        s.register_file("F1", VsamOrganization::Ksds).unwrap();
        s.register_file("F1", VsamOrganization::Ksds).unwrap(); // no error
        assert!(s.is_registered("F1"));
    }

    #[test]
    fn case_insensitive_access() {
        let mut s = store();
        s.register_file("myfile", VsamOrganization::Ksds).unwrap();
        s.write("MYFILE", "K1", "data").unwrap();
        assert_eq!(s.read("myfile", "K1").unwrap(), "data");
    }
}
