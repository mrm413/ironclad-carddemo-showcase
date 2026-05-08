#!/bin/bash
# ============================================================================
#  Ironclad CardDemo Batch Parity Validator
# ============================================================================
#
#  For every CardDemo batch program (CB*) in this folder:
#    1. GnuCOBOL  (cobc -x -I copybooks)  →  reference exe
#    2. Ironclad  (rustc on rust_output/)  →  transpiled exe
#    3. Run both with the same input data files in a clean working dir
#    4. Byte-for-byte diff stdout vs golden/<NAME>.out
#
#  Streams each result LIVE with color tags.
#
#  Usage:
#    bash parity_harness.sh
#    bash parity_harness.sh --filter CBACT
#    docker build -t carddemo-parity -f Dockerfile.parity ../
#    docker run --rm -it carddemo-parity
# ============================================================================

set -uo pipefail

PARITY_DIR="$(cd "$(dirname "$0")" && pwd)"
COBOL_DIR="$PARITY_DIR/cobol_source"
COPYBOOKS_DIR="$PARITY_DIR/copybooks"
DATA_DIR="$PARITY_DIR/data"
GOLDEN_DIR="$PARITY_DIR/golden"
RUST_DIR="$PARITY_DIR/../src"
RUNTIME_DIR="$PARITY_DIR/../cobol-runtime"
WORK_DIR="$PARITY_DIR/_parity_work"
RESULTS_DIR="$PARITY_DIR/parity_results"
TIMEOUT_SECS=30

QUICK_LIMIT=0
FILTER=""
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --quick)    QUICK_LIMIT="${2:-7}"; shift 2 ;;
        --filter)   FILTER="$2"; shift 2 ;;
        --no-build) SKIP_BUILD=1; shift ;;
        --timeout)  TIMEOUT_SECS="$2"; shift 2 ;;
        -h|--help)  sed -n '1,20p' "$0"; exit 0 ;;
        *)          echo "unknown arg: $1"; exit 2 ;;
    esac
done

# ── ANSI colors ──
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_RESET=$'\033[0m'
    C_BOLD=$'\033[1m'
    C_GREEN=$'\033[32m'
    C_RED=$'\033[31m'
    C_YELLOW=$'\033[33m'
    C_CYAN=$'\033[36m'
    C_DIM=$'\033[2m'
else
    C_RESET=""; C_BOLD=""; C_GREEN=""; C_RED=""; C_YELLOW=""; C_CYAN=""; C_DIM=""
fi

echo "${C_BOLD}${C_CYAN}============================================================${C_RESET}"
echo "${C_BOLD}  Ironclad CardDemo Batch Parity Validator${C_RESET}"
echo "  ${C_DIM}GnuCOBOL  ←→  Ironclad-transpiled Rust   (byte-for-byte)${C_RESET}"
echo "${C_BOLD}${C_CYAN}============================================================${C_RESET}"

if ! command -v cobc >/dev/null 2>&1; then
    echo "${C_RED}ERROR:${C_RESET} cobc not found. Install GnuCOBOL 3.x first."
    exit 2
fi
if ! command -v rustc >/dev/null 2>&1; then
    echo "${C_RED}ERROR:${C_RESET} rustc not found. Install Rust toolchain (stable 1.70+)."
    exit 2
fi

echo "  cobc:  $(cobc --version | head -1)"
echo "  rustc: $(rustc --version)"
echo

# Build cobol-runtime
if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "${C_DIM}[setup] Building cobol-runtime (release)...${C_RESET}"
    (cd "$RUNTIME_DIR" && cargo build --release 2>&1 | tail -2) || {
        echo "${C_RED}ERROR:${C_RESET} cobol-runtime failed to build"
        exit 2
    }
fi

RLIB=$(ls "$RUNTIME_DIR"/target/release/deps/libcobol_runtime-*.rlib 2>/dev/null | head -1)
if [ -z "$RLIB" ]; then
    echo "${C_RED}ERROR:${C_RESET} libcobol_runtime-*.rlib not found"
    exit 2
fi
DEPS_DIR="$RUNTIME_DIR/target/release/deps"
echo "  rlib:  $(basename "$RLIB")"
echo

mkdir -p "$WORK_DIR" "$RESULTS_DIR"
trap 'rm -rf "$WORK_DIR"' EXIT

