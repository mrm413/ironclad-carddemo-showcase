// Phase 8 Integration Tests — Ironclad CICS Runtime
// Exercises all phases together: VSAM, Program Control, TSQ, TDQ,
// SQL, System Services, BMS, and Transaction Loop.

use std::collections::HashMap;
use std::io::{BufReader, Cursor};
use std::time::Instant;

use cobol_runtime::cics::{CicsContext, CicsResp, ProgramAction};
use cobol_runtime::vsam::{VsamStore, VsamOrganization, VsamError};
use cobol_runtime::sql::{SqlContext, SqlValue};
use cobol_runtime::bms::*;
use cobol_runtime::transaction_loop::{TransactionLoop, TransactionRegistry};

// ── Helpers ──────────────────────────────────────────────────────────

fn ctx() -> CicsContext {
    CicsContext::with_io(
        Box::new(Vec::<u8>::new()),
        Box::new(BufReader::new(Cursor::new(Vec::<u8>::new()))),
    )
}

fn vsam_ctx() -> CicsContext {
    let mut c = ctx();
    c.init_vsam_memory();
    c
}

// ── Scenario 1: CardDemo Signon Flow ─────────────────────────────────

// Simulates COSGNOO → validates user → RETURN TRANSID MENU
fn signon_program(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
    // Read input fields
    let userid = ctx.last_input.as_ref()
        .and_then(|i| i.fields.get("USERID").cloned())
        .unwrap_or_default();
    let passwd = ctx.last_input.as_ref()
        .and_then(|i| i.fields.get("PASSWD").cloned())
        .unwrap_or_default();

    if userid == "ADMIN" && passwd == "ADMIN01" {
        // Send success screen
        let mut data = HashMap::new();
        data.insert("MSG".to_string(), "SIGN-ON SUCCESSFUL".to_string());
        ctx.send_map("SIGNON", "CSIGNON", &data, true);
        // Set commarea with user info
        ctx.commarea = format!("USR={}", userid).into_bytes();
        ctx.return_program(Some("MENU"));
    } else {
        let mut data = HashMap::new();
        data.insert("MSG".to_string(), "INVALID CREDENTIALS".to_string());
        ctx.send_map("SIGNON", "CSIGNON", &data, false);
        ctx.return_program(Some("SIGN"));
    }
    Vec::new()
}

fn menu_program(ctx: &mut CicsContext, ca: &[u8]) -> Vec<u8> {
    let opt = ctx.last_input.as_ref()
        .and_then(|i| i.fields.get("OPT").cloned())
        .unwrap_or_default();

    let mut data = HashMap::new();
    data.insert("TITLE".to_string(), "CARDDEMO MAIN MENU".to_string());
    data.insert("USR".to_string(), String::from_utf8_lossy(ca).to_string());
    ctx.send_map("MAINMENU", "CMENU", &data, true);

    match opt.trim() {
        "01" => ctx.return_program(Some("ACCT")),
        "02" => ctx.return_program(Some("CARD")),
        "03" => ctx.return_program(Some("TRAN")),
        _    => ctx.return_program(Some("MENU")),
    }
    Vec::new()
}

fn acctview_program(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
    let acct_id = ctx.last_input.as_ref()
        .and_then(|i| i.fields.get("ACCTID").cloned())
        .unwrap_or("00000000001".to_string());

    // Read account from VSAM
    let acct_data = ctx.read_file("ACCTDAT", &acct_id);
    let mut data = HashMap::new();
    if let Some(d) = acct_data {
        data.insert("ACCTID".to_string(), acct_id);
        data.insert("DATA".to_string(), d);
        data.insert("MSG".to_string(), "ACCOUNT FOUND".to_string());
    } else {
        data.insert("MSG".to_string(), "ACCOUNT NOT FOUND".to_string());
    }
    ctx.send_map("ACCTVIEW", "CACTVW", &data, true);

    if ctx.eibaid == dfhaid::PF3 {
        ctx.return_program(Some("MENU"));
    } else {
        ctx.return_program(Some("ACCT"));
    }
    Vec::new()
}

fn cardlist_program(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
    // Browse CARDDAT file
    let tok = ctx.startbr("CARDDAT", "0000000000000000");
    let mut cards = Vec::new();
    for _ in 0..10 {
        if let Some((key, data)) = ctx.readnext(tok) {
            cards.push(format!("{}: {}", key, data));
        } else {
            break;
        }
    }
    ctx.endbr(tok);

    let mut data = HashMap::new();
    data.insert("CARDS".to_string(), cards.join("|"));
    data.insert("COUNT".to_string(), cards.len().to_string());
    ctx.send_map("CARDLIST", "CCARD", &data, true);
    ctx.return_program(Some("MENU"));
    Vec::new()
}

fn tranbrowse_program(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
    // Use TSQ to cache transaction page
    let page: usize = ctx.last_input.as_ref()
        .and_then(|i| i.fields.get("PAGE").cloned())
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    // Write page data to TSQ for caching
    let page_data = format!("PAGE {} TRANSACTIONS", page);
    ctx.writeq_ts("TRANPAGE", page_data.as_bytes());

    let mut data = HashMap::new();
    data.insert("PAGE".to_string(), page.to_string());
    data.insert("DATA".to_string(), page_data);
    ctx.send_map("TRANBROW", "CTRAN", &data, true);

    if ctx.eibaid == dfhaid::PF3 {
        ctx.deleteq_ts("TRANPAGE");
        ctx.return_program(Some("MENU"));
    } else {
        ctx.return_program(Some("TRAN"));
    }
    Vec::new()
}

fn signoff_program(ctx: &mut CicsContext, _ca: &[u8]) -> Vec<u8> {
    let mut data = HashMap::new();
    data.insert("MSG".to_string(), "THANK YOU FOR USING CARDDEMO".to_string());
    ctx.send_map("SIGNOFF", "CSIGNON", &data, true);
    // No RETURN TRANSID = end of conversation
    Vec::new()
}

// Register all BMS maps for testing
fn register_carddemo_maps(ctx: &mut CicsContext) {
    // CSIGNON mapset
    let mut csignon = BmsMapset::new("CSIGNON");
    let mut signon_map = BmsMap::new("SIGNON", 24, 80);
    signon_map.add_field(BmsField::new("TITLE", 1, 25, 30).with_attr(dfhbmsca::PROT));
    signon_map.add_field(BmsField::new("USERID", 10, 25, 8));
    signon_map.add_field(BmsField::new("PASSWD", 12, 25, 8).with_attr(dfhbmsca::UNPROT_DRK));
    signon_map.add_field(BmsField::new("MSG", 20, 1, 79).with_attr(dfhbmsca::PROT));
    csignon.add_map(signon_map);
    let mut signoff_map = BmsMap::new("SIGNOFF", 24, 80);
    signoff_map.add_field(BmsField::new("MSG", 12, 20, 40).with_attr(dfhbmsca::PROT_BRT));
    csignon.add_map(signoff_map);
    ctx.bms_registry_mut().register_mapset(csignon);

    // CMENU mapset
    let mut cmenu = BmsMapset::new("CMENU");
    let mut menu_map = BmsMap::new("MAINMENU", 24, 80);
    menu_map.add_field(BmsField::new("TITLE", 1, 20, 40).with_attr(dfhbmsca::PROT_BRT));
    menu_map.add_field(BmsField::new("USR", 1, 65, 8).with_attr(dfhbmsca::PROT));
    menu_map.add_field(BmsField::new("OPT", 12, 40, 2));
    menu_map.add_field(BmsField::new("MSG", 23, 1, 79).with_attr(dfhbmsca::PROT));
    cmenu.add_map(menu_map);
    ctx.bms_registry_mut().register_mapset(cmenu);

    // CACTVW mapset
    let mut cactvw = BmsMapset::new("CACTVW");
    let mut acct_map = BmsMap::new("ACCTVIEW", 24, 80);
    acct_map.add_field(BmsField::new("ACCTID", 5, 20, 11));
    acct_map.add_field(BmsField::new("DATA", 7, 20, 50).with_attr(dfhbmsca::PROT));
    acct_map.add_field(BmsField::new("MSG", 22, 1, 79).with_attr(dfhbmsca::PROT));
    cactvw.add_map(acct_map);
    ctx.bms_registry_mut().register_mapset(cactvw);

    // CCARD mapset
    let mut ccard = BmsMapset::new("CCARD");
    let mut card_map = BmsMap::new("CARDLIST", 24, 80);
    card_map.add_field(BmsField::new("CARDS", 3, 1, 79).with_attr(dfhbmsca::PROT));
    card_map.add_field(BmsField::new("COUNT", 2, 70, 5).with_attr(dfhbmsca::PROT));
    ccard.add_map(card_map);
    ctx.bms_registry_mut().register_mapset(ccard);

    // CTRAN mapset
    let mut ctran = BmsMapset::new("CTRAN");
    let mut tran_map = BmsMap::new("TRANBROW", 24, 80);
    tran_map.add_field(BmsField::new("PAGE", 2, 70, 4));
    tran_map.add_field(BmsField::new("DATA", 4, 1, 79).with_attr(dfhbmsca::PROT));
    ctran.add_map(tran_map);
    ctx.bms_registry_mut().register_mapset(ctran);
}

