# Ironclad CardDemo Showcase

**44/44 AWS CardDemo CICS/COBOL programs transpiled to Rust (100% compile + production CICS runtime) | 30,175 lines COBOL | 118,907 lines Rust | 268 tests | React 3270 UI | No AI**

Built by **Torsova LLC** using the **Ironclad** deterministic COBOL-to-Rust transpiler.

---

## What Is This?

This repository contains the complete Rust output from transpiling [AWS CardDemo](https://github.com/aws-samples/aws-mainframe-modernization-carddemo) through **Ironclad**, plus a **production-grade CICS runtime** with SQLite-backed VSAM storage, real SQL execution, BMS screen engine, and a browser-based 3270 terminal UI.

| Metric | Value |
|---|---|
| COBOL Source Programs | 44 |
| COBOL Copybooks Processed | 75 |
| COBOL Source Lines | 30,175 |
| Rust Output Lines | 118,907 |
| Compile Pass Rate | **44/44 (100%)** |
| Transpile Time | ~100 ms |
| Transpile Speed | ~1.2 million lines/sec |
| Runtime Tests | **268** (185 unit + 83 integration) |
| Performance | <5ms per CICS command |

Every program passes `cargo check` with zero errors.

---

## Programs

| Program | Lines | Description |
|---|---:|---|
| CBACT01C | 1,816 | Batch account file processor |
| CBACT02C | 554 | Batch account card cross-reference |
| CBACT03C | 540 | Batch account card cross-reference (alt) |
| CBACT04C | 2,393 | Batch account interest calculator |
| CBCUS01C | 615 | Batch customer file processor |
| CBEXPORT | 2,504 | Batch data export utility |
| CBIMPORT | 2,206 | Batch data import utility |
| CBPAUP0C | 797 | Batch password update processor |
| CBSTM03A | 2,715 | Batch statement generator (accounts) |
| CBSTM03B | 953 | Batch statement generator (transactions) |
| CBTRN01C | 2,221 | Batch transaction file processor |
| CBTRN02C | 2,677 | Batch transaction category balance |
| CBTRN03C | 2,416 | Batch transaction report generator |
| COACCT01 | 1,907 | Online account display (CICS) |
| COACTUPC | 10,147 | Online account update (CICS) |
| COACTVWC | 4,930 | Online account view (CICS) |
| COADM01C | 2,748 | Online admin menu (CICS) |
| COBIL00C | 2,345 | Online bill payment (CICS) |
| COBSWAIT | 76 | Busy wait screen handler |
| COBTUPDT | 488 | Batch update driver |
| COCRDLIC | 5,515 | Online credit card list (CICS) |
| COCRDSLC | 3,242 | Online credit card search/select (CICS) |
| COCRDUPC | 3,993 | Online credit card update (CICS) |
| CODATE01 | 1,590 | Date conversion utility |
| COMEN01C | 2,788 | Online main menu (CICS) |
| COPAUA0C | 2,844 | Online password authentication (CICS) |
| COPAUS0C | 6,740 | Online pause screen handler (CICS) |
| COPAUS1C | 3,814 | Online pause screen variant 1 (CICS) |
| COPAUS2C | 463 | Online pause screen variant 2 (CICS) |
| CORPT00C | 3,031 | Online report selection (CICS) |
| COSGN00C | 1,955 | Online sign-on screen (CICS) |
| COTRN00C | 5,728 | Online transaction menu (CICS) |
| COTRN01C | 2,782 | Online transaction add (CICS) |
| COTRN02C | 3,348 | Online transaction detail (CICS) |
| COTRTLIC | 6,238 | Online transaction list (CICS) |
| COTRTUPC | 3,841 | Online transaction update (CICS) |
| COUSR00C | 5,744 | Online user management menu (CICS) |
| COUSR01C | 2,040 | Online user add (CICS) |
| COUSR02C | 2,172 | Online user update (CICS) |
| COUSR03C | 2,067 | Online user delete (CICS) |
| CSUTLDTC | 410 | Utility date/time conversion |
| DBUNLDGS | 1,065 | Database unload utility |
| PAUDBLOD | 1,268 | Audit trail data loader |
| PAUDBUNL | 1,181 | Audit trail data unloader |

---

## Build

```bash
# Verify all 44 programs compile
cargo check

# Build all binaries
cargo build --release

# Run all 268 tests
cargo test
```

Requires Rust 1.70+ (edition 2021). The runtime depends on `rusqlite` (bundled SQLite — no system install needed).

---

## Repository Structure

```
ironclad-carddemo-showcase/
  Cargo.toml                    # Workspace: 44 binary targets
  cobol-runtime/                # Production CICS runtime library
    src/
      lib.rs                    # Module root
      fixed_string.rs           # FixedString<N> — fixed-length COBOL PIC X fields
      decimal.rs                # Decimal — COBOL PIC 9 with implied decimal
      packed_decimal.rs         # PackedDecimal<N> — COBOL COMP-3
      cics.rs                   # CICS transaction server (~1050 lines)
      vsam.rs                   # SQLite-backed VSAM storage engine
      sql.rs                    # Real SQL/DB2 execution via SQLite
      bms.rs                    # BMS screen engine (DFHBMSCA/DFHAID)
      transaction_loop.rs       # Pseudo-conversational dispatcher
    tests/
      integration.rs            # 83 integration tests
    carddemo-ui/                # React 3270 terminal UI
      src/
        App.tsx                 # Session management
        Terminal.tsx            # 80x24 character grid with field I/O
        api.ts                  # REST API client
        types.ts                # TypeScript types matching Rust BMS model
  src/                          # 44 transpiled Rust programs
    CBACT01C.rs ... PAUDBUNL.rs
```

---

## Type Mapping

Ironclad maps COBOL data types to native Rust equivalents:

| COBOL | Rust | Notes |
|---|---|---|
| PIC X(n) | `FixedString<N>` | Fixed-length, space-padded, EBCDIC-aware |
| PIC 9(n) | `Decimal` | Signed, scaled, arbitrary precision |
| PIC 9(n) COMP-3 | `PackedDecimal<N>` | Packed BCD with sign nibble |
| PIC 9(n) COMP / BINARY | `i32` / `i64` | Native integer |
| PIC 9(n)V9(m) | `Decimal` | Implied decimal point |
| GROUP items | `struct` | Nested structs mirror COBOL hierarchy |
| OCCURS n TIMES | `Vec<T>` / `[T; N]` | Fixed or variable-length arrays |
| 88-level conditions | Named boolean checks | Condition name evaluation |
| REDEFINES | Union-style access | Byte-level reinterpretation |
| FILE STATUS | `FileStatus` | Two-character status codes |
| SQLCA | `Sqlca` | DB2 communication area |
| DFHCOMMAREA | `CicsContext` | CICS pseudo-conversational state |

---

## What Makes This Hard

AWS CardDemo is not a toy benchmark. It exercises real mainframe patterns that break most transpilers:

1. **CICS pseudo-conversational programming** — 30 of 44 programs use EXEC CICS with SEND MAP, RECEIVE MAP, RETURN TRANSID, and HANDLE CONDITION. State survives across terminal interactions via COMMAREA.

2. **Embedded SQL / DB2** — Multiple programs execute dynamic SQL with host variables, cursors, FETCH loops, and SQLCA status checking.

3. **BMS screen maps** — Online programs send and receive 3270 terminal maps with attribute bytes, cursor positioning, and field-level validation.

4. **Mixed batch and online** — The system includes both batch file-processing programs (CB*) and online CICS programs (CO*), with shared copybook data structures.

5. **75 copybooks with deep nesting** — Data definitions span dozens of copybooks with multi-level COPY REPLACING, nested group items, and REDEFINES chains.

6. **Reference modification** — Substring operations like `FIELD(3:5)` requiring byte-level access across type boundaries.

7. **Complex control flow** — PERFORM THRU, GO TO, nested EVALUATE/WHEN, paragraph fall-through, and COBOL's unique scoping rules all translate to Rust's strict control flow.

---

## Runtime Library

The `cobol-runtime` crate provides a production-grade CICS runtime:

### Type System
- **FixedString\<N\>** — Fixed-length strings with COBOL comparison semantics (space-padded, case-sensitive)
- **Decimal** — Arbitrary-precision signed decimal with COBOL truncation and rounding rules
- **PackedDecimal\<N\>** — Packed BCD representation matching COMP-3 storage

### CICS Runtime (Production)
- **VSAM Storage** — SQLite-backed keyed storage (KSDS/RRDS/ESDS) with B-tree indexed access, browse cursors (forward + backward), and ACID transactions via SQLite
- **Program Control** — XCTL (transfer, no return), LINK (call + return), RETURN TRANSID + COMMAREA (pseudo-conversational), START/RETRIEVE, HANDLE ABEND
- **TSQ** — Random-access temporary storage queues with item-level read/write/rewrite and NUMITEMS
- **TDQ** — Transient data queues with trigger-level automatic program initiation
- **SQL/DB2** — Real SQL execution against SQLite with host variable binding, cursors, SQLCA, and CardDemo 7-table schema
- **BMS Screen Engine** — Structured screen I/O with DFHBMSCA attribute bytes, DFHAID attention identifiers, and pluggable screen channels
- **System Services** — ASKTIME, FORMATTIME, ASSIGN, INQUIRE
- **Transaction Loop** — Pseudo-conversational dispatcher with session management and COMMAREA chaining

### React 3270 Terminal UI
- 80x24 monospace character grid matching IBM 3270
- Protected/unprotected field rendering with color attributes
- Keyboard: Enter, PF1-PF24, Tab/Backtab, CLEAR
- Client-side numeric field validation and length enforcement
- REST API integration for screen I/O

---

## What Makes This Different

1. **Deterministic** — Same input always produces identical output. No randomness, no neural networks, no LLMs. Pure compiler technology.

2. **Complete type safety** — Every COBOL data item maps to a concrete Rust type. No `unsafe` blocks in transpiler output. No raw pointer manipulation.

3. **Structural preservation** — COBOL paragraph structure maps to Rust functions. Data hierarchy maps to nested structs. The output reads like the source.

4. **Speed** — 44 programs transpiled in ~100 ms. That is over 1.2 million lines of Rust per second.

5. **Production runtime** — Not stubs: real SQLite-backed VSAM, real SQL execution, real BMS screens, real ACID transactions. 268 tests prove it.

6. **Production architecture** — Ironclad handles CICS, DB2, VSAM, BMS maps, and batch JCL — the patterns that define real mainframe systems, not textbook exercises.

---

## Related Showcases

| Repository | Description |
|---|---|
| [cms-medicare-ironclad-showcase](https://github.com/mrm413/cms-medicare-ironclad-showcase) | 55 CMS Medicare COBOL pricers transpiled to Rust (100%) |
| [lazarus-carddemo-showcase](https://github.com/mrm413/lazarus-carddemo-showcase) | Same 44 CardDemo programs transpiled to C++17 by Lazarus (100%) |
| [cms-medicare-lazarus-showcase](https://github.com/mrm413/cms-medicare-lazarus-showcase) | 55 CMS Medicare COBOL pricers transpiled to C++17 (100%) |
| [lazarus-cobol-showcase](https://github.com/mrm413/lazarus-cobol-showcase) | GnuCOBOL 3.2 test suite: 1,545 programs transpiled to C++17 (100%) |
| [Ironclad-COBOL-to-Rust](https://github.com/mrm413/Ironclad-COBOL-to-Rust) | GnuCOBOL 3.2 test suite: 1,314 programs transpiled to Rust (100%) |

---

## Built By

**Torsova LLC** — Deterministic mainframe modernization. No AI. No guesswork. Compiler-grade transpilation.

- **Ironclad**: COBOL to Rust
- **Lazarus**: COBOL to C++17

---

## License

MIT
