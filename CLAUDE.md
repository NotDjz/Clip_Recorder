# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Replay screen recorder for Windows — continuous background capture of screen + audio, `Ctrl+Alt+R` saves the last X seconds as MP4. Open-source Medal/ShadowPlay alternative. Single-file Python app (`src/clip_recorder.pyw`, ~1900 lines) plus a small companion installer (`src/setup.pyw`, ~200 lines).

## Design Principles

- **Zero config**: works out of the box, auto-detects audio/video devices
- **Ultra lightweight**: no visible overlay, no game performance impact
- **Portable app, real installer for distribution**: the app itself (`ClipRecorder.exe`) stays portable — no admin rights, no registry, `config.json` beside it. But it's no longer what users download directly: `ClipRecorderSetup.exe` (built from `setup.pyw`) is the only GitHub release asset, and handles placement (see "Setup / installer" below) so the app never ends up duplicated in Downloads
- **One hotkey only**: single customizable hotkey (default Ctrl+Alt+R). No pause/resume, no multi-hotkeys
- **All UI text in English**: settings window, tray menu, notification banner, dialogs, docstrings — no French strings in the source (code/comments were already English; the UI was translated to match)
- **Always use 2nd monitor**: when using computer-use tools (screenshots, clicks, running the app), always work on the VG258 monitor, never touch the primary (PL2590HS)

## Working with the user

Always ask clarifying questions to refine scope/approach before implementing, rather than assuming — especially for anything destructive (deleting files, changing what a release ships) or where more than one reasonable design exists.

## Project layout

```
src/      clip_recorder.pyw (the app), setup.pyw (the installer)
scripts/  build.bat, build_all.bat, build_setup.bat, download_ffmpeg.py, generate_icon.py
tests/    test_invariants.py, test_render_window.py, mutation_check.py,
          timecode_sync_test.py, av_sync_test.py
assets/   icon.ico          docs/  CONTEXT.md
ffmpeg.exe, build/, dist/   -> repo root, all gitignored
```

`CLAUDE.md` stays at the repo **root** — Claude Code loads it from there by convention, so moving it into `docs/` would silently drop these rules.