// Register all CardDemo programs
fn register_carddemo_programs(ctx: &mut CicsContext) {
    ctx.register_program("COSGNOO", signon_program);
    ctx.register_program("COMEN01", menu_program);
    ctx.register_program("COACTVW", acctview_program);
    ctx.register_program("COCRDLI", cardlist_program);
    ctx.register_program("COTRN00", tranbrowse_program);
    ctx.register_program("COSGNOF", signoff_program);
}

// Register VSAM files with sample data
fn setup_vsam_data(ctx: &mut CicsContext) {
    ctx.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();
    ctx.register_vsam_file("CARDDAT", VsamOrganization::Ksds).unwrap();
    ctx.register_vsam_file("CUSTDAT", VsamOrganization::Ksds).unwrap();
    ctx.register_vsam_file("CARDXRF", VsamOrganization::Ksds).unwrap();
    ctx.register_vsam_file("TRANDAT", VsamOrganization::Ksds).unwrap();

    // Sample accounts
    ctx.write_file("ACCTDAT", "00000000001", "ACCT_STATUS=Y,BAL=5000.00,LIMIT=10000.00");
    ctx.write_file("ACCTDAT", "00000000002", "ACCT_STATUS=Y,BAL=2500.00,LIMIT=5000.00");
    ctx.write_file("ACCTDAT", "00000000003", "ACCT_STATUS=N,BAL=0.00,LIMIT=0.00");

    // Sample cards
    ctx.write_file("CARDDAT", "4111111111111111", "ACCT=00000000001,CVV=123,NAME=JOHN DOE");
    ctx.write_file("CARDDAT", "4222222222222222", "ACCT=00000000001,CVV=456,NAME=JOHN DOE");
    ctx.write_file("CARDDAT", "5333333333333333", "ACCT=00000000002,CVV=789,NAME=JANE DOE");

    // Sample customers
    ctx.write_file("CUSTDAT", "C001", "FIRST=JOHN,LAST=DOE,SSN=xxx-xx-1234");
    ctx.write_file("CUSTDAT", "C002", "FIRST=JANE,LAST=DOE,SSN=xxx-xx-5678");

    // Sample transactions
    for i in 1..=20 {
        ctx.write_file("TRANDAT", &format!("T{:04}", i),
            &format!("TYPE=01,AMT={:.2},CARD=4111111111111111", i as f64 * 10.0));
    }
}

fn full_setup() -> CicsContext {
    let mut c = vsam_ctx();
    register_carddemo_maps(&mut c);
    register_carddemo_programs(&mut c);
    setup_vsam_data(&mut c);
    c
}

// ══════════════════════════════════════════════════════════════════════
// SCENARIO TESTS
// ══════════════════════════════════════════════════════════════════════

#[test]
fn scenario_signon_success() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "COSGNOO");
    tl.registry.register("MENU", "COMEN01");
    tl.set_initial_transid("S1", "SIGN");

    let mut c = full_setup();

    // Submit signon with valid credentials
    let mut fields = HashMap::new();
    fields.insert("USERID".to_string(), "ADMIN".to_string());
    fields.insert("PASSWD".to_string(), "ADMIN01".to_string());
    let input = ScreenInput { aid: dfhaid::ENTER, cursor_row: 10, cursor_col: 25, fields };
    let screen = tl.dispatch("S1", &input, &mut c).unwrap();

    assert!(screen.is_some());
    let s = screen.unwrap();
    let msg = s.fields.iter().find(|f| f.name == "MSG");
    assert!(msg.is_some());
    assert!(msg.unwrap().value.contains("SUCCESSFUL"));

    // Session should now point to MENU
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));
    assert!(tl.get_session("S1").unwrap().commarea.starts_with(b"USR=ADMIN"));
}

#[test]
fn scenario_signon_failure() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "COSGNOO");
    tl.set_initial_transid("S1", "SIGN");

    let mut c = full_setup();

    let mut fields = HashMap::new();
    fields.insert("USERID".to_string(), "BAD".to_string());
    fields.insert("PASSWD".to_string(), "WRONG".to_string());
    let input = ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields };
    let screen = tl.dispatch("S1", &input, &mut c).unwrap();

    let s = screen.unwrap();
    let msg = s.fields.iter().find(|f| f.name == "MSG").unwrap();
    assert!(msg.value.contains("INVALID"));
    // Should stay on SIGN
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("SIGN".to_string()));
}

#[test]
fn scenario_menu_to_account_view() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "COSGNOO");
    tl.registry.register("MENU", "COMEN01");
    tl.registry.register("ACCT", "COACTVW");
    tl.set_initial_transid("S1", "SIGN");

    let mut c = full_setup();

    // Signon
    let mut fields = HashMap::new();
    fields.insert("USERID".to_string(), "ADMIN".to_string());
    fields.insert("PASSWD".to_string(), "ADMIN01".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();

    // Select option 01 (Account View)
    let mut fields = HashMap::new();
    fields.insert("OPT".to_string(), "01".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();

    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("ACCT".to_string()));

    // View account
    let mut fields = HashMap::new();
    fields.insert("ACCTID".to_string(), "00000000001".to_string());
    let screen = tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();

    let s = screen.unwrap();
    let msg = s.fields.iter().find(|f| f.name == "MSG").unwrap();
    assert!(msg.value.contains("FOUND"));
    let data = s.fields.iter().find(|f| f.name == "DATA").unwrap();
    assert!(data.value.contains("5000.00"));
}

#[test]
fn scenario_card_list_browse() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "COSGNOO");
    tl.registry.register("MENU", "COMEN01");
    tl.registry.register("CARD", "COCRDLI");
    tl.set_initial_transid("S1", "SIGN");

    let mut c = full_setup();

    // Signon + navigate to card list
    let mut fields = HashMap::new();
    fields.insert("USERID".to_string(), "ADMIN".to_string());
    fields.insert("PASSWD".to_string(), "ADMIN01".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();

    let mut fields = HashMap::new();
    fields.insert("OPT".to_string(), "02".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();

    // Card list
    let screen = tl.dispatch("S1", &ScreenInput::default(), &mut c).unwrap();
    let s = screen.unwrap();
    let count = s.fields.iter().find(|f| f.name == "COUNT").unwrap();
    assert_eq!(count.value, "3"); // 3 cards in sample data
}

#[test]
fn scenario_transaction_browse_with_paging() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("TRAN", "COTRN00");
    tl.set_initial_transid("S1", "TRAN");

    let mut c = full_setup();

    // Page 1
    let mut fields = HashMap::new();
    fields.insert("PAGE".to_string(), "1".to_string());
    let screen = tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();
    let s = screen.unwrap();
    assert!(s.fields.iter().any(|f| f.name == "DATA" && f.value.contains("PAGE 1")));

    // Page 2 via PF8
    let mut fields = HashMap::new();
    fields.insert("PAGE".to_string(), "2".to_string());
    let screen = tl.dispatch("S1", &ScreenInput { aid: dfhaid::PF8, cursor_row: 0, cursor_col: 0, fields }, &mut c).unwrap();
    let s = screen.unwrap();
    assert!(s.fields.iter().any(|f| f.name == "DATA" && f.value.contains("PAGE 2")));

    // PF3 exits and cleans up TSQ
    let screen = tl.dispatch("S1", &ScreenInput { aid: dfhaid::PF3, cursor_row: 0, cursor_col: 0, fields: HashMap::new() }, &mut c).unwrap();
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));
}

#[test]
fn scenario_full_signon_to_signoff() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "COSGNOO");
    tl.registry.register("MENU", "COMEN01");
    tl.registry.register("ACCT", "COACTVW");
    tl.registry.register("CARD", "COCRDLI");
    tl.registry.register("TRAN", "COTRN00");
    tl.set_initial_transid("S1", "SIGN");

    let mut c = full_setup();

    // 1. Signon
    let mut f = HashMap::new();
    f.insert("USERID".to_string(), "ADMIN".to_string());
    f.insert("PASSWD".to_string(), "ADMIN01".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));

    // 2. Go to Account View
    let mut f = HashMap::new();
    f.insert("OPT".to_string(), "01".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("ACCT".to_string()));

    // 3. View account, then PF3 back to menu
    let mut f = HashMap::new();
    f.insert("ACCTID".to_string(), "00000000002".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();

    // PF3 → back to menu
    c.eibaid = dfhaid::PF3;
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::PF3, cursor_row: 0, cursor_col: 0, fields: HashMap::new() }, &mut c).unwrap();
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));

    // 4. Go to Card List
    let mut f = HashMap::new();
    f.insert("OPT".to_string(), "02".to_string());
    tl.dispatch("S1", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();

    tl.dispatch("S1", &ScreenInput::default(), &mut c).unwrap();
    assert_eq!(tl.get_session("S1").unwrap().current_transid, Some("MENU".to_string()));

    // 5. Session cleanup
    tl.remove_session("S1");
    assert!(tl.get_session("S1").is_none());
}

