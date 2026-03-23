// CICS Runtime for Ironclad-generated Rust programs.
// Replaces IBM CICS transaction server with native Rust equivalents.
// All EXEC CICS commands route through CicsContext::execute().

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write, BufRead, BufReader, BufWriter};
use std::fs::{File, OpenOptions};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::vsam::{VsamStore, VsamError, VsamOrganization};
use crate::bms::{BmsRegistry, ScreenOutput, ScreenInput, ScreenChannel, dfhaid};

// ── Response Codes ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CicsResp {
    Normal        = 0,
    Error         = 1,
    Eof           = 2,
    NotFound      = 13,
    DuplicateKey  = 14,
    DuplicateRec  = 15,
    InvalidReq    = 16,
    IoErr         = 17,
    Disabled      = 18,
    NotOpen       = 19,
    EndData       = 20,
    LenErr        = 22,
    QIdErr        = 26,
    ItemErr       = 27,
    PgmIdErr      = 28,
    NotAuth       = 70,
}

impl CicsResp {
    pub fn code(self) -> i32 { self as i32 }
}

fn vsam_err_to_resp(e: VsamError) -> CicsResp {
    match e {
        VsamError::Normal     => CicsResp::Normal,
        VsamError::NotFound   => CicsResp::NotFound,
        VsamError::DuplicateKey => CicsResp::DuplicateKey,
        VsamError::NotOpen    => CicsResp::NotOpen,
        VsamError::EndData    => CicsResp::EndData,
        VsamError::IoErr      => CicsResp::IoErr,
        VsamError::QIdErr     => CicsResp::QIdErr,
        VsamError::ItemErr    => CicsResp::ItemErr,
        VsamError::InvalidReq => CicsResp::InvalidReq,
    }
}

// ── Program Action (Phase 2) ────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ProgramAction {
    Continue,
    Xctl { program: String, commarea: Vec<u8> },
    Return { transid: Option<String>, commarea: Vec<u8> },
    Abend { code: String },
}

// ── Supporting types ────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum ConditionAction {
    Label(String),
    Ignore,
    Default,
}

struct FlatBrowseCursor {
    reader: BufReader<File>,
    ridfld: String,
}

#[derive(Debug, Clone)]
struct JournalEntry {
    operation: String,
    file: String,
    key: String,
    before_image: Vec<u8>,
}

struct TsQueue {
    items: Vec<Vec<u8>>,
    cursor: usize,
}

impl TsQueue {
    fn new() -> Self { Self { items: Vec::new(), cursor: 0 } }
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub program: String,
    pub data: Vec<u8>,
    pub interval: u32,
}

// ── CICS Context ────────────────────────────────────────────────────

pub struct CicsContext {
    // EIB fields
    pub resp: i32,
    pub resp2: i32,
    pub calen: i32,
    pub tran_id: String,
    pub commarea: Vec<u8>,
    pub eibaid: u8,

    // VSAM storage (Phase 1) — None = flat-file mode
    vsam_store: Option<VsamStore>,

    // In-memory TSQ (used when vsam_store is None)
    ts_queues: HashMap<String, TsQueue>,

    // Extrapartition TDQ (file-backed)
    td_queues: HashMap<String, String>,

    // Flat-file browse cursors (used when vsam_store is None)
    browse_cursors: HashMap<u32, FlatBrowseCursor>,
    next_browse_token: u32,

    // Programs
    programs: HashMap<String, fn(&mut CicsContext, &[u8]) -> Vec<u8>>,

    // Condition + abend handlers
    handlers: HashMap<String, ConditionAction>,
    abend_handlers: HashMap<String, String>,

    // Journal (flat-file mode rollback)
    journal: Vec<JournalEntry>,

    // Program control (Phase 2)
    pub last_action: ProgramAction,
    start_queue: Vec<StartRequest>,
    retrieve_data: Option<Vec<u8>>,
    pub return_transid: Option<String>,
    pub return_commarea: Option<Vec<u8>>,

    // System services (Phase 6)
    pub abstime: u64,
    pub applid: String,
    pub sysid: String,
    pub userid: String,

    // BMS (Phase 7)
    bms_registry: BmsRegistry,
    screen_channel: Option<Box<dyn ScreenChannel>>,
    pub current_screen: Option<ScreenOutput>,
    pub last_input: Option<ScreenInput>,

    // I/O (legacy)
    output: Box<dyn Write + Send>,
    input: Box<dyn BufRead + Send>,
}

impl Default for CicsContext {
    fn default() -> Self { Self::new() }
}

impl std::fmt::Debug for CicsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CicsContext")
            .field("resp", &self.resp)
            .field("resp2", &self.resp2)
            .field("calen", &self.calen)
            .field("tran_id", &self.tran_id)
            .finish()
    }
}

impl Clone for CicsContext {
    fn clone(&self) -> Self { Self::new() }
}

impl CicsContext {
    pub fn new() -> Self {
        Self {
            resp: 0, resp2: 0, calen: 0,
            tran_id: String::new(), commarea: Vec::new(), eibaid: 0,
            vsam_store: None,
            ts_queues: HashMap::new(),
            td_queues: HashMap::new(),
            browse_cursors: HashMap::new(),
            next_browse_token: 1,
            programs: HashMap::new(),
            handlers: HashMap::new(),
            abend_handlers: HashMap::new(),
            journal: Vec::new(),
            last_action: ProgramAction::Continue,
            start_queue: Vec::new(),
            retrieve_data: None,
            return_transid: None,
            return_commarea: None,
            abstime: 0,
            applid: "CARDDEMO".into(),
            sysid: "CICS".into(),
            userid: "CICSUSER".into(),
            bms_registry: BmsRegistry::new(),
            screen_channel: None,
            current_screen: None,
            last_input: None,
            output: Box::new(io::stdout()),
            input: Box::new(BufReader::new(io::stdin())),
        }
    }

    pub fn with_io(output: Box<dyn Write + Send>, input: Box<dyn BufRead + Send>) -> Self {
        let mut ctx = Self::new();
        ctx.output = output;
        ctx.input = input;
        ctx
    }

    // ── VSAM Setup ──────────────────────────────────────────────────

    /// Initialize VSAM store with a database file.
    pub fn init_vsam(&mut self, db_path: &str) -> Result<(), String> {
        self.vsam_store = Some(VsamStore::new(db_path)?);
        Ok(())
    }

    /// Initialize VSAM store in-memory (for testing).
    pub fn init_vsam_memory(&mut self) {
        self.vsam_store = Some(VsamStore::new_in_memory());
    }

    /// Register a VSAM file.
    pub fn register_vsam_file(&mut self, name: &str, org: VsamOrganization) -> Result<(), String> {
        if let Some(ref mut vs) = self.vsam_store {
            vs.register_file(name, org)
        } else {
            Err("VSAM store not initialized".into())
        }
    }

    pub fn register_program(&mut self, name: &str, func: fn(&mut CicsContext, &[u8]) -> Vec<u8>) {
        self.programs.insert(name.to_uppercase(), func);
    }

