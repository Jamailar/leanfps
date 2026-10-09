# LeanFPS

Tiny game performance optimizer for Windows 10/11, written in Rust. One ~6 MB executable,
no installer, no .NET, no WebView.

Plan and decisions: `docs/plans/2026-10-09-leanfps-plan.md` (Chinese). Competitor teardown:
`docs/plans/2026-10-09-game-optimizer-research.md`.

> This folder lives in the Sites repo temporarily. It moves to its own private repo
> (`Jamailar/leanfps`) with full history once that repo exists:
> `git subtree split --prefix apps/leanfps -b leanfps-split` and push that branch.

## What it does

- **System tweaks** (persistent): power plan, core parking, Game Mode, background recording,
  plus advanced opt-ins (HAGS, MPO). The real previous value is saved before every change and
  restored on undo, one tweak at a time or all at once.
- **While gaming** (session): when a game is in the foreground LeanFPS raises its priority,
  opts it out of power throttling, keeps the PC awake and lowers background apps. Everything
  is undone when the game exits, and after a crash on the next start.
- **Game profiles**: the game is identified (built-in database, Steam and Epic libraries,
  or fullscreen fallback), its icon is shown, and genre-specific tweaks run (e.g. mouse
  acceleration off for shooters, standby-memory purge for open-world games).
- Never: Defender, HVCI/VBS, firewall, Windows Update, CPU mitigations, game injection or
  overlays.

## Layout

```
data/tweaks.toml      tweak catalog (what, why, risk, sources, actions)
data/games.toml       known games: exe -> name, genre; tips per genre
src/catalog.rs        catalog types + validation
src/engine.rs         apply / revert with a journal of real prior values
src/session.rs        per-game session tweaks, crash recovery
src/watcher.rs        background thread: foreground game detection every 2 s
src/games/            game database, Steam/Epic library scan, classification
src/platform/         Platform trait; win.rs = Win32, mock.rs = tests and Linux dev
src/ui.rs             egui window (380 x 640)
src/selftest.rs       `--selftest`: apply/verify/revert everything on a real system
```

## Build and test

```bash
cargo test                      # any OS, uses the mock platform
cargo build --release           # on Windows: target/release/leanfps.exe
# cross-compile from Linux (needs mingw-w64 and `rustup target add x86_64-pc-windows-gnu`)
cargo build --release --target x86_64-pc-windows-gnu
```

On Linux `cargo run` opens the window against a demo mock (CS2 "running") for UI work.
Data goes to `%ProgramData%\LeanFPS` (override with `LEANFPS_DATA_DIR`).

Command line:

```
leanfps                 open the window
leanfps --restore-all   undo every change, then exit (used by uninstall)
leanfps --selftest      apply, verify and undo every tweak; writes leanfps-selftest.log
```

`--selftest` changes real system settings. Run it only on a throwaway machine; CI runs it on
the GitHub Windows runner (`.github/workflows/leanfps.yml`), which also enforces an 8 MB
size budget.

## Game database

Executable names in `data/games.toml` come from public sources and still need checking on real
installs. A wrong entry only means the game is detected through the Steam/Epic library or the
fullscreen fallback instead.

## Status

See the milestone table in the plan. Done: engine, catalog, detection, sessions, window,
selftest, CI. Next: tray + autostart, frametime measurement (ETW), license and purchase flow
via the shared gateway, website, code signing.
