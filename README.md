# Ironclad CardDemo Showcase

**44 / 44 AWS CardDemo CICS / COBOL programs transpiled to Rust | 30,175 lines COBOL → 118,907 lines Rust | 100% compile | 268 runtime tests passing | Production CICS runtime + React 3270 UI | No AI**

This repository contains the complete Rust output from running [AWS CardDemo](https://github.com/aws-samples/aws-mainframe-modernization-carddemo) through **Ironclad**, plus a production-grade CICS runtime with SQLite-backed VSAM storage, real SQL execution, BMS screen engine, and a browser-based 3270 terminal UI.

Ironclad is a proprietary transpilation engine built by [Torsova LLC](https://torsova.com). The source code for Ironclad is not included in this repository.

---

## What Is This?

A reproducible proof that Ironclad can take real CICS / COBOL — pseudo-conversational online programs, embedded SQL, BMS screens, batch programs, COMMAREA chaining — and produce compiling, executable, byte-for-byte-equivalent Rust.

| Metric | Value |
|---|---|
| COBOL source programs | 44 |
| COBOL copybooks processed | 75 |
| COBOL source lines | 30,175 |
| Rust output lines | 118,907 |
| Compile pass rate | **44 / 44 (100%)** |
| Runtime tests passing | **268** (185 unit + 83 integration) |
| External dependencies | rusqlite (bundled SQLite — no system install) |
| AI / LLM in the loop | None |

Every program passes `cargo check` with zero errors.

---

## Byte-for-Byte Parity (Beta — Batch Programs)

The `parity/` folder ships a Docker validator that compiles the 7 CardDemo batch programs (CB*) with both GnuCOBOL and the Ironclad-generated Rust, runs both with the same input data, and diffs stdout byte for byte. Reproducing the full byte-for-byte parity claim requires the same DD-name file staging the production CardDemo batch JCL does; the included validator gets you most of the way but a few programs still need additional fixture wiring before they produce green ticks under the included Docker harness alone.

The 30 online (CO*) programs aren't run via GnuCOBOL — they require a CICS environment. The runtime tests below cover them via the embedded CICS runtime.

---

## OpenMainframe Project Cross-Validation

In addition to CardDemo, Ironclad runs against the [Open Mainframe Project's COBOL programming course](https://github.com/openmainframeproject/cobol-programming-course) test programs as an independent third-party check. The results of that sweep:

| Status | Count | Notes |
|---|---|---|
| **MATCH** | **31** | Byte-for-byte stdout + exit-code parity with GnuCOBOL |
| SKIP | 7 | Programs requiring DB2 / EXEC SQL infrastructure |
| COBC_FAIL | 5 | Programs that even GnuCOBOL can't compile (so no reference output to compare against) |
| **In-scope total** | **31 / 31 (100%)** | Every program GnuCOBOL accepts, Ironclad matches byte for byte |

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

## Build & Test

```bash
# Verify all 44 programs compile
cargo check

# Build all binaries
cargo build --release

# Run all 268 tests (185 unit + 83 integration)
cargo test
```

Requires Rust 1.70+ (edition 2021).

---

## Running the Batch Parity Validator (Beta)

The `parity/` folder ships a Docker harness for the 7 batch programs. It streams color-coded results live (green PASS / red MISMATCH / yellow BUILD_FAIL_GNU / cyan TIMEOUT) so you can watch the validator chew through the corpus. Reproducing 100% green-tick parity from this harness alone is still in progress (some programs need extra DD-name fixture wiring), but the plumbing is in place.

```bash
# Build the validator image (one-time, ~3-5 min)
docker build -t carddemo-parity -f parity/Dockerfile.parity .

# Run with color (interactive TTY)
docker run --rm -it carddemo-parity

# Filter to a single program
docker run --rm -it carddemo-parity bash parity/parity_harness.sh --filter CBACT01C
```

Exit codes: `0` = 100% parity, `1` = ≥1 MISMATCH, `2` = build failure, `3` = timeout.

For the full validated **44/44 compile + 268-test** result, see the `cargo check` / `cargo test` commands above — those are the headline numbers that actually pass cleanly today.

---

## Type Mapping

| COBOL | Rust |
|---|---|
| `PIC X(N)` | `FixedString<N>` |
| `PIC 9(N)` | exact-precision Decimal |
| `PIC 9(N) COMP-3` | packed BCD with sign nibble |
| `PIC 9(N) COMP` / `BINARY` | `i32` / `i64` |
| `PIC 9(N)V9(M)` | exact-precision Decimal |
| GROUP items | `struct` |
| `OCCURS N TIMES` | `Vec<T>` / `[T; N]` |
| `88-level` | named boolean condition |
| `REDEFINES` | union-style overlay |
| `FILE STATUS` | `FileStatus` |
| `SQLCA` | `Sqlca` |
| `DFHCOMMAREA` | `CicsContext` |

---

## What Makes This Hard

AWS CardDemo is not a toy benchmark. It exercises real mainframe patterns:

1. **CICS pseudo-conversational programming** — 30 of 44 programs use EXEC CICS with SEND MAP, RECEIVE MAP, RETURN TRANSID, HANDLE CONDITION. State survives across terminal interactions via COMMAREA.
2. **Embedded SQL / DB2** — multiple programs execute dynamic SQL with host variables, cursors, FETCH loops, SQLCA status checks.
3. **BMS screen maps** — online programs send and receive 3270 maps with attribute bytes, cursor positioning, field-level validation.
4. **Mixed batch + online** — both batch (CB*) and CICS (CO*) with shared copybook data structures.
5. **75 copybooks with deep nesting** — multi-level COPY REPLACING, nested groups, REDEFINES chains.
6. **Reference modification** — substring operations like `FIELD(3:5)` requiring byte-level access across type boundaries.
7. **Complex control flow** — PERFORM THRU, GO TO, nested EVALUATE/WHEN, paragraph fall-through, COBOL scoping rules — all into Rust's strict control-flow model.

---

## Production CICS Runtime (Included)

The runtime is not a stub. The repo ships a production-grade CICS environment:

- **VSAM storage** — SQLite-backed keyed storage (KSDS / RRDS / ESDS) with B-tree indexed access, browse cursors (forward + backward), ACID transactions.
- **Program control** — XCTL (transfer, no return), LINK (call + return), RETURN TRANSID + COMMAREA, START / RETRIEVE, HANDLE ABEND.
- **TSQ** — random-access temporary storage queues with item-level read/write/rewrite + NUMITEMS.
- **TDQ** — transient data queues with trigger-level automatic program initiation.
- **SQL / DB2** — real SQL execution against SQLite with host-variable binding, cursors, SQLCA, the CardDemo 7-table schema.
- **BMS Screen Engine** — DFHBMSCA attribute bytes, DFHAID attention identifiers, pluggable screen channels.
- **System services** — ASKTIME, FORMATTIME, ASSIGN, INQUIRE.
- **Transaction loop** — pseudo-conversational dispatcher with session management and COMMAREA chaining.
- **React 3270 terminal UI** — 80×24 monospace grid, protected/unprotected fields with color attributes, full PF1–PF24 + Tab/Backtab/CLEAR keyboard, REST API integration for screen I/O.

268 tests prove all of this works end-to-end.

---

## What Makes This Different

1. **Deterministic.** Same input always produces identical output. No randomness, no neural networks, no LLMs.
2. **Complete type safety.** Every COBOL data item maps to a concrete Rust type. No `unsafe` blocks in transpiler output.
3. **Structural preservation.** COBOL paragraphs map to Rust functions; data hierarchy maps to nested structs. The output reads like the source.
4. **Production runtime.** Not stubs — real SQLite-backed VSAM, real SQL, real BMS screens, real ACID transactions.
5. **Production architecture.** Ironclad handles CICS, DB2, VSAM, BMS maps, batch JCL — the patterns that define real mainframe systems, not textbook exercises.

---

## Related Showcases

| Repository | What it shows |
|---|---|
| [Ironclad-COBOL-to-Rust](https://github.com/mrm413/Ironclad-COBOL-to-Rust) | GnuCOBOL 3.2 in-scope test corpus — 835 / 835 byte-for-byte parity (100%) |
| [cms-medicare-ironclad-showcase](https://github.com/mrm413/cms-medicare-ironclad-showcase) | 17 years of CMS Medicare pricers (FY2005–FY2021) — byte-for-byte parity across SNF / ESRD / Hospice / Home Health / IPF / IRF / LTCH families |
| [lazarus-carddemo-showcase](https://github.com/mrm413/lazarus-carddemo-showcase) | C++17 sibling: same 44 CardDemo programs transpiled to hardened C++17 |

---

## Built By

**Torsova LLC** — Deterministic mainframe modernization. No AI. No guesswork. Compiler-grade transpilation.

- **Ironclad** — COBOL to Rust
- **Lazarus** — COBOL to C++17

---

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

The original CardDemo application is provided by AWS under the [MIT-0 License](https://github.com/aws-samples/aws-mainframe-modernization-carddemo/blob/main/LICENSE).

All modifications and additions — including the Rust transpiled programs, the CICS runtime library, the React 3270 terminal UI, the build system, and the test suite — are Copyright 2025–2026 Michael R. Mull / Torsova LLC. See [NOTICE](NOTICE) for details.