    pub fn register_td_queue(&mut self, name: &str, path: &str) {
        self.td_queues.insert(name.to_uppercase(), path.to_string());
    }

    pub fn bms_registry_mut(&mut self) -> &mut BmsRegistry { &mut self.bms_registry }

    pub fn set_screen_channel(&mut self, ch: Box<dyn ScreenChannel>) {
        self.screen_channel = Some(ch);
    }

    fn set_resp(&mut self, r: CicsResp) {
        self.resp = r.code();
        self.resp2 = 0;
    }

    // ── SEND ────────────────────────────────────────────────────────

    pub fn send(&mut self, data: &str, erase: bool) {
        if erase { let _ = self.output.write_all(b"\x1B[2J\x1B[H"); }
        let _ = self.output.write_all(data.as_bytes());
        let _ = self.output.write_all(b"\n");
        let _ = self.output.flush();
        self.set_resp(CicsResp::Normal);
    }

    pub fn send_map(&mut self, map: &str, mapset: &str, data: &HashMap<String, String>, erase: bool) {
        // Try BMS registry first
        if let Some(screen) = self.bms_registry.build_screen(map, mapset, data, erase, None) {
            if let Some(ref mut ch) = self.screen_channel {
                let _ = ch.send_screen(&screen);
            } else {
                if erase { let _ = self.output.write_all(b"\x1B[2J\x1B[H"); }
                let _ = self.output.write_all(screen.to_text().as_bytes());
                let _ = self.output.flush();
            }
            self.current_screen = Some(screen);
        } else {
            // Fallback: legacy text output
            if erase { let _ = self.output.write_all(b"\x1B[2J\x1B[H"); }
            let _ = writeln!(self.output, "--- MAP: {} MAPSET: {} ---", map, mapset);
            for (field, value) in data {
                let _ = writeln!(self.output, "  {}: {}", field, value);
            }
            let _ = writeln!(self.output, "---");
            let _ = self.output.flush();
        }
        self.set_resp(CicsResp::Normal);
    }

    // ── RECEIVE ─────────────────────────────────────────────────────

    pub fn receive(&mut self, into: &mut HashMap<String, String>) {
        if let Some(ref mut ch) = self.screen_channel {
            match ch.receive_screen() {
                Ok(input) => {
                    self.eibaid = input.aid;
                    *into = input.fields.clone();
                    self.last_input = Some(input);
                    self.set_resp(CicsResp::Normal);
                }
                Err(_) => self.set_resp(CicsResp::Error),
            }
        } else {
            let mut line = String::new();
            match self.input.read_line(&mut line) {
                Ok(0) => self.set_resp(CicsResp::Eof),
                Ok(_) => {
                    for pair in line.trim().split(',') {
                        if let Some((k, v)) = pair.trim().split_once('=') {
                            into.insert(k.trim().to_uppercase(), v.trim().to_string());
                        }
                    }
                    self.set_resp(CicsResp::Normal);
                }
                Err(_) => self.set_resp(CicsResp::Error),
            }
        }
    }

    pub fn receive_map(&mut self, _map: &str, _mapset: &str) -> HashMap<String, String> {
        let mut fields = HashMap::new();
        self.receive(&mut fields);
        fields
    }

    // ── FILE CONTROL (VSAM + flat-file) ─────────────────────────────