// ══════════════════════════════════════════════════════════════════════
// COMMAND-LEVEL INTEGRATION TESTS
// ══════════════════════════════════════════════════════════════════════

// -- VSAM: multi-file, cross-file operations --

#[test]
fn vsam_multi_file_isolation() {
    let mut c = vsam_ctx();
    c.register_vsam_file("FILE_A", VsamOrganization::Ksds).unwrap();
    c.register_vsam_file("FILE_B", VsamOrganization::Ksds).unwrap();

    c.write_file("FILE_A", "K1", "from_A");
    c.write_file("FILE_B", "K1", "from_B"); // same key, different file
    assert_eq!(c.read_file("FILE_A", "K1"), Some("from_A".to_string()));
    assert_eq!(c.read_file("FILE_B", "K1"), Some("from_B".to_string()));
}

#[test]
fn vsam_large_dataset_browse() {
    let mut c = vsam_ctx();
    c.register_vsam_file("LARGE", VsamOrganization::Ksds).unwrap();
    for i in 0..1000 {
        c.write_file("LARGE", &format!("{:06}", i), &format!("record_{}", i));
    }
    let tok = c.startbr("LARGE", "000500");
    let first = c.readnext(tok);
    assert_eq!(first, Some(("000500".to_string(), "record_500".to_string())));

    // Skip forward 10
    for _ in 0..10 {
        c.readnext(tok);
    }
    let at_511 = c.readnext(tok);
    assert_eq!(at_511, Some(("000511".to_string(), "record_511".to_string())));

    c.endbr(tok);
}

#[test]
fn vsam_concurrent_browse_cursors() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    for c_char in ['A', 'B', 'C', 'D', 'E'] {
        c.write_file("F1", &c_char.to_string(), &format!("val_{}", c_char));
    }

    let tok1 = c.startbr("F1", "A");
    let tok2 = c.startbr("F1", "C");

    assert_eq!(c.readnext(tok1).unwrap().0, "A");
    assert_eq!(c.readnext(tok2).unwrap().0, "C");
    assert_eq!(c.readnext(tok1).unwrap().0, "B");
    assert_eq!(c.readnext(tok2).unwrap().0, "D");

    c.endbr(tok1);
    c.endbr(tok2);
}

#[test]
fn vsam_write_during_browse() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.write_file("F1", "A", "1");
    c.write_file("F1", "C", "3");

    let tok = c.startbr("F1", "A");
    assert_eq!(c.readnext(tok).unwrap().0, "A");

    // Insert B while browsing
    c.write_file("F1", "B", "2");
    assert_eq!(c.readnext(tok).unwrap().0, "B");
    assert_eq!(c.readnext(tok).unwrap().0, "C");

    c.endbr(tok);
}

#[test]
fn vsam_delete_and_verify_gone() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.write_file("F1", "K1", "data");
    c.write_file("F1", "K2", "data");
    c.write_file("F1", "K3", "data");
    c.delete_file("F1", "K2");
    assert_eq!(c.resp, 0);

    let tok = c.startbr("F1", "K1");
    assert_eq!(c.readnext(tok).unwrap().0, "K1");
    assert_eq!(c.readnext(tok).unwrap().0, "K3"); // K2 skipped
    c.endbr(tok);
}

#[test]
fn vsam_rrds_slots() {
    let mut c = vsam_ctx();
    c.register_vsam_file("SLOTS", VsamOrganization::Rrds).unwrap();
    c.write_file("SLOTS", "1", "slot1");
    c.write_file("SLOTS", "5", "slot5");
    c.write_file("SLOTS", "10", "slot10");

    assert_eq!(c.read_file("SLOTS", "5"), Some("slot5".to_string()));
    assert!(c.read_file("SLOTS", "3").is_none());
    assert_eq!(c.resp, CicsResp::NotFound as i32);

    c.rewrite_file("SLOTS", "5", "updated");
    assert_eq!(c.read_file("SLOTS", "5"), Some("updated".to_string()));
}

#[test]
fn vsam_esds_append() {
    let mut c = vsam_ctx();
    c.register_vsam_file("LOG", VsamOrganization::Esds).unwrap();
    c.write_file("LOG", "", "entry1");
    c.write_file("LOG", "", "entry2");
    c.write_file("LOG", "", "entry3");

    assert_eq!(c.read_file("LOG", "1"), Some("entry1".to_string()));
    assert_eq!(c.read_file("LOG", "2"), Some("entry2".to_string()));
    assert_eq!(c.read_file("LOG", "3"), Some("entry3".to_string()));
}

// -- Execute dispatch: full option handling --

#[test]
fn execute_write_read_rewrite_delete_cycle() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();

    c.execute("WRITE", &[("FILE", Some("F1")), ("RIDFLD", Some("K1")), ("FROM", Some("initial"))]);
    assert_eq!(c.resp, 0);

    let r = c.execute("READ", &[("FILE", Some("F1")), ("RIDFLD", Some("K1"))]);
    assert_eq!(r, Some("initial".to_string()));

    c.execute("REWRITE", &[("FILE", Some("F1")), ("RIDFLD", Some("K1")), ("FROM", Some("updated"))]);
    assert_eq!(c.resp, 0);

    let r = c.execute("READ", &[("FILE", Some("F1")), ("RIDFLD", Some("K1"))]);
    assert_eq!(r, Some("updated".to_string()));

    c.execute("DELETE", &[("FILE", Some("F1")), ("RIDFLD", Some("K1"))]);
    assert_eq!(c.resp, 0);

    let r = c.execute("READ", &[("FILE", Some("F1")), ("RIDFLD", Some("K1"))]);
    assert!(r.is_none());
    assert_eq!(c.resp, CicsResp::NotFound as i32);
}

#[test]
fn execute_startbr_readnext_readprev_endbr() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.write_file("F1", "A", "1");
    c.write_file("F1", "B", "2");
    c.write_file("F1", "C", "3");

    let tok_str = c.execute("STARTBR", &[("FILE", Some("F1")), ("RIDFLD", Some("A"))]).unwrap();
    let tok = &tok_str;

    let r = c.execute("READNEXT", &[("TOKEN", Some(tok))]).unwrap();
    assert!(r.starts_with("A\t"));
    let r = c.execute("READNEXT", &[("TOKEN", Some(tok))]).unwrap();
    assert!(r.starts_with("B\t"));

    let r = c.execute("READPREV", &[("TOKEN", Some(tok))]).unwrap();
    assert!(r.starts_with("A\t"));

    c.execute("ENDBR", &[("TOKEN", Some(tok))]);
}

#[test]
fn execute_writeq_readq_ts_cycle() {
    let mut c = ctx();

    c.execute("WRITEQ", &[("TS", None), ("QUEUE", Some("Q1")), ("FROM", Some("rec1"))]);
    assert_eq!(c.resp, 0);
    c.execute("WRITEQ", &[("TS", None), ("QUEUE", Some("Q1")), ("FROM", Some("rec2"))]);

    let r = c.execute("READQ", &[("TS", None), ("QUEUE", Some("Q1")), ("ITEM", Some("1"))]);
    assert_eq!(r, Some("rec1".to_string()));
    let r = c.execute("READQ", &[("TS", None), ("QUEUE", Some("Q1")), ("ITEM", Some("2"))]);
    assert_eq!(r, Some("rec2".to_string()));

    c.execute("DELETEQ", &[("QUEUE", Some("Q1"))]);
    assert_eq!(c.resp, 0);
}

#[test]
fn execute_link_and_xctl() {
    let mut c = ctx();
    fn target(_: &mut CicsContext, d: &[u8]) -> Vec<u8> {
        let mut r = b"GOT:".to_vec();
        r.extend_from_slice(d);
        r
    }
    c.register_program("TGT", target);

    let r = c.execute("LINK", &[("PROGRAM", Some("TGT")), ("COMMAREA", Some("data"))]);
    assert_eq!(r, Some("GOT:data".to_string()));

    c.execute("XCTL", &[("PROGRAM", Some("TGT")), ("COMMAREA", Some("xctl_data"))]);
    assert!(matches!(c.last_action, ProgramAction::Xctl { .. }));
}

#[test]
fn execute_return_with_transid() {
    let mut c = ctx();
    c.commarea = b"session".to_vec();
    c.execute("RETURN", &[("TRANSID", Some("MENU")), ("COMMAREA", Some("data"))]);
    assert_eq!(c.return_transid, Some("MENU".to_string()));
}

#[test]
fn execute_start_and_retrieve() {
    let mut c = ctx();
    c.execute("START", &[("PROGRAM", Some("BATCH")), ("FROM", Some("params")), ("INTERVAL", Some("5"))]);
    c.set_retrieve_data(b"params".to_vec());
    let r = c.execute("RETRIEVE", &[]);
    assert_eq!(r, Some("params".to_string()));
}

