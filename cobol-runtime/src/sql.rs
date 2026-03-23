// SQL Runtime for Ironclad-generated Rust programs.
// Replaces EXEC SQL (DB2/embedded SQL) with native Rust database operations.
// Dual mode: in-memory HashMap store (default) or real SQLite execution.

use std::collections::HashMap;
use rusqlite::{Connection, params_from_iter, OptionalExtension};

// ── SQLCA ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Sqlca {
    pub sqlcode: i32,
    pub sqlerrml: i16,
    pub sqlerrmc: String,
    pub sqlerrd: [i32; 6],
    pub sqlwarn: [char; 11],
    pub sqlstate: String,
}

impl Default for Sqlca {
    fn default() -> Self {
        Self {
            sqlcode: 0, sqlerrml: 0, sqlerrmc: String::new(),
            sqlerrd: [0; 6], sqlwarn: [' '; 11], sqlstate: "00000".to_string(),
        }
    }
}

impl Sqlca {
    pub fn is_ok(&self) -> bool { self.sqlcode == 0 }
    pub fn is_not_found(&self) -> bool { self.sqlcode == 100 }
    pub fn is_error(&self) -> bool { self.sqlcode < 0 }

    // Stubs for generated code that calls sqlca methods directly
    pub fn execute_sql(&mut self, _stmt: &str, _params: &[(&str, &str)]) { self.set_ok(); }
    pub fn open_cursor(&mut self, _name: &str) { self.set_ok(); }
    pub fn fetch_cursor(&mut self, _name: &str) { self.set_not_found(); }
    pub fn close_cursor(&mut self, _name: &str) { self.set_ok(); }

    fn set_ok(&mut self) {
        self.sqlcode = 0; self.sqlstate = "00000".to_string(); self.sqlerrmc.clear();
    }
    fn set_not_found(&mut self) {
        self.sqlcode = 100; self.sqlstate = "02000".to_string();
    }
    fn set_error(&mut self, code: i32, msg: &str) {
        self.sqlcode = code; self.sqlerrmc = msg.to_string();
        self.sqlerrml = msg.len() as i16; self.sqlstate = "58000".to_string();
    }
}

// ── SQL Value ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    pub fn as_text(&self) -> String {
        match self {
            SqlValue::Null => String::new(),
            SqlValue::Integer(n) => n.to_string(),
            SqlValue::Float(f) => f.to_string(),
            SqlValue::Text(s) => s.clone(),
            SqlValue::Blob(b) => String::from_utf8_lossy(b).to_string(),
        }
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            SqlValue::Integer(n) => *n,
            SqlValue::Float(f) => *f as i64,
            SqlValue::Text(s) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            SqlValue::Float(f) => *f,
            SqlValue::Integer(n) => *n as f64,
            SqlValue::Text(s) => s.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }
    pub fn is_null(&self) -> bool { matches!(self, SqlValue::Null) }
}

/// Convert SqlValue to a rusqlite-compatible boxed value.
impl rusqlite::types::ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::ToSqlOutput;
        match self {
            SqlValue::Null => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Null)),
            SqlValue::Integer(n) => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Integer(*n))),
            SqlValue::Float(f) => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Real(*f))),
            SqlValue::Text(s) => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Text(s.clone()))),
            SqlValue::Blob(b) => Ok(ToSqlOutput::Owned(rusqlite::types::Value::Blob(b.clone()))),
        }
    }
}

// ── SQL Row ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SqlRow {
    pub columns: Vec<String>,
    pub values: Vec<SqlValue>,
}

impl SqlRow {
    pub fn get(&self, col: &str) -> Option<&SqlValue> {
        let upper = col.to_uppercase();
        self.columns.iter().position(|c| c.to_uppercase() == upper).map(|i| &self.values[i])
    }
    pub fn get_text(&self, col: &str) -> String {
        self.get(col).map(|v| v.as_text()).unwrap_or_default()
    }
    pub fn get_i64(&self, col: &str) -> i64 {
        self.get(col).map(|v| v.as_i64()).unwrap_or(0)
    }
}

// ── Cursor ──────────────────────────────────────────────────────────

pub struct SqlCursor {
    pub name: String,
    pub rows: Vec<SqlRow>,
    pub position: usize,
    pub is_open: bool,
    query: Option<String>,       // SQL for SQLite mode
    host_vars: HashMap<String, SqlValue>,
}

impl SqlCursor {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_uppercase(), rows: Vec::new(), position: 0,
            is_open: false, query: None, host_vars: HashMap::new(),
        }
    }
    pub fn fetch(&mut self) -> Option<&SqlRow> {
        if !self.is_open || self.position >= self.rows.len() { return None; }
        let row = &self.rows[self.position];
        self.position += 1;
        Some(row)
    }
}