    pub fn read_file(&mut self, file: &str, ridfld: &str) -> Option<String> {
        // VSAM path
        if let Some(ref vs) = self.vsam_store {
            if vs.is_registered(file) {
                return match vs.read(file, ridfld) {
                    Ok(data) => { self.set_resp(CicsResp::Normal); Some(data) }
                    Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
                };
            }
        }
        // Flat-file fallback
        let f = match File::open(file) {
            Ok(f) => f, Err(_) => { self.set_resp(CicsResp::NotOpen); return None; }
        };
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Some((key, data)) = line.split_once('\t') {
                if key == ridfld {
                    self.set_resp(CicsResp::Normal);
                    return Some(data.to_string());
                }
            }
        }
        self.set_resp(CicsResp::NotFound);
        None
    }

    pub fn write_file(&mut self, file: &str, ridfld: &str, data: &str) {
        if let Some(ref vs) = self.vsam_store {
            if vs.is_registered(file) {
                match vs.write(file, ridfld, data) {
                    Ok(_) => self.set_resp(CicsResp::Normal),
                    Err(e) => self.set_resp(vsam_err_to_resp(e)),
                }
                return;
            }
        }
        match OpenOptions::new().create(true).append(true).open(file) {
            Ok(mut f) => {
                let _ = writeln!(f, "{}\t{}", ridfld, data);
                self.journal.push(JournalEntry {
                    operation: "WRITE".into(), file: file.into(),
                    key: ridfld.into(), before_image: Vec::new(),
                });
                self.set_resp(CicsResp::Normal);
            }
            Err(_) => self.set_resp(CicsResp::NotOpen),
        }
    }

    pub fn rewrite_file(&mut self, file: &str, ridfld: &str, data: &str) {
        if let Some(ref vs) = self.vsam_store {
            if vs.is_registered(file) {
                match vs.rewrite(file, ridfld, data) {
                    Ok(_) => self.set_resp(CicsResp::Normal),
                    Err(e) => self.set_resp(vsam_err_to_resp(e)),
                }
                return;
            }
        }
        let lines: Vec<String> = match std::fs::read_to_string(file) {
            Ok(c) => c.lines().map(|l| l.to_string()).collect(),
            Err(_) => { self.set_resp(CicsResp::NotOpen); return; }
        };
        let mut found = false;
        let mut output = Vec::new();
        for line in &lines {
            if let Some((key, old_data)) = line.split_once('\t') {
                if key == ridfld {
                    found = true;
                    self.journal.push(JournalEntry {
                        operation: "REWRITE".into(), file: file.into(),
                        key: ridfld.into(), before_image: old_data.as_bytes().to_vec(),
                    });
                    output.push(format!("{}\t{}", ridfld, data));
                    continue;
                }
            }
            output.push(line.clone());
        }
        if found {
            let _ = std::fs::write(file, output.join("\n") + "\n");
            self.set_resp(CicsResp::Normal);
        } else {
            self.set_resp(CicsResp::NotFound);
        }
    }

    pub fn delete_file(&mut self, file: &str, ridfld: &str) {
        if let Some(ref vs) = self.vsam_store {
            if vs.is_registered(file) {
                match vs.delete(file, ridfld) {
                    Ok(_) => self.set_resp(CicsResp::Normal),
                    Err(e) => self.set_resp(vsam_err_to_resp(e)),
                }
                return;
            }
        }
        let lines: Vec<String> = match std::fs::read_to_string(file) {
            Ok(c) => c.lines().map(|l| l.to_string()).collect(),
            Err(_) => { self.set_resp(CicsResp::NotOpen); return; }
        };
        let before_len = lines.len();
        let filtered: Vec<&String> = lines.iter()
            .filter(|l| !l.starts_with(&format!("{}\t", ridfld)))
            .collect();
        if filtered.len() < before_len {
            let content: Vec<&str> = filtered.iter().map(|s| s.as_str()).collect();
            let _ = std::fs::write(file, content.join("\n") + "\n");
            self.set_resp(CicsResp::Normal);
        } else {
            self.set_resp(CicsResp::NotFound);
        }
    }

    // ── PROGRAM CONTROL (Phase 2) ───────────────────────────────────

    pub fn link(&mut self, program: &str, commarea: &[u8]) -> Vec<u8> {
        let key = program.to_uppercase();
        if let Some(func) = self.programs.get(&key).copied() {
            self.set_resp(CicsResp::Normal);
            func(self, commarea)
        } else {
            self.set_resp(CicsResp::PgmIdErr);
            Vec::new()
        }
    }

    pub fn xctl(&mut self, program: &str, commarea: &[u8]) -> Vec<u8> {
        self.last_action = ProgramAction::Xctl {
            program: program.to_uppercase(),
            commarea: commarea.to_vec(),
        };
        self.link(program, commarea)
    }

    pub fn return_program(&mut self, transid: Option<&str>) {
        if let Some(t) = transid {
            self.tran_id = t.to_uppercase();
            self.return_transid = Some(t.to_uppercase());
        }
        self.return_commarea = Some(self.commarea.clone());
        self.last_action = ProgramAction::Return {
            transid: transid.map(|s| s.to_uppercase()),
            commarea: self.commarea.clone(),
        };
        self.set_resp(CicsResp::Normal);
    }

    pub fn abend(&mut self, code: &str) {
        // Check HANDLE ABEND first
        if let Some(label) = self.abend_handlers.get(&code.to_uppercase()).cloned() {
            eprintln!("CICS ABEND {} handled by {}", code, label);
            self.set_resp(CicsResp::Normal);
            return;
        }
        // Check default abend handler
        if let Some(label) = self.abend_handlers.get("*").cloned() {
            eprintln!("CICS ABEND {} handled by default handler {}", code, label);
            self.set_resp(CicsResp::Normal);
            return;
        }
        self.last_action = ProgramAction::Abend { code: code.to_string() };
        eprintln!("CICS ABEND: {}", code);
        self.set_resp(CicsResp::Error);
    }

    pub fn start(&mut self, program: &str, data: &[u8], interval: u32) {
        self.start_queue.push(StartRequest {
            program: program.to_uppercase(),
            data: data.to_vec(),
            interval,
        });
        self.set_resp(CicsResp::Normal);
    }

    pub fn retrieve(&mut self) -> Option<Vec<u8>> {
        if let Some(data) = self.retrieve_data.take() {
            self.set_resp(CicsResp::Normal);
            Some(data)
        } else {
            self.set_resp(CicsResp::NotFound);
            None
        }
    }

    /// Set data to be returned by RETRIEVE (used by START dispatch).
    pub fn set_retrieve_data(&mut self, data: Vec<u8>) {
        self.retrieve_data = Some(data);
    }

    pub fn drain_start_queue(&mut self) -> Vec<StartRequest> {
        std::mem::take(&mut self.start_queue)
    }

    // ── TEMPORARY STORAGE QUEUES (Phase 3) ──────────────────────────

    pub fn writeq_ts(&mut self, queue: &str, data: &[u8]) {
        self.writeq_ts_item(queue, data, None);
    }

    pub fn writeq_ts_item(&mut self, queue: &str, data: &[u8], item: Option<usize>) {
        if let Some(ref vs) = self.vsam_store {
            match vs.tsq_write(queue, data, item) {
                Ok(_) => self.set_resp(CicsResp::Normal),
                Err(e) => self.set_resp(vsam_err_to_resp(e)),
            }
            return;
        }
        let key = queue.to_uppercase();
        let q = self.ts_queues.entry(key).or_insert_with(TsQueue::new);
        match item {
            Some(n) if n >= 1 && n <= q.items.len() => {
                q.items[n - 1] = data.to_vec();
            }
            Some(_) => { self.set_resp(CicsResp::ItemErr); return; }
            None => { q.items.push(data.to_vec()); }
        }
        self.set_resp(CicsResp::Normal);
    }

    pub fn readq_ts(&mut self, queue: &str) -> Option<Vec<u8>> {
        self.readq_ts_item(queue, None)
    }

    pub fn readq_ts_item(&mut self, queue: &str, item: Option<usize>) -> Option<Vec<u8>> {
        if let Some(ref vs) = self.vsam_store {
            return match item {
                Some(n) => match vs.tsq_read(queue, n) {
                    Ok(d) => { self.set_resp(CicsResp::Normal); Some(d) }
                    Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
                },
                None => {
                    // NEXT: need cursor tracking
                    let cursor = self.ts_queues.entry(queue.to_uppercase())
                        .or_insert_with(TsQueue::new).cursor;
                    match vs.tsq_read_next(queue, cursor) {
                        Ok((d, n)) => {
                            self.ts_queues.get_mut(&queue.to_uppercase()).unwrap().cursor = n;
                            self.set_resp(CicsResp::Normal);
                            Some(d)
                        }
                        Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
                    }
                }
            };
        }
        let key = queue.to_uppercase();
        // Extract result without holding mutable borrow across set_resp
        let result = if let Some(q) = self.ts_queues.get_mut(&key) {
            match item {
                Some(n) if n >= 1 && n <= q.items.len() => {
                    Ok(Some(q.items[n - 1].clone()))
                }
                Some(_) => Err(CicsResp::ItemErr),
                None => {
                    if q.cursor < q.items.len() {
                        let data = q.items[q.cursor].clone();
                        q.cursor += 1;
                        Ok(Some(data))
                    } else {
                        Err(CicsResp::ItemErr)
                    }
                }
            }
        } else {
            Err(CicsResp::QIdErr)
        };
        match result {
            Ok(data) => { self.set_resp(CicsResp::Normal); data }
            Err(resp) => { self.set_resp(resp); None }
        }
    }

    pub fn tsq_numitems(&mut self, queue: &str) -> usize {
        if let Some(ref vs) = self.vsam_store {
            return vs.tsq_numitems(queue);
        }
        self.ts_queues.get(&queue.to_uppercase()).map(|q| q.items.len()).unwrap_or(0)
    }

    pub fn deleteq_ts(&mut self, queue: &str) {
        if let Some(ref vs) = self.vsam_store {
            match vs.tsq_delete(queue) {
                Ok(_) => { self.set_resp(CicsResp::Normal); return; }
                Err(e) => { self.set_resp(vsam_err_to_resp(e)); return; }
            }
        }
        let key = queue.to_uppercase();
        if self.ts_queues.remove(&key).is_some() {
            self.set_resp(CicsResp::Normal);
        } else {
            self.set_resp(CicsResp::QIdErr);
        }
    }

    // ── TRANSIENT DATA QUEUES ───────────────────────────────────────

    pub fn writeq_td(&mut self, queue: &str, data: &[u8]) {
        let key = queue.to_uppercase();
        // Extrapartition (file-backed)
        if let Some(path) = self.td_queues.get(&key).cloned() {
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(mut f) => {
                    let _ = f.write_all(data);
                    let _ = f.write_all(b"\n");
                    self.set_resp(CicsResp::Normal);
                }
                Err(_) => self.set_resp(CicsResp::Disabled),
            }
            return;
        }
        // Intrapartition (SQLite-backed)
        if let Some(ref mut vs) = self.vsam_store {
            match vs.tdq_write(queue, data) {
                Ok(_) => self.set_resp(CicsResp::Normal),
                Err(e) => self.set_resp(vsam_err_to_resp(e)),
            }
            return;
        }
        self.set_resp(CicsResp::QIdErr);
    }

    pub fn readq_td(&mut self, queue: &str) -> Option<Vec<u8>> {
        let key = queue.to_uppercase();
        // Extrapartition
        if let Some(path) = self.td_queues.get(&key).cloned() {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c, Err(_) => { self.set_resp(CicsResp::QIdErr); return None; }
            };
            let mut lines: Vec<&str> = content.lines().collect();
            if lines.is_empty() {
                self.set_resp(CicsResp::QIdErr);
                return None;
            }
            let first = lines.remove(0).as_bytes().to_vec();
            let _ = std::fs::write(&path, lines.join("\n") + if lines.is_empty() { "" } else { "\n" });
            self.set_resp(CicsResp::Normal);
            return Some(first);
        }
        // Intrapartition
        if let Some(ref vs) = self.vsam_store {
            return match vs.tdq_read(queue) {
                Ok(d) => { self.set_resp(CicsResp::Normal); Some(d) }
                Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
            };
        }
        self.set_resp(CicsResp::QIdErr);
        None
    }

    // ── BROWSE ──────────────────────────────────────────────────────

    pub fn startbr(&mut self, file: &str, ridfld: &str) -> u32 {
        if let Some(ref mut vs) = self.vsam_store {
            if vs.is_registered(file) {
                return match vs.start_browse(file, ridfld) {
                    Ok(tok) => { self.set_resp(CicsResp::Normal); tok }
                    Err(e) => { self.set_resp(vsam_err_to_resp(e)); 0 }
                };
            }
        }
        // Flat-file
        match File::open(file) {
            Ok(f) => {
                let token = self.next_browse_token;
                self.next_browse_token += 1;
                self.browse_cursors.insert(token, FlatBrowseCursor {
                    reader: BufReader::new(f), ridfld: ridfld.to_string(),
                });
                self.set_resp(CicsResp::Normal);
                token
            }
            Err(_) => { self.set_resp(CicsResp::NotOpen); 0 }
        }
    }

    pub fn readnext(&mut self, token: u32) -> Option<(String, String)> {
        if let Some(ref mut vs) = self.vsam_store {
            return match vs.read_next(token) {
                Ok(r) => { self.set_resp(CicsResp::Normal); Some(r) }
                Err(VsamError::EndData) => { self.set_resp(CicsResp::EndData); None }
                Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
            };
        }
        if let Some(cursor) = self.browse_cursors.get_mut(&token) {
            let mut line = String::new();
            match cursor.reader.read_line(&mut line) {
                Ok(0) => { self.set_resp(CicsResp::EndData); None }
                Ok(_) => {
                    if let Some((key, data)) = line.trim().split_once('\t') {
                        self.set_resp(CicsResp::Normal);
                        Some((key.to_string(), data.to_string()))
                    } else {
                        self.set_resp(CicsResp::Error); None
                    }
                }
                Err(_) => { self.set_resp(CicsResp::Error); None }
            }
        } else {
            self.set_resp(CicsResp::NotOpen); None
        }
    }

    pub fn readprev(&mut self, token: u32) -> Option<(String, String)> {
        if let Some(ref mut vs) = self.vsam_store {
            return match vs.read_prev(token) {
                Ok(r) => { self.set_resp(CicsResp::Normal); Some(r) }
                Err(VsamError::EndData) => { self.set_resp(CicsResp::EndData); None }
                Err(e) => { self.set_resp(vsam_err_to_resp(e)); None }
            };
        }
        // Flat-file doesn't support READPREV
        self.set_resp(CicsResp::InvalidReq);
        None
    }

    pub fn endbr(&mut self, token: u32) {
        if let Some(ref mut vs) = self.vsam_store {
            match vs.end_browse(token) {
                Ok(_) => { self.set_resp(CicsResp::Normal); return; }
                Err(_) => {} // fall through to flat-file
            }
        }
        if self.browse_cursors.remove(&token).is_some() {
            self.set_resp(CicsResp::Normal);
        } else {
            self.set_resp(CicsResp::NotOpen);
        }
    }

    // ── HANDLE CONDITION / HANDLE ABEND ─────────────────────────────

    pub fn handle_condition(&mut self, condition: &str, action: &str) {
        let act = if action.eq_ignore_ascii_case("IGNORE") {
            ConditionAction::Ignore
        } else {
            ConditionAction::Label(action.to_uppercase())
        };
        self.handlers.insert(condition.to_uppercase(), act);
    }

    pub fn check_handler(&self, condition: &str) -> Option<String> {
        match self.handlers.get(&condition.to_uppercase()) {
            Some(ConditionAction::Label(lbl)) => Some(lbl.clone()),
            _ => None,
        }
    }

    pub fn handle_abend(&mut self, code: &str, label: &str) {
        self.abend_handlers.insert(code.to_uppercase(), label.to_uppercase());
    }

    // ── SYNCPOINT / ROLLBACK (Phase 4) ──────────────────────────────

    pub fn syncpoint(&mut self) {
        if let Some(ref mut vs) = self.vsam_store {
            if vs.is_in_transaction() { let _ = vs.commit(); }
        }
        self.journal.clear();
        self.set_resp(CicsResp::Normal);
    }

    pub fn rollback(&mut self) {
        if let Some(ref mut vs) = self.vsam_store {
            if vs.is_in_transaction() {
                let _ = vs.rollback_transaction();
                self.journal.clear();
                self.set_resp(CicsResp::Normal);
                return;
            }
        }
        // Flat-file journal replay
        for entry in self.journal.iter().rev() {
            match entry.operation.as_str() {
                "WRITE" => { let _ = self.delete_file_internal(&entry.file, &entry.key); }
                "REWRITE" => {
                    let data = String::from_utf8_lossy(&entry.before_image);
                    self.rewrite_file_internal(&entry.file, &entry.key, &data);
                }
                _ => {}
            }
        }
        self.journal.clear();
        self.set_resp(CicsResp::Normal);
    }

    /// Begin a transaction (for VSAM mode).
    pub fn begin_transaction(&mut self) {
        if let Some(ref mut vs) = self.vsam_store {
            let _ = vs.begin_transaction();
        }
    }

    fn delete_file_internal(&self, file_path: &str, ridfld: &str) {
        if let Ok(content) = std::fs::read_to_string(file_path) {
            let filtered: Vec<&str> = content.lines()
                .filter(|l| !l.starts_with(&format!("{}\t", ridfld)))
                .collect();
            let _ = std::fs::write(file_path, filtered.join("\n") + "\n");
        }
    }

    fn rewrite_file_internal(&self, file_path: &str, ridfld: &str, data: &str) {
        if let Ok(content) = std::fs::read_to_string(file_path) {
            let output: Vec<String> = content.lines().map(|line| {
                if line.starts_with(&format!("{}\t", ridfld)) {
                    format!("{}\t{}", ridfld, data)
                } else {
                    line.to_string()
                }
            }).collect();
            let _ = std::fs::write(file_path, output.join("\n") + "\n");
        }
    }

    // ── SYSTEM SERVICES (Phase 6) ───────────────────────────────────

    /// EXEC CICS ASKTIME — capture current time as ABSTIME.
    pub fn asktime(&mut self) {
        // ABSTIME = microseconds since 1900-01-01
        // Unix epoch is 1970-01-01 = 2208988800 seconds after 1900
        const EPOCH_OFFSET: u64 = 2_208_988_800;
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        self.abstime = (unix_secs + EPOCH_OFFSET) * 1_000_000;
        self.set_resp(CicsResp::Normal);
    }

    /// EXEC CICS FORMATTIME — format ABSTIME to date/time strings.
    pub fn formattime(&self, abstime: u64, format: &str, datesep: Option<char>) -> String {
        const EPOCH_OFFSET: u64 = 2_208_988_800;
        let total_secs = abstime / 1_000_000;
        let unix_secs = if total_secs >= EPOCH_OFFSET { total_secs - EPOCH_OFFSET } else { 0 };

        // Convert unix seconds to date/time
        let days = (unix_secs / 86400) as i64;
        let time_secs = (unix_secs % 86400) as u32;
        let hh = time_secs / 3600;
        let mm = (time_secs % 3600) / 60;
        let ss = time_secs % 60;

        // Gregorian calendar from Unix day count
        let (year, month, day) = days_to_ymd(days + 719468); // shift to 0000-03-01 epoch

        let sep = datesep.unwrap_or('\0');
        let fmt = format.to_uppercase();
        match fmt.as_str() {
            "YYYYMMDD" => {
                if sep != '\0' { format!("{:04}{}{:02}{}{:02}", year, sep, month, sep, day) }
                else { format!("{:04}{:02}{:02}", year, month, day) }
            }
            "DDMMYYYY" => {
                if sep != '\0' { format!("{:02}{}{:02}{}{:04}", day, sep, month, sep, year) }
                else { format!("{:02}{:02}{:04}", day, month, year) }
            }
            "MMDDYYYY" => {
                if sep != '\0' { format!("{:02}{}{:02}{}{:04}", month, sep, day, sep, year) }
                else { format!("{:02}{:02}{:04}", month, day, year) }
            }
            "TIME" => format!("{:02}:{:02}:{:02}", hh, mm, ss),
            _ => format!("{:04}{:02}{:02}", year, month, day),
        }
    }

    /// EXEC CICS ASSIGN — return system values.
    pub fn assign(&self, option: &str) -> String {
        match option.to_uppercase().as_str() {
            "APPLID"  => self.applid.clone(),
            "SYSID"   => self.sysid.clone(),
            "USERID"  => self.userid.clone(),
            "OPID"    => self.userid.clone(),
            "NETNAME" => self.sysid.clone(),
            _ => String::new(),
        }
    }

    /// EXEC CICS INQUIRE PROGRAM/FILE — check resource status.
    pub fn inquire(&self, resource_type: &str, name: &str) -> String {
        match resource_type.to_uppercase().as_str() {
            "PROGRAM" => {
                if self.programs.contains_key(&name.to_uppercase()) { "INSTALLED" }
                else { "NOTINSTALLED" }
            }
            "FILE" => {
                if let Some(ref vs) = self.vsam_store {
                    if vs.is_registered(name) { "ENABLED" } else { "DISABLED" }
                } else { "DISABLED" }
            }
            _ => "UNKNOWN",
        }.to_string()
    }

    // ── MASTER EXECUTE ──────────────────────────────────────────────

    pub fn execute(&mut self, command: &str, options: &[(&str, Option<&str>)]) -> Option<String> {
        let cmd = command.to_uppercase();
        let opt = |key: &str| -> Option<&str> {
            options.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .and_then(|(_, v)| *v)
        };
        let has = |key: &str| -> bool {
            options.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
        };

        match cmd.as_str() {
            "SEND" => {
                let erase = has("ERASE");
                if let Some(map) = opt("MAP") {
                    let mapset = opt("MAPSET").unwrap_or("DFHBMS");
                    // Collect field data from FROM or build from context
                    let mut data = HashMap::new();
                    if let Some(from) = opt("FROM") {
                        data.insert("_FROM".to_string(), from.to_string());
                    }
                    self.send_map(map, mapset, &data, erase);
                } else if let Some(from) = opt("FROM") {
                    self.send(from, erase);
                } else {
                    self.send("", erase);
                }
                None
            }
            "RECEIVE" => {
                if let Some(map) = opt("MAP") {
                    let mapset = opt("MAPSET").unwrap_or("DFHBMS");
                    let fields = self.receive_map(map, mapset);
                    let result: Vec<String> = fields.iter().map(|(k,v)| format!("{}={}", k, v)).collect();
                    Some(result.join(","))
                } else {
                    let mut fields = HashMap::new();
                    self.receive(&mut fields);
                    let result: Vec<String> = fields.iter().map(|(k,v)| format!("{}={}", k, v)).collect();
                    Some(result.join(","))
                }
            }
            "READ" => {
                let file = opt("FILE").or_else(|| opt("DATASET")).unwrap_or("");
                let ridfld = opt("RIDFLD").unwrap_or("");
                self.read_file(file, ridfld)
            }
            "WRITE" => {
                let file = opt("FILE").or_else(|| opt("DATASET")).unwrap_or("");
                let ridfld = opt("RIDFLD").unwrap_or("");
                let from = opt("FROM").unwrap_or("");
                self.write_file(file, ridfld, from);
                None
            }
            "REWRITE" => {
                let file = opt("FILE").or_else(|| opt("DATASET")).unwrap_or("");
                let ridfld = opt("RIDFLD").unwrap_or("");
                let from = opt("FROM").unwrap_or("");
                self.rewrite_file(file, ridfld, from);
                None
            }
            "DELETE" => {
                let file = opt("FILE").or_else(|| opt("DATASET")).unwrap_or("");
                let ridfld = opt("RIDFLD").unwrap_or("");
                self.delete_file(file, ridfld);
                None
            }
            "LINK" => {
                let prog = opt("PROGRAM").unwrap_or("");
                let comm = opt("COMMAREA").unwrap_or("").as_bytes();
                let result = self.link(prog, comm);
                if result.is_empty() { None } else { Some(String::from_utf8_lossy(&result).to_string()) }
            }
            "XCTL" => {
                let prog = opt("PROGRAM").unwrap_or("");
                let comm = opt("COMMAREA").unwrap_or("").as_bytes();
                let result = self.xctl(prog, comm);
                if result.is_empty() { None } else { Some(String::from_utf8_lossy(&result).to_string()) }
            }
            "RETURN" => {
                let transid = opt("TRANSID");
                if let Some(ca) = opt("COMMAREA") {
                    self.commarea = ca.as_bytes().to_vec();
                }
                self.return_program(transid);
                None
            }
            "ABEND" => {
                let code = opt("ABCODE").unwrap_or("????");
                self.abend(code);
                None
            }
            "START" => {
                let prog = opt("PROGRAM").unwrap_or("");
                let data = opt("FROM").unwrap_or("").as_bytes();
                let interval: u32 = opt("INTERVAL").and_then(|s| s.parse().ok()).unwrap_or(0);
                self.start(prog, data, interval);
                None
            }
            "RETRIEVE" => {
                self.retrieve().map(|d| String::from_utf8_lossy(&d).to_string())
            }
            "WRITEQ" => {
                let data = opt("FROM").unwrap_or("").as_bytes();
                if has("TS") {
                    let q = opt("QUEUE").unwrap_or("");
                    let item: Option<usize> = opt("ITEM").and_then(|s| s.parse().ok());
                    self.writeq_ts_item(q, data, item);
                } else {
                    let q = opt("QUEUE").unwrap_or("");
                    self.writeq_td(q, data);
                }
                None
            }
            "READQ" => {
                if has("TS") {
                    let q = opt("QUEUE").unwrap_or("");
                    let item: Option<usize> = opt("ITEM").and_then(|s| s.parse().ok());
                    self.readq_ts_item(q, item).map(|d| String::from_utf8_lossy(&d).to_string())
                } else {
                    let q = opt("QUEUE").unwrap_or("");
                    self.readq_td(q).map(|d| String::from_utf8_lossy(&d).to_string())
                }
            }
            "DELETEQ" => {
                let q = opt("QUEUE").unwrap_or("");
                self.deleteq_ts(q);
                None
            }
            "STARTBR" => {
                let file = opt("FILE").or_else(|| opt("DATASET")).unwrap_or("");
                let ridfld = opt("RIDFLD").unwrap_or("");
                let token = self.startbr(file, ridfld);
                Some(token.to_string())
            }
            "READNEXT" => {
                let token: u32 = opt("TOKEN").and_then(|s| s.parse().ok()).unwrap_or(0);
                self.readnext(token).map(|(k, d)| format!("{}\t{}", k, d))
            }
            "READPREV" => {
                let token: u32 = opt("TOKEN").and_then(|s| s.parse().ok()).unwrap_or(0);
                self.readprev(token).map(|(k, d)| format!("{}\t{}", k, d))
            }
            "ENDBR" => {
                let token: u32 = opt("TOKEN").and_then(|s| s.parse().ok()).unwrap_or(0);
                self.endbr(token);
                None
            }
            "SYNCPOINT" => {
                if has("ROLLBACK") { self.rollback(); } else { self.syncpoint(); }
                None
            }
            "HANDLE" => {
                if has("ABEND") {
                    let code = opt("ABCODE").unwrap_or("*");
                    let label = opt("LABEL").unwrap_or("");
                    self.handle_abend(code, label);
                } else {
                    // HANDLE CONDITION
                    let condition = opt("CONDITION").unwrap_or("");
                    let action = opt("ACTION").unwrap_or("DEFAULT");
                    self.handle_condition(condition, action);
                }
                None
            }
            "ASKTIME" => {
                self.asktime();
                Some(self.abstime.to_string())
            }
            "FORMATTIME" => {
                let abstime = opt("ABSTIME").and_then(|s| s.parse().ok()).unwrap_or(self.abstime);
                let format = opt("YYYYMMDD").map(|_| "YYYYMMDD")
                    .or_else(|| opt("DDMMYYYY").map(|_| "DDMMYYYY"))
                    .or_else(|| opt("MMDDYYYY").map(|_| "MMDDYYYY"))
                    .or_else(|| opt("TIME").map(|_| "TIME"))
                    .unwrap_or("YYYYMMDD");
                let datesep = opt("DATESEP").and_then(|s| s.chars().next());
                Some(self.formattime(abstime, format, datesep))
            }
            "ASSIGN" => {
                // Return first recognized option value
                for &opt_name in &["APPLID", "SYSID", "USERID", "OPID", "NETNAME"] {
                    if has(opt_name) { return Some(self.assign(opt_name)); }
                }
                None
            }
            "INQUIRE" => {
                if let Some(prog) = opt("PROGRAM") {
                    Some(self.inquire("PROGRAM", prog))
                } else if let Some(file) = opt("FILE") {
                    Some(self.inquire("FILE", file))
                } else {
                    None
                }
            }
            _ => {
                eprintln!("CICS: unrecognized command '{}', continuing", cmd);
                self.set_resp(CicsResp::InvalidReq);
                None
            }
        }
    }
}