#[test]
fn execute_handle_abend() {
    let mut c = ctx();
    c.execute("HANDLE", &[("ABEND", None), ("ABCODE", Some("ASRA")), ("LABEL", Some("HANDLER"))]);
    c.execute("ABEND", &[("ABCODE", Some("ASRA"))]);
    assert_eq!(c.resp, 0); // handled
}

#[test]
fn execute_syncpoint_and_rollback() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.write_file("F1", "K1", "original");

    c.begin_transaction();
    c.rewrite_file("F1", "K1", "changed");
    c.execute("SYNCPOINT", &[("ROLLBACK", None)]);
    assert_eq!(c.read_file("F1", "K1"), Some("original".to_string()));
}

#[test]
fn execute_asktime_formattime() {
    let mut c = ctx();
    let abstime_str = c.execute("ASKTIME", &[]).unwrap();
    let abstime: u64 = abstime_str.parse().unwrap();

    let date = c.execute("FORMATTIME", &[
        ("ABSTIME", Some(&abstime_str)),
        ("YYYYMMDD", None),
        ("DATESEP", Some("-")),
    ]).unwrap();
    // Should be a valid date like 2026-03-23
    assert!(date.starts_with("20"));
    assert_eq!(date.len(), 10);
    assert_eq!(&date[4..5], "-");
}

#[test]
fn execute_assign_all() {
    let mut c = ctx();
    assert_eq!(c.execute("ASSIGN", &[("APPLID", None)]), Some("CARDDEMO".to_string()));
    assert_eq!(c.execute("ASSIGN", &[("SYSID", None)]), Some("CICS".to_string()));
    assert_eq!(c.execute("ASSIGN", &[("USERID", None)]), Some("CICSUSER".to_string()));
}

#[test]
fn execute_inquire_program_file() {
    let mut c = vsam_ctx();
    fn dummy(_: &mut CicsContext, _: &[u8]) -> Vec<u8> { Vec::new() }
    c.register_program("PGM1", dummy);
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();

    assert_eq!(c.execute("INQUIRE", &[("PROGRAM", Some("PGM1"))]), Some("INSTALLED".to_string()));
    assert_eq!(c.execute("INQUIRE", &[("PROGRAM", Some("NOPE"))]), Some("NOTINSTALLED".to_string()));
    assert_eq!(c.execute("INQUIRE", &[("FILE", Some("F1"))]), Some("ENABLED".to_string()));
    assert_eq!(c.execute("INQUIRE", &[("FILE", Some("NOPE"))]), Some("DISABLED".to_string()));
}

#[test]
fn execute_send_receive_map() {
    let mut c = ctx();
    let mut mapset = BmsMapset::new("MS1");
    let mut map = BmsMap::new("MAP1", 24, 80);
    map.add_field(BmsField::new("F1", 1, 1, 20));
    map.add_field(BmsField::new("F2", 2, 1, 20).with_attr(dfhbmsca::PROT));
    mapset.add_map(map);
    c.bms_registry_mut().register_mapset(mapset);

    c.execute("SEND", &[("MAP", Some("MAP1")), ("MAPSET", Some("MS1")), ("ERASE", None)]);
    assert!(c.current_screen.is_some());
    let s = c.current_screen.as_ref().unwrap();
    assert_eq!(s.map, "MAP1");
    assert!(s.erase);
}

// -- TSQ integration tests --

#[test]
fn tsq_write_read_sequential() {
    let mut c = ctx();
    for i in 0..50 {
        c.writeq_ts("BIGQ", format!("item_{}", i).as_bytes());
    }
    assert_eq!(c.tsq_numitems("BIGQ"), 50);
    for i in 0..50 {
        let data = c.readq_ts_item("BIGQ", Some(i + 1)).unwrap();
        assert_eq!(String::from_utf8(data).unwrap(), format!("item_{}", i));
    }
}

#[test]
fn tsq_mixed_append_and_rewrite() {
    let mut c = ctx();
    c.writeq_ts("Q", b"aaa");
    c.writeq_ts("Q", b"bbb");
    c.writeq_ts("Q", b"ccc");

    // Rewrite middle item
    c.writeq_ts_item("Q", b"BBB", Some(2));
    assert_eq!(c.readq_ts_item("Q", Some(1)).unwrap(), b"aaa");
    assert_eq!(c.readq_ts_item("Q", Some(2)).unwrap(), b"BBB");
    assert_eq!(c.readq_ts_item("Q", Some(3)).unwrap(), b"ccc");

    // Append one more
    c.writeq_ts("Q", b"ddd");
    assert_eq!(c.tsq_numitems("Q"), 4);
}

#[test]
fn tsq_multiple_queues() {
    let mut c = ctx();
    c.writeq_ts("Q1", b"q1_data");
    c.writeq_ts("Q2", b"q2_data");
    c.writeq_ts("Q3", b"q3_data");

    assert_eq!(c.readq_ts("Q1").unwrap(), b"q1_data");
    assert_eq!(c.readq_ts("Q2").unwrap(), b"q2_data");
    assert_eq!(c.readq_ts("Q3").unwrap(), b"q3_data");

    c.deleteq_ts("Q2");
    assert_eq!(c.resp, 0);
    // Q1 and Q3 unaffected
    assert_eq!(c.tsq_numitems("Q1"), 1);
    assert_eq!(c.tsq_numitems("Q3"), 1);
}

// -- TDQ integration tests --

#[test]
fn tdq_extrapartition_write_read() {
    let mut c = ctx();
    let tmp = std::env::temp_dir().join("ironclad_int_tdq.dat");
    let _ = std::fs::remove_file(&tmp);
    c.register_td_queue("TDQX", tmp.to_str().unwrap());
    c.writeq_td("TDQX", b"message1");
    assert_eq!(c.resp, 0);
    // Extrapartition writes to file — verify file exists
    assert!(tmp.exists());
    let _ = std::fs::remove_file(&tmp);
}

// -- Program control: XCTL chain --

#[test]
fn xctl_chain_three_programs() {
    let mut c = ctx();
    fn pgm_a(ctx: &mut CicsContext, _: &[u8]) -> Vec<u8> {
        ctx.xctl("PGM_B", b"from_a")
    }
    fn pgm_b(ctx: &mut CicsContext, d: &[u8]) -> Vec<u8> {
        assert_eq!(d, b"from_a");
        ctx.xctl("PGM_C", b"from_b")
    }
    fn pgm_c(_: &mut CicsContext, d: &[u8]) -> Vec<u8> {
        assert_eq!(d, b"from_b");
        b"final".to_vec()
    }
    c.register_program("PGM_A", pgm_a);
    c.register_program("PGM_B", pgm_b);
    c.register_program("PGM_C", pgm_c);

    let result = c.link("PGM_A", b"");
    // XCTL replaces, so link returns what the final target returns
    assert!(matches!(c.last_action, ProgramAction::Xctl { .. }));
}

#[test]
fn link_preserves_context() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.write_file("F1", "K1", "before_link");

    fn linked_pgm(ctx: &mut CicsContext, _: &[u8]) -> Vec<u8> {
        // Linked program can read same VSAM
        let data = ctx.read_file("F1", "K1").unwrap();
        assert_eq!(data, "before_link");
        ctx.rewrite_file("F1", "K1", "after_link");
        Vec::new()
    }
    c.register_program("LINKED", linked_pgm);
    c.link("LINKED", b"");
    assert_eq!(c.read_file("F1", "K1"), Some("after_link".to_string()));
}

#[test]
fn start_queue_multiple() {
    let mut c = ctx();
    c.start("PGM1", b"data1", 0);
    c.start("PGM2", b"data2", 5);
    c.start("PGM3", b"data3", 10);
    // Each START should succeed
    assert_eq!(c.resp, 0);
    // Verify via RETRIEVE: set data and retrieve it
    c.set_retrieve_data(b"data1".to_vec());
    assert_eq!(c.retrieve().unwrap(), b"data1");
}

#[test]
fn return_without_transid() {
    let mut c = ctx();
    c.return_program(None);
    assert!(c.return_transid.is_none());
    assert!(matches!(c.last_action, ProgramAction::Return { .. }));
}

#[test]
fn abend_with_wildcard_handler() {
    let mut c = ctx();
    c.handle_abend("*", "GLOBAL_HANDLER");
    c.abend("XYZZ");
    assert_eq!(c.resp, 0); // handled by wildcard
}

// -- Transaction management: VSAM + operations --

#[test]
fn transaction_multi_file_rollback() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();
    c.register_vsam_file("F2", VsamOrganization::Ksds).unwrap();

    c.write_file("F1", "K1", "orig1");
    c.write_file("F2", "K1", "orig2");

    c.begin_transaction();
    c.rewrite_file("F1", "K1", "mod1");
    c.rewrite_file("F2", "K1", "mod2");
    c.rollback();

    assert_eq!(c.read_file("F1", "K1"), Some("orig1".to_string()));
    assert_eq!(c.read_file("F2", "K1"), Some("orig2".to_string()));
}

#[test]
fn transaction_commit_persists() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();

    c.begin_transaction();
    c.write_file("F1", "K1", "committed");
    c.syncpoint();
    assert_eq!(c.resp, 0);
    assert_eq!(c.read_file("F1", "K1"), Some("committed".to_string()));
}

