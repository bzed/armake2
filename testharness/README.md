# armake2 DayZ test harness

End-to-end test for armake2 against the current DayZ server (1.29 stable and
1.30 experimental): builds a small test mod, binarizes its config, packs it
into a PBO, signs it, and boots a real DayZ server with the mod loaded to
prove the PBO loads, the scripts compile and the code runs.

## What it does

1. `cargo build --release` (or uses `$ARMAKE2_BIN`)
2. `armake2 keygen` - generates `.biprivatekey` / `.bikey`
3. `armake2 build` - preprocesses, rapifies `config.cpp` to `config.bin`
   (binarization), packs the addon into a PBO with `prefix=` header from the
   `$PREFIX$` file
4. `armake2 sign` - V3 signature (`.bisign`)
5. `armake2 verify` - checks the signature against the public key
6. `armake2 unpack` round-trip check of the packed files
7. Boots the local DayZ server (Steam app 223350, and app 1042420 with
   `--experimental`/`--both`) twice from a disposable symlink tree:
   vanilla baseline, then with `-servermod=@armake2_test`
8. Fails unless the Mission script module grows by exactly one file, the
   test mod's `Print` marker appears in `script_*.log`, there are no script
   compile errors for the mod, and no signature errors in the RPT

## Usage

```sh
testharness/run.sh               # build + PBO checks + stable server test
testharness/run.sh --both        # also boot the experimental 1.30 server
testharness/run.sh --no-server   # build + PBO/signature checks only
testharness/run.sh --keep        # keep testharness/build for inspection
```

Requirements: Rust toolchain, a Steam installation with DayZ Server
(app 223350) for the server test, `python3` for port picking (optional).

The harness never writes into the Steam install: it builds its own server
tree of symlinks in `testharness/build/server-tree-*`, picks random free
game/query/RCon ports, and cleans up after itself.

## Test mod

`mod/addons/armake2_test/` is a minimal script-only DayZ mod: a
`config.cpp` (`CfgPatches` + `CfgMods` with a `missionScriptModule`), a
`$PREFIX$` file and one Enforce script that overrides
`MissionServer.OnMissionStart()` to print a marker. It uses only APIs
present in both 1.29 and 1.30, so the same PBO loads on both versions.

## Notes

- On 1.30 experimental the server ends with a vanilla shutdown assertion
  (`Leaked 'BunkerBroadcastManager'`); that is vanilla noise, not caused by
  the test mod, and does not fail the harness.
- The engine logs `Can't load @armake2_test/Anims/cfg/skeletons.anim.xml`
  style lines while probing each loaded mod folder for animation configs;
  expected for a script-only mod.