Because the app now lives in `src/`, `BUNDLE_DIR` is `src/` when running from source, so `_find_ffmpeg()` also checks the parent directory before falling back to a PATH `ffmpeg`. Frozen builds are unaffected (`SCRIPT_DIR` is the exe's own folder).

## Commands

### Run from source
```
pip install -r requirements.txt
python src\clip_recorder.pyw
```

### Build exe
```
scripts\build_all.bat
```
Builds the app then the setup exe, in order (`build_setup.bat` embeds `dist\ClipRecorder.exe`, so it must run after `build.bat`). Each script `cd`s to the repo root itself and `build_all.bat` calls them by absolute path, so they work from any directory. Run them separately if you only need one. Or manually, **from the repo root**:
```
py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorder --icon=assets\icon.ico --add-data "ffmpeg.exe;." --hidden-import pystray._win32 src\clip_recorder.pyw
py -m PyInstaller --noconfirm --onefile --windowed --name ClipRecorderSetup --icon=assets\icon.ico --add-data "dist\ClipRecorder.exe;." src\setup.pyw
```
**Must close ClipRecorder.exe (and ClipRecorderSetup.exe) before rebuilding** — otherwise PermissionError.

`build.bat` deletes `dist\ClipRecorder.exe` **before** building and checks every step, and `build_all.bat` stops instead of chaining. Without that, a failed build left the previous exe in place and `build_setup.bat` happily embedded that stale exe into the installer — the only asset published to Releases.

**The `.bat` files must stay CRLF.** They were committed with LF-only endings and `cmd.exe` misparses those — it reads `REM` as `M` and `cd /d` as `/d`, so the build dies before it starts with two "not recognized as an internal command" lines. All three were broken this way at once. `.gitattributes` now pins `*.bat text eol=crlf` so a checkout cannot reintroduce it.

Prefer `taskkill` **without** `/F` where possible: a forced kill skips the app's shutdown and strands its ffmpeg child (see "Orphan cleanup" under Win32 specifics).

### Dependencies
- Python 3.10+, `pystray`, `Pillow`, `pyaudiowpatch`
- FFmpeg 8.1+ with ddagrab support (bundled in exe; `scripts/download_ffmpeg.py` fetches it)

### Testing
**Start with the two deterministic suites — they need no devices, no ffmpeg and no screen, and run in about a second:**

```
py tests\test_invariants.py     # 27 guard rails: the invariants this project already paid for
py tests\test_render_window.py  # 6 unit tests on the real-time audio reconstruction
py tests\mutation_check.py      # proves the guard rails still bite (breaks each invariant on purpose)
```

`test_invariants.py` locks down what past bugs cost: the keyframe/segment relationship, `-flush_packets`, no `format=yuv420p` on ddagrab+NVENC, the in-progress segment never being selected, concat referencing only snapshots, never seeking mid-segment, audio window == video `-t`, the fallback anchoring at the oldest segment's *open* time, `restart_video()` not touching audio, `_wipe_segments()` sparing an in-flight save, no file I/O in the audio callbacks, the uninstall file list with no `-Recurse`, the log cap, the orphan-ffmpeg contract (the PID is recorded, `_wipe_segments()` keeps it, a foreign process is never killed, a missing/garbage PID file is tolerated), and the job-object contract (the documented Win32 flag values, the job established before any ffmpeg is spawned, and the uninstall helper breaking away from it). It fakes `subprocess` and the tk root, so it exercises the real command-building and selection code without capturing anything.

**Running one test.** Neither harness takes a filter argument — each collects its `test_*` globals and runs them all. To run a single one, import it:

```
py -c "import sys; sys.path.insert(0,'tests'); import test_invariants as t; t.test_log_is_capped()"
```

Silence means it passed (the tests assert; only `main()` prints). Same pattern for `test_render_window.py`. `mutation_check.py` takes no arguments at all — it always runs the full list.

**There is no type checker wired up, and the `pyright-lsp` plugin cannot supply one:** it handles `.py`/`.pyi` only, and all the code lives in a `.pyw`, so the LSP answers `No LSP server available for file type: .pyw`. Pyright *can* still be run, on a throwaway `.py` copy: `npx pyright --outputjson` with `reportMissingModuleSource: false` (no stubs exist for `pystray`, `pyaudiowpatch`, `PIL`). Done once — 18 diagnostics, **0 real defects**: all were guards pyright cannot narrow across methods, plus the normal PyInstaller `sys._MEIPASS` idiom. Not worth repeating unless the file gains type annotations. The syntax check is `py -m py_compile src\clip_recorder.pyw` — worth running after editing, because a `.pyw` is not exercised by any casual run and a syntax error otherwise surfaces only when the app silently fails to launch (windowed, no console).

**Run `mutation_check.py` after touching `test_invariants.py`.** A guard rail that cannot fail is worse than none — it buys false confidence. That checker caught two tests of mine that were silently useless: one matched on filenames that snapshotting had already renamed, and one only ever exercised the no-audio branch because its fake never wrote the WAV to disk. It currently breaks 17 invariants on purpose and all 17 are caught by their named test. Keep mutation anchors SHORT: an anchor that quotes a log message or four consecutive statements dies on any unrelated edit, and a dead anchor degrades to SKIP — a guard rail that has quietly stopped being checked. It earned its keep again on the job-object work: `test_uninstall_helper_breaks_away_from_the_job` was matching the flag name in the *comment* above the call, so it could not fail. Assert on the `creationflags=` expression, never on a name appearing somewhere nearby.

The ways to verify end-to-end (all default to `--monitor 1`, the second screen — **tell the user before running one**, they must close Spotify/games or that audio lands in the loopback capture and corrupts the measurement):
- **Manual**: run the app, press Ctrl+Alt+R, check the output MP4 (and `clip_recorder.log`, see "Diagnostic logging" below).
- **`tests/test_render_window.py`**: deterministic unit tests for `AudioCapture._render_window` (the real-time audio reconstruction) — no devices/ffmpeg, pure logic (silence-gap placement, window clipping, dead-stream, contiguous reconstruct). Run `py tests\test_render_window.py`. Fast; run it for any change to the audio timeline reconstruction.
- **`tests/av_sync_test.py`**: the integration harness — objective A/V sync measurement via a synthetic capture+save+analyze cycle, with a decode-corruption check, an audio-vs-video duration-equality assertion, `--repeats N` (successive saves) and `--change-buffer N` (exercises the real duration-change path). Mandatory for any change to the save/segment/audio path. Known blind spot: a synthetic solid-color signal encodes nearly for free and its always-active tone means loopback never goes silent, so it does **not** reproduce the segment-corruption / loopback-silence-drop bugs that only surface on real content or a real first-launch — those were caught by `clip_recorder.log` from real usage plus the deterministic `test_render_window.py`, not by this tool.

## Architecture

Single file, six classes:

| Class | Role |
|-------|------|
| `AudioCapture` | WASAPI loopback + mic via pyaudiowpatch, timestamped-chunk deques |
| `FFmpegCapture` | FFmpeg process management, rolling .ts segments, save_replay() |
| `NotificationBanner` | Click-through overlay, auto-hides after 3s |
| `HotkeyManager` | Win32 RegisterHotKey with message pump thread |
| `SettingsWindow` | Dark tkinter Toplevel |
| `TrayIcon` | pystray system tray icon |

`SettingsWindow` has a single **Save** button (applies + persists to `config.json`). There used to be a separate "Apply" (live, non-persisting) button — removed as a confusing strict subset of Save; don't re-add it.

**`_build()` produces values that `_apply()` PARSES — these formats are a contract.** Break one and device selection fails with no exception at all, just the wrong monitor or a microphone pinned to a device literally named "(Auto — system default)":
- The monitor label must stay `"N: name (WxH)"`, N 1-based — `_apply()` reads it back with `int(value.split(":")[0]) - 1`. A redesign that switches the separator to an em dash looks better and silently selects the wrong screen.
- The auto sentinel must keep its `"(Auto"` prefix, matched by `startswith`.
- `fps_var` and `buffer_var` must stay int-parseable strings.
- **`hotkey_btn` must stay a classic `tk.Button`**: `_start_hotkey_capture()` recolours it with `.config(bg=, fg=)`, which every ttk widget refuses.
`test_settings_vars_match_what_apply_parses` pins the first, second and fourth; the int-parseable rule is covered only by `_apply()` raising, so keep it in mind by hand.

Two Windows details the redesign needed. Dropdowns are `ttk.Combobox`, the only one tkinter can render dark — but only under the `clam` theme, and its popup is a classic Listbox that `ttk.Style` cannot reach, so that half goes through `option_add("*TCombobox*Listbox.…")`.

And the title bar: `_use_dark_titlebar()` sets **`DWMWA_CAPTION_COLOR` (35)**, not just `DWMWA_USE_IMMERSIVE_DARK_MODE` (20). Dark mode alone is not enough — with Windows' "show accent colour on title bars" switched on, the **active** window's bar takes the accent regardless, so on a bright accent it sits across the top of a dark window like a stripe. This cost a wrong conclusion once: an early attempt looked fixed because the screenshot happened to catch the window **inactive**, and inactive bars never take the accent. Verify this one with the window focused, or you are measuring the wrong state. Attributes 35/36 are Win11 22000+; older builds return an error that is dropped, keeping the plain dark-mode bar.

### Video pipeline — critical invariants

FFmpeg runs continuously producing rolling MPEG-TS segments (`SEGMENT_DURATION` = **1s** each — short on purpose, see "Round 7" below: the in-progress segment is unusable, so segment length is exactly how much of the moment before the hotkey is lost):
- **ddagrab** (DXGI Desktop Duplication) for 120/240 FPS — uses `-f lavfi -i "ddagrab=..."` syntax
- **gdigrab** fallback for 30/60 FPS — uses `-f gdigrab -i desktop` syntax
- **Keyframe interval must equal FPS × SEGMENT_DURATION** — without this, concat produces visual artifacts
- **Never use `-vf format=yuv420p` with ddagrab + NVENC** — causes "Invalid argument"; NVENC handles pixel format internally
- Monitor selection: ddagrab uses `output_idx=N`, gdigrab uses `offset_x/offset_y/video_size`

### Audio pipeline — critical invariants

- **Loopback detection is exact-prefix FIRST, then the library resolver.** `_detect()` matches a loopback whose name `startswith` the default output's, and only falls back to pyaudiowpatch's `get_default_wasapi_loopback()`. Order matters: the resolver matches with `in`, so with a default output named "Headphones" it also matches a lower-indexed "USB Headphones [Loopback]" and captures the wrong card silently. Each device is adopted *after* its rate and channels are read, never before — adopting first would describe hardware we are not capturing and skip the resolver. A failed lookup is logged, not swallowed: a silent miss downgrades the clip to mic-only.
- pyaudiowpatch captures system audio (WASAPI loopback) and mic in separate **timestamped-chunk deques** (`_loopback_chunks`/`_mic_chunks` of `(t_arrival_wallclock, pcm_bytes)`) with separate locks. This replaced flat circular `bytearray`s + byte-count-to-duration math — see "Real-time-anchored audio" below for why that was the core fragility.
- `frames_per_buffer = 4096` — lower values (1024) cause dropouts at high FPS
- On save, audio is **reconstructed on the real wall-clock timeline** by `_render_window()`: exactly `duration*rate` frames ending at `save_time`, each chunk placed at its real arrival time, gaps left as silence. This anchors loopback, mic and video to one timeline. WAV framerate is the **nominal** `self._rate` (no more `_effective_rate`).
- Mixing loopback + mic requires explicit resample to 48kHz stereo BEFORE amix, with `normalize=0` — without this: volume pumping or buzz
- **Audio streams are restarted ONLY on a device change** (`loopback_device`/`mic_device`), never for monitor/fps/`buffer_seconds` — those go through `FFmpegCapture.restart_video()` (video process only). Restarting audio needlessly re-exposes the flaky WASAPI loopback (re)open; the audio buffer is always 120s regardless of `buffer_seconds`.

### Save replay flow (Ctrl+Alt+R)

1. List .ts segments by mtime, **exclude the newest one** (still being actively written by the live capture process — see "A third bug class" below for why this must never be skipped)
2. Take N most recent segments, write concat.txt (UUID-named for rapid successive saves)
3. Concat → video-only MP4 (`-c copy`, no re-encode)
4. Reconstruct loopback/mic WAVs on the real timeline (`save_wav(path, t_end=save_time, duration=total_duration)`) — no `end_offset`; samples that arrived after `save_time` are excluded by their timestamp
5. Mix loopback + mic if both exist
6. Mux video + audio → final MP4 (`-c:v copy -c:a aac -b:a 192k -shortest -movflags +faststart`)
7. Play `SystemExclamation` sound + show notification banner
8. Cleanup temp files

Runs in a daemon thread to avoid blocking capture.

### Win32 specifics

- Single instance via `CreateMutexW("ClipRecorder_SingleInstance")`
- **The kill-on-close job object is the PRIMARY defence against orphaned ffmpegs; startup cleanup is the backstop.** `_ensure_kill_on_close_job()` puts this process in a job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so when it dies by **any** means (`taskkill /F`, Task Manager, an access violation in a C extension) the OS kills every child in the job. Children inherit membership, so this covers the capture ffmpeg **and `save_replay()`'s three**, which are recorded in no PID file and were previously unrecoverable. Called from `FFmpegCapture.__init__`, not `main()`, so the test harnesses — which construct `FFmpegCapture` directly and never reach `main()` — are covered too.
  - **`uninstall()`'s PowerShell helper MUST keep `CREATE_BREAKAWAY_FROM_JOB`.** It waits for this process to exit *before* deleting the exe, so inside the job it is killed with us and uninstall silently does nothing at all. That is why the job sets `JOB_OBJECT_LIMIT_BREAKAWAY_OK`. Never use `SILENT_BREAKAWAY_OK` — the ffmpegs would escape too and the job would be pointless. Verified with real processes both ways: without the flag the helper *is* killed; with it, it survives.
  - **Declare `argtypes`/`restype` for these calls.** `GetCurrentProcess()` returns the pseudo-handle `(HANDLE)-1` = `0xFFFFFFFFFFFFFFFF`; with ctypes' default int marshalling that argument raises `OverflowError`, and the surrounding `except` swallowed it — the job was created, the process was never in it, and the whole mechanism was inert with no symptom. This shipped-broken-once trap is why the handler now logs the exception instead of `pass`.
  - Verifying this needs real processes, not the deterministic suite: force-kill a parent that spawned an ffmpeg and check the child died — **with a control run where the job is disabled**, or the test proves nothing (a first attempt "passed" only because `testsrc` without `-re` encodes 300 s in about a second and ffmpeg had already exited on its own).
  - **Verified on the frozen exe**, which is what ships and what the source-level test cannot answer. Two builds, one with the job neutered: the control's ffmpeg **survived** the force-kill as an orphan, the real build's **died with its parent**. PyInstaller onefile runs two processes (bootloader + Python child); the child is the one that joins the job and the one that parents ffmpeg, so onefile does not break the mechanism. Name the test exes something other than `ClipRecorder.exe` and remember the process name follows the *filename* — a first attempt killed nothing and both runs "passed", and the single-instance mutex silently made the second trial a no-op because the first was still alive.
- **Orphan cleanup on startup kills the PROCESS, not just the folder.** A force-killed or crashed instance leaves its ffmpeg child alive; that child keeps writing to temp **and holds an NVENC session**, and consumer GPUs allow only a handful — so a few orphans make every later launch encode nothing at all (no segments, `save_replay` logging `ABORT: no segment files found`, nothing on screen to explain it). Each capture writes its ffmpeg PID to `FFMPEG_PID_FILE` beside its segments; `_kill_orphan_ffmpeg()` reads it at startup and terminates that process before `shutil.rmtree` removes the folder. The sweep catches **per directory**, not around the whole loop: orphans accumulate in numbers, so one failure must not leave the rest holding NVENC sessions. The per-orphan wait is floored at `ORPHAN_KILL_FLOOR_MS` even once the shared `ORPHAN_KILL_WAIT_MS` budget is spent — a zero wait means the following rmtree races the still-open segment file and the directory survives, which is the race the wait exists to close. Two traps it has to handle: PIDs are **reused**, so the image name is checked with `QueryFullProcessImageNameW` and anything that is not `ffmpeg.exe` is left alone (killing a stranger's process is worse than leaving an orphan); and `TerminateProcess` is **asynchronous**, so it waits with `WaitForSingleObject` — which silently does nothing unless `SYNCHRONIZE` is in the `OpenProcess` access mask, or the following rmtree races the still-open segment file and leaves the directory behind.
- FFmpeg process: `CREATE_NO_WINDOW` flag, graceful kill via `stdin.write(b"q")` → wait → terminate → kill
- Notification banner: `WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` + `WDA_EXCLUDEFROMCAPTURE`, applied while the window is still `withdraw()`n (before first `deiconify()`) — without `WS_EX_NOACTIVATE`, showing the banner can steal foreground activation from windowed/borderless games (exclusive-fullscreen games are protected by Windows from this; windowed ones are not)
- `HotkeyManager.restart_hotkeys()` (in `main()`) must never write `config["hotkey"]` to a value it hasn't *confirmed* is actually registered (`.registered` checked after `._ready.wait()`) — doing so unconditionally on failure (as a prior version did) permanently pins the config to a string that looks fine but isn't backed by a real registration, and since the Settings change-detection is a plain string compare, the user can never trigger a retry by re-selecting that same value. Leave `config["hotkey"]` untouched on failure so a later change is correctly detected. `HotkeyManager` captures `ctypes.windll.kernel32.GetLastError()` into `self.last_error` on failure (e.g. 1409 = `ERROR_HOTKEY_ALREADY_REGISTERED`) and retries once after a short sleep before giving up.

## Config (config.json)

```json
{
  "monitor": 0,
  "fps": 60,
  "buffer_seconds": 60,
  "output_folder": "...",
  "loopback_device": "",
  "mic_device": "",
  "hotkey": "Ctrl+Alt+R"
}
```

Empty string for audio devices = auto-detect. FPS options 120/240 only available when ddagrab is detected. Hotkey format: `Modifier+...+Key` (e.g. `Ctrl+Shift+F9`).

Empty `output_folder` defaults to `~\Videos\ClipRecorder` (`get_output_folder()`, matches ShadowPlay/Medal/OBS convention — predictable regardless of where the portable exe sits, e.g. Desktop/Downloads/USB). The real Videos folder is resolved via `SHGetKnownFolderPath(FOLDERID_Videos)`, not `os.path.expanduser("~") + "Videos"` — the API respects a relocated Videos library (Properties > Location) and is locale-independent (on-disk folder names stay English regardless of Windows display language; only Explorer's label is translated).

## Setup / installer (`setup.pyw` → `ClipRecorderSetup.exe`)

Separate single-file script, its own PyInstaller build (`build_setup.bat`), bundling the already-built `dist\ClipRecorder.exe` via `--add-data "dist\ClipRecorder.exe;."` (extracted at setup runtime from `BUNDLE_DIR`/`sys._MEIPASS`, same technique already used to bundle `ffmpeg.exe` inside the app itself). This is the ONLY thing users download from GitHub Releases now — running it shows a dialog asking where to install (defaults to `%LOCALAPPDATA%\Programs\ClipRecorder`, browsable, optional desktop shortcut), copies the embedded app exe there, launches it, and exits. It does not run the app itself.

- Checks the app isn't currently running first (same `ClipRecorder_SingleInstance` mutex name, `CreateMutexW(None, False, ...)` + `GetLastError() == 183`, closing its own handle right after the check) — gives a clear "please close it first" message instead of failing on a locked file.
- **Only writes `config.json` at the target if one doesn't already exist there** — re-running Setup to update an existing install must not wipe the user's saved settings. The exe itself is always overwritten.
- The old approach — the app copying *itself* on first launch (`prompt_install_location()`, now removed from `clip_recorder.pyw` entirely) — left the original download as a fully-functional duplicate app in Downloads, which is confusing; Setup fixes this by being a distinct, disposable installer.

**Caution when testing**: don't name a test folder `cliprec_*` — that prefix collides with the orphan-cleanup scan in `clip_recorder.pyw`'s `main()` (`tempfile.gettempdir()` + `cliprec_*`, used for FFmpeg segment dirs) and the folder gets deleted as a false-positive orphan.

## Uninstall

An "Uninstall..." button in Settings (`SettingsWindow._confirm_uninstall`) opens a confirmation dialog (no clip-deletion option — the clips folder is user-configurable at any time via Settings, so uninstall must never assume it knows where they currently are, or that whatever folder is currently configured is safe to bulk-delete). On confirm, `uninstall()` (in `main()`) tears down capture/audio/hotkeys/tray, then — only when `sys.frozen` — spawns a detached, hidden PowerShell helper (`Wait-Process -Id <pid> -Timeout 10` then a short sleep, then individual `Remove-Item` calls) that deletes the exe, `config.json`, `clip_recorder.log`, and the desktop shortcut once this process has actually exited (a running `.exe` can't delete itself — the file is locked). **Deliberately never deletes the exe's containing folder recursively, and never touches the clips folder** — a portable exe can sit anywhere (Desktop, Downloads), so only the specific known app files are removed, never `-Recurse` on `SCRIPT_DIR`.

## Dev workflow — build vs. run

`dist\` is disposable PyInstaller build output only — never run the daily-driver exe from there, it pollutes the build tree with `config.json` + real recordings. Copy the built app exe to a separate folder outside the repo (e.g. `%USERPROFILE%\Apps\ClipRecorder\`) for actual testing/daily use — or test the real install path by running `dist\ClipRecorderSetup.exe` from somewhere outside the repo and installing to a disposable test folder.

The daily-driver install lives at `%LOCALAPPDATA%\Programs\ClipRecorder\` (it holds the real `config.json` and `clip_recorder.log`); after a rebuild the new exe has to be copied there manually — the build scripts deliberately do not touch it.

## Known Open Issue

None currently open. The long-running audio buzz/crackling was finally root-caused and fixed in round 7 (see below): `_render_window` was punching a 1-5 ms hole at every chunk boundary. Measured 38 discontinuities (2.5/s) before, **0** after, across four consecutive runs of `timecode_sync_test.py`.

### Audio/video sync — critical design notes

This exact bug (audio ends up seconds ahead of / overlapping the video) recurred **five** separate times before the root cause was finally found by logging real usage (not the synthetic test). **Verify any future change here with `tests/av_sync_test.py` (including `--change-buffer N` and `--repeats N`) AND `tests/test_render_window.py`** across multiple FPS/`buffer_seconds` combinations before considering it fixed.

**Round 6 (the real root cause — read this first).** The underlying fragility across all earlier rounds was that `AudioCapture` reconstructed *how much time* its raw PCM buffer represented by counting bytes × sample rate. That is only valid if the source delivers continuously — but the **WASAPI loopback stream does not**: on real hardware it drops/under-delivers during silence (confirmed in a real `clip_recorder.log`: loopback buffer held ~17s of audio after ~59s of capture, while the continuous mic held the full ~60s). So loopback and mic sat on **different timelines** and `amix` overlapped/echoed them — worst right after a duration change, because a `buffer_seconds` change was needlessly restarting the audio streams and re-exposing the flaky loopback (re)open. Fixes:
  - **Real-time-anchored audio.** Capture stores **timestamped chunks** (`(t_arrival, pcm)` deques); on save, `_render_window()` reconstructs exactly `duration*rate` frames ending at `save_time`, placing each chunk at its real arrival time and leaving **silence** in gaps. Loopback, mic and video now share one wall-clock timeline by construction, regardless of delivery irregularity. This **subsumes and replaces** `audio_offset`/`end_offset` (round 1) and `_effective_rate` (round 5) — both deleted. WAV framerate is nominal `self._rate`.
  - **Audio restarts only on a device change.** monitor/fps/`buffer_seconds` go through `FFmpegCapture.restart_video()` (ffmpeg process only); audio streams keep running. This removes the flaky-loopback reopen from every duration/fps/monitor change — the direct trigger of the reported desync.
  - **Self-heal judges health by `is_active()`, never by frame count.** It reopens a stream that failed to open or has stopped, and retries a transiently-failing (re)open (WASAPI can throw `-9999` right after a close), but it **must not** close a stream that is open and active merely because frames==0 — that is just silence (reconstruction handles it), and closing a healthy stream can hit `-9999` and leave it dead (observed on a silent desktop). A `_heal_gen` counter retires stale heal threads from superseded start() cycles.
  - Validated by `tests/test_render_window.py` (deterministic silence-gap/placement/clipping unit tests) + `av_sync_test.py` audio-vs-video **duration-equality** assertion (a mismatch *is* the overlap symptom) + `--change-buffer` exercising the real path (verified: audio streams untouched across the change, durations match to ~10ms).

The earlier-round notes below still describe real mechanisms in the *video* path (segment selection, keyframe grid, TOCTOU) — those remain in force. The *audio*-side notes marked "(superseded)" are kept only as history.

- `save_time = time.time()` must be captured at the very start of `save_replay()` — all timing derives from it (audio window end + video span both anchor to it).
- **(superseded by round 6)** ~~`audio_offset` processing-delay trim~~ — replaced by timestamp-based extraction: samples arriving after `save_time` are excluded by their timestamp, so no trim is needed.
- `-flush_packets 1` on the capture FFmpeg ensures segment files are up-to-date when snapshotted.
- **Never seek into the middle of a segment for the video side.** `save_replay()` selects whole segment files only, walking backward from the newest and accumulating real mtime-based span until it covers `replay_secs`, then uses `-ss 0` unconditionally (see the loop building `selected`/`total_duration` near the top of `save_replay()`). This replaced an earlier approach that computed a real `total_duration` (correctly, from mtimes) but then quantized the resulting seek point (`ss`) down to a multiple of the *nominal* `SEGMENT_DURATION` constant — since real per-segment duration drifts from nominal under capture load (`keyframe_interval` is a frame count, not a time, so sustaining it takes longer than 5.0s in real time whenever delivered FPS dips), the real keyframe grid in the concatenated stream doesn't line up with clean nominal multiples of 5, and `-c copy` would snap to a real keyframe at a different point than the Python model assumed — an error that compounded per segment, worse at high FPS and longer buffers (confirmed: ~3s off at 240 FPS / 2 minutes). Selecting whole segments and never seeking mid-stream eliminates this class of bug entirely, at the cost of the saved clip running `replay_secs` to `replay_secs + ~SEGMENT_DURATION` long instead of exactly `replay_secs` — an accepted trade-off.
- `audio_duration` must equal the same `total_duration` the video side settled on (not a fixed `replay_secs`) — both sides need to agree on the same real window.
- **The window starts where the oldest SELECTED segment opens, not where it closes.** A segment opens when the previous one closes, so the normal path anchors on `complete[n - count - 1]`'s mtime — the segment *before* the selection — which is correct. The not-enough-history fallback (everything selected, taken right after a start/restart) has no earlier segment, and used `complete[0]`'s mtime directly: that is the oldest segment's *close* time even though its content is included, so the audio was anchored one whole segment late. Result: exactly `SEGMENT_DURATION` of A/V offset on any save made before the buffer has refilled — reported by the user as "after changing a setting I must wait the full new duration or the sound is 1s off", and worth 5s back when segments were 5s (part of the original "audio is seconds ahead" reports). The fallback now estimates the open time as `complete[0].mtime - SEGMENT_DURATION`. Verified with `timecode_sync_test.py --buffer 60 --save-after 20` (forces the fallback): offset median +0 ms, and the log shows `selected 19/19` with `total_duration` one segment longer than the raw mtime span.
- **Snapshot every selected segment, not just the newest.** `save_replay()` copies each entry of `selected` to a uniquely-named `snap_{concat_id}_{i}.ts` immediately after selection, and the concat list references only these copies. The live capture FFmpeg process never stops running and, per `-segment_wrap`, cyclically **overwrites** old numbered segment files — the actual concat read only happens later, in the background `_run()` thread, *after* synchronous audio processing. Referencing original filenames directly risked one of them being rewritten mid-read: a TOCTOU race whose window recurs identically on every save (not cumulative — each save does a fresh listdir/scan) but whose damage when it hits is bounded at exactly one segment's duration (confirmed: reports of "up to five seconds," and worse the more times you save in a session, matched this exactly — more saves = more independent rolls of the same probabilistic collision, not accumulating drift). Copying every selected segment up front — extending the technique already used for the last, in-progress segment — eliminates the race entirely: a live process can never touch a file under a name it never wrote.
- **(superseded by round 6)** ~~`_effective_rate()` measured the delivered sample rate to correct byte-count-to-duration math.~~ The whole byte-count-to-duration approach is gone — audio is now placed by real timestamps (`_render_window`), so a slightly-off or irregular delivery rate no longer mis-sizes the window. The insight that motivated it (real audio clocks/delivery ≠ nominal, which is why OBS timestamps every sample) is exactly what round 6 implements properly.

### Round 7 — crackling, and how much of the moment you lose

Two defects found with `tests/timecode_sync_test.py` (see below), both measured rather than guessed:

- **Crackling (regression introduced by round 6).** `_render_window()` placed **every** chunk at its own rounded arrival timestamp. Real callback arrival jitters by a few ms, so each chunk boundary (~12/s) got a 1–5 ms silence hole punched into it or samples overwritten — objectively measured as **38 sample-level discontinuities (2.5/s)**. Fix: a run of chunks that arrived roughly on schedule is now kept **strictly sample-contiguous**, and the writer only jumps to the real timestamp when divergence exceeds a **30 ms tolerance** (a genuine loopback-silence gap, which must stay silent). Comparing against the *absolute* real position each time means a contiguous run can never drift past the tolerance. Measured after the fix: **0 discontinuities**, 4 runs in a row. Regression test: `test_jittered_arrivals_stay_contiguous`.
- **The clip stops before the hotkey press.** The in-progress segment is excluded (it decodes as a torn frame), so the clip can only end at the last *complete* segment — losing up to `SEGMENT_DURATION`. Verified the in-progress segment is genuinely unrecoverable: snapshotting and remuxing it (`-c copy -fflags +discardcorrupt`) yields **0.01 s and still decodes with errors**. So the only lever is segment length: **`SEGMENT_DURATION` 5 → 1**, which cut the measured loss from **1.9 s to a stable 0.87 s** across 4 runs. Costs more, smaller files (`segment_wrap` = 122 at a 120 s buffer) and a 1 s keyframe interval.

**A/V alignment itself measured clean** (median 0 to −10 ms, no drift) — so the theory that audio ran ~5 s ahead of the video was *not* confirmed, and the planned re-anchoring rework was dropped. Measurement beat theory; don't reintroduce that rework without evidence.

### `tests/timecode_sync_test.py` — ground truth decided in advance

The strongest test in the repo. It draws a running **timecode** full-screen on the capture monitor and plays a beep on a schedule **decided in advance** (every `BEEP_EVERY`=5 s, lasting `BEEP_LEN`=2 s), so it is known exactly when there must be sound and when there must be silence. A large **indicator block** is white for exactly the scheduled beep windows — and because it lives in the video track, the analyzer recovers "sound was supposed to play here" straight from the picture and compares it to real audio energy. No OCR, no assumption about where the clip starts.

It reports, and fails on: decode errors, audio-vs-video duration mismatch, **sample-level discontinuities** (crackle), per-beep A/V offset + drift, sound where silence was scheduled (and vice versa), and **`LOST BEFORE PRESS`** — how much of the moment right before the hotkey is missing.

```
py tests\timecode_sync_test.py --generate --fps 60 --buffer 15 --monitor 1
py tests\timecode_sync_test.py --analyze "C:\path\to\Clip_....mp4"
```

Two gotchas when reading it: the schedule is **periodic**, so pinning the clip to the timecode axis is only unique modulo `BEEP_EVERY` — the analyzer takes the smallest physically-possible loss. And the beep is played at low amplitude on purpose; the discontinuity threshold keys off the 95th-percentile step so it stays sensitive at that level (a `median*100` threshold can never trigger — int16 caps at 32767).

**Run tests on the second monitor (`--monitor 1`, the VG258), never the primary** — and tell the user before running, so they can close Spotify/games whose audio would otherwise leak into the loopback capture and corrupt the measurement.

### Diagnostic logging (`log()`, `clip_recorder.log`)

The synthetic `av_sync_test.py` signal has repeatedly measured clean results even in the same release where real-world usage still showed multi-second desync — meaning the synthetic signal isn't reproducing whatever real screen/game content triggers it. `save_replay()` therefore writes a thread-safe `log()` trail to `LOG_FILE` (`clip_recorder.log`, beside the exe/script, gitignored) covering: segment scan/selection (including `avg_seg_duration` vs nominal — reveals real segment durations drifting under capture load), snapshot timing, `audio_offset`/`audio_duration`, measured-vs-nominal audio sample rate for loopback and mic (via `_effective_rate()`), and — previously a complete blind spot — the `returncode`/`stderr` tail of all three ffmpeg subprocess calls (video concat, audio mix, final mux) plus any exception traceback. When chasing a real (not synthetic) desync report, get the user's `clip_recorder.log` from an actual failing save before guessing at further mechanisms.

### A fifth, *different* bug class this logging caught: silent fallback to the wrong audio source

A real user log showed `save_wav(loopback) ok=False` on every save in a session, with `frames_received=0` the whole time (visible via the `delta=+0.00` on the effective-rate line, since `_effective_rate()` falls back to nominal when `frames <= 0`) — the loopback (system/game audio) WASAPI stream had opened without error but never delivered a single callback. `has_loopback` in `save_replay()` is derived from `AudioCapture.available`, which only checks whether a loopback *device* was detected at `_detect()` time, never whether its capture *stream* is actually alive — so this failure was completely invisible: no exception, no error, just a clip whose audio silently downgrades to mic-only via the existing (correct) fallback logic in `save_replay()`'s `audio_wav` resolution. This is **not a timing bug** like rounds 1-4 above — it's the wrong audio source entirely, which is very plausibly what at least some of the long-running "desync" reports actually were.

The log showed every capture restart triggered by a Settings change (`FFmpegCapture.restart()` → `self.audio.stop()` + `self.audio.start()`, confirmed via the `loopback stream opened`/`mic stream opened` log lines that appear on every fps/`buffer_seconds` save) came back with a healthy, flowing stream — only the very first stream open of the session (before anything ever triggered a restart) got stuck silent. This matches a known WASAPI loopback quirk: opening loopback capture before the audio render engine has an active session can silently produce a stream that "opens" fine but never delivers data.

**Fix**: `AudioCapture.start()` (via `_open_loopback_stream()`/`_open_mic_stream()`, refactored out for reuse) now spawns a one-shot `_heal_dead_streams()` check 1.5s after opening each stream — if a stream that opened without error still has zero frames received, close and reopen it once (logged either way). Verified via the object-identity comparison (`self._loopback_stream is loopback_stream`) that a `stop()`/`start()` cycle happening before the 1.5s elapses correctly makes the stale check a no-op instead of clobbering a newer, already-healthy stream.

### A third, *different* bug class: selecting the in-progress segment (real video corruption, not a timing issue)

The whole-segment-selection rewrite (round 3, above) claims "select whole segments only" and the older documented flow said "skip the last one (still being written)" — but the actual slicing (`selected = files_with_mtime[-count:]`) includes the newest entry for **any** `count >= 1`, in both the normal and the not-enough-history fallback path. So the segment FFmpeg is still actively appending to was *always* being included and snapshotted — confirmed via `ffmpeg -i clip.mp4 -f null -` reporting `error while decoding MB ... corrupt decoded frame` on a repro (restart capture, save ~2s later). Snapshotting a file mid-write can catch a torn/incomplete frame; the resulting glitch/freeze in the video, combined with the audio track (plain PCM, structurally unaffected) continuing to play cleanly, reads exactly like an A/V desync even though it's pure video corruption. It's worst right after any restart (Settings change *or* app launch) because the corrupted segment is then most of a short clip — matching a real report of "the desync happens specifically when I change the buffer duration and save shortly after."

**Fix**: `save_replay()` now builds `complete = files_with_mtime[:-1]` up front and selects only from `complete` — the newest segment is never eligible. If `complete` is empty (only the in-progress segment exists — i.e. capture just started or just restarted, nothing complete yet), it aborts cleanly (logged) instead of producing a corrupted clip. Verified: the same repro that reported `corrupt decoded frame` before the fix now either aborts (too soon after restart) or decodes with zero warnings (once at least one segment has rotated), confirmed via `ffmpeg -f null -` on both the repro clips and a full `av_sync_test.py --repeats` run.

### `tests/av_sync_test.py` — automated, objective A/V sync measurement

Since "does it sound synced" isn't a reliable way to catch this bug (see above), this script measures it directly: shows a full-screen color-flip signal (1 flip/sec, driven by `time.monotonic()`) with a synchronized tone played through the default output device (via `pyaudiowpatch`, not `winsound.Beep` — the latter isn't guaranteed to route through WASAPI loopback), saves a real replay via `FFmpegCapture.save_replay()`, then decodes the clip to find the actual video color-transition timestamps and audio tone-onset timestamps and reports the measured offset in ms. No new dependencies (stdlib + the already-installed `pyaudiowpatch`).

Every analyzed clip also gets a **decode-corruption check** (`ffmpeg -v error -f null -`, fails on any decoder error) and an **audio-vs-video duration-equality assertion** (a mismatch *is* the overlap/desync symptom) — both return nonzero so a regression fails the run objectively, not just by eyeballing the offset.

`--repeats N` keeps the signal running and triggers N successive `save_replay()` calls a few seconds apart within the *same* session, analyzing each — the only way to catch a bug (like the segment-rotation race above) that only shows up across repeated saves. `--change-buffer N` changes `buffer_seconds` to N via the real `restart_video()` path after the first save and keeps saving — the only way to exercise the duration-change path that triggered the round-6 desync (asserts audio stays in sync and, via the logs, that the audio streams are *not* restarted).

```
py tests\av_sync_test.py --generate --fps 240 --buffer 120 --monitor 0 --repeats 5
py tests\av_sync_test.py --generate --fps 60 --buffer 15 --change-buffer 30
py tests\av_sync_test.py --analyze "C:\path\to\Clip_....mp4"
```

Residual offsets of a couple hundred ms that don't grow with duration/FPS and flip sign between runs are test-harness measurement noise (audio driver latency, `tk.after()` dispatch, ~50ms video sampling quantization) — not a regression. What to watch for is an offset that scales with `buffer_seconds` or FPS, which is the signature of the underlying bug class this tool exists to catch.

## Release notes

Never add "🤖 Généré avec Claude Code" (or any AI-attribution line/emoji) to GitHub release notes, commit messages shown to end users, or anywhere user-facing. Keep release notes plain and professional — just the changes.

**Versioning restarted at v1.0 on 2026-07-21.** The previous 8 releases and tags (v1.0, v2.0–v2.6) were deleted deliberately — they had been published while the audio-sync bugs were still unresolved, and the user wanted the fixed build to be the first real version. Tags keep the `v` prefix. **`ClipRecorderSetup.exe` is the only asset**; never attach the raw `ClipRecorder.exe`, or the app ends up duplicated in the user's Downloads.

```
scripts\build_all.bat
gh release create vX.Y "dist/ClipRecorderSetup.exe" --title "..." --notes-file ...
```

Deleting or replacing a release is destructive and irreversible (published notes and binaries are lost, existing links 404) — always confirm the exact scope with the user first. When refreshing an existing tag, move it with `git tag -f` + `git push --force`, then `gh release upload ... --clobber`, and verify the published asset size matches the local build.