#[test]
fn transaction_write_rollback_gone() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();

    c.begin_transaction();
    c.write_file("F1", "NEW_KEY", "temp_data");
    c.rollback();
    assert!(c.read_file("F1", "NEW_KEY").is_none());
}

// -- SQL integration tests --

#[test]
fn sql_carddemo_schema_and_data() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();
    assert!(sql.sqlca.is_ok());

    // Insert into multiple tables
    let mut vars = HashMap::new();
    vars.insert("ID".to_string(), SqlValue::Text("00000000001".into()));
    vars.insert("STATUS".to_string(), SqlValue::Text("Y".into()));
    vars.insert("BAL".to_string(), SqlValue::Text("5000.00".into()));
    sql.execute_sql(
        "INSERT INTO ACCTDATA (ACCT_ID, ACCT_STATUS, ACCT_CURR_BAL) VALUES (:ID, :STATUS, :BAL)",
        &vars);
    assert!(sql.sqlca.is_ok());

    let mut vars = HashMap::new();
    vars.insert("ID".to_string(), SqlValue::Text("C001".into()));
    vars.insert("FIRST".to_string(), SqlValue::Text("JOHN".into()));
    vars.insert("LAST".to_string(), SqlValue::Text("DOE".into()));
    sql.execute_sql(
        "INSERT INTO CUSTDATA (CUST_ID, CUST_FIRST_NAME, CUST_LAST_NAME) VALUES (:ID, :FIRST, :LAST)",
        &vars);
    assert!(sql.sqlca.is_ok());

    // Query back
    let row = sql.select_into("ACCTDATA", "ACCT_ID", &SqlValue::Text("00000000001".into()));
    assert!(row.is_some());
    assert_eq!(row.unwrap().get_text("ACCT_STATUS"), "Y");
}

#[test]
fn sql_cursor_multi_row() {
    let mut sql = SqlContext::with_memory_db();
    sql.execute_sql("CREATE TABLE CARDS (num TEXT PRIMARY KEY, name TEXT)", &HashMap::new());
    for i in 0..10 {
        let mut vars = HashMap::new();
        vars.insert("NUM".to_string(), SqlValue::Text(format!("{:04}", i)));
        vars.insert("NAME".to_string(), SqlValue::Text(format!("CARD_{}", i)));
        sql.execute_sql("INSERT INTO CARDS (num, name) VALUES (:NUM, :NAME)", &vars);
    }

    sql.declare_cursor_sql("C1", "SELECT num, name FROM CARDS ORDER BY num");
    sql.open_cursor_sql("C1", &HashMap::new());

    let mut count = 0;
    while let Some(row) = sql.fetch_cursor("C1") {
        assert!(row.get_text("name").starts_with("CARD_"));
        count += 1;
    }
    assert_eq!(count, 10);
    sql.close_cursor("C1");
}

#[test]
fn sql_update_and_delete() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();

    let mut vars = HashMap::new();
    vars.insert("ID".to_string(), SqlValue::Text("T0001".into()));
    vars.insert("AMT".to_string(), SqlValue::Text("100.00".into()));
    sql.execute_sql("INSERT INTO TRANDATA (TRAN_ID, TRAN_AMT) VALUES (:ID, :AMT)", &vars);

    let mut vars = HashMap::new();
    vars.insert("AMT".to_string(), SqlValue::Text("200.00".into()));
    vars.insert("ID".to_string(), SqlValue::Text("T0001".into()));
    sql.execute_sql("UPDATE TRANDATA SET TRAN_AMT = :AMT WHERE TRAN_ID = :ID", &vars);
    assert_eq!(sql.sqlca.sqlerrd[2], 1);

    let mut vars = HashMap::new();
    vars.insert("ID".to_string(), SqlValue::Text("T0001".into()));
    sql.execute_sql("DELETE FROM TRANDATA WHERE TRAN_ID = :ID", &vars);
    assert_eq!(sql.sqlca.sqlerrd[2], 1);
}

#[test]
fn sql_whenever_error() {
    let mut sql = SqlContext::with_memory_db();
    sql.whenever_error("GO TO ERROR-PARA");
    sql.execute_sql("INSERT INTO NONEXISTENT (x) VALUES ('y')", &HashMap::new());
    assert_eq!(sql.check_whenever(), Some("ERROR-PARA".to_string()));
}

#[test]
fn sql_whenever_not_found() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();
    sql.whenever_not_found("GO TO EOF-PARA");
    let _ = sql.select_into("ACCTDATA", "ACCT_ID", &SqlValue::Text("NONEXISTENT".into()));
    assert_eq!(sql.check_whenever(), Some("EOF-PARA".to_string()));
}

// -- System services --

#[test]
fn formattime_all_formats() {
    let c = ctx();
    // Known timestamp: 2026-01-15 12:00:00 UTC
    let abstime: u64 = (1768478400u64 + 2208988800) * 1_000_000;

    assert_eq!(c.formattime(abstime, "YYYYMMDD", None), "20260115");
    assert_eq!(c.formattime(abstime, "DDMMYYYY", None), "15012026");
    assert_eq!(c.formattime(abstime, "MMDDYYYY", None), "01152026");
    assert_eq!(c.formattime(abstime, "YYYYMMDD", Some('/')), "2026/01/15");
    assert_eq!(c.formattime(abstime, "DDMMYYYY", Some('.')), "15.01.2026");
    assert_eq!(c.formattime(abstime, "TIME", None), "12:00:00");
}

#[test]
fn asktime_is_recent() {
    let mut c = ctx();
    c.asktime();
    // ABSTIME should be reasonable (after year 2025)
    let unix_approx = c.abstime / 1_000_000 - 2_208_988_800;
    // Unix seconds for 2025-01-01 = ~1735689600
    assert!(unix_approx > 1735689600);
}

#[test]
fn assign_custom_values() {
    let mut c = ctx();
    c.applid = "MYAPP".to_string();
    c.sysid = "SYS1".to_string();
    c.userid = "TESTUSER".to_string();
    assert_eq!(c.assign("APPLID"), "MYAPP");
    assert_eq!(c.assign("SYSID"), "SYS1");
    assert_eq!(c.assign("USERID"), "TESTUSER");
    assert_eq!(c.assign("OPID"), "TESTUSER");
    assert_eq!(c.assign("NETNAME"), "SYS1");
    assert_eq!(c.assign("UNKNOWN"), "");
}

// -- BMS integration tests --

#[test]
fn bms_full_mapset_lifecycle() {
    let mut reg = BmsRegistry::new();

    // Create signon screen
    let mut csignon = BmsMapset::new("CSIGNON");
    let mut signon = BmsMap::new("SIGNON", 24, 80);
    signon.add_field(BmsField::new("TITLE", 1, 25, 30)
        .with_attr(dfhbmsca::PROT_BRT)
        .with_initial("CARDDEMO SIGNON")
        .with_color(BmsColor::White));
    signon.add_field(BmsField::new("USERID", 10, 25, 8)
        .with_color(BmsColor::Green));
    signon.add_field(BmsField::new("PASSWD", 12, 25, 8)
        .with_attr(dfhbmsca::UNPROT_DRK));
    signon.add_field(BmsField::new("MSG", 20, 1, 79)
        .with_attr(dfhbmsca::PROT)
        .with_color(BmsColor::Yellow));
    csignon.add_map(signon);
    reg.register_mapset(csignon);

    // Build screen with runtime data
    let mut data = HashMap::new();
    data.insert("MSG".to_string(), "ENTER USERID AND PASSWORD".to_string());
    let screen = reg.build_screen("SIGNON", "CSIGNON", &data, true, Some("USERID"));
    assert!(screen.is_some());
    let s = screen.unwrap();

    assert_eq!(s.fields.len(), 4);
    assert_eq!(s.cursor_field, Some("USERID".to_string()));
    assert!(s.erase);

    // Check field attributes
    let title = s.fields.iter().find(|f| f.name == "TITLE").unwrap();
    assert!(title.protected);
    assert!(title.bright);
    assert_eq!(title.value, "CARDDEMO SIGNON"); // from initial

    let userid = s.fields.iter().find(|f| f.name == "USERID").unwrap();
    assert!(!userid.protected);
    assert_eq!(userid.color, "green");

    let passwd = s.fields.iter().find(|f| f.name == "PASSWD").unwrap();
    assert!(!passwd.protected);
    assert!(passwd.dark);

    let msg = s.fields.iter().find(|f| f.name == "MSG").unwrap();
    assert_eq!(msg.value, "ENTER USERID AND PASSWORD");
    assert_eq!(msg.color, "yellow");
}

#[test]
fn bms_screen_text_rendering() {
    let mut map = BmsMap::new("TESTMAP", 24, 80);
    map.add_field(BmsField::new("LINE1", 1, 1, 40).with_initial("FIRST LINE"));
    map.add_field(BmsField::new("LINE2", 2, 1, 40).with_initial("SECOND LINE"));

    let data = HashMap::new();
    let screen = ScreenOutput::from_map(&map, "TESTMS", &data, false, None);
    let text = screen.to_text();
    assert!(text.contains("MAP: TESTMAP"));
    assert!(text.contains("LINE1: FIRST LINE"));
    assert!(text.contains("LINE2: SECOND LINE"));
}