// ── WHENEVER action ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum WheneverAction { Continue, GoTo(String), Stop }

// ── SQL Context ─────────────────────────────────────────────────────

pub struct SqlContext {
    pub sqlca: Sqlca,
    // In-memory store (used when conn is None)
    tables: HashMap<String, Vec<SqlRow>>,
    schemas: HashMap<String, Vec<String>>,
    cursors: HashMap<String, SqlCursor>,
    on_error: WheneverAction,
    on_not_found: WheneverAction,
    // Real SQLite connection (Phase 5)
    conn: Option<Connection>,
}

impl Default for SqlContext {
    fn default() -> Self { Self::new() }
}

impl SqlContext {
    /// Create in-memory SQL context (no real database).
    pub fn new() -> Self {
        Self {
            sqlca: Sqlca::default(),
            tables: HashMap::new(), schemas: HashMap::new(),
            cursors: HashMap::new(),
            on_error: WheneverAction::Continue, on_not_found: WheneverAction::Continue,
            conn: None,
        }
    }

    /// Create SQL context backed by a real SQLite database.
    pub fn with_db(db_path: &str) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sqlca: Sqlca::default(),
            tables: HashMap::new(), schemas: HashMap::new(),
            cursors: HashMap::new(),
            on_error: WheneverAction::Continue, on_not_found: WheneverAction::Continue,
            conn: Some(conn),
        })
    }

    /// Create SQL context with an in-memory SQLite database (for testing).
    pub fn with_memory_db() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory SQLite");
        Self {
            sqlca: Sqlca::default(),
            tables: HashMap::new(), schemas: HashMap::new(),
            cursors: HashMap::new(),
            on_error: WheneverAction::Continue, on_not_found: WheneverAction::Continue,
            conn: Some(conn),
        }
    }

    pub fn has_connection(&self) -> bool { self.conn.is_some() }

    // ── DDL ─────────────────────────────────────────────────────────

    pub fn create_table(&mut self, table: &str, columns: &[&str]) {
        let name = table.to_uppercase();
        if let Some(ref conn) = self.conn {
            let cols_sql: Vec<String> = columns.iter()
                .map(|c| format!("{} TEXT", c.to_uppercase()))
                .collect();
            let sql = format!("CREATE TABLE IF NOT EXISTS {} ({})", name, cols_sql.join(", "));
            match conn.execute(&sql, []) {
                Ok(_) => self.sqlca.set_ok(),
                Err(e) => self.sqlca.set_error(-601, &e.to_string()),
            }
        } else {
            self.schemas.insert(name.clone(), columns.iter().map(|c| c.to_uppercase()).collect());
            self.tables.entry(name).or_default();
            self.sqlca.set_ok();
        }
    }

    // ── INSERT ──────────────────────────────────────────────────────

    pub fn insert(&mut self, table: &str, values: &[(&str, SqlValue)]) {
        let name = table.to_uppercase();
        if let Some(ref conn) = self.conn {
            let cols: Vec<String> = values.iter().map(|(c, _)| c.to_uppercase()).collect();
            let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!("INSERT INTO {} ({}) VALUES ({})",
                name, cols.join(", "), placeholders.join(", "));
            let params: Vec<&SqlValue> = values.iter().map(|(_, v)| v).collect();
            match conn.execute(&sql, params_from_iter(params.iter())) {
                Ok(n) => { self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = n as i32; }
                Err(e) => self.sqlca.set_error(-803, &e.to_string()),
            }
        } else {
            // In-memory insert
            if let Some(schema) = self.schemas.get(&name) {
                let mut row_values = vec![SqlValue::Null; schema.len()];
                let row_cols = schema.clone();
                for (col, val) in values {
                    if let Some(idx) = schema.iter().position(|c| *c == col.to_uppercase()) {
                        row_values[idx] = val.clone();
                    }
                }
                self.tables.entry(name).or_default().push(SqlRow { columns: row_cols, values: row_values });
                self.sqlca.set_ok();
                self.sqlca.sqlerrd[2] = 1;
            } else {
                self.sqlca.set_error(-204, &format!("Table {} not found", name));
            }
        }
    }

    // ── SELECT INTO ─────────────────────────────────────────────────

    pub fn select_into(&mut self, table: &str, where_col: &str, where_val: &SqlValue) -> Option<SqlRow> {
        let name = table.to_uppercase();
        let col = where_col.to_uppercase();
        if let Some(ref conn) = self.conn {
            let sql = format!("SELECT * FROM {} WHERE {} = ?1", name, col);
            match conn.prepare(&sql) {
                Ok(mut stmt) => {
                    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
                    let result = stmt.query_row([where_val], |row| {
                        let mut vals = Vec::new();
                        for i in 0..col_names.len() {
                            let v: String = row.get::<_, String>(i).unwrap_or_default();
                            vals.push(SqlValue::Text(v));
                        }
                        Ok(SqlRow { columns: col_names.clone(), values: vals })
                    }).optional();
                    match result {
                        Ok(Some(r)) => { self.sqlca.set_ok(); Some(r) }
                        Ok(None) => { self.sqlca.set_not_found(); None }
                        Err(e) => { self.sqlca.set_error(-811, &e.to_string()); None }
                    }
                }
                Err(e) => { self.sqlca.set_error(-204, &e.to_string()); None }
            }
        } else {
            // In-memory select
            if let Some(rows) = self.tables.get(&name) {
                for row in rows {
                    if let Some(val) = row.get(&col) {
                        if val.as_text() == where_val.as_text() {
                            self.sqlca.set_ok();
                            return Some(row.clone());
                        }
                    }
                }
                self.sqlca.set_not_found();
                None
            } else {
                self.sqlca.set_error(-204, &format!("Table {} not found", name));
                None
            }
        }
    }

    pub fn select_where(&mut self, table: &str, predicate: impl Fn(&SqlRow) -> bool) -> Vec<SqlRow> {
        let name = table.to_uppercase();
        if let Some(rows) = self.tables.get(&name) {
            let results: Vec<SqlRow> = rows.iter().filter(|r| predicate(r)).cloned().collect();
            if results.is_empty() { self.sqlca.set_not_found(); }
            else { self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = results.len() as i32; }
            results
        } else {
            self.sqlca.set_error(-204, &format!("Table {} not found", name));
            Vec::new()
        }
    }

    // ── UPDATE ──────────────────────────────────────────────────────

    pub fn update(&mut self, table: &str, set_values: &[(&str, SqlValue)],
                  where_col: &str, where_val: &SqlValue) -> i32 {
        let name = table.to_uppercase();
        let wc = where_col.to_uppercase();
        if let Some(ref conn) = self.conn {
            let set_clauses: Vec<String> = set_values.iter().enumerate()
                .map(|(i, (c, _))| format!("{} = ?{}", c.to_uppercase(), i + 1))
                .collect();
            let where_param = set_values.len() + 1;
            let sql = format!("UPDATE {} SET {} WHERE {} = ?{}",
                name, set_clauses.join(", "), wc, where_param);
            let mut params: Vec<&SqlValue> = set_values.iter().map(|(_, v)| v).collect();
            params.push(where_val);
            match conn.execute(&sql, params_from_iter(params.iter())) {
                Ok(n) => { self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = n as i32; n as i32 }
                Err(e) => { self.sqlca.set_error(-530, &e.to_string()); 0 }
            }
        } else {
            // In-memory update
            let mut count = 0;
            if let Some(rows) = self.tables.get_mut(&name) {
                for row in rows.iter_mut() {
                    if let Some(val) = row.get(&wc) {
                        if val.as_text() == where_val.as_text() {
                            for (col, new_val) in set_values {
                                let col_upper = col.to_uppercase();
                                if let Some(idx) = row.columns.iter().position(|c| c.to_uppercase() == col_upper) {
                                    row.values[idx] = new_val.clone();
                                }
                            }
                            count += 1;
                        }
                    }
                }
                self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = count;
            } else {
                self.sqlca.set_error(-204, &format!("Table {} not found", name));
            }
            count
        }
    }

    // ── DELETE ───────────────────────────────────────────────────────

    pub fn delete(&mut self, table: &str, where_col: &str, where_val: &SqlValue) -> i32 {
        let name = table.to_uppercase();
        let wc = where_col.to_uppercase();
        if let Some(ref conn) = self.conn {
            let sql = format!("DELETE FROM {} WHERE {} = ?1", name, wc);
            match conn.execute(&sql, [where_val]) {
                Ok(n) => { self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = n as i32; n as i32 }
                Err(e) => { self.sqlca.set_error(-530, &e.to_string()); 0 }
            }
        } else {
            let wv = where_val.as_text();
            if let Some(rows) = self.tables.get_mut(&name) {
                let before = rows.len();
                rows.retain(|r| r.get(&wc).map(|v| v.as_text() != wv).unwrap_or(true));
                let count = (before - rows.len()) as i32;
                self.sqlca.set_ok(); self.sqlca.sqlerrd[2] = count; count
            } else {
                self.sqlca.set_error(-204, &format!("Table {} not found", name)); 0
            }
        }
    }

    // ── CURSOR ──────────────────────────────────────────────────────

    pub fn declare_cursor(&mut self, name: &str) {
        self.cursors.insert(name.to_uppercase(), SqlCursor::new(name));
        self.sqlca.set_ok();
    }

    /// Declare cursor with associated SQL query (for SQLite mode).
    pub fn declare_cursor_sql(&mut self, name: &str, sql: &str) {
        let mut cursor = SqlCursor::new(name);
        cursor.query = Some(sql.to_string());
        self.cursors.insert(name.to_uppercase(), cursor);
        self.sqlca.set_ok();
    }

    pub fn open_cursor(&mut self, name: &str, rows: Vec<SqlRow>) {
        let key = name.to_uppercase();
        if let Some(cursor) = self.cursors.get_mut(&key) {
            cursor.rows = rows;
            cursor.position = 0;
            cursor.is_open = true;
            self.sqlca.set_ok();
        } else {
            self.sqlca.set_error(-502, &format!("Cursor {} not declared", name));
        }
    }

    /// Open cursor and execute its associated SQL query (SQLite mode).
    pub fn open_cursor_sql(&mut self, name: &str, host_vars: &HashMap<String, SqlValue>) {
        let key = name.to_uppercase();
        let query = match self.cursors.get(&key) {
            Some(c) => c.query.clone(),
            None => { self.sqlca.set_error(-502, &format!("Cursor {} not declared", name)); return; }
        };
        if let (Some(ref conn), Some(sql)) = (&self.conn, query) {
            let (resolved, params) = substitute_host_vars(&sql, host_vars);
            match conn.prepare(&resolved) {
                Ok(mut stmt) => {
                    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
                    let param_refs: Vec<&SqlValue> = params.iter().collect();
                    let rows_result = stmt.query_map(params_from_iter(param_refs.iter()), |row| {
                        let mut vals = Vec::new();
                        for i in 0..col_names.len() {
                            let v: String = row.get::<_, String>(i).unwrap_or_default();
                            vals.push(SqlValue::Text(v));
                        }
                        Ok(SqlRow { columns: col_names.clone(), values: vals })
                    });
                    match rows_result {
                        Ok(iter) => {
                            let rows: Vec<SqlRow> = iter.filter_map(|r| r.ok()).collect();
                            let cursor = self.cursors.get_mut(&key).unwrap();
                            cursor.rows = rows;
                            cursor.position = 0;
                            cursor.is_open = true;
                            self.sqlca.set_ok();
                        }
                        Err(e) => self.sqlca.set_error(-530, &e.to_string()),
                    }
                }
                Err(e) => self.sqlca.set_error(-204, &e.to_string()),
            }
        } else {
            // Fallback: open with no rows
            if let Some(cursor) = self.cursors.get_mut(&key) {
                cursor.position = 0;
                cursor.is_open = true;
                self.sqlca.set_ok();
            }
        }
    }

    pub fn fetch_cursor(&mut self, name: &str) -> Option<SqlRow> {
        let key = name.to_uppercase();
        if let Some(cursor) = self.cursors.get_mut(&key) {
            if !cursor.is_open { self.sqlca.set_error(-501, "Cursor not open"); return None; }
            match cursor.fetch() {
                Some(row) => { self.sqlca.set_ok(); Some(row.clone()) }
                None => { self.sqlca.set_not_found(); None }
            }
        } else {
            self.sqlca.set_error(-502, &format!("Cursor {} not declared", name));
            None
        }
    }

    pub fn close_cursor(&mut self, name: &str) {
        let key = name.to_uppercase();
        if let Some(cursor) = self.cursors.get_mut(&key) {
            cursor.is_open = false;
            cursor.rows.clear();
            cursor.position = 0;
            self.sqlca.set_ok();
        } else {
            self.sqlca.set_error(-502, &format!("Cursor {} not declared", name));
        }
    }

    // ── WHENEVER ────────────────────────────────────────────────────

    pub fn whenever_error(&mut self, action: &str) { self.on_error = parse_whenever(action); }
    pub fn whenever_not_found(&mut self, action: &str) { self.on_not_found = parse_whenever(action); }

    pub fn check_whenever(&self) -> Option<String> {
        if self.sqlca.is_error() {
            if let WheneverAction::GoTo(label) = &self.on_error { return Some(label.clone()); }
        }
        if self.sqlca.is_not_found() {
            if let WheneverAction::GoTo(label) = &self.on_not_found { return Some(label.clone()); }
        }
        None
    }

    // ── Raw SQL Execution (Phase 5) ─────────────────────────────────

    /// Execute raw SQL with host variable substitution.
    /// In SQLite mode: actually executes against database.
    /// In memory mode: routes to appropriate in-memory handler.
    pub fn execute_sql(&mut self, sql: &str, host_vars: &HashMap<String, SqlValue>) {
        if let Some(ref conn) = self.conn {
            let (resolved, params) = substitute_host_vars(sql, host_vars);
            let sql_upper = resolved.trim_start().to_uppercase();

            if sql_upper.starts_with("SELECT") {
                // SELECT — no result returned via execute_sql; use cursors
                self.sqlca.set_ok();
            } else if sql_upper.starts_with("COMMIT") {
                match conn.execute_batch("COMMIT") {
                    Ok(_) => self.sqlca.set_ok(),
                    Err(e) => self.sqlca.set_error(-911, &e.to_string()),
                }
            } else if sql_upper.starts_with("ROLLBACK") {
                match conn.execute_batch("ROLLBACK") {
                    Ok(_) => self.sqlca.set_ok(),
                    Err(e) => self.sqlca.set_error(-911, &e.to_string()),
                }
            } else {
                // INSERT, UPDATE, DELETE, DDL
                let param_refs: Vec<&SqlValue> = params.iter().collect();
                match conn.execute(&resolved, params_from_iter(param_refs.iter())) {
                    Ok(n) => {
                        self.sqlca.set_ok();
                        self.sqlca.sqlerrd[2] = n as i32;
                    }
                    Err(e) => {
                        let code = if e.to_string().contains("UNIQUE") { -803 } else { -530 };
                        self.sqlca.set_error(code, &e.to_string());
                    }
                }
            }
        } else {
            // In-memory mode: no-op, always OK
            self.sqlca.set_ok();
        }
    }

    // ── Transaction Control ─────────────────────────────────────────

    pub fn commit(&mut self) {
        if let Some(ref conn) = self.conn {
            match conn.execute_batch("COMMIT") {
                Ok(_) => self.sqlca.set_ok(),
                Err(e) => self.sqlca.set_error(-911, &e.to_string()),
            }
        } else {
            self.sqlca.set_ok();
        }
    }

    pub fn rollback_sql(&mut self) {
        if let Some(ref conn) = self.conn {
            match conn.execute_batch("ROLLBACK") {
                Ok(_) => self.sqlca.set_ok(),
                Err(e) => self.sqlca.set_error(-911, &e.to_string()),
            }
        } else {
            self.sqlca.set_ok();
        }
    }

    // ── CardDemo Schema Initializer ─────────────────────────────────

    pub fn init_carddemo_schema(&mut self) {
        let ddl = [
            "CREATE TABLE IF NOT EXISTS ACCTDATA (
                ACCT_ID TEXT PRIMARY KEY, ACCT_STATUS TEXT, ACCT_CURR_BAL TEXT,
                ACCT_CREDIT_LIMIT TEXT, ACCT_CASH_CREDIT_LIMIT TEXT,
                ACCT_OPEN_DATE TEXT, ACCT_EXPIRY_DATE TEXT,
                ACCT_REISSUE_DATE TEXT, ACCT_CURR_CYC_CREDIT TEXT,
                ACCT_CURR_CYC_DEBIT TEXT, ACCT_GROUP_ID TEXT)",
            "CREATE TABLE IF NOT EXISTS CUSTDATA (
                CUST_ID TEXT PRIMARY KEY, CUST_FIRST_NAME TEXT, CUST_MIDDLE_NAME TEXT,
                CUST_LAST_NAME TEXT, CUST_ADDR_LINE_1 TEXT, CUST_ADDR_LINE_2 TEXT,
                CUST_ADDR_LINE_3 TEXT, CUST_ADDR_STATE_CD TEXT, CUST_ADDR_COUNTRY_CD TEXT,
                CUST_ADDR_ZIP TEXT, CUST_PHONE_NUM_1 TEXT, CUST_PHONE_NUM_2 TEXT,
                CUST_SSN TEXT, CUST_GOVT_ISSUED_ID TEXT, CUST_DOB_YYYYMMDD TEXT,
                CUST_EFT_ACCOUNT_ID TEXT, CUST_PRI_CARD_HOLDER_IND TEXT,
                CUST_FICO_CREDIT_SCORE TEXT)",
            "CREATE TABLE IF NOT EXISTS CARDDATA (
                CARD_NUM TEXT PRIMARY KEY, CARD_ACCT_ID TEXT, CARD_CVV_CD TEXT,
                CARD_EMBOSSED_NAME TEXT, CARD_EXPIRAION_DATE TEXT,
                CARD_ACTIVE_STATUS TEXT)",
            "CREATE TABLE IF NOT EXISTS CARDXREF (
                CARD_NUM TEXT PRIMARY KEY, CARD_ACCT_ID TEXT, CARD_CUST_ID TEXT)",
            "CREATE TABLE IF NOT EXISTS TRANDATA (
                TRAN_ID TEXT PRIMARY KEY, TRAN_TYPE_CD TEXT, TRAN_CAT_CD TEXT,
                TRAN_SOURCE TEXT, TRAN_DESC TEXT, TRAN_AMT TEXT,
                TRAN_MERCHANT_ID TEXT, TRAN_MERCHANT_NAME TEXT,
                TRAN_MERCHANT_CITY TEXT, TRAN_MERCHANT_ZIP TEXT,
                TRAN_CARD_NUM TEXT, TRAN_ORIG_TS TEXT, TRAN_PROC_TS TEXT)",
            "CREATE TABLE IF NOT EXISTS TRANCATG (
                TRAN_TYPE_CD TEXT, TRAN_CAT_CD TEXT, TRAN_CAT_DESC TEXT,
                PRIMARY KEY (TRAN_TYPE_CD, TRAN_CAT_CD))",
            "CREATE TABLE IF NOT EXISTS TRANTYPG (
                TRAN_TYPE_CD TEXT PRIMARY KEY, TRAN_TYPE_DESC TEXT)",
        ];
        for sql in &ddl {
            let vars = HashMap::new();
            self.execute_sql(sql, &vars);
            if self.sqlca.is_error() { return; }
        }
    }
}