// ── Date helpers ────────────────────────────────────────────────────

fn days_to_ymd(day_count: i64) -> (i32, u32, u32) {
    // Civil calendar from day count (era-based algorithm)
    let era = if day_count >= 0 { day_count } else { day_count - 146096 } / 146097;
    let doe = (day_count - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_ctx() -> CicsContext {
        let output = Box::new(Vec::<u8>::new());
        let input = Box::new(BufReader::new(Cursor::new(Vec::<u8>::new())));
        CicsContext::with_io(output, input)
    }

    fn vsam_ctx() -> CicsContext {
        let mut ctx = test_ctx();
        ctx.init_vsam_memory();
        ctx
    }

    // -- Original 11 tests (preserved exactly) --

    #[test]
    fn test_ts_queue_write_read() {
        let mut ctx = test_ctx();
        ctx.writeq_ts("MYQUEUE", b"record1");
        ctx.writeq_ts("MYQUEUE", b"record2");
        assert_eq!(ctx.resp, 0);
        let r1 = ctx.readq_ts("MYQUEUE").unwrap();
        assert_eq!(r1, b"record1");
        let r2 = ctx.readq_ts("MYQUEUE").unwrap();
        assert_eq!(r2, b"record2");
        assert!(ctx.readq_ts("MYQUEUE").is_none());
        assert_eq!(ctx.resp, CicsResp::ItemErr as i32);
    }

    #[test]
    fn test_ts_queue_delete() {
        let mut ctx = test_ctx();
        ctx.writeq_ts("Q1", b"data");
        ctx.deleteq_ts("Q1");
        assert_eq!(ctx.resp, 0);
        ctx.deleteq_ts("Q1");
        assert_eq!(ctx.resp, CicsResp::QIdErr as i32);
    }

    #[test]
    fn test_file_read_write() {
        let mut ctx = test_ctx();
        let tmp = std::env::temp_dir().join("ironclad_cics_test.dat");
        let path = tmp.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        ctx.write_file(path, "KEY001", "John Doe,100");
        assert_eq!(ctx.resp, 0);
        ctx.write_file(path, "KEY002", "Jane Doe,200");
        let r = ctx.read_file(path, "KEY001");
        assert_eq!(r, Some("John Doe,100".to_string()));
        let r = ctx.read_file(path, "KEY999");
        assert!(r.is_none());
        assert_eq!(ctx.resp, CicsResp::NotFound as i32);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_file_rewrite() {
        let mut ctx = test_ctx();
        let tmp = std::env::temp_dir().join("ironclad_cics_rewrite.dat");
        let path = tmp.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        ctx.write_file(path, "K1", "old_value");
        ctx.rewrite_file(path, "K1", "new_value");
        assert_eq!(ctx.resp, 0);
        let r = ctx.read_file(path, "K1");
        assert_eq!(r, Some("new_value".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_file_delete() {
        let mut ctx = test_ctx();
        let tmp = std::env::temp_dir().join("ironclad_cics_delete.dat");
        let path = tmp.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        ctx.write_file(path, "K1", "data1");
        ctx.write_file(path, "K2", "data2");
        ctx.delete_file(path, "K1");
        assert_eq!(ctx.resp, 0);
        assert!(ctx.read_file(path, "K1").is_none());
        assert_eq!(ctx.read_file(path, "K2"), Some("data2".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_link_program() {
        let mut ctx = test_ctx();
        fn echo(_ctx: &mut CicsContext, data: &[u8]) -> Vec<u8> {
            let mut result = b"ECHO:".to_vec();
            result.extend_from_slice(data);
            result
        }
        ctx.register_program("MYPROG", echo);
        let result = ctx.link("MYPROG", b"hello");
        assert_eq!(result, b"ECHO:hello");
        assert_eq!(ctx.resp, 0);
        let result = ctx.link("NOPROG", b"");
        assert!(result.is_empty());
        assert_eq!(ctx.resp, CicsResp::PgmIdErr as i32);
    }

    #[test]
    fn test_syncpoint_rollback() {
        let mut ctx = test_ctx();
        let tmp = std::env::temp_dir().join("ironclad_cics_sync.dat");
        let path = tmp.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        ctx.write_file(path, "K1", "original");
        ctx.syncpoint();
        assert!(ctx.journal.is_empty());
        ctx.rewrite_file(path, "K1", "modified");
        assert_eq!(ctx.journal.len(), 1);
        ctx.rollback();
        let r = ctx.read_file(path, "K1");
        assert_eq!(r, Some("original".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_execute_dispatch() {
        let mut ctx = test_ctx();
        let tmp = std::env::temp_dir().join("ironclad_cics_exec.dat");
        let path = tmp.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        ctx.execute("WRITE", &[("FILE", Some(path)), ("RIDFLD", Some("K1")), ("FROM", Some("data"))]);
        assert_eq!(ctx.resp, 0);
        let r = ctx.execute("READ", &[("FILE", Some(path)), ("RIDFLD", Some("K1"))]);
        assert_eq!(r, Some("data".to_string()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_handle_condition() {
        let mut ctx = test_ctx();
        ctx.handle_condition("NOTFND", "ERROR-HANDLER");
        assert_eq!(ctx.check_handler("NOTFND"), Some("ERROR-HANDLER".to_string()));
        ctx.handle_condition("DUPKEY", "IGNORE");
        assert_eq!(ctx.check_handler("DUPKEY"), None);
    }

    #[test]
    fn test_receive_input() {
        let input_data = b"NAME=JOHN,AGE=30\n";
        let output = Box::new(Vec::<u8>::new());
        let input = Box::new(BufReader::new(Cursor::new(input_data.to_vec())));
        let mut ctx = CicsContext::with_io(output, input);
        let mut fields = HashMap::new();
        ctx.receive(&mut fields);
        assert_eq!(ctx.resp, 0);
        assert_eq!(fields.get("NAME"), Some(&"JOHN".to_string()));
        assert_eq!(fields.get("AGE"), Some(&"30".to_string()));
    }

    #[test]
    fn test_resp_codes() {
        let mut ctx = test_ctx();
        ctx.set_resp(CicsResp::Normal);
        assert_eq!(ctx.resp, 0);
        ctx.set_resp(CicsResp::NotFound);
        assert_eq!(ctx.resp, 13);
    }

    // -- Phase 1: VSAM file I/O tests --

    #[test]
    fn test_vsam_file_crud() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();
        ctx.write_file("ACCTDAT", "001", "John,100");
        assert_eq!(ctx.resp, 0);
        assert_eq!(ctx.read_file("ACCTDAT", "001"), Some("John,100".to_string()));
        ctx.rewrite_file("ACCTDAT", "001", "John,200");
        assert_eq!(ctx.read_file("ACCTDAT", "001"), Some("John,200".to_string()));
        ctx.delete_file("ACCTDAT", "001");
        assert!(ctx.read_file("ACCTDAT", "001").is_none());
    }

    #[test]
    fn test_vsam_duplicate_key() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        ctx.write_file("F1", "K1", "data");
        ctx.write_file("F1", "K1", "dup");
        assert_eq!(ctx.resp, CicsResp::DuplicateKey as i32);
    }

    #[test]
    fn test_vsam_browse() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        ctx.write_file("F1", "A", "1");
        ctx.write_file("F1", "B", "2");
        ctx.write_file("F1", "C", "3");
        let tok = ctx.startbr("F1", "A");
        assert_eq!(ctx.readnext(tok), Some(("A".into(), "1".into())));
        assert_eq!(ctx.readnext(tok), Some(("B".into(), "2".into())));
        assert_eq!(ctx.readprev(tok), Some(("A".into(), "1".into())));
        ctx.endbr(tok);
    }

    // -- Phase 2: Program control tests --

    #[test]
    fn test_xctl_sets_action() {
        let mut ctx = test_ctx();
        fn target(_: &mut CicsContext, _: &[u8]) -> Vec<u8> { b"OK".to_vec() }
        ctx.register_program("TARGET", target);
        ctx.xctl("TARGET", b"data");
        assert!(matches!(ctx.last_action, ProgramAction::Xctl { .. }));
    }

    #[test]
    fn test_return_transid_commarea() {
        let mut ctx = test_ctx();
        ctx.commarea = b"session_data".to_vec();
        ctx.return_program(Some("MENU"));
        assert_eq!(ctx.return_transid, Some("MENU".to_string()));
        assert_eq!(ctx.return_commarea, Some(b"session_data".to_vec()));
        assert!(matches!(ctx.last_action, ProgramAction::Return { .. }));
    }

    #[test]
    fn test_start_and_retrieve() {
        let mut ctx = test_ctx();
        ctx.start("BATCHPGM", b"param_data", 0);
        assert_eq!(ctx.start_queue.len(), 1);
        assert_eq!(ctx.start_queue[0].program, "BATCHPGM");
        // Simulate dispatch setting retrieve data
        ctx.set_retrieve_data(b"param_data".to_vec());
        let data = ctx.retrieve().unwrap();
        assert_eq!(data, b"param_data");
        assert!(ctx.retrieve().is_none()); // consumed
    }

    #[test]
    fn test_handle_abend() {
        let mut ctx = test_ctx();
        ctx.handle_abend("ASRA", "ASRA_HANDLER");
        ctx.abend("ASRA");
        assert_eq!(ctx.resp, 0); // handled, not error
    }

    #[test]
    fn test_abend_unhandled() {
        let mut ctx = test_ctx();
        ctx.abend("ZZZZ");
        assert_eq!(ctx.resp, CicsResp::Error as i32);
    }

    #[test]
    fn test_link_nested() {
        let mut ctx = test_ctx();
        fn inner(_: &mut CicsContext, d: &[u8]) -> Vec<u8> {
            let mut r = b"INNER:".to_vec();
            r.extend_from_slice(d);
            r
        }
        fn outer(ctx: &mut CicsContext, _: &[u8]) -> Vec<u8> {
            ctx.link("INNER", b"nested")
        }
        ctx.register_program("INNER", inner);
        ctx.register_program("OUTER", outer);
        let result = ctx.link("OUTER", b"");
        assert_eq!(result, b"INNER:nested");
    }

    // -- Phase 3: TSQ enhancement tests --

    #[test]
    fn test_tsq_item_access() {
        let mut ctx = test_ctx();
        ctx.writeq_ts("Q", b"aaa");
        ctx.writeq_ts("Q", b"bbb");
        ctx.writeq_ts("Q", b"ccc");
        assert_eq!(ctx.readq_ts_item("Q", Some(2)).unwrap(), b"bbb");
        assert_eq!(ctx.readq_ts_item("Q", Some(1)).unwrap(), b"aaa");
        assert!(ctx.readq_ts_item("Q", Some(99)).is_none());
        assert_eq!(ctx.resp, CicsResp::ItemErr as i32);
    }

    #[test]
    fn test_tsq_rewrite_item() {
        let mut ctx = test_ctx();
        ctx.writeq_ts("Q", b"old");
        ctx.writeq_ts_item("Q", b"new", Some(1));
        assert_eq!(ctx.readq_ts_item("Q", Some(1)).unwrap(), b"new");
    }

    #[test]
    fn test_tsq_numitems() {
        let mut ctx = test_ctx();
        assert_eq!(ctx.tsq_numitems("Q"), 0);
        ctx.writeq_ts("Q", b"a");
        ctx.writeq_ts("Q", b"b");
        assert_eq!(ctx.tsq_numitems("Q"), 2);
    }

    // -- Phase 4: Transaction tests --

    #[test]
    fn test_vsam_transaction_commit() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        ctx.begin_transaction();
        ctx.write_file("F1", "K1", "data");
        ctx.syncpoint();
        assert_eq!(ctx.read_file("F1", "K1"), Some("data".to_string()));
    }

    #[test]
    fn test_vsam_transaction_rollback() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        ctx.write_file("F1", "K1", "keep");
        ctx.begin_transaction();
        ctx.rewrite_file("F1", "K1", "discard");
        ctx.rollback();
        assert_eq!(ctx.read_file("F1", "K1"), Some("keep".to_string()));
    }

    // -- Phase 6: System services tests --

    #[test]
    fn test_asktime() {
        let mut ctx = test_ctx();
        ctx.asktime();
        assert!(ctx.abstime > 0);
        assert_eq!(ctx.resp, 0);
    }

    #[test]
    fn test_formattime_yyyymmdd() {
        let ctx = test_ctx();
        // 2026-01-15 12:00:00 UTC
        // Unix: 1768478400, ABSTIME: (1768478400 + 2208988800) * 1000000
        let abstime: u64 = (1768478400u64 + 2208988800) * 1_000_000;
        let result = ctx.formattime(abstime, "YYYYMMDD", None);
        assert_eq!(result, "20260115");
    }

    #[test]
    fn test_formattime_with_sep() {
        let ctx = test_ctx();
        let abstime: u64 = (1768478400u64 + 2208988800) * 1_000_000;
        let result = ctx.formattime(abstime, "YYYYMMDD", Some('/'));
        assert_eq!(result, "2026/01/15");
    }

    #[test]
    fn test_formattime_time() {
        let ctx = test_ctx();
        let abstime: u64 = (1768478400u64 + 2208988800) * 1_000_000; // 12:00:00 UTC
        let result = ctx.formattime(abstime, "TIME", None);
        assert_eq!(result, "12:00:00");
    }

    #[test]
    fn test_assign_values() {
        let ctx = test_ctx();
        assert_eq!(ctx.assign("APPLID"), "CARDDEMO");
        assert_eq!(ctx.assign("SYSID"), "CICS");
        assert_eq!(ctx.assign("USERID"), "CICSUSER");
    }

    #[test]
    fn test_inquire_program() {
        let mut ctx = test_ctx();
        fn dummy(_: &mut CicsContext, _: &[u8]) -> Vec<u8> { Vec::new() }
        ctx.register_program("PGM1", dummy);
        assert_eq!(ctx.inquire("PROGRAM", "PGM1"), "INSTALLED");
        assert_eq!(ctx.inquire("PROGRAM", "NOPE"), "NOTINSTALLED");
    }

    #[test]
    fn test_inquire_file() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        assert_eq!(ctx.inquire("FILE", "F1"), "ENABLED");
        assert_eq!(ctx.inquire("FILE", "NOPE"), "DISABLED");
    }

    // -- Phase 7: BMS tests --

    #[test]
    fn test_send_map_with_bms() {
        use crate::bms::{BmsMapset, BmsMap, BmsField};
        let mut ctx = test_ctx();
        let mut mapset = BmsMapset::new("CSIGNON");
        let mut map = BmsMap::new("SIGNON", 24, 80);
        map.add_field(BmsField::new("MSG", 20, 1, 40));
        mapset.add_map(map);
        ctx.bms_registry_mut().register_mapset(mapset);
        let mut data = HashMap::new();
        data.insert("MSG".to_string(), "WELCOME".to_string());
        ctx.send_map("SIGNON", "CSIGNON", &data, false);
        assert!(ctx.current_screen.is_some());
        assert_eq!(ctx.current_screen.as_ref().unwrap().fields[0].value, "WELCOME");
    }

    #[test]
    fn test_send_map_fallback() {
        let mut ctx = test_ctx();
        let mut data = HashMap::new();
        data.insert("F1".to_string(), "val".to_string());
        ctx.send_map("NOMAP", "NOMS", &data, false);
        assert_eq!(ctx.resp, 0); // still succeeds with text fallback
    }

    #[test]
    fn test_execute_asktime() {
        let mut ctx = test_ctx();
        let result = ctx.execute("ASKTIME", &[]);
        assert!(result.is_some());
        let abstime: u64 = result.unwrap().parse().unwrap();
        assert!(abstime > 0);
    }

    #[test]
    fn test_execute_assign() {
        let mut ctx = test_ctx();
        let result = ctx.execute("ASSIGN", &[("APPLID", None)]);
        assert_eq!(result, Some("CARDDEMO".to_string()));
    }

    #[test]
    fn test_execute_readprev() {
        let mut ctx = vsam_ctx();
        ctx.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
        ctx.write_file("F1", "A", "1");
        ctx.write_file("F1", "B", "2");
        let tok = ctx.startbr("F1", "B");
        ctx.readnext(tok); // reads B
        let r = ctx.execute("READPREV", &[("TOKEN", Some(&tok.to_string()))]);
        assert!(r.is_some());
        assert!(r.unwrap().starts_with("A\t"));
    }
}