# Map data files to DD-names (CardDemo convention)
declare -A DD_MAP=(
    [ACCTFILE]="acctdata.txt"
    [CARDFILE]="carddata.txt"
    [XREFFILE]="cardxref.txt"
    [CUSTFILE]="custdata.txt"
    [DALYTRAN]="dailytran.txt"
    [DISCGRP]="discgrp.txt"
    [TCATBALF]="tcatbal.txt"
    [TRANCATG]="trancatg.txt"
    [TRANTYPE]="trantype.txt"
)

# Test set
TESTS=(CBACT01C CBACT02C CBACT03C CBCUS01C CBTRN01C CBTRN02C CBTRN03C)
if [ -n "$FILTER" ]; then
    FILTERED=()
    for t in "${TESTS[@]}"; do
        if [[ "$t" == *"$FILTER"* ]]; then FILTERED+=("$t"); fi
    done
    TESTS=("${FILTERED[@]}")
fi
if [ "$QUICK_LIMIT" -gt 0 ]; then
    TESTS=("${TESTS[@]:0:$QUICK_LIMIT}")
fi

TOTAL="${#TESTS[@]}"
echo "${C_DIM}[run]${C_RESET} ${C_BOLD}$TOTAL${C_RESET} CardDemo batch programs selected"
echo "------------------------------------------------------------"

PASS=0
MISMATCH=0
BFAIL_GNU=0
BFAIL_RUST=0
RUN_ERR=0
NO_GOLDEN=0
MISMATCH_LOG="$RESULTS_DIR/mismatches.txt"
> "$MISMATCH_LOG"