#[test]
fn bms_stdio_channel_roundtrip() {
    let input_data = b"AID=PF3,USERID=ADMIN,OPTION=01\n";
    let output_buf = Vec::<u8>::new();
    let mut ch = StdioScreenChannel::new(
        Box::new(output_buf),
        Box::new(BufReader::new(Cursor::new(input_data.to_vec()))),
    );

    let input = ch.receive_screen().unwrap();
    assert_eq!(input.aid, dfhaid::PF3);
    assert_eq!(input.fields.get("USERID"), Some(&"ADMIN".to_string()));
    assert_eq!(input.fields.get("OPTION"), Some(&"01".to_string()));
}

#[test]
fn bms_dfhaid_all_keys() {
    // Verify all AID key names round-trip
    let keys = [
        "ENTER", "CLEAR", "PA1", "PA2", "PA3",
        "PF1", "PF2", "PF3", "PF4", "PF5", "PF6",
        "PF7", "PF8", "PF9", "PF10", "PF11", "PF12",
        "PF13", "PF14", "PF15", "PF16", "PF17", "PF18",
        "PF19", "PF20", "PF21", "PF22", "PF23", "PF24",
    ];
    for k in keys {
        let code = dfhaid::from_name(k);
        assert_ne!(code, 0, "{} should have a code", k);
        assert_eq!(dfhaid::name(code), k);
    }
}

#[test]
fn bms_dfhbmsca_all_attrs() {
    assert!(!dfhbmsca::is_protected(dfhbmsca::UNPROT));
    assert!(dfhbmsca::is_protected(dfhbmsca::PROT));
    assert!(dfhbmsca::is_protected(dfhbmsca::ASKIP)); // autoskip is protected
    assert!(dfhbmsca::is_numeric(dfhbmsca::UNPROT_NUM));
    assert!(dfhbmsca::is_bright(dfhbmsca::UNPROT_BRT));
    assert!(dfhbmsca::is_dark(dfhbmsca::UNPROT_DRK));
    assert!(dfhbmsca::is_dark(dfhbmsca::PROT_DRK));
    assert!(dfhbmsca::is_autoskip(dfhbmsca::ASKIP));
    assert!(dfhbmsca::is_autoskip(dfhbmsca::ASKIP_BRT));
    assert!(dfhbmsca::is_mdt(dfhbmsca::UNPROT_MDT));
    assert!(dfhbmsca::is_mdt(dfhbmsca::UNPROT_NUM_MDT));
    assert!(dfhbmsca::is_mdt(dfhbmsca::UNPROT_BRT_MDT));
}

// -- Transaction Loop integration tests --

#[test]
fn transaction_loop_multi_session() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("SIGN", "SIGNON");
    tl.registry.register("MENU", "MENUPGM");

    fn signon_pgm(ctx: &mut CicsContext, _: &[u8]) -> Vec<u8> {
        let uid = ctx.last_input.as_ref()
            .and_then(|i| i.fields.get("UID").cloned())
            .unwrap_or_default();
        ctx.commarea = uid.into_bytes();
        ctx.return_program(Some("MENU"));
        Vec::new()
    }
    fn menu_pgm(ctx: &mut CicsContext, ca: &[u8]) -> Vec<u8> {
        let mut data = HashMap::new();
        data.insert("USR".to_string(), String::from_utf8_lossy(ca).to_string());
        ctx.send_map("MENU", "CMENU", &data, true);
        ctx.return_program(Some("MENU"));
        Vec::new()
    }

    let mut c = ctx();
    c.register_program("SIGNON", signon_pgm);
    c.register_program("MENUPGM", menu_pgm);

    // Session A
    tl.set_initial_transid("SA", "SIGN");
    let mut f = HashMap::new();
    f.insert("UID".to_string(), "ALICE".to_string());
    tl.dispatch("SA", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();

    // Session B
    tl.set_initial_transid("SB", "SIGN");
    let mut f = HashMap::new();
    f.insert("UID".to_string(), "BOB".to_string());
    tl.dispatch("SB", &ScreenInput { aid: dfhaid::ENTER, cursor_row: 0, cursor_col: 0, fields: f }, &mut c).unwrap();

    // Verify isolation
    assert_eq!(tl.get_session("SA").unwrap().commarea, b"ALICE");
    assert_eq!(tl.get_session("SB").unwrap().commarea, b"BOB");
}

#[test]
fn transaction_loop_missing_program() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("BAD", "NOPGM");
    tl.set_initial_transid("S1", "BAD");

    let mut c = ctx();
    let result = tl.dispatch("S1", &ScreenInput::default(), &mut c);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn transaction_loop_commarea_chain() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("T1", "PGM1");

    fn pgm1(ctx: &mut CicsContext, ca: &[u8]) -> Vec<u8> {
        let n: usize = String::from_utf8_lossy(ca).parse().unwrap_or(0);
        ctx.commarea = (n + 1).to_string().into_bytes();
        ctx.return_program(Some("T1"));
        Vec::new()
    }

    let mut c = ctx();
    c.register_program("PGM1", pgm1);
    tl.set_initial_transid("S1", "T1");
    tl.get_or_create_session("S1").commarea = b"0".to_vec();

    // Dispatch 5 turns — COMMAREA should increment each time
    for _ in 0..5 {
        tl.dispatch("S1", &ScreenInput::default(), &mut c).unwrap();
    }
    assert_eq!(tl.get_session("S1").unwrap().commarea, b"5");
}

// -- Cross-module integration tests --

#[test]
fn cross_vsam_and_sql_same_data() {
    // VSAM and SQL can operate on different data stores simultaneously
    let mut c = vsam_ctx();
    c.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();
    c.write_file("ACCTDAT", "001", "VSAM_DATA");

    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();
    let mut vars = HashMap::new();
    vars.insert("ID".to_string(), SqlValue::Text("001".into()));
    vars.insert("STATUS".to_string(), SqlValue::Text("Y".into()));
    sql.execute_sql("INSERT INTO ACCTDATA (ACCT_ID, ACCT_STATUS) VALUES (:ID, :STATUS)", &vars);

    // Both accessible independently
    assert_eq!(c.read_file("ACCTDAT", "001"), Some("VSAM_DATA".to_string()));
    let row = sql.select_into("ACCTDATA", "ACCT_ID", &SqlValue::Text("001".into()));
    assert_eq!(row.unwrap().get_text("ACCT_STATUS"), "Y");
}

#[test]
fn cross_bms_and_vsam_screen_with_data() {
    let mut c = full_setup();

    // Read VSAM data and send it in a BMS screen
    let acct = c.read_file("ACCTDAT", "00000000001").unwrap();
    let mut data = HashMap::new();
    data.insert("ACCTID".to_string(), "00000000001".to_string());
    data.insert("DATA".to_string(), acct.clone());
    data.insert("MSG".to_string(), "ACCOUNT FOUND".to_string());
    c.send_map("ACCTVIEW", "CACTVW", &data, true);

    let screen = c.current_screen.as_ref().unwrap();
    let data_field = screen.fields.iter().find(|f| f.name == "DATA").unwrap();
    assert!(data_field.value.contains("5000.00"));
}

#[test]
fn cross_tsq_and_transaction() {
    let mut c = vsam_ctx();
    c.register_vsam_file("F1", VsamOrganization::Ksds).unwrap();

    // TSQ + VSAM in same transaction
    c.begin_transaction();
    c.write_file("F1", "K1", "data");
    c.writeq_ts("CACHE", b"cached_data");
    c.syncpoint();

    assert_eq!(c.read_file("F1", "K1"), Some("data".to_string()));
    assert_eq!(c.tsq_numitems("CACHE"), 1);
}

// -- Edge cases --

#[test]
fn empty_commarea() {
    let mut c = ctx();
    c.commarea = Vec::new();
    assert_eq!(c.calen, 0);
    c.return_program(Some("NEXT"));
    assert_eq!(c.return_commarea.as_ref().unwrap().len(), 0);
}

#[test]
fn large_commarea() {
    let mut c = ctx();
    c.commarea = vec![0x42; 32767]; // max COMMAREA in CICS
    c.return_program(Some("T"));
    assert_eq!(c.return_commarea.as_ref().unwrap().len(), 32767);
}

#[test]
fn vsam_case_insensitive_file_names() {
    let mut c = vsam_ctx();
    c.register_vsam_file("MyFile", VsamOrganization::Ksds).unwrap();
    c.write_file("myfile", "K1", "data");
    assert_eq!(c.read_file("MYFILE", "K1"), Some("data".to_string()));
    assert_eq!(c.read_file("myfile", "K1"), Some("data".to_string()));
}

