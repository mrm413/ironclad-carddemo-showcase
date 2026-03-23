// Pseudo-Conversational Transaction Loop.
// Manages CICS transaction dispatch: RETURN TRANSID → next screen → program lookup.
// Each "conversation" is a sequence of transactions sharing a COMMAREA.

use std::collections::HashMap;
use crate::cics::CicsContext;
use crate::bms::{ScreenOutput, ScreenInput};

// ── Session State ───────────────────────────────────────────────────

pub struct SessionState {
    pub session_id: String,
    pub current_transid: Option<String>,
    pub commarea: Vec<u8>,
    pub userid: String,
    pub last_screen: Option<ScreenOutput>,
}

impl SessionState {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            current_transid: None,
            commarea: Vec::new(),
            userid: "GUEST".to_string(),
            last_screen: None,
        }
    }
}

// ── Transaction Registry ────────────────────────────────────────────

/// Maps transaction IDs to program names.
pub struct TransactionRegistry {
    transid_to_program: HashMap<String, String>,
}

impl TransactionRegistry {
    pub fn new() -> Self {
        Self { transid_to_program: HashMap::new() }
    }

    pub fn register(&mut self, transid: &str, program: &str) {
        self.transid_to_program.insert(
            transid.to_uppercase(), program.to_uppercase());
    }

    pub fn lookup(&self, transid: &str) -> Option<&str> {
        self.transid_to_program.get(&transid.to_uppercase()).map(|s| s.as_str())
    }
}

impl Default for TransactionRegistry {
    fn default() -> Self { Self::new() }
}

// ── Transaction Loop ────────────────────────────────────────────────

pub struct TransactionLoop {
    pub registry: TransactionRegistry,
    sessions: HashMap<String, SessionState>,
}

impl TransactionLoop {
    pub fn new() -> Self {
        Self {
            registry: TransactionRegistry::new(),
            sessions: HashMap::new(),
        }
    }

    /// Create or get a session.
    pub fn get_or_create_session(&mut self, session_id: &str) -> &mut SessionState {
        self.sessions.entry(session_id.to_string())
            .or_insert_with(|| SessionState::new(session_id))
    }

    /// Set initial transaction for a session.
    pub fn set_initial_transid(&mut self, session_id: &str, transid: &str) {
        let session = self.get_or_create_session(session_id);
        session.current_transid = Some(transid.to_uppercase());
    }

    /// Dispatch one pseudo-conversational turn.
    /// Takes screen input, runs the transaction's program, returns screen output.
    pub fn dispatch(
        &mut self,
        session_id: &str,
        input: &ScreenInput,
        ctx: &mut CicsContext,
    ) -> Result<Option<ScreenOutput>, String> {
        let session = self.sessions.get_mut(session_id)
            .ok_or_else(|| "Session not found".to_string())?;

        let transid = session.current_transid.clone()
            .ok_or_else(|| "No active transaction".to_string())?;

        let program = self.registry.lookup(&transid)
            .ok_or_else(|| format!("No program for TRANSID {}", transid))?
            .to_string();

        // Set up context for this transaction
        ctx.tran_id = transid.clone();
        ctx.commarea = session.commarea.clone();
        ctx.calen = session.commarea.len() as i32;
        ctx.eibaid = input.aid;
        ctx.userid = session.userid.clone();
        ctx.last_input = Some(input.clone());

        // Execute program via LINK
        let ca = session.commarea.clone();
        let result = ctx.link(&program, &ca);
        if ctx.resp == crate::cics::CicsResp::PgmIdErr as i32 {
            return Err(format!("Program {} not found", program));
        }

        // Process RETURN TRANSID
        let session = self.sessions.get_mut(session_id).unwrap();
        if let Some(ref transid) = ctx.return_transid {
            session.current_transid = Some(transid.clone());
        }
        if let Some(ref ca) = ctx.return_commarea {
            session.commarea = ca.clone();
        }

        // Capture screen output
        let screen = ctx.current_screen.clone();
        session.last_screen = screen.clone();

        Ok(screen)
    }

    pub fn get_session(&self, id: &str) -> Option<&SessionState> {
        self.sessions.get(id)
    }

    pub fn remove_session(&mut self, id: &str) {
        self.sessions.remove(id);
    }
}

impl Default for TransactionLoop {
    fn default() -> Self { Self::new() }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bms::dfhaid;
    use std::io::{BufReader, Cursor};

    fn test_ctx() -> CicsContext {
        CicsContext::with_io(
            Box::new(Vec::<u8>::new()),
            Box::new(BufReader::new(Cursor::new(Vec::<u8>::new()))),
        )
    }

    #[test]
    fn test_registry() {
        let mut reg = TransactionRegistry::new();
        reg.register("MENU", "COMEN01");
        assert_eq!(reg.lookup("MENU"), Some("COMEN01"));
        assert!(reg.lookup("NOPE").is_none());
    }

    #[test]
    fn test_session_lifecycle() {
        let mut tl = TransactionLoop::new();
        let session = tl.get_or_create_session("S1");
        assert_eq!(session.session_id, "S1");
        assert!(session.current_transid.is_none());
        tl.set_initial_transid("S1", "SIGN");
        assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("SIGN".to_string()));
        tl.remove_session("S1");
        assert!(tl.get_session("S1").is_none());
    }

    #[test]
    fn test_dispatch_basic() {
        let mut tl = TransactionLoop::new();
        tl.registry.register("TEST", "TESTPGM");
        tl.set_initial_transid("S1", "TEST");

        fn test_pgm(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
            ctx.return_program(Some("MENU"));
            Vec::new()
        }

        let mut ctx = test_ctx();
        ctx.register_program("TESTPGM", test_pgm);

        let input = ScreenInput {
            aid: dfhaid::ENTER,
            cursor_row: 0, cursor_col: 0,
            fields: HashMap::new(),
        };

        let result = tl.dispatch("S1", &input, &mut ctx);
        assert!(result.is_ok());
        // After dispatch, session should have MENU as next transid
        assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));
    }

    #[test]
    fn test_dispatch_commarea_passing() {
        let mut tl = TransactionLoop::new();
        tl.registry.register("T1", "PGM1");
        tl.set_initial_transid("S1", "T1");

        fn pgm1(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
            ctx.commarea = b"session_state".to_vec();
            ctx.return_program(Some("T1"));
            Vec::new()
        }

        let mut ctx = test_ctx();
        ctx.register_program("PGM1", pgm1);

        let input = ScreenInput::default();
        tl.dispatch("S1", &input, &mut ctx).unwrap();

        // COMMAREA should be preserved for next turn
        assert_eq!(tl.get_session("S1").unwrap().commarea, b"session_state");
    }

    #[test]
    fn test_dispatch_no_session() {
        let mut tl = TransactionLoop::new();
        let mut ctx = test_ctx();
        let result = tl.dispatch("NOPE", &ScreenInput::default(), &mut ctx);
        assert!(result.is_err());
    }
}
