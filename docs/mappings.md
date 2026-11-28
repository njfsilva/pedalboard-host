# MIDI, OSC and application mappings

Pedalboard Host can be controlled from the keyboard, MIDI controllers and OSC messages. There are two kinds of mapping:

- **Processor mappings** control one processor's parameters in the current patch.
- **Application mappings** run menu, patch and transport commands, and apply across all patches.

## Inputs

- **MIDI**: select MIDI devices in **Options → Audio Settings**.
  - Program Change messages can switch patches.
  - MMC messages can control the transport.
- **OSC**: enable **OSC Input** and set the UDP port in **Options → Misc Settings**. The default port is `5678`, and multicast is optional.

## Map a processor parameter

Click **m** on a processor box to map MIDI CCs or OSC messages to that processor's parameters. CC mappings support latch mode and lower/upper bounds.

## Map a shortcut or controller to a command

Open **Options → Application Mappings** and click **+** beside a command, such as **Patch → Next Patch** or **Main Transport → Tap Tempo**. Choose an input, then click **OK**.

| Input | How to assign it |
|---|---|
| Keyboard | Choose **Add Key Mapping** and press the shortcut. If another command uses it, confirming moves the shortcut to the new command. |
| MIDI footswitch or controller | Choose **Add MIDI CC Mapping**. Select **MIDI Learn** and operate the control, or pick its CC number. Leave **Channel** on **Any** or choose 1–16. Each matching CC message with a value of 65–127 runs the command. |
| OSC | Choose **Add Open Sound Control Mapping**. Type an address such as `/pedalboard/next` or pick one from **Received addresses**. **Parameter** counts float arguments from zero. The command runs when the selected float exceeds 0.5. |

To remove one assignment, click it and choose **Remove Mapping**. **Reset to defaults** clears custom assignments and restores the original keyboard shortcuts.

## Where mappings are stored

Application mappings are saved in `%APPDATA%\PedalboardHost\AppMappings.xml`. On first use, the host imports any existing Pedalboard2 application mappings and controller assignments from older rebuild settings.