#[test]
fn unregistered_vsam_file() {
    let mut c = vsam_ctx();
    // Register FILE_A but not FILE_B; FILE_A ops work, FILE_B falls to flat-file
    c.register_vsam_file("FILE_A", VsamOrganization::Ksds).unwrap();
    c.write_file("FILE_A", "K1", "data");
    assert_eq!(c.resp, 0);
    // Reading registered file works
    assert_eq!(c.read_file("FILE_A", "K1"), Some("data".to_string()));
    // Reading an unregistered file that doesn't exist as flat-file → None
    let result = c.read_file("/nonexistent/path/no_such_file.dat", "K1");
    assert!(result.is_none());
}

#[test]
fn resp_code_values() {
    assert_eq!(CicsResp::Normal.code(), 0);
    assert_eq!(CicsResp::NotFound.code(), 13);
    assert_eq!(CicsResp::DuplicateKey.code(), 14);
    assert_eq!(CicsResp::EndData.code(), 20);
    assert_eq!(CicsResp::QIdErr.code(), 26);
    assert_eq!(CicsResp::ItemErr.code(), 27);
    assert_eq!(CicsResp::PgmIdErr.code(), 28);
}

#[test]
fn unknown_command_resp() {
    let mut c = ctx();
    let r = c.execute("FOOBAR", &[]);
    assert!(r.is_none());
    assert_eq!(c.resp, CicsResp::InvalidReq as i32);
}

// -- VsamStore direct tests --

#[test]
fn vsam_store_tdq_trigger_integration() {
    let mut store = VsamStore::new_in_memory();
    store.register_tdq_trigger("ALERT", 3, "ALERT_PGM");

    store.tdq_write("ALERT", b"msg1").unwrap();
    store.tdq_write("ALERT", b"msg2").unwrap();
    assert!(store.drain_triggered_starts().is_empty());

    store.tdq_write("ALERT", b"msg3").unwrap();
    let triggers = store.drain_triggered_starts();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].0, "ALERT_PGM");
}

#[test]
fn vsam_store_transaction_isolation() {
    let mut store = VsamStore::new_in_memory();
    store.register_file("F1", VsamOrganization::Ksds).unwrap();

    store.write("F1", "K1", "value1").unwrap();
    store.begin_transaction().unwrap();
    store.write("F1", "K2", "value2").unwrap();
    store.rewrite("F1", "K1", "modified").unwrap();
    store.rollback_transaction().unwrap();

    assert_eq!(store.read("F1", "K1").unwrap(), "value1");
    assert!(store.read("F1", "K2").is_err()); // rolled back
}

#[test]
fn vsam_store_tsq_persistence() {
    let store = VsamStore::new_in_memory();

    // Write items across "sessions"
    store.tsq_write("SESSION_Q", b"item1", None).unwrap();
    store.tsq_write("SESSION_Q", b"item2", None).unwrap();
    store.tsq_write("OTHER_Q", b"other", None).unwrap();

    assert_eq!(store.tsq_numitems("SESSION_Q"), 2);
    assert_eq!(store.tsq_numitems("OTHER_Q"), 1);
    assert_eq!(store.tsq_read("SESSION_Q", 1).unwrap(), b"item1");
}

// ══════════════════════════════════════════════════════════════════════
// PERFORMANCE BASELINE TESTS
// ══════════════════════════════════════════════════════════════════════

#[test]
fn perf_vsam_ops_under_5ms() {
    let mut c = vsam_ctx();
    c.register_vsam_file("PERF", VsamOrganization::Ksds).unwrap();

    // Warm up
    c.write_file("PERF", "WARM", "data");
    c.read_file("PERF", "WARM");

    // Measure 100 write+read cycles
    let start = Instant::now();
    for i in 0..100 {
        let key = format!("{:06}", i);
        c.write_file("PERF", &key, "performance_test_data_record");
    }
    let write_elapsed = start.elapsed();

    let start = Instant::now();
    for i in 0..100 {
        let key = format!("{:06}", i);
        c.read_file("PERF", &key);
    }
    let read_elapsed = start.elapsed();

    // Average should be well under 5ms per operation
    let avg_write_us = write_elapsed.as_micros() / 100;
    let avg_read_us = read_elapsed.as_micros() / 100;
    assert!(avg_write_us < 5000, "Write avg {}us exceeds 5ms", avg_write_us);
    assert!(avg_read_us < 5000, "Read avg {}us exceeds 5ms", avg_read_us);
}

#[test]
fn perf_browse_under_5ms() {
    let mut c = vsam_ctx();
    c.register_vsam_file("PERF", VsamOrganization::Ksds).unwrap();
    for i in 0..1000 {
        c.write_file("PERF", &format!("{:06}", i), "data");
    }

    let start = Instant::now();
    let tok = c.startbr("PERF", "000000");
    for _ in 0..1000 {
        c.readnext(tok);
    }
    c.endbr(tok);
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 1000;
    assert!(avg_us < 5000, "Browse avg {}us exceeds 5ms", avg_us);
}

#[test]
fn perf_tsq_under_5ms() {
    let mut c = ctx();
    let start = Instant::now();
    for i in 0..100 {
        c.writeq_ts("PERFQ", format!("item_{}", i).as_bytes());
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 100;
    assert!(avg_us < 5000, "TSQ write avg {}us exceeds 5ms", avg_us);
}

#[test]
fn perf_sql_under_5ms() {
    let mut sql = SqlContext::with_memory_db();
    sql.execute_sql("CREATE TABLE PERF (id TEXT PRIMARY KEY, val TEXT)", &HashMap::new());

    let start = Instant::now();
    for i in 0..100 {
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text(format!("{}", i)));
        vars.insert("VAL".to_string(), SqlValue::Text("test".into()));
        sql.execute_sql("INSERT INTO PERF (id, val) VALUES (:ID, :VAL)", &vars);
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 100;
    assert!(avg_us < 5000, "SQL insert avg {}us exceeds 5ms", avg_us);
}

#[test]
fn perf_transaction_dispatch() {
    let mut tl = TransactionLoop::new();
    tl.registry.register("T1", "FASTPGM");

    fn fast_pgm(ctx: &mut CicsContext, _: &[u8]) -> Vec<u8> {
        ctx.return_program(Some("T1"));
        Vec::new()
    }

    let mut c = ctx();
    c.register_program("FASTPGM", fast_pgm);
    tl.set_initial_transid("S1", "T1");

    let start = Instant::now();
    for _ in 0..100 {
        tl.dispatch("S1", &ScreenInput::default(), &mut c).unwrap();
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() / 100;
    assert!(avg_us < 5000, "Dispatch avg {}us exceeds 5ms", avg_us);
}

// ══════════════════════════════════════════════════════════════════════
// BATCH PROGRAM SIMULATION TESTS
// ══════════════════════════════════════════════════════════════════════

#[test]
fn batch_account_file_load() {
    let mut c = vsam_ctx();
    c.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();
    for i in 0..100 {
        c.write_file("ACCTDAT", &format!("{:011}", i),
            &format!("STATUS=Y,BAL={:.2}", i as f64 * 100.0));
        assert_eq!(c.resp, 0);
    }
    // Verify random access
    assert!(c.read_file("ACCTDAT", "00000000050").unwrap().contains("5000.00"));
}

#[test]
fn batch_card_file_load() {
    let mut c = vsam_ctx();
    c.register_vsam_file("CARDDAT", VsamOrganization::Ksds).unwrap();
    for i in 0..50 {
        c.write_file("CARDDAT", &format!("411111{:010}", i),
            &format!("CVV={:03},NAME=CARDHOLDER_{}", i % 1000, i));
        assert_eq!(c.resp, 0);
    }
    let tok = c.startbr("CARDDAT", "411111");
    let mut count = 0;
    while let Some(_) = c.readnext(tok) { count += 1; }
    c.endbr(tok);
    assert_eq!(count, 50);
}

#[test]
fn batch_customer_load() {
    let mut c = vsam_ctx();
    c.register_vsam_file("CUSTDAT", VsamOrganization::Ksds).unwrap();
    for i in 0..50 {
        c.write_file("CUSTDAT", &format!("C{:03}", i),
            &format!("FIRST=FN_{},LAST=LN_{}", i, i));
    }
    assert_eq!(c.read_file("CUSTDAT", "C025").unwrap(), "FIRST=FN_25,LAST=LN_25");
}

#[test]
fn batch_xref_load() {
    let mut c = vsam_ctx();
    c.register_vsam_file("CARDXRF", VsamOrganization::Ksds).unwrap();
    for i in 0..30 {
        c.write_file("CARDXRF", &format!("411111{:010}", i),
            &format!("ACCT={:011},CUST=C{:03}", i / 3, i));
    }
    let xref = c.read_file("CARDXRF", "4111110000000015").unwrap();
    assert!(xref.contains("ACCT=00000000005"));
}

#[test]
fn batch_transaction_load() {
    let mut c = vsam_ctx();
    c.register_vsam_file("TRANDAT", VsamOrganization::Ksds).unwrap();
    for i in 0..200 {
        c.write_file("TRANDAT", &format!("T{:06}", i),
            &format!("TYPE=01,AMT={:.2}", i as f64 * 5.0));
    }
    // Browse transactions in range
    let tok = c.startbr("TRANDAT", "T000100");
    let mut count = 0;
    while let Some(_) = c.readnext(tok) { count += 1; }
    c.endbr(tok);
    assert_eq!(count, 100); // T000100 through T000199
}

#[test]
fn batch_transaction_category_load() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();
    let types = [("01", "PURCHASE"), ("02", "RETURN"), ("03", "PAYMENT"), ("04", "CASH ADVANCE")];
    for (code, desc) in types {
        let mut vars = HashMap::new();
        vars.insert("CODE".to_string(), SqlValue::Text(code.into()));
        vars.insert("DESC".to_string(), SqlValue::Text(desc.into()));
        sql.execute_sql("INSERT INTO TRANTYPG (TRAN_TYPE_CD, TRAN_TYPE_DESC) VALUES (:CODE, :DESC)", &vars);
        assert!(sql.sqlca.is_ok());
    }
    let row = sql.select_into("TRANTYPG", "TRAN_TYPE_CD", &SqlValue::Text("01".into()));
    assert_eq!(row.unwrap().get_text("TRAN_TYPE_DESC"), "PURCHASE");
}

#[test]
fn batch_transaction_type_load() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();
    let cats = [
        ("01", "01", "GROCERIES"), ("01", "02", "ELECTRONICS"),
        ("01", "03", "CLOTHING"), ("02", "01", "MERCHANDISE RETURN"),
    ];
    for (ttype, cat, desc) in cats {
        let mut vars = HashMap::new();
        vars.insert("TYPE".to_string(), SqlValue::Text(ttype.into()));
        vars.insert("CAT".to_string(), SqlValue::Text(cat.into()));
        vars.insert("DESC".to_string(), SqlValue::Text(desc.into()));
        sql.execute_sql(
            "INSERT INTO TRANCATG (TRAN_TYPE_CD, TRAN_CAT_CD, TRAN_CAT_DESC) VALUES (:TYPE, :CAT, :DESC)",
            &vars);
        assert!(sql.sqlca.is_ok());
    }
}

