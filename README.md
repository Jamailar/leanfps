# LeanFPS

Tiny game performance optimizer for Windows 10/11, written in Rust. One ~6 MB executable,
no installer, no .NET, no WebView.

How it compares with other optimizers: `docs/comparison.md` (Chinese).

## What it does

- **System tweaks** (persistent): power plan, core parking, Game Mode, background recording,
  Windows 11 optimizations for windowed games, plus advanced opt-ins (HAGS, MPO). The real previous value is saved before every change and
  restored on undo, one tweak at a time or all at once.
- **While gaming** (session): when a game is in the foreground LeanFPS raises its priority,
  opts it out of power throttling, keeps the PC awake and lowers background apps. On Ryzen
  X3D CPUs with two dies the game runs on the V-Cache die (Windows CPU sets). Everything
  is undone when the game exits, and after a crash on the next start.
- **Game profiles**: the game is identified (built-in database, Steam and Epic libraries,
  or fullscreen fallback), its icon is shown, and genre-specific tweaks run (e.g. mouse
  acceleration off for shooters, standby-memory purge for open-world games).
- Never: Defender, HVCI/VBS, firewall, Windows Update, CPU mitigations, game injection or
  overlays.

## Safety rules

- **Record first, then modify.** Every change reads the current value and writes it to the
  undo journal on disk (flushed) before touching the system. If the journal can't be saved,
  nothing is changed. If a change fails half-way, what was done is undone.
- **Nothing is forgotten.** A value that can't be restored stays in the journal (or
  `session.json`) and is retried by the next start or **Restore all**. A journal file that
  can't be read is moved aside as `*.corrupt-<time>`, never overwritten.
- **Exact restore.** Registry values of any type are restored byte for byte.
- **No files, no drives.** LeanFPS never deletes or edits user files. It only writes its own
  JSON files in its data folder. No disk cleanup, no driver or service changes.
- **No freezes.** Priority goes up to High at most, never Realtime. Windows processes
  (`dwm.exe`, `csrss.exe`, `audiodg.exe`, `explorer.exe`, ...) are never slowed down; the
  catalog is rejected if it lists one. Mouse changes live in memory only, so a reboot always
  resets them.

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
src/ui.rs             egui window (380 x 660), light and dark themes from the icon colors
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

## Build on your own Windows PC

1. Install **Visual Studio Build Tools 2022** with the workload "Desktop development with C++"
   (MSVC compiler + Windows SDK).
2. Install Rust from https://rustup.rs (default toolchain `stable-x86_64-pc-windows-msvc`).
3. In a new terminal:

   ```powershell
   git clone https://github.com/Jamailar/leanfps
   cd leanfps
   cargo test
   cargo build --release
   ```

   The result is `target\release\leanfps.exe` (single file, about 6.5 MB). The first build
   downloads dependencies and takes a few minutes.
4. Double-click `leanfps.exe`. Windows asks for administrator rights (needed for power plans
   and process priority). The exe is not code-signed yet, so SmartScreen may show
   "Windows protected your PC": choose *More info* > *Run anyway*.

Or install it: CI also builds `LeanFPS-Setup-<version>.exe` from `installer/leanfps.iss`
(Inno Setup). It installs to Program Files, adds a Start menu entry and a desktop shortcut,
and its uninstaller runs `--restore-all` before removing the program.

If the window does not open, LeanFPS shows the reason and writes it to
`C:\ProgramData\LeanFPS\leanfps.log`. It draws with OpenGL and falls back to Direct3D 12
(GPU or Windows' software renderer) on PCs without a usable OpenGL driver, such as virtual
machines.

Undo everything at any time with the **Restore all** button or `leanfps.exe --restore-all`.
Settings and the undo journal are in `C:\ProgramData\LeanFPS`. Do not run `--selftest` on your
own PC: it is meant for throwaway test machines and refuses to run unless `CI=true` or
`LEANFPS_SELFTEST=1` is set.

Command line:

```
leanfps                 open the window
leanfps --restore-all   undo every change, then exit (used by uninstall)
leanfps --selftest      apply, verify and undo every tweak; writes leanfps-selftest.log
```

`--selftest` changes real system settings. Run it only on a throwaway machine; CI runs it on
the GitHub Windows runner (`.github/workflows/ci.yml`), which also enforces a 12 MB
size budget.

## Game database

Executable names in `data/games.toml` come from public sources and still need checking on real
installs. A wrong entry only means the game is detected through the Steam/Epic library or the
fullscreen fallback instead.

## Status

Done: engine, catalog, detection, sessions, window,
selftest, CI, Pro license key activation. Next: frametime measurement (ETW), code signing.

## Pro license

Pro is a one-time purchase on https://leanfps.com/pro/. The buyer gets a key by email and
enters it with **Enter key** (footer) or *Settings > License*. `src/license.rs` activates it
through the license server (`https://api.hyperchaos.dev/leanfps/v1/licenses/...`) and saves the signed file as `license.json` in
the data folder. Every start verifies it offline (Ed25519, public keys in `PUBLIC_KEYS`);
a background refresh runs when the file asks for one. Network errors never remove the file;
only a definite refusal from the server (refunded, device removed) does.
`tools/license_fixture.mjs` regenerates the signed test fixture.

## Icons

Sources are SVG in `res/icon/` (`app.svg` large, `app-small.svg` for 16-32 px, `tray-idle.svg`,
`tray-active.svg`). `res/icon/build-icons.sh` regenerates `res/leanfps.ico`,
`res/tray-*.ico` and `res/window-64.rgba`. Preview: `docs/icons-preview.png`.

The window uses Inter (SIL Open Font License, `res/fonts/OFL.txt`), subset to Latin to keep
the exe small. Window preview in both themes: `docs/ui-preview.png`.

## License

MIT, see `LICENSE`. The Inter fonts in `res/fonts/` are under the SIL Open Font License.
