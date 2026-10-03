#!/usr/bin/env bash
# armake2 DayZ test harness.
#
# Builds the armake2 test mod, binarizes it (rapify), packs it into a PBO,
# signs it, verifies the signature, then boots a local DayZ server with the
# PBO loaded and checks the script logs for proof that the mod loaded,
# compiled and ran.
#
# Usage: testharness/run.sh [--no-server] [--experimental] [--both] [--keep]
#
# Environment:
#   ARMAKE2_BIN       use this armake2 binary instead of building one
#   DAYZ_SERVER_DIR   DayZ server install to use (default: auto-detected)
#
# Exits 0 if every step passed.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$ROOT/testharness"
BUILD="$HARNESS/build"
MODNAME="armake2_test"
MARKER="[armake2_test] scripts loaded, compiled and running"

RUN_SERVER=1
RUN_EXPERIMENTAL=0
KEEP=0
for arg in "$@"; do
    case "$arg" in
        --no-server) RUN_SERVER=0 ;;
        --experimental) RUN_EXPERIMENTAL=1 ;;
        --both) RUN_EXPERIMENTAL=1 ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "unknown option: $arg (see --help)" >&2; exit 1 ;;
    esac
done

say()  { printf '\n== %s\n' "$*"; }
pass() { printf '   PASS: %s\n' "$*"; }
fail() { printf '   FAIL: %s\n' "$*"; FAILED=1; }
FAILED=0

# ---------------------------------------------------------------------------
# 1. armake2 binary
# ---------------------------------------------------------------------------
if [[ -n "${ARMAKE2_BIN:-}" ]]; then
    ARMAKE2="$ARMAKE2_BIN"
else
    say "Building armake2 (cargo build --release)"
    (cd "$ROOT" && cargo build --release) >&2
    ARMAKE2="$ROOT/target/release/armake2"
fi
[[ -x "$ARMAKE2" ]] || { echo "armake2 binary not found: $ARMAKE2" >&2; exit 1; }
pass "armake2 available: $ARMAKE2 ($("$ARMAKE2" --version))"

# ---------------------------------------------------------------------------
# 2. Build the test mod
# ---------------------------------------------------------------------------
say "Building test mod $MODNAME"
rm -rf "$BUILD"
mkdir -p "$BUILD/keys" "$BUILD/@$MODNAME/addons"

# staging copy of the addon source (packing must not pick up build output)
cp -r "$HARNESS/mod/addons/$MODNAME" "$BUILD/modsrc"
cp "$HARNESS/mod/mod.cpp" "$BUILD/@$MODNAME/mod.cpp"

"$ARMAKE2" keygen -f "$BUILD/keys/$MODNAME"

# build = preprocess + rapify (binarize) config.cpp -> config.bin, then pack
"$ARMAKE2" build -f -e "product=dayz" \
    "$BUILD/modsrc" "$BUILD/@$MODNAME/addons/$MODNAME.pbo"

# sign with the key generated above (v3 signature)
"$ARMAKE2" sign -f "$BUILD/keys/$MODNAME.biprivatekey" \
    "$BUILD/@$MODNAME/addons/$MODNAME.pbo"

PBO="$BUILD/@$MODNAME/addons/$MODNAME.pbo"
SIG="$PBO.$MODNAME.bisign"
[[ -f "$PBO" && -f "$SIG" ]] && pass "PBO built and signed: $PBO" || fail "PBO or bisign missing"

# ---------------------------------------------------------------------------
# 3. Check the PBO
# ---------------------------------------------------------------------------
say "Checking PBO contents"
INSPECT="$("$ARMAKE2" inspect "$PBO")"
echo "$INSPECT"
echo "$INSPECT" | grep -q "prefix=$MODNAME" \
    && pass "PBO prefix header is $MODNAME" \
    || fail "PBO prefix header missing/wrong"
echo "$INSPECT" | grep -q "config.bin" \
    && pass "config.cpp binarized to config.bin" \
    || fail "config.bin not in PBO (binarization failed)"
echo "$INSPECT" | grep -q "scripts\\\\5_Mission\\\\$MODNAME.c" \
    && pass "script packed: scripts\\5_Mission\\$MODNAME.c" \
    || fail "script file missing from PBO"

say "Verifying signature"
"$ARMAKE2" verify "$BUILD/keys/$MODNAME.bikey" "$PBO" "$SIG" \
    && pass "signature verifies against public key" \
    || fail "signature verification failed"