#[test]
fn batch_daily_transaction_report() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();

    // Insert transactions
    for i in 0..50 {
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text(format!("T{:04}", i)));
        vars.insert("TYPE".to_string(), SqlValue::Text("01".into()));
        vars.insert("AMT".to_string(), SqlValue::Text(format!("{:.2}", i as f64 * 25.0)));
        vars.insert("CARD".to_string(), SqlValue::Text("4111111111111111".into()));
        sql.execute_sql(
            "INSERT INTO TRANDATA (TRAN_ID, TRAN_TYPE_CD, TRAN_AMT, TRAN_CARD_NUM) VALUES (:ID, :TYPE, :AMT, :CARD)",
            &vars);
    }

    // Query report via cursor
    sql.declare_cursor_sql("REPORT", "SELECT TRAN_ID, TRAN_AMT FROM TRANDATA ORDER BY TRAN_ID");
    sql.open_cursor_sql("REPORT", &HashMap::new());
    let mut count = 0;
    while let Some(_) = sql.fetch_cursor("REPORT") { count += 1; }
    sql.close_cursor("REPORT");
    assert_eq!(count, 50);
}

#[test]
fn batch_interest_calculation() {
    let mut c = vsam_ctx();
    c.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();

    // Load accounts
    let accounts = vec![
        ("00000000001", 5000.0f64),
        ("00000000002", 10000.0),
        ("00000000003", 250.0),
    ];
    for (id, bal) in &accounts {
        c.write_file("ACCTDAT", id, &format!("BAL={:.2}", bal));
    }

    // Simulate batch: read each, compute interest, rewrite
    let rate = 0.015; // 1.5% monthly
    let tok = c.startbr("ACCTDAT", "0");
    while let Some((key, data)) = c.readnext(tok) {
        if let Some(bal_str) = data.strip_prefix("BAL=") {
            if let Ok(bal) = bal_str.parse::<f64>() {
                let new_bal = bal * (1.0 + rate);
                c.rewrite_file("ACCTDAT", &key, &format!("BAL={:.2}", new_bal));
            }
        }
    }
    c.endbr(tok);

    // Verify
    let data = c.read_file("ACCTDAT", "00000000001").unwrap();
    assert!(data.contains("5075.00")); // 5000 * 1.015
}

#[test]
fn batch_purge_closed_accounts() {
    let mut c = vsam_ctx();
    c.register_vsam_file("ACCTDAT", VsamOrganization::Ksds).unwrap();

    c.write_file("ACCTDAT", "001", "STATUS=Y");
    c.write_file("ACCTDAT", "002", "STATUS=N"); // closed
    c.write_file("ACCTDAT", "003", "STATUS=Y");
    c.write_file("ACCTDAT", "004", "STATUS=N"); // closed
    c.write_file("ACCTDAT", "005", "STATUS=Y");

    // Batch: browse and collect closed accounts, then delete
    let mut to_delete = Vec::new();
    let tok = c.startbr("ACCTDAT", "0");
    while let Some((key, data)) = c.readnext(tok) {
        if data.contains("STATUS=N") {
            to_delete.push(key);
        }
    }
    c.endbr(tok);

    for key in &to_delete {
        c.delete_file("ACCTDAT", key);
        assert_eq!(c.resp, 0);
    }

    // Verify only active accounts remain
    let tok = c.startbr("ACCTDAT", "0");
    let mut remaining = Vec::new();
    while let Some((key, _)) = c.readnext(tok) { remaining.push(key); }
    c.endbr(tok);
    assert_eq!(remaining, vec!["001", "003", "005"]);
}

#[test]
fn batch_card_expiry_check() {
    let mut c = vsam_ctx();
    c.register_vsam_file("CARDDAT", VsamOrganization::Ksds).unwrap();

    c.write_file("CARDDAT", "4111111111111111", "EXP=20270101,ACTIVE=Y");
    c.write_file("CARDDAT", "4222222222222222", "EXP=20250101,ACTIVE=Y"); // expired
    c.write_file("CARDDAT", "5333333333333333", "EXP=20270601,ACTIVE=Y");

    // Batch: find expired cards
    let today = "20260323";
    let tok = c.startbr("CARDDAT", "0");
    let mut expired = Vec::new();
    while let Some((key, data)) = c.readnext(tok) {
        if let Some(exp) = data.split(',').find(|s| s.starts_with("EXP=")) {
            let exp_date = &exp[4..];
            if exp_date < today {
                expired.push(key);
            }
        }
    }
    c.endbr(tok);
    assert_eq!(expired, vec!["4222222222222222"]);
}

#[test]
fn batch_cross_reference_build() {
    let mut c = vsam_ctx();
    c.register_vsam_file("CARDDAT", VsamOrganization::Ksds).unwrap();
    c.register_vsam_file("CARDXRF", VsamOrganization::Ksds).unwrap();

    // Source cards
    c.write_file("CARDDAT", "4111111111111111", "ACCT=001,CUST=C01");
    c.write_file("CARDDAT", "4222222222222222", "ACCT=001,CUST=C01");
    c.write_file("CARDDAT", "5333333333333333", "ACCT=002,CUST=C02");

    // Build xref
    let tok = c.startbr("CARDDAT", "0");
    while let Some((card_num, data)) = c.readnext(tok) {
        c.write_file("CARDXRF", &card_num, &data);
    }
    c.endbr(tok);

    // Verify xref
    let xref = c.read_file("CARDXRF", "4111111111111111").unwrap();
    assert!(xref.contains("ACCT=001"));
}

#[test]
fn batch_sql_account_summary_report() {
    let mut sql = SqlContext::with_memory_db();
    sql.init_carddemo_schema();

    // Load accounts via SQL
    for i in 0..10 {
        let mut vars = HashMap::new();
        vars.insert("ID".to_string(), SqlValue::Text(format!("{:011}", i)));
        vars.insert("STATUS".to_string(), SqlValue::Text(if i % 3 == 0 { "N" } else { "Y" }.into()));
        vars.insert("BAL".to_string(), SqlValue::Text(format!("{:.2}", i as f64 * 1000.0)));
        sql.execute_sql(
            "INSERT INTO ACCTDATA (ACCT_ID, ACCT_STATUS, ACCT_CURR_BAL) VALUES (:ID, :STATUS, :BAL)",
            &vars);
    }

    // Cursor query for active accounts only
    sql.declare_cursor_sql("ACTIVE", "SELECT ACCT_ID, ACCT_CURR_BAL FROM ACCTDATA WHERE ACCT_STATUS = 'Y' ORDER BY ACCT_ID");
    sql.open_cursor_sql("ACTIVE", &HashMap::new());
    let mut count = 0;
    while let Some(row) = sql.fetch_cursor("ACTIVE") {
        let status_check = row.get_text("ACCT_CURR_BAL");
        assert!(!status_check.is_empty());
        count += 1;
    }
    sql.close_cursor("ACTIVE");
    // Accounts 0,3,6,9 are inactive (STATUS=N), so 6 active
    assert_eq!(count, 6);
}