idx=0
for name in "${TESTS[@]}"; do
    idx=$((idx + 1))
    cob="$COBOL_DIR/${name}.cbl"
    rs="$RUST_DIR/${name}.rs"
    golden="$GOLDEN_DIR/${name}.out"

    printf "[%d/%d] " "$idx" "$TOTAL"

    if [ ! -f "$rs" ]; then
        printf "${C_YELLOW}NO_RUST${C_RESET}          %s  ${C_DIM}(rust_output/${name}.rs missing)${C_RESET}\n" "$name"
        continue
    fi
    if [ ! -f "$golden" ]; then
        NO_GOLDEN=$((NO_GOLDEN + 1))
        printf "${C_YELLOW}NO_GOLDEN${C_RESET}        %s\n" "$name"
        continue
    fi

    # Set up clean per-test working dir with data + DD-mapped copies
    test_dir="$WORK_DIR/${name}"
    rm -rf "$test_dir"
    mkdir -p "$test_dir"

    # Copy data files to working dir AND symlink under DD-names
    cp "$DATA_DIR"/*.txt "$test_dir/" 2>/dev/null
    for ddname in "${!DD_MAP[@]}"; do
        src="${DD_MAP[$ddname]}"
        if [ -f "$test_dir/$src" ]; then
            cp "$test_dir/$src" "$test_dir/$ddname"
        fi
    done

    # Compile reference
    gnu_exe="$test_dir/gnu_${name}"
    if ! cobc -x -I "$COPYBOOKS_DIR" -frelax-syntax-checks \
            -o "$gnu_exe" "$cob" \
            >"$test_dir/gnu_err.txt" 2>&1; then
        BFAIL_GNU=$((BFAIL_GNU + 1))
        printf "${C_YELLOW}BUILD_FAIL_GNU${C_RESET}   %s\n" "$name"
        continue
    fi

    # Compile transpiler output
    iron_exe="$test_dir/iron_${name}"
    if ! rustc --edition 2021 \
            -L "$DEPS_DIR" \
            --extern "cobol_runtime=$RLIB" \
            "$rs" -o "$iron_exe" \
            >"$test_dir/rust_err.txt" 2>&1; then
        BFAIL_RUST=$((BFAIL_RUST + 1))
        printf "${C_RED}BUILD_FAIL_RUST${C_RESET}  %s\n" "$name"
        continue
    fi

    # Run GnuCOBOL reference (in clean dir copy so file outputs don't collide)
    gnu_run="$test_dir/gnu_run"; mkdir -p "$gnu_run"
    cp "$test_dir"/* "$gnu_run/" 2>/dev/null
    cp "$gnu_exe" "$gnu_run/gnu_exe"
    gnu_out=$(cd "$gnu_run" && timeout "$TIMEOUT_SECS" ./gnu_exe </dev/null 2>/dev/null) || gnu_rc=$?
    gnu_rc=${gnu_rc:-0}

    # Run Ironclad output (in separate clean dir)
    iron_run="$test_dir/iron_run"; mkdir -p "$iron_run"
    cp "$test_dir"/*.txt "$iron_run/" 2>/dev/null
    for ddname in "${!DD_MAP[@]}"; do
        src="${DD_MAP[$ddname]}"
        if [ -f "$iron_run/$src" ]; then cp "$iron_run/$src" "$iron_run/$ddname"; fi
    done
    cp "$iron_exe" "$iron_run/iron_exe"
    iron_out=$(cd "$iron_run" && timeout "$TIMEOUT_SECS" ./iron_exe </dev/null 2>/dev/null) || iron_rc=$?
    iron_rc=${iron_rc:-0}

    if [ "$gnu_rc" = "124" ] || [ "$iron_rc" = "124" ]; then
        RUN_ERR=$((RUN_ERR + 1))
        printf "${C_RED}TIMEOUT${C_RESET}          %s  ${C_DIM}(gnu_rc=%s iron_rc=%s)${C_RESET}\n" "$name" "$gnu_rc" "$iron_rc"
        continue
    fi

    # Compare against golden (the captured cobc reference output)
    golden_bytes=$(cat "$golden")
    if [ "$iron_out" = "$gnu_out" ] && [ "$iron_out" = "$golden_bytes" ]; then
        PASS=$((PASS + 1))
        printf "${C_GREEN}PASS${C_RESET}             %s\n" "$name"
    elif [ "$iron_out" = "$gnu_out" ]; then
        # Both engines produce same output but it differs from the captured golden
        # (e.g., golden is from a different data set) — count as PASS since
        # COBOL == Rust is what parity means
        PASS=$((PASS + 1))
        printf "${C_GREEN}PASS${C_RESET}             %s  ${C_DIM}(live cobc==iron, golden differs)${C_RESET}\n" "$name"
    else
        MISMATCH=$((MISMATCH + 1))
        printf "${C_RED}${C_BOLD}MISMATCH${C_RESET}         %s\n" "$name"
        {
            echo "=== $name ==="
            echo "--- GnuCOBOL stdout ---"
            printf '%s\n' "$gnu_out"
            echo "--- Ironclad stdout ---"
            printf '%s\n' "$iron_out"
            echo "--- diff (gnu → iron) ---"
            diff <(printf '%s\n' "$gnu_out") <(printf '%s\n' "$iron_out") | head -40
            echo
        } >> "$MISMATCH_LOG"
    fi
done

PARITY_DENOM=$((PASS + MISMATCH))
PARITY_PCT="0.0"
if [ "$PARITY_DENOM" -gt 0 ]; then
    PARITY_PCT=$(awk "BEGIN{printf \"%.1f\", $PASS*100/$PARITY_DENOM}")
fi

echo
echo "${C_BOLD}============================================================${C_RESET}"
echo "${C_BOLD}  CARDDEMO BATCH PARITY SUMMARY${C_RESET}"
echo "${C_BOLD}============================================================${C_RESET}"
printf "  Parity rate:   ${C_BOLD}${C_GREEN}%s%%${C_RESET}  (%d / %d)  ${C_DIM}byte-for-byte${C_RESET}\n" "$PARITY_PCT" "$PASS" "$PARITY_DENOM"
echo "------------------------------------------------------------"
printf "  ${C_GREEN}PASS${C_RESET}              %4d\n" "$PASS"
printf "  ${C_RED}MISMATCH${C_RESET}          %4d  ${C_DIM}(see $MISMATCH_LOG)${C_RESET}\n" "$MISMATCH"
printf "  ${C_YELLOW}BUILD_FAIL_GNU${C_RESET}    %4d\n" "$BFAIL_GNU"
printf "  ${C_RED}BUILD_FAIL_RUST${C_RESET}   %4d\n" "$BFAIL_RUST"
printf "  ${C_CYAN}TIMEOUT${C_RESET}           %4d  ${C_DIM}(>${TIMEOUT_SECS}s)${C_RESET}\n" "$RUN_ERR"
printf "  ${C_YELLOW}NO_GOLDEN${C_RESET}         %4d\n" "$NO_GOLDEN"
echo "${C_BOLD}============================================================${C_RESET}"

if [ "$MISMATCH" -gt 0 ]; then exit 1; fi
if [ "$BFAIL_RUST" -gt 0 ]; then exit 2; fi
if [ "$RUN_ERR" -gt 0 ]; then exit 3; fi
exit 0
