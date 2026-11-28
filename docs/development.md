# Development notes

## Build and test

```powershell
cd app
cargo build --release   # writes app/target/release/pedalhost.exe
cargo test
```

The **`stable-x86_64-pc-windows-gnu`** toolchain is pinned in `app/rust-toolchain.toml`, so `rustup` installs it automatically. Debug builds are also compiled with optimisation (`opt-level = 2`) so the audio path stays fast during development.

## Command line

```text
pedalhost.exe [file.pdl] [--screenshot out.png]
```

| Option | Description |
|---|---|
| `<file.pdl>` | Open a patch file at startup |
| `--screenshot <file.png>` | Draw about 120 frames, save a screenshot of the window, then exit (used for automated checks) |
| `--scan-plugin <file>` | *Internal.* Prints a description of one plugin file as JSON. The plugin scanner uses this. |

## Project layout

```text
app/
├── Cargo.toml            # crate "pedalhost"
├── MECHANICS.md          # values from the original program and how the rebuild uses them
├── mechanics.json        # the same values in machine-readable form
├── vendor/webbrowser/    # patched crate (avoids linking shlwapi on the GNU toolchain)
└── src/
    ├── main.rs           # entry point, fonts, 1 ms timer precision
    ├── app.rs            # main application state and patch handling
    ├── field.rs          # plugin canvas: boxes, pins and cables
    ├── graph.rs          # processing graph and the precompiled plan
    ├── bars.rs           # menu bar and bottom transport bar
    ├── dialogs.rs        # settings, built-in and generic processor editors
    ├── editors.rs        # processor mappings, patch organiser and preset manager
    ├── app_mappings.rs   # application commands and AppMappings.xml
    ├── app_mappings/     # mapping editor and keyboard shortcut handling
    ├── subwin.rs         # desktop dialogs, prompts and drop-down lists
    ├── builtins.rs       # built-in processors (+ builtins/view.rs for their controls)
    ├── engine/           # audio engine: asio.rs, wasapi.rs, mixed.rs (ASIO + WASAPI),
    │                     #   bridge.rs (cross-device resampling), latency.rs
    ├── plugins/          # VST2 (vst2.rs), VST3 (vst3/), plugin editor windows
    ├── plugin_list.rs    # plugin folders, out-of-process scanning and the known plugins
    ├── midi.rs, osc.rs   # MIDI and OSC input
    ├── pdl.rs            # .pdl patch file reading and writing
    ├── presets.rs        # per-processor preset bars and .fxp preset files
    ├── colours.rs        # colour schemes and JUCE-style colour maths
    ├── settings.rs       # settings stored as JSON
    ├── tempo.rs          # tap tempo
    ├── tray.rs           # system tray icon
    ├── widgets.rs        # shared drawing helpers: fonts, gradient buttons, scrollbars
    └── watchdog.rs       # freeze and crash notes
```

## Design notes

### Real-time audio

See [How the audio engine works](audio.md#how-the-audio-engine-works) for the audio thread rules, the plan hand-off and the mixed-route bridge.

### Responsiveness

- Requests **1 ms timer precision** (`timeBeginPeriod(1)`) and opts out of Windows 11 power throttling. Without this, plugin GUIs that pace themselves with timers drop from about 49 fps to about 13 fps.
- VST2 editors get an idle call every 10 ms from a dedicated ticker thread.
- Vsync is off because plugin windows share the main window's thread.
- The plugin scanner runs each file in a separate process, so a plugin that crashes during a scan can't take the host down.
- A watchdog writes a note and a memory snapshot if the window stops responding for 5 s or the program crashes.

### Fidelity to the original

The UI recreates Pedalboard2's layout, colours, fonts (Verdana) and JUCE's software colour blending. The values come from the original binary and from measurements of its screenshots. [`app/MECHANICS.md`](../app/MECHANICS.md) lists each value with its source address, and marks the ones that were estimated.

Patch files (`.pdl`), plugin state (JUCE base64), mapping XML and colour schemes use the original's formats.