say "Unpack round-trip"
"$ARMAKE2" unpack -f "$PBO" "$BUILD/unpacked"
cmp -s "$BUILD/modsrc/scripts/5_Mission/$MODNAME.c" \
        "$BUILD/unpacked/scripts/5_Mission/$MODNAME.c" \
    && pass "script file survived pack/unpack unchanged" \
    || fail "script file corrupted by pack/unpack"
[[ -f "$BUILD/unpacked/config.bin" ]] \
    && pass "config.bin present after unpack" \
    || fail "config.bin lost"

if [[ "$RUN_SERVER" -ne 1 ]]; then
    say "Skipping server test (--no-server)"
    if [[ "$FAILED" -ne 0 ]]; then
        echo "FAILED. Build output kept in $BUILD"
        exit 1
    fi
    echo "ALL PASSED."
    [[ "$KEEP" -ne 1 ]] && rm -rf "$BUILD"
    exit 0
fi

# ---------------------------------------------------------------------------
# 4. Find the DayZ server install
# ---------------------------------------------------------------------------
find_server() {  # $1: app id
    local appid=$1 root rootpath vdf installdir lib
    for root in "${DAYZ_SERVER_DIR:-}" "${STEAM_ROOT:-}" \
                "$HOME/.steam/debian-installation" "$HOME/.steam/steam" \
                "$HOME/.local/share/Steam"; do
        [[ -z "$root" || ! -d "$root" ]] && continue
        for vdf in "$root"/steamapps/libraryfolders.vdf "$root"/libraryfolders.vdf; do
            [[ -f "$vdf" ]] || continue
            while read -r lib; do
                [[ -d "$lib/steamapps" ]] && [[ -f "$lib/steamapps/appmanifest_$appid.acf" ]] || continue
                installdir=$(grep -m1 installdir "$lib/steamapps/appmanifest_$appid.acf" \
                             | sed 's/.*"\(.*\)".*/\1/')
                echo "$lib/steamapps/common/$installdir"
                return 0
            done < <(grep '"path"' "$vdf" | sed 's/.*"\(.*\)".*/\1/')
        done
    done
    return 1
}

free_ports() {  # prints 3 free ports (game, query, rcon)
    if command -v python3 >/dev/null 2>&1; then
        python3 - <<'EOF' || echo $((24000 + RANDOM % 20000)) $((24500 + RANDOM % 20000)) $((25000 + RANDOM % 20000))
import random, socket

ports = []
while len(ports) < 3:
    p = random.randrange(2310, 61000)
    if p in ports or (2300 <= p <= 2310):
        continue
    ok = True
    for t in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
        s = socket.socket(socket.AF_INET, t)
        try:
            s.bind(("", p))
        except OSError:
            ok = False
        finally:
            s.close()
    if ok:
        ports.append(p)
print(" ".join(map(str, ports)))
EOF
    else
        echo $((24000 + RANDOM % 20000)) $((24500 + RANDOM % 20000)) $((25000 + RANDOM % 20000))
    fi
}

make_tree() {  # $1: tree dir, $2: steam server dir
    local tree=$1 src=$2 entry mission file
    mkdir -p "$tree"
    for entry in "$src"/*; do
        case "$(basename "$entry")" in
            mpmissions|profiles|keys|battleye|serverDZ.cfg) continue ;;
        esac
        ln -sfn "$entry" "$tree/$(basename "$entry")"
    done
    cp "$src/serverDZ.cfg" "$tree/serverDZ.cfg"
    mkdir -p "$tree/mpmissions" "$tree/profiles/battleye" "$tree/keys" "$tree/battleye"
    for entry in "$src/battleye"/*; do
        [[ -e "$entry" ]] && ln -sfn "$entry" "$tree/battleye/$(basename "$entry")"
    done
    for mission in "$src/mpmissions"/*/; do
        [[ -d "$mission" ]] || continue
        mkdir -p "$tree/mpmissions/$(basename "$mission")"
        for file in "$mission"/*; do
            [[ -e "$file" ]] && ln -sfn "$file" \
                "$tree/mpmissions/$(basename "$mission")/$(basename "$file")"
        done
    done
}

boot_server() {  # $1: tree, $2: game port, $3: query port, $4: rcon port, $5: modarg ("" for none)
    local tree=$1 game=$2 query=$3 rcon=$4 modarg=$5
    grep -q '^steamQueryPort' "$tree/serverDZ.cfg" \
        || sed -i '1i steamQueryPort = 0;' "$tree/serverDZ.cfg"
    sed -i "s/^steamQueryPort = .*/steamQueryPort = $query;/" "$tree/serverDZ.cfg"
    rm -f "$tree"/profiles/beserver_x64_active_*.cfg
    printf 'RConPassword %s\nRestrictRCon 0\nRConPort %s\n' \
        "$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c10)" "$rcon" \
        > "$tree/profiles/battleye/beserver_x64.cfg"

    rm -f "$tree"/profiles/script_*.log "$tree"/profiles/*.RPT
    local args=(-config=serverDZ.cfg -profiles=profiles -port=$game -nosplash -nopause -dologs)
    [[ -n "$modarg" ]] && args+=("$modarg")
    (
        cd "$tree" && ulimit -c 0
        timeout -k 15 60 ./DayZServer "${args[@]}" >/dev/null 2>&1 || true
    )
    rm -f "$tree"/core.* "$tree"/profiles/*.mdmp
    ls -t "$tree"/profiles/script_*.log 2>/dev/null | head -1
}

mission_files() {  # $1: script log -> Mission module file count
    grep -m1 'Module: Mission; loaded' "$1" 2>/dev/null \
        | sed 's/.*loaded \([0-9]*\)x files.*/\1/'
}

