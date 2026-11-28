# Audio setup and latency

Pedalboard Host plays audio through **ASIO** or **Windows Audio (WASAPI)**. You can use one system for both directions, or mix them.

## Audio systems

- **ASIO**: the host talks to the driver directly over COM and processes audio inside the driver's buffer-switch callback, without an extra host buffer. Use ASIO for the lowest latency.
- **WASAPI**: Windows shared-mode audio through the system's audio engine. It works with any Windows audio device.

Choose the device in **Options → Audio Settings**. Settings are stored in `%APPDATA%\PedalboardHost\settings.json`.

## Mix ASIO and Windows Audio

In **Options → Audio Settings**, choose the **Input system** and **Output system** independently, then select the corresponding devices. When both are ASIO, choose one driver for both directions. The mixed routes support:

- **ASIO input → WASAPI output**
- **WASAPI input → ASIO output**

In mixed mode, **Sample rate** and **Buffer size** configure the ASIO device. Windows Audio uses a supported rate and its own shared-mode buffering.

### The audio bridge

When the devices use different sample rates or run on independent clocks, the host bridges the audio between them:

- The bridge converts between the two sample rates and follows small differences between the device clocks.
- It keeps a small reserve sized from the real capture and render periods, so Windows Audio callbacks of varying length don't cause gaps.
- If capture stalls or falls behind, the bridge fades out, skips to the newest audio and fades back in, so you hear a short fade instead of a click.

## Latency display

The status bar shows how much delay the current setup adds:

- **Single audio system**: plug-in delay on the longest connected path. Device latency is not included.
- **Mixed audio systems**: an **estimated** input-to-output latency:

  ```text
  input-device delay + bridge buffering + plug-in delay (longest connected path) + output-device delay
  ```

  This is an estimate, not a guaranteed round-trip measurement.

## How the audio engine works

### Real-time audio

- The audio callback **never allocates, takes a lock or frees memory**.
- When a patch changes, the window thread compiles a new **plan** (processing order, buffer pool and pointer tables) and sends it to the audio thread through a lock-free queue (`rtrb`). Plans that are replaced are sent back and freed on the window thread.
- Denormals are flushed to zero.
- The audio thread applies MIDI and OSC mappings to processor parameters. The main window handles application commands. Tap Tempo uses the time each input arrived.

### Mixed-route bridge

- Audio passes between the two devices through lock-free ring buffers.
- The output side resamples with linear interpolation and steers its rate gently to follow the input clock. The correction is weighted by elapsed audio time, so a run of short callbacks can't push the clock off.
- The reserve is sized from the largest capture and render periods seen. A short render call doesn't shrink it.
- Capture losses are marked so the reader never interpolates across a gap. Backlog is only discarded when it builds up several periods deep.
- Every jump is covered by a 2 ms fade that carries across callbacks.

### Driver resets

ASIO driver reset requests are debounced by 500 ms, as in the original. As an extra guard, automatic restarts are limited to 4 in any 15 seconds, so a driver that keeps asking to be reset can't hang the program.