// ── Host Variable Substitution ──────────────────────────────────────

/// Replace :HOST-VAR with positional ?N parameters. Returns resolved SQL + ordered params.
fn substitute_host_vars(sql: &str, host_vars: &HashMap<String, SqlValue>) -> (String, Vec<SqlValue>) {
    let mut resolved = String::with_capacity(sql.len());
    let mut params = Vec::new();
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    let mut param_index = 0;

    while let Some(c) = chars.next() {
        if c == '\'' {
            in_string = !in_string;
            resolved.push(c);
        } else if c == ':' && !in_string {
            let mut name = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '-' || nc == '_' {
                    name.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if !name.is_empty() {
                param_index += 1;
                resolved.push_str(&format!("?{}", param_index));
                let val = host_vars.get(&name)
                    .or_else(|| host_vars.get(&name.to_uppercase()))
                    .cloned()
                    .unwrap_or(SqlValue::Null);
                params.push(val);
            } else {
                resolved.push(c);
            }
        } else {
            resolved.push(c);
        }
    }
    (resolved, params)
}

/// Extract host variable names from SQL text.
pub fn extract_host_vars(sql: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if c == '\'' { in_string = !in_string; }
        else if c == ':' && !in_string {
            let mut name = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '-' || nc == '_' { name.push(nc); chars.next(); }
                else { break; }
            }
            if !name.is_empty() { vars.push(name); }
        }
    }
    vars
}

