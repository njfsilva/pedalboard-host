# Pedalboard Host

**A live VST2/VST3 effects host for performers, rebuilt in Rust and egui.**

![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust)
![egui](https://img.shields.io/badge/UI-egui%200.31-blue)
![Platform](https://img.shields.io/badge/platform-Windows%20x64-lightgrey?logo=windows)
![Audio](https://img.shields.io/badge/audio-ASIO%20%7C%20WASAPI-green)
![Plugins](https://img.shields.io/badge/plugins-VST2%20%7C%20VST3-purple)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue.svg)](LICENSE)

Pedalboard Host is a modern rebuild of **Pedalboard2 v2.09 (64-bit)**, the JUCE-based live effects host. It keeps the original's look, file formats and behaviour. It adds **VST3 support** and an audio path built for **low latency** with ASIO. A core intention of the project is to preserve a simple, focused UI while keeping resource use to a minimum, which is why the project is built with rust.

<p align="center">
  <img src="docs/screenshot.png" alt="Pedalboard Host main window" width="800">
</p>

---

## Table of contents

- [Features](#features)
- [Getting started](#getting-started)
- [Usage](#usage)
- [Documentation](#documentation)
- [Differences from the original](#differences-from-the-original)
- [Status](#status)
- [License](#license)

---

## Features

- 🎛️ **Patch canvas**: add processors with a double-click and connect audio and parameter pins with cables. Loaded patches are fitted to the available space.
- 🔌 **VST2 and VST3 plugins**, scanned in a separate process so a crashing plugin can't take the host down.
- 🧩 **[Built-in processors](docs/built-in-processors.md)**: level, output toggle, VU meter, file player, recorder, metronome and looper.
- ⚡ **[Low-latency audio](docs/audio.md)** through ASIO or Windows Audio (WASAPI).
- 🔀 **[Mixed audio systems](docs/audio.md#mix-asio-and-windows-audio)**: use ASIO and WASAPI independently for input and output, bridged across sample rates and clocks.
- 📊 **[Latency display](docs/audio.md#latency-display)**: plug-in delay, plus estimated input-to-output latency on mixed routes.
- 🎹 **[MIDI and OSC control](docs/mappings.md)**: map CCs and OSC messages to plugin parameters, switch patches with Program Change and drive the transport with MMC.
- 🧰 **[Application mappings](docs/mappings.md#map-a-shortcut-or-controller-to-a-command)**: assign keyboard shortcuts, MIDI CCs and OSC messages to menu, patch and transport commands.
- 🗂️ **[Patches](docs/patches-and-presets.md)**: several patches per `.pdl` file, next/previous switching with optional wrap-around, and an organiser with import and export.
- 🎚️ **[Presets](docs/patches-and-presets.md#use-the-processor-preset-bar)**: per-processor preset bars, `.fxp` import/export and a preset manager.
- ⏱️ **Main transport**: tempo, tap tempo, play/pause and return to zero.
- 🎨 **Colour schemes**: compatible with Pedalboard2's `.colourscheme` files.
- 🪟 **[System tray and desktop windows](docs/system-tray.md)**: keep audio running with the window hidden, and place dialogs and editors outside the main window.
- 🛟 **Freeze and crash reports**: a watchdog writes a note and a memory snapshot if the window stops responding for 5 s or the program crashes.

## Getting started

### Requirements

- Windows 10/11 x64
- [Rust](https://rustup.rs/) with the **`stable-x86_64-pc-windows-gnu`** toolchain. This is pinned in `app/rust-toolchain.toml`, so `rustup` installs it automatically.
- An ASIO driver (recommended) or any WASAPI audio device

### Build

```powershell
cd app
cargo build --release
```

The executable is written to `app/target/release/pedalhost.exe`. Run the tests with `cargo test`. See the [development notes](docs/development.md) for more.

## Usage

```text
pedalhost.exe [file.pdl]
```

### Quick start

1. Choose your device in **Options → Audio Settings**.
2. Scan for plugins in **Options → Plugin List**.
3. **Double-click** an empty part of the canvas to add a processor.
4. Drag from an output pin to an input pin to connect them.
5. Click **e** on a processor box to open its editor or **m** to map controls to that processor's parameters.

Settings are stored in `%APPDATA%\PedalboardHost\settings.json`, and the default patch is `default.pdl` in the same folder.

## Documentation

| Guide | Covers |
|---|---|
| [Audio setup and latency](docs/audio.md) | ASIO and WASAPI, mixing systems, the audio bridge, the latency display and how the engine works |
| [MIDI, OSC and application mappings](docs/mappings.md) | Controller inputs, parameter mappings and command shortcuts |
| [Patch and preset management](docs/patches-and-presets.md) | The patch organiser, preset bars, the preset library and file locations |
| [Built-in processors](docs/built-in-processors.md) | The processors included with the host |
| [System tray](docs/system-tray.md) | Running hidden, starting in the tray and desktop windows |
| [Development notes](docs/development.md) | Building, command-line options, project layout and design notes |
| [Mechanics notes](app/MECHANICS.md) | Values taken from the original program and compatibility details |

## Differences from the original

- **VST3 support** (the original hosts VST2 only)
- Input and output can use ASIO and WASAPI independently, with cross-system audio bridging and estimated end-to-end latency
- Plugins are scanned in a separate process
- The Metronome uses the device's actual sample rate instead of assuming 44.1 kHz
- The Looper keeps loops in memory (up to 120 s). Loops aren't saved with the patch.
- Boxes can be renamed by double-clicking their title
- Host dialogs and built-in and generic editors share the main window's taskbar button

See [`app/MECHANICS.md`](app/MECHANICS.md) for the full list.

## Status

🚧 **Work in progress.** While not done yet, the main workflows for the application work well, the project builds, starts and draws the main window. Its behaviour has not yet been measured against the original, and a few values in the mechanics sheet are estimates.

## License

Pedalboard Host is free software: you can redistribute it and/or modify it under the terms of the **GNU General Public License v3.0** (or any later version), as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.

## Acknowledgements

- **[Pedalboard2 by Niall Moody](https://www.niallmoody.com/work/pedalboard2/)**, the original application this project recreates
- [egui / eframe](https://github.com/emilk/egui), [cpal](https://github.com/RustAudio/cpal), [midir](https://github.com/Boddlnagg/midir), [rtrb](https://github.com/mgeier/rtrb), [hound](https://github.com/ruuda/hound), [roxmltree](https://github.com/RazrFalcon/roxmltree)
