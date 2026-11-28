# Built-in processors

Double-click an empty part of the canvas to add a processor. Besides VST2 and VST3 plugins, the host includes these processors:

| Processor | I/O | Description |
|---|---|---|
| **Level** | 2 in / 2 out | Gain dial, 0–2× (unity at the centre) |
| **Output Toggle** | 1 in / 2 out | Crossfades the signal between two outputs |
| **VU Meter** | 2 in | Stereo peak meter with an over light |
| **File Player** | 2 out | Plays WAV/AIFF files, with looping and sync to the main transport |
| **Audio Recorder** | 2 in | Records 16-bit stereo WAV at the device's sample rate |
| **Metronome** | 1 out | Sine clicks or custom accent/click samples, any time signature |
| **Looper** | 2 in / 2 out | Bar-quantised looping with auto-play, input/loop levels and seamless crossfaded joins |

Notes:

- The Metronome uses the device's actual sample rate instead of assuming 44.1 kHz, as the original did.
- The Looper keeps loops in memory (up to 120 s). Loops aren't saved with the patch.
- Built-in processors can save and load presets through the same preset bar and manager as plugins. See the [patch and preset guide](patches-and-presets.md).