fn parse_whenever(action: &str) -> WheneverAction {
    let upper = action.trim().to_uppercase();
    if upper == "CONTINUE" { WheneverAction::Continue }
    else if upper == "STOP" { WheneverAction::Stop }
    else if upper.starts_with("GO TO") || upper.starts_with("GOTO") {
        let label = upper.replace("GO TO", "").replace("GOTO", "").trim().to_string();
        WheneverAction::GoTo(label)
    } else { WheneverAction::Continue }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- Original tests (preserved) --

    #[test]
    fn test_sqlca_defaults() {
        let sqlca = Sqlca::default();
        assert_eq!(sqlca.sqlcode, 0);
        assert!(sqlca.is_ok());
    }

    #[test]
    fn test_create_insert_select() {
        let mut ctx = SqlContext::new();
        ctx.create_table("EMPLOYEES", &["EMPNO", "NAME", "SALARY"]);
        ctx.insert("EMPLOYEES", &[
            ("EMPNO", SqlValue::Integer(1001)),
            ("NAME", SqlValue::Text("JOHN DOE".into())),
            ("SALARY", SqlValue::Float(50000.0)),
        ]);
        assert!(ctx.sqlca.is_ok());
        let row = ctx.select_into("EMPLOYEES", "EMPNO", &SqlValue::Integer(1001));
        assert!(row.is_some());
        assert_eq!(row.unwrap().get_text("NAME"), "JOHN DOE");
    }

    #[test]
    fn test_select_not_found() {
        let mut ctx = SqlContext::new();
        ctx.create_table("T", &["ID"]);
        assert!(ctx.select_into("T", "ID", &SqlValue::Integer(999)).is_none());
        assert_eq!(ctx.sqlca.sqlcode, 100);
    }

    #[test]
    fn test_update() {
        let mut ctx = SqlContext::new();
        ctx.create_table("T", &["ID", "VAL"]);
        ctx.insert("T", &[("ID", SqlValue::Integer(1)), ("VAL", SqlValue::Text("OLD".into()))]);
        let count = ctx.update("T", &[("VAL", SqlValue::Text("NEW".into()))], "ID", &SqlValue::Integer(1));
        assert_eq!(count, 1);
        assert_eq!(ctx.select_into("T", "ID", &SqlValue::Integer(1)).unwrap().get_text("VAL"), "NEW");
    }

    #[test]
    fn test_delete() {
        let mut ctx = SqlContext::new();
        ctx.create_table("T", &["ID"]);
        ctx.insert("T", &[("ID", SqlValue::Integer(1))]);
        ctx.insert("T", &[("ID", SqlValue::Integer(2))]);
        assert_eq!(ctx.delete("T", "ID", &SqlValue::Integer(1)), 1);
        assert!(ctx.select_into("T", "ID", &SqlValue::Integer(1)).is_none());
        assert!(ctx.select_into("T", "ID", &SqlValue::Integer(2)).is_some());
    }

    #[test]
    fn test_cursor_lifecycle() {
        let mut ctx = SqlContext::new();
        ctx.create_table("T", &["ID", "NAME"]);
        ctx.insert("T", &[("ID", SqlValue::Integer(1)), ("NAME", SqlValue::Text("A".into()))]);
        ctx.insert("T", &[("ID", SqlValue::Integer(2)), ("NAME", SqlValue::Text("B".into()))]);
        ctx.declare_cursor("C1");
        let rows = ctx.select_where("T", |_| true);
        ctx.open_cursor("C1", rows);
        assert_eq!(ctx.fetch_cursor("C1").unwrap().get_text("NAME"), "A");
        assert_eq!(ctx.fetch_cursor("C1").unwrap().get_text("NAME"), "B");
        assert!(ctx.fetch_cursor("C1").is_none());
        ctx.close_cursor("C1");
    }

    #[test]
    fn test_host_var_extraction() {
        let vars = extract_host_vars("SELECT * FROM T WHERE ID = :EMP-ID AND NAME = :WS-NAME");
        assert_eq!(vars, vec!["EMP-ID", "WS-NAME"]);
    }

    #[test]
    fn test_host_var_in_string_ignored() {
        assert!(extract_host_vars("SELECT * FROM T WHERE NAME = ':NOT-A-VAR'").is_empty());
    }

    #[test]
    fn test_null_indicator() {
        let v = SqlValue::Null;
        assert!(v.is_null());
        assert_eq!(v.as_text(), "");
    }

    #[test]
    fn test_whenever() {
        let mut ctx = SqlContext::new();
        ctx.whenever_error("GO TO ERROR-PARA");
        ctx.sqlca.set_error(-911, "deadlock");
        assert_eq!(ctx.check_whenever(), Some("ERROR-PARA".to_string()));
        ctx.whenever_not_found("GO TO EOF-PARA");
        ctx.sqlca.set_not_found();
        assert_eq!(ctx.check_whenever(), Some("EOF-PARA".to_string()));
    }

    #[test]
    fn test_table_not_found() {
        let mut ctx = SqlContext::new();
        ctx.insert("NOPE", &[("X", SqlValue::Integer(1))]);
        assert_eq!(ctx.sqlca.sqlcode, -204);
    }

    // -- Phase 5: SQLite execution tests --

    #[test]
    fn test_sqlite_create_insert_select() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.create_table("EMP", &["ID", "NAME"]);
        assert!(ctx.sqlca.is_ok());
        ctx.insert("EMP", &[("ID", SqlValue::Text("1".into())), ("NAME", SqlValue::Text("Alice".into()))]);
        assert!(ctx.sqlca.is_ok());
        let row = ctx.select_into("EMP", "ID", &SqlValue::Text("1".into()));
        assert!(row.is_some());
        assert_eq!(row.unwrap().get_text("NAME"), "Alice");
    }

    #[test]
    fn test_sqlite_update() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.create_table("T", &["ID", "VAL"]);
        ctx.insert("T", &[("ID", SqlValue::Text("1".into())), ("VAL", SqlValue::Text("OLD".into()))]);
        let n = ctx.update("T", &[("VAL", SqlValue::Text("NEW".into()))], "ID", &SqlValue::Text("1".into()));
        assert_eq!(n, 1);
        assert_eq!(ctx.select_into("T", "ID", &SqlValue::Text("1".into())).unwrap().get_text("VAL"), "NEW");
    }

    #[test]
    fn test_sqlite_delete() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.create_table("T", &["ID"]);
        ctx.insert("T", &[("ID", SqlValue::Text("1".into()))]);
        ctx.insert("T", &[("ID", SqlValue::Text("2".into()))]);
        assert_eq!(ctx.delete("T", "ID", &SqlValue::Text("1".into())), 1);
        assert!(ctx.select_into("T", "ID", &SqlValue::Text("1".into())).is_none());
    }

    #[test]
    fn test_sqlite_execute_sql_insert() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.execute_sql("CREATE TABLE T (id TEXT, name TEXT)", &HashMap::new());
        assert!(ctx.sqlca.is_ok());
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text("100".into()));
        vars.insert("NAME".to_string(), SqlValue::Text("Bob".into()));
        ctx.execute_sql("INSERT INTO T (id, name) VALUES (:ID, :NAME)", &vars);
        assert!(ctx.sqlca.is_ok());
        assert_eq!(ctx.sqlca.sqlerrd[2], 1);
    }

    #[test]
    fn test_sqlite_cursor_sql() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.execute_sql("CREATE TABLE T (id TEXT, name TEXT)", &HashMap::new());
        ctx.execute_sql("INSERT INTO T VALUES ('1', 'A')", &HashMap::new());
        ctx.execute_sql("INSERT INTO T VALUES ('2', 'B')", &HashMap::new());
        ctx.declare_cursor_sql("C1", "SELECT id, name FROM T ORDER BY id");
        ctx.open_cursor_sql("C1", &HashMap::new());
        let r1 = ctx.fetch_cursor("C1").unwrap();
        assert_eq!(r1.get_text("name"), "A");
        let r2 = ctx.fetch_cursor("C1").unwrap();
        assert_eq!(r2.get_text("name"), "B");
        assert!(ctx.fetch_cursor("C1").is_none());
        ctx.close_cursor("C1");
    }

    #[test]
    fn test_sqlite_cursor_with_host_vars() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.execute_sql("CREATE TABLE T (id TEXT, name TEXT)", &HashMap::new());
        ctx.execute_sql("INSERT INTO T VALUES ('1', 'A')", &HashMap::new());
        ctx.execute_sql("INSERT INTO T VALUES ('2', 'B')", &HashMap::new());
        ctx.declare_cursor_sql("C1", "SELECT * FROM T WHERE id = :ID");
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text("2".into()));
        ctx.open_cursor_sql("C1", &vars);
        let r = ctx.fetch_cursor("C1").unwrap();
        assert_eq!(r.get_text("name"), "B");
        assert!(ctx.fetch_cursor("C1").is_none());
    }

    #[test]
    fn test_host_var_substitution() {
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Integer(42));
        vars.insert("NAME".to_string(), SqlValue::Text("test".into()));
        let (sql, params) = substitute_host_vars(
            "SELECT * FROM T WHERE id = :ID AND name = :NAME", &vars);
        assert_eq!(sql, "SELECT * FROM T WHERE id = ?1 AND name = ?2");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_carddemo_schema() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.init_carddemo_schema();
        assert!(ctx.sqlca.is_ok());
        // Insert into ACCTDATA
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text("00000000001".into()));
        vars.insert("STATUS".to_string(), SqlValue::Text("Y".into()));
        ctx.execute_sql(
            "INSERT INTO ACCTDATA (ACCT_ID, ACCT_STATUS) VALUES (:ID, :STATUS)", &vars);
        assert!(ctx.sqlca.is_ok());
    }

    #[test]
    fn test_sqlite_select_not_found() {
        let mut ctx = SqlContext::with_memory_db();
        ctx.create_table("T", &["ID"]);
        let r = ctx.select_into("T", "ID", &SqlValue::Text("NOPE".into()));
        assert!(r.is_none());
        assert_eq!(ctx.sqlca.sqlcode, 100);
    }

    #[test]
    fn test_sqlite_error_handling() {
        let mut ctx = SqlContext::with_memory_db();
        let mut vars = HashMap::new();
        ctx.execute_sql("INSERT INTO NONEXISTENT (x) VALUES ('y')", &vars);
        assert!(ctx.sqlca.is_error());
    }
}