run_server_tests() {  # $1: label, $2: server dir
    local label=$1 src=$2 tree baseline modlog basecount modcount
    say "Server test [$label]: $(grep -m1 Version "$src" 2>/dev/null || echo "$src")"

    tree="$BUILD/server-tree-$label"
    make_tree "$tree" "$src"
    ln -sfn "$BUILD/@$MODNAME" "$tree/@$MODNAME"
    cp "$BUILD/keys/$MODNAME.bikey" "$tree/keys/"

    read -r GAME QUERY RCON < <(free_ports)

    baseline=$(boot_server "$tree" "$GAME" "$QUERY" "$RCON" "")
    [[ -n "$baseline" ]] || { fail "[$label] vanilla server produced no script log"; return; }
    basecount=$(mission_files "$baseline")
    [[ -n "$basecount" ]] && pass "[$label] vanilla baseline: Mission module $basecount files" \
                            || fail "[$label] vanilla baseline: no Mission module line"

    modlog=$(boot_server "$tree" "$GAME" "$QUERY" "$RCON" "-servermod=@$MODNAME")
    [[ -n "$modlog" ]] || { fail "[$label] server with mod produced no script log"; return; }
    modcount=$(mission_files "$modlog")

    [[ -n "$modcount" && "$modcount" -eq $((basecount + 1)) ]] \
        && pass "[$label] Mission module grew by exactly one file ($basecount -> $modcount)" \
        || fail "[$label] Mission module file count wrong: baseline=$basecount modded=${modcount:-none}"

    grep -Fq "$MARKER" "$modlog" \
        && pass "[$label] test script ran (marker found in script log)" \
        || fail "[$label] marker not found: scripts did not load or run"

    local errs
    errs=$(grep 'SCRIPT.*(E)' "$modlog" | grep -i "$MODNAME" || true)
    [[ -z "$errs" ]] && pass "[$label] no script compile errors for $MODNAME" \
                     || { fail "[$label] compile errors:"; echo "$errs"; }

    local sig
    sig=$(grep -i 'signature' "$tree"/profiles/*.RPT 2>/dev/null | grep -vi 'verifysignatures' || true)
    [[ -z "$sig" ]] && pass "[$label] no signature errors in RPT" \
                     || { fail "[$label] signature errors in RPT:"; echo "$sig"; }
}

STABLE_DIR=$(find_server 223350 || true)
if [[ -n "$STABLE_DIR" ]]; then
    run_server_tests "stable-129" "$STABLE_DIR"
else
    say "Skipping stable server test (DayZ Server app 223350 not found)"
    [[ "$RUN_EXPERIMENTAL" -ne 1 ]] && { echo "Install the DayZ Server via Steam (app 223350) to run server tests." >&2; exit 1; }
fi

if [[ "$RUN_EXPERIMENTAL" -eq 1 ]]; then
    EXP_DIR=$(find_server 1042420 || true)
    if [[ -n "$EXP_DIR" ]]; then
        run_server_tests "experimental-130" "$EXP_DIR"
    else
        say "Skipping experimental server test (DayZ Server Exp app 1042420 not found)"
    fi
fi

# ---------------------------------------------------------------------------
say "Result"
if [[ "$FAILED" -ne 0 ]]; then
    echo "FAILED. Build output kept in $BUILD"
    exit 1
fi
echo "ALL PASSED."
[[ "$KEEP" -ne 1 ]] && rm -rf "$BUILD"
exit 0
