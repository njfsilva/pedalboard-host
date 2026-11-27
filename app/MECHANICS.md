# Mechanics sheet

This sheet describes the original program, Pedalboard2 v2.09 (64-bit), and how the rebuild uses each value.

- **Addresses** are functions in `Pedalboard2-64bit.exe` (image base 0x140000000). They were read as Ghidra pseudocode.
- **"Measured"** means the value was taken from the pixels of the original's screenshot.
- **"Estimated"** means I looked for the value in the code without settling it, then chose one.

The same values are kept in `mechanics.json`.

## Main window: MainPanel::resized (0x1400358a0)

Symbols used below:

- `W` and `H` are the panel's width and height. The panel is the client area under the 24 px menu bar (measured).
- `cx` is round(W / 2).

| Element | Bounds (x, y, w, h) |
|---|---|
| Plugin field (scrolling view) | 0, 0, W, H-40 |
| "Patch:" label | 8, H-33, 48, 24 |
| Patch combo box | 56, H-33, 200, 24 (drawn at 55..253 in the picture) |
| Previous patch button | 264, H-33, 24, 24 |
| Next patch button | 288, H-33, 24, 24 |
| "CPU Usage:" label | W-236, H-33, 78, 24 |
| CPU meter | W-156, H-33, 150, 24 |
| "Tempo:" label | cx-169, H-33, 64, 24 |
| Tempo editor | cx-105, H-33, 52, 24 (text "120.00", allowed characters `0123456789.`) |
| Tap-tempo arrow | cx-49, H-27, 10, 16 |
| Play button | cx-18, H-38, 36, 36 |
| Return-to-zero button | cx+20, H-32, 24, 24 |

Other window values:

- **Client area:** 1010 × 573 px (measured).
- **Plugin field edges:** the original draws them one pixel lower than H-40: the scrollbar rims and the corner end at H-39 (measured). The rebuild draws the field down to H-39.
- **Scrollbars:**
  - 18 px thick, with arrow buttons 20 px long (measured).
  - Thumb and buttons have a light leading half and a darker trailing half, with rounded rims.
  - The track is shaded across its width.
  - The arrows are 2 px chevrons.
- **Field bevel:** a dark line along the top, then a light line; a light line on the left (measured).
- **Fonts:** Verdana, JUCE's default on Windows. Sizes: bottom bar 15, box titles 13, pin labels 12, hint 14 (JUCE's default, as the field's paint 0x140078d80 sets no font). Text baselines are rounded to whole pixels, as JUCE does.
- **Tooltips:** "Previous patch", "Next patch", "Tap tempo", "Play (main transport)", "Return to zero (main transport)".

## Menus (strings at 0x1404108a0–0x140411300)

- **File:** New, Open..., Save, Save As..., Save As Default, Reset Default, Exit.
- **Edit:** Delete selected connection(s), Organise patches, User Preset Management.
- **Options:** Misc Settings, Audio Settings, Plugin List, Colour Schemes, Application Mappings.
- **Help:** Documentation, Event Log, About.
- **Documentation** (its case in the menu handler at 0x140038f08; text "Loads the documentation in your default browser.", 0x140410f60): F1 is its key (0x10070, no modifiers, set at 0x140037706 for addDefaultKeypress at 0x14003770e). It takes the program's own file (getSpecialLocation 7), its folder, then "documentation" (0x140411308) and "index.html" (0x140411318). When that's a file (GetFileAttributesW without the folder flag) it opens it with URL::launchInDefaultBrowser (0x14011d660): ShellExecuteW with the default action and show mode 10, ignoring a failure. Otherwise a warning box titled "Documentation Missing" (0x140411350) says "Could not find documentation/index.html" (0x140411328). The original's folder ships that page; it opens no window of its own.
  - **Rebuild:** the same page beside the rebuild's program opens the same way. Where it isn't there, or the shell can't open it, the rebuild shows its own short guide instead of the warning (**rebuild's own**: the rebuild ships no documentation folder, and the guide says where to put the original's). The guide closes with Escape (**rebuild's own**).

## Dialog windows (showModalDialog 0x140030940, non-modal helper 0x140030b30, centreAroundComponent 0x14021f700)

- **Which:** seven dialogs go through JUCE's DialogWindow::showModalDialog (0x140030940). It builds a TempDialogWindow, gives it the content (0x14021cdc0), places it (0x14021f700) and runs the modal loop (0x1401acc50). Each is a desktop window of its own, not a panel inside the main window:
  - Patch Organiser (title 0x1404112d0, call 0x140038576), User Preset Management (0x140410d10, call 0x14003866b), Misc Settings (0x140410d88, call 0x140038854), Audio Settings (0x140410de8, call 0x14003895f), Application Mappings (0x1404112f0, call 0x140038cbf) and About (0x1404110b0, call 0x140038eb3). Each is on the scheme's "Window Background", looked up just before the call (0x1402829f8).
  - A box's "<name> Mappings" (" Mappings" appended in 0x140073980, call 0x140073b55), on a fixed 0xFFEEECE1 and centred on the plugin field.
- **Placement** (0x14021f700): centred on the component it's given, or on the active window when given none (0x14021f8a0). With no active window it's centred on the screen (0x1401abfc0). The centred bounds are then moved, and shrunk if bigger, to fit inside the monitor's usable area reduced by 12 px on every side (0x1401901c0 with 12, constrainedWithin 0x14022da20).
- **Frame:** the helper gives each one only a close button and always makes it resizable (setResizable(true, false), 0x14021d340).
- **Escape** is the helper's fifth argument. It closes Patch Organiser (1 at 0x140038563), User Preset Management (0x140038658), Misc Settings (0x140038841), Audio Settings (0x14003894b) and About (0x140038ea0). It doesn't close Application Mappings (0 at 0x140038cae) or a box's "<name> Mappings" (0 at 0x140073b37).
- **Non-modal windows** (helper 0x140030b30, constructor 0x140030840): Colour Schemes (0x140038b6c) and Event Log (0x1400392d1). Escape closes them, they're centred with the same 12 px gap and resizable, and the fourth argument keeps the window on top of every other window: on for the Event Log, off for Colour Schemes.
- **Plugin List** (constructor 0x1400319b0, opened at 0x140038a92 and brought to the front at 0x140038aa6): a window of its own titled "Available Plugins", 300 × 400 and centred, with minimise and close buttons, resizable, with the system's title bar. It goes back to its last place ("listWindowPos", 0x14021ddc0) and closing deletes it (0x140031fe0).
- **Taskbar:** JUCE's window flags (0x14021c5a0) always ask for a taskbar button, so each of the original's dialogs has one.
- **Message boxes** (AlertWindow keys 0x1402188d0, standard boxes made in 0x1401e4050). A button's key needs no Ctrl, Alt or Shift, and letters ignore case.
  - One button: Return and Escape press it.
  - Two buttons: the first gets Return and its first letter; the second gets Escape and its first letter, unless that letter is the first button's.
  - Three buttons: the first two get their first letters and the third gets Escape; Return does nothing.
  - A box built with its own buttons only answers their keys: Escape cancels it only when it has no buttons, and Return presses the button when there's just one. Closing a box any other way gives 0, the cancel answer (0x140216d20).
  - The plain message box (showMessageBox 0x140218ec0, which the preset errors call with the warning icon, 2 at 0x14008b99d; and 0x140218bd0, a copy fixed to the warning icon, called from nine places and for "Documentation Missing") has one button. Its text is "ok" (0x140425664) unless the caller names one, and the fixed copy never does.
- **Save prompt** (saveIfNeededAndUserAgrees 0x1402324e0, through showYesNoCancelBox 0x140219400): titled "Closing document..." (0x14042e138), it asks `Do you want to save the changes to "<name>"?` (0x14042e110) with "save" (0x14042e100), "discard changes" (0x14042e0f0) and "cancel" (0x14042436c). So S saves, D discards and Escape cancels. The name is the document's title (0x14002cd10): the file's name without its extension, or "Unnamed" (0x14040ffc0). Its only caller is the main window's close button (0x14001a970).
- **Rebuild:**
  - Every dialog, prompt and message the app draws is a desktop window of its own, owned by the window that opened it: it stays in front of that window and hides with it. A small main window never cuts one off. None has a taskbar button (**rebuild's own**: the original's have one).
  - Escape closes the windows the original closes with it, as listed above. While the person types in a text box or a drop-down list is open, Escape only leaves the text box or closes the list (**estimated** from how JUCE's text boxes and menus keep Escape; not read in this program).
  - The settings windows (Misc, Audio, Colour Schemes, Application Mappings) and About can be resized like the original's; what doesn't fit scrolls. The Event Log stays on top of other windows, as in the original.
  - The Plugin List is titled "Available Plugins". It opens at 680 × 470 because the rebuild's list has more columns, and it doesn't go back to its last place (**rebuild's own**).
  - Message boxes take the keys above, and the plain one's button reads "ok". It shows no warning icon (**rebuild's own**). The save prompt has the original's title, question, buttons and keys. It also comes before New and Open discard changes (**rebuild's own**: the original's New and Open don't ask).
  - A dialog opens centred on its owner, at least 12 px inside the screen's usable area and no bigger than that area; what doesn't fit scrolls.
  - The gap is counted from the window's whole frame. On Windows 10 and 11 that includes a few invisible resize pixels, so the gap you see at the sides and bottom is a little smaller (**estimated** equivalence).
  - Drop-down lists open under their box (above it when there's no room) and may reach the screen's edge. A dialog the person drags to the edge stays there when its contents change size.
  - With `--screenshot`, dialogs are drawn inside the main window so the picture shows them (**rebuild's own**).

## Colours: default scheme (0x140026970)

All values are ARGB.

| Name | Value | Name | Value |
|---|---|---|---|
| Window Background | FFEEECE1 | Field Background | FFF3F2EB |
| Text Colour | FF000000 | Plugin Border | FFB0B0FF |
| Plugin Background | FFFFFFFF | Button Colour | FFEEECE1 |
| Button Highlight | B0B0B0FF | Text Editor Colour | FFFFFFFF |
| Dialog Inner Background | FFFFFFFF | CPU Meter Colour | B0B0B0FF |
| Slider Colour | FF9A9181 | List Selected Colour | FFB0B0FF |
| VU Meter Lower Colour | 7F00BF00 | VU Meter Upper Colour | 7FFFFF00 |
| VU Meter Over Colour | 7FFF0000 | Vector Colour | 80000000 |
| Menu Selection Colour | 40000000 | Waveform Colour | FFB0B0FF |
| Level Dial Colour | FF8C8CCC | Tick Box Colour | 809A9181 |
| Audio Connection | FF8C8CCC | Parameter Connection | FFD3B3 darker(0.25) = FFCCA88F |

JUCE colour arithmetic truncates:

- darker(a): c' = c / (1 + a)
- brighter(a): c' = 255 − (255 − c) / (1 + a)

See-through colours blend as JUCE's software renderer does: each channel is premultiplied in sRGB values, c·a rounded as (c·a + 0x7f) >> 8 (PixelARGB::premultiply, 0x140146880: alpha 255 unchanged, alpha 0 gives black), and drawn as src + dst·(1 − a). So the Tick Box colour 809A9181 shows as CCC8C0 over white. egui's own conversion premultiplies in linear light and would draw it as EFE8DC, so the rebuild converts each colour when it draws it. Colour mixes (Colour::interpolatedWith) also work on the premultiplied values.

Colour-scheme files:

- The root element is `Pedalboard2ColourScheme`, with `<Colour name=".." value="ffrrggbb">` children.
- The file is named `default.colourscheme`.

## Plugin field, boxes, pins and cables

- **Box painting (0x140072960):**
  1. Black rounded outline at (1, 1, W-2, H-2), radius 5.
  2. White fill at (2, 2, W-4, H-4).
  3. A 4 px Plugin Border stroke at (3, 3, W-6, H-6).
  4. A title strip, 16 px plus a 5 px square-cornered skirt, in Plugin Border.
- **Box size (0x140073d10):**
  - Height = max(inputs, outputs) × 13 + 34 for the Audio, MIDI and OSC boxes, and + 60 for processors. The 13 is only used for the height; pins themselves step 12.
  - Width = max(title + 12, widest input name + widest output name + 30).
  - Built-ins (0x140074d56..0x140074d91, the processor branch skips the pin loops): t = title width + 20 (0x140284690); width = trunc(t + 20) when t > control width + 24 (0x14043d468), otherwise control width + 24; height = control height + 52 (0x34). So a long title makes the box title + 40 wide.
  - The control sits inside its box at x = trunc(W / 2) − trunc(control width / 2), y = 24 (0x140072569..0x1400725bb: getSize through vtable offset 0x138, then setBounds 0x1401abc10, setVisible and addChildComponent 0x1401ac730).
- **Pins (create 0x140074e40, constructor 0x140075510, paint 0x140075630):**
  - Each pin is a 10 × 12 child of its box at x = -5 for inputs or W - 5 for outputs, and y = 25 + 12 × i (25 = 0x19, step 0xC). The MIDI/parameter pin takes the slot after the audio pins. So pin centres sit exactly on the box's left and right edges.
  - Painting: a black ellipse outline at (1, 1, W - 2, H - 2) = (1, 1, 8, 10), 1 px wide (the 2.0 at 0x14042efc0), then the same ellipse filled with the Audio or Parameter Connection colour. The box clips its children, so a pin shows as a half-ellipse 4 px deep facing into the box, with a thin black rim.
  - Pin labels also step 12 px (measured from the original's picture).
  - Pin names are "Input N" / "Output N", or none when "Ignore pin names" is on. The MIDI/parameter pin is "param".
- **Cables (paint 0x1400767e0, shape 0x140076c90, hit test 0x140076a10, endpoints 0x140076a70):**
  - The centre line runs from pin centre to pin centre (each pin component's position + (5, 6)). It is a cubic whose two control points sit at the horizontal midpoint (factor 0.5 at 0x14042ee44), level with each end.
  - It is stroked into a 9 px band (0x140438b98) with mitered joints and round ends.
  - Painting: first a 1 px black stroke around the band (Colours::black, width 0x14042ee48), or a 4 px red one when selected (Colours::red, width 0x14042f204), instead of the black. Then the band is filled with the Audio or Parameter Connection colour, brightened by 0.75 under the mouse (0x140433888).
  - A point hits the cable inside the band with 1 px tolerance, except within 5 px of its left end (component x > 10).
  - Stacking: cables are added to the field after the boxes they join, both when a patch loads (0x14007dd40) and when a connection is made (0x140080500), with no call that sends them to the back. So they draw on top of the boxes, their round ends cover the pins they plug into, and they take the mouse where they cross a box. The rebuild draws all cables above all boxes; the original would draw a box added later above older cables.
  - The original computes a wrong end point when the destination lies left of and below the source, so the curve leaves its bounds. The rebuild draws those cables like the rest.
- **Default boxes:**
  - Audio Input at (10, 10), Midi Input at (10, 120) and Audio Output at (892, 10), set in 0x14002cc30.
  - OSC Input at (10, 215), set in 0x14007b120.
  - The hidden "Midi Interceptor" node is not drawn.
- **Empty-field hint (PluginField paint 0x140078d80):** after filling Field Background, it draws "<double-click to add processor>" with drawText into the rectangle (400, 230, 300, 30) in field coordinates, so it scrolls with the field and doesn't follow the window size.
  - Justification 0x21 is centred-left: the text starts at x = 400 and is centred vertically in the 30 px. Paint sets no font, so JUCE's default 14 applies. The colour is Text Colour with its alpha multiplied by 0.5 (0x14042ee44).
  - It shows while the flag at PluginField + 0x298 is set. Only the constructor (0x140077df0) sets it, at 0x140078036. It's cleared after a processor is added from the add menu (0x140079160), after a dropped file adds one (0x14007a418), and by clearDoubleClickMessage (0x140080130), which switchPatch (0x140039720, call at 0x140039979) runs after loading a stored patch. So the hint doesn't come back when every processor is deleted.
  - The rebuild hides it the same way: when a processor is added from the panel, and when a stored patch loads.
- **Add menu (PluginField mouseDown 0x140078fc0):** opens at the mouse on the press that makes exactly a double-click (getNumberOfClicks() == 2), with any button, on empty field.
  - Contents: KnownPluginList::addToMenu (0x140101280) on the tree from createTree (0x140101400), sorted alphabetically. That's one flat list with no headings or folders. Each item is the plain name, enabled and unticked, with id = list index + 0x324503F4.
  - The list holds every scanned plugin plus the 7 built-ins. The MainPanel constructor (0x140032030) adds the built-ins with KnownPluginList::addType (0x1400ffad0) at 0x140034439..0x140034514, in the order Level, File Player, Output Toggle, VU Meter, Audio Recorder, Metronome, Looper (category "Pedalboard Processors", format "Internal", 0x140062f80). The Audio, Midi and OSC boxes and the Midi Interceptor are not in it.
  - Sorting: by name with compareLexicographically (0x140120080). It skips leading characters that aren't letters or digits (iswalnum), then compares the rest with _stricmp, which lowers A-Z and compares bytes, so "_" sorts before the letters. Inserting uses findInsertIndexInSortedArray (0x140105e30), which puts an equal name after the ones already there.
  - Equal names therefore keep the plugin list's order. The saved list loads first (recreateFromXml 0x140100ec0, called at 0x140034302), and addType then keeps a saved built-in's place or appends it, so the built-ins lead unless the list was re-sorted. The rebuild lists the built-ins first, then the plugins in list order.
  - Rebuild additions (**estimated**): VST3 plugins get " (VST3)" after their name, since the original couldn't load them. Items show tips (built-in description, plugin maker). The list scrolls past 440 px instead of splitting into JUCE's columns. A "No plugins yet" line shows when no plugins are scanned.
- **Adding a processor (addFilter 0x14002c6b0):**
  - When the processor can't be created, a message titled "Couldn't create filter" (0x14040ffa8) shows the error (0x140218bd0). The rebuild uses that title and also names the plugin.
  - Otherwise it stores the click's whole-pixel x and y as the node's "x" (0x14040ff9c) and "y" (0x14040ffa0) properties, and wraps every processor except the I/O boxes in a bypass wrapper (0x1400013a0).
  - The new box (0x14007b550) reads them back with toInt and calls setBounds (0x1401abc10, at 0x14007b866), so its top-left lands on the click point. The rebuild floors the press point to whole field pixels and puts the box there.
- **Dropping files on the field (0x140079a10):** the original checks a dropped file's extension against .dll and the sound types .wav, .aif, .aiff, .ogg, .flac and .wma (strings 0x140417370..0x140417398, 0x140283690), and adds a box where it lands. Plugins go through addFilter. A sound file gets a processor built for it (0x140064150, used only here, probably a File Player), added through 0x14002c9a0. **Not in the rebuild yet.**
- **Double-clicks (JUCE getNumberOfMultipleClicks, 0x1401b4ae0):**
  - The last 4 presses are kept.
  - An earlier press i (1 = the press just before) joins the current one when all of these hold:
    - it came less than 400 ms × min(i, 2) before it (imul at 0x1401b4b42);
    - it lies less than 8 px away on each axis (0x1401b4b74, 0x1401b4b84);
    - it used the same buttons;
    - it landed in the same window.
  - Counting stops at the first press that doesn't join.
  - The system's double-click time isn't read.
  - Only a count of exactly 2 opens the menu. A quick third press closes it and leaves it shut, as in the original. The rebuild's "same window" means the field versus a dialog or menu over it.
- **Right-click menus (rebuild additions, all **estimated**):** the original has no single-right-click menu. No box menu strings exist; "Open plugin editor" (0x140416940) and "Open mappings editor" (0x140416958) are only the button tips in 0x1400716c0. The rebuild adds:
  - A right-button press on empty field opens the add panel at once, on the press like the original's double-click.
  - A right-click on a processor box opens its menu when the button comes up within 6 px of the press; further, it is a right-drag that moves the box. The menu has Open plugin editor, Open mappings editor, Programs (when the plugin has more than one), User presets... (plugins only), Bypass, then Delete.
  - Right-clicks on cables and on the Audio/Midi/OSC boxes do nothing.
  - One menu shows at a time. It closes on a choice, on a press outside it (which still acts on what it lands on), on Escape, when the window loses focus, when its box is deleted, or when another patch loads.
  - Presses on dialogs or other menus over the field don't count as field presses; presses on tooltips do.
  - The field reads the mouse presses itself rather than egui's click and button-held state, so a button release lost outside the window can't leave a menu stuck or swallowing clicks.
- **Box buttons (PluginComponent constructor 0x1400716c0):** on every box except Audio Input, Midi Input, Audio Output and OSC Input. Bounds are in the box component's coordinates (W × H):
  - "e": TextButton (10, H − 30, 20, 20), tip "Open plugin editor", on every processor (see Editor windows).
  - "m": TextButton (32, H − 30, 24, 20), tip "Open mappings editor".
  - Bypass: DrawableButton "BypassFilterButton" (style 3, a picture on a button face) at (W − 30, H − 30, 20, 20); a click toggles. The rebuild draws a "b" on the face, bold while bypassed (the picture is **estimated**).
  - Delete: DrawableButton "DeleteFilterButton" (style 1, the bare picture) at (W − 17, 5, 12, 12). A cross at half alpha, white normally and black under the mouse, moved 1 px down and right while pressed.
  - Title label: (5, 0, W − 10, 20) on I/O boxes and (5, 0, W − 17, 20) on processors, bold. The decompile reads size 14 (DAT_140284694); the original's picture measures 13, which the rebuild uses.

## Tap tempo (0x140089060, display 0x140088e30)

- The last 4 tap times are kept.
- The history resets if any gap is ≤ 0 or longer than 2.0 s.
- BPM = 60 / mean of the positive gaps, applied only when > 0, and shown with 2 decimals.
- The default tempo is 120.00.

## Built-in processors

| Processor | Rule | Source |
|---|---|---|
| Level | 2 in / 2 out; out = in × level × 2; default level 0.5 (unity) | 0x140062ed0, 0x140063200 |
| Output Toggle | 1 in / 2 out; fade moves 0.001 per sample; out1 = in × (1 − f), out2 = in × f | 0x1400660e0, 0x140066410 |
| VU Meter | 2 in / 0 out; per-sample peak hold that falls 1e-5 per sample; drawn in the VU lower/upper/over colours | 0x140067350, 0x1400676b0 |
| File Player | 0 in / 2 out | 0x140063fa0 |
| Audio Recorder | 2 in / 0 out; writes a 16-bit stereo WAV at the device rate | 0x1400682c0, 0x140068610 |
| Metronome | 0 in / **1 out**; see below | 0x14006a0c0, 0x14006ab90 |
| Looper | 2 in / 2 out; see below | 0x14006c750, 0x14006dce0 |

### Parameters, in the original's order

MIDI and OSC mappings address parameters by number, so the order matters. Buttons act when the value is above 0.5 (DAT_14042ee44); "Return to Zero" acts on any value. Each class keeps its parameter names in slot 18 of its vtable (offset 0x90), getParameter in slot 19 and setParameter in slot 22 (offset 0xB0); the Output Toggle's slots hold the two functions decoded first (0x140062d00, 0x140066680), which confirms the pattern.

| Processor | Parameters (number: name) | Names / get / set |
|---|---|---|
| Level | 0 Level | 0x140062c00 / 0x140062c40 / 0x140063310 |
| File Player | 0 Play (toggles), 1 Return to Zero, 2 Looping, 3 Read Position (jump to that fraction of the file), 4 Sync to Main Transport, 5 Trigger (back to the start and play) | 0x140064b20 / 0x140064ce0 / 0x140064e80 |
| Output Toggle | 0 Output (on = value > 0.5, absolute, not a toggle) | 0x140062d00 / 0x140062d40 / 0x140066680 |
| VU Meter | none | — |
| Audio Recorder | 0 Record (toggles), 1 Sync to Main Transport | 0x140068f70 / 0x140069060 / 0x140069180 |
| Metronome | 0 Play (toggles), 1 (unnamed) beats per bar = int(value), 2 (unnamed) beat length = int(value), 3 Sync to Main Transport | 0x14006b020 / 0x14006b110 / 0x14006b250 |
| Looper | 0 Play/Pause (toggles), 1 Return to Zero, 2 Record, 3 Read Position, 4 Sync to Main Transport, 5 Stop After Bar, 6 Auto-play, 7 Bar Numerator = int(value), 8 Bar Denominator = int(value), 9 Input Level (gain = value × 2), 10 Loop Level (gain = value × 2) | 0x14006ef10 / 0x14006f4d0 / 0x14006f5a0 |

- Looper Record (0x14006f5a0): stops the loop playing, then starts a new take, or ends the take that is running. The original's record thread waits up to 2 s for playback to stop first.
- getParameter: buttons (Play, Return to Zero, Record, Read Position, Trigger) read 0; switches read 1.0 (DAT_14042ee48) or 0; the bar values read as numbers (the Metronome converts its two integers with cvtdq2ps); Level reads its level, and the Looper's levels read as gain / 2.
- Rebuild guard: the bar values are kept between 1 and 32, because a bar of 0 beats would never end.

### Saved settings

Each built-in saves one XML element; switches are written "1" or "0". Missing numbers read as 0 and missing switches as off, except as noted. Attributes the rebuild doesn't use (editor window bounds editorX/Y/W/H, the Looper's soundFile) are kept and written back unchanged.

| Element | Attributes | setState (vtable slot 32, offset 0x100) |
|---|---|---|
| Pedalboard2LevelSettings | level (**0.5 when missing**, DAT_14042ee30) | 0x140063a30 |
| Pedalboard2OutputToggleSettings | toggle (default 0) | 0x140066e20 |
| Pedalboard2FilePlayerSettings | file, looping, syncToMainTransport | 0x1400659d0 |
| Pedalboard2RecorderSettings | file, syncToMainTransport | 0x140069aa0 |
| Pedalboard2MetronomeSettings | syncToMainTransport, numerator, denominator, accentFile, clickFile | 0x14006be90 |
| Pedalboard2LooperSettings | syncToMainTransport, stopAfterBar, autoPlay (**default 1**), barNumerator, barDenominator, inputLevel and loopLevel (gains; **0.5 when missing**), soundFile | 0x140070760 |

Defaults of a new box (constructors): Looper input and loop gain 1.0, Auto-play on, Stop After Bar and Sync off, bar 4/4 (0x14006c750); Metronome 4/4 with Sync **off** (0x14006a0c0); Level 0.5 with 2 inputs and 2 outputs (0x140062ed0).

### Looper (process 0x14006dce0, record thread 0x14006d060)

- **Take start:** the length, read position and bar counters go to 0, playback stops, and the last 128 input samples are kept as the pre-roll.
- **Recording:** the raw input is stored; Input Level only scales what passes through to the outputs.
- **Stop After Bar:** a countdown starts at 0 and loses one sample's time (1 / sample rate) per sample. Each time it reaches 0 a beat begins, and (4 / bar denominator) × (60 / bpm) seconds are added, with bpm = 120 when the transport has no tempo (DAT 120.0). The sample that would begin beat numerator + 1 ends the take, so the loop is exactly one bar. The original compares with "== numerator + 1" in 32-bit floats; the rebuild uses "> numerator" and 64-bit counters.
- **Take end:** with Auto-play on, the loop starts playing at once, from the start, and fades in from silence over the rest of that block. The 128 input samples after the take become the post-roll.
- **Playback:** out = in × input gain + loop × loop gain. Over the first 128 samples of each pass the post-roll is added, fading out as (1 − j / 128) (DAT 1/128); over the last 128 the pre-roll is added, fading in as (1 − e / 127) (DAT 127). This smooths the join.
- Read Position jumps to that fraction of the loop. The Looper ignores Sync to Main Transport while playing.

### Metronome (0x14006ab90)

- One beat = (4 / denominator) × 60 / bpm; 120 bpm without a playhead.
- Beat 1 sounds the accent, the other beats the click: a sine of 1760 Hz or 880 Hz by magic circle (coefficient 2·sin(π·f / sr)) with an envelope falling from 1 by 1e-4 per sample.
- When an accent file or click file is chosen, that beat plays the file from its start instead of the sine (its first channel, mixed into the single output).

### File Player and Audio Recorder

- File Player: Return to Zero, then Trigger, then Read Position are applied at the start of a block. With Sync it follows the main transport's position.
- Sound files: the File Player and the Metronome's sound pickers offer the original's wildcard "*.wav;*.aif" (0x1402813a0), which JUCE reads with its WAV and AIFF formats. The rebuild reads WAV and AIFF, and uncompressed AIFF-C, choosing by the file's header. The Recorder's save dialog offers WAV only, since it always writes WAV.
- Audio Recorder (0x140068610): deletes an existing file first. If that fails it says "Could not delete existing file" and "Have you got the file open elsewhere? (e.g. in another File Player)". It then writes 16-bit stereo WAV at the device rate through a background writer.

Control sizes: each built-in's getSize is the last entry (offset 0x138) of its vtable, which was matched to its class through the MSVC type records.

| Processor | Control (W × H) | getSize | Vtable |
|---|---|---|---|
| Level | 64 × 64 | 0x140062bd0 | 0x140415dc8 |
| Output Toggle | 48 × 48 | 0x140062ca0 | 0x140416088 |
| VU Meter | 64 × 128 | 0x140062d60 | 0x1404161d8 |
| Metronome | 170 × 100 | 0x140062e20 | 0x140416498 |
| File Player | 300 × 100 | 0x140062dc0 (shared) | 0x140415f18 |
| Audio Recorder | 300 × 100 | 0x140062dc0 (shared) | 0x140416328 |
| Looper | 300 × 100 | 0x140062dc0 (shared) | 0x140416758 |

Differences in the rebuild:

- The original metronome assumes 44.1 kHz. The rebuild uses the device's real sample rate, so the pitch stays right at any rate.
- The rebuild's Looper keeps its loop in memory, allocated before playback so the audio thread never allocates or waits on the disk. It caps a loop at 120 s (the rebuild's own limit); reaching the cap ends the take with the original's out-of-memory message ("Looper Error" / "Not enough memory to continue. Recording stopped."). The original streams each take to disk with no fixed cap. The rebuild keeps the soundFile attribute in the patch but doesn't write or reload the loop audio, so a loop lasts until the program closes.
- With Auto-play, the original begins the new loop's fade-in at sample 0 of the block in which the take ends. The rebuild begins it at the sample where the take ended, so no part of that block plays twice.
- Stop After Bar uses 64-bit counters and a "> numerator" test instead of 32-bit floats and "== numerator + 1", so long bars don't drift.
- Bar values (Metronome and Looper) are kept between 1 and 32.
- Metronome custom sounds: the rebuild plays the file's first channel, resampled linearly to the device rate (**estimated**; the original's resampling wasn't decoded).
- File Player with Sync to Main Transport: the original starts and stops a synced player with the main transport and rewinds it on Return to Zero (change callback 0x140064660), and each player keeps its own position. The rebuild locks a synced player's position to the main transport's, which gives the same result when they start together from zero.
- Transport auto-stop (decoded, same result): a File Player that stops by itself, or a synced Recorder whose recording ends, calls 0x140041ed0. That lowers a countdown and stops the main transport when the countdown reaches 1. Play (0x140041f20) sets the countdown to the number of registered panels, and only the main panel registers (0x140041e70, called once at 0x1400340a0). So the countdown goes from 1 to 0 and never stops the transport. The rebuild doesn't stop it either.

### Controls inside the boxes

Bounds are (x, y, width, height) in the control's own space, which is w × h.

- **Tick boxes** (drawToggleButton 0x140024cf0, drawTickBox 0x140025020):
  - Font fs = min(15, h × 0.75). The box is tw = fs × 1.1 wide at x 4, centred vertically. Inside it sits a glass sphere 0.7 tw wide in the Tick Box colour, outlined 1.1 wide while hovered or held, else 0.5.
  - The tick is the path (1.5, 3) → (3, 6) → (6, 0) scaled by tw / 9, stroke 2.5. The text starts at trunc(tw) + 5.
- **Level dial** (LevelControl constructor 0x14005f560, timer 0x14005fa60; dial drawing 0x1401e9490):
  - Radius R = min(w/2, h/2) − 2, swept from 1.2π to 2.8π. The value band lies at 0.85 R and is 0.3 R wide. The pointer is 0.77 R long on a circle 0.2 R wide.
  - Outline black at alpha 0x66: 2.0 wide under the mouse, else 1.2. Fill in the Level Dial colour: alpha 1.0 under the mouse, else 0.7.
  - Rotary vertical drag, 250 px for the whole range (JUCE's default), range 0 to 2. A double-click returns to 1.0.
- **Transport pictures:**
  - Play is a triangle (−4, −5.67), (4.33, 0), (−4, 5.67).
  - Pause is two bars, x −4.33 to −1 and 1 to 4.33, y ±5.33.
  - Return to zero is a triangle (−6, 0), (3, −6), (3, 6) plus a bar at x 3.5 to 6.
  - Disabled pictures draw at alpha 0.4.
  - Record (a red dot, radius 5.5, 0xD02020) and Stop (a 10 × 10 square) are **estimated**.
- **Waveform display** (paint 0x14008d050, constructor 0x14008cd20; the Recorder's lighter version: constructor 0x14008d6d0, paint 0x14008d8b0; mouse 0x14008d580):
  - Window Background fill, black outline at alpha 0.25 (DAT_14042f4e0), the waveform in Waveform Colour inside (1, 1, w − 2, h − 2). Levels are rounded to 1/127, as in JUCE's thumbnail.
  - The playhead is a Text Colour line at alpha 0.25 at x = (w − 2) × position + 1.
  - The time "M:SS" is at font 10 (DAT_14042f520), right-aligned in (w − 53, 1, 50, 12).
  - A click at 0 < x < w − 1 seeks to x / (w − 2).
- **VU meter** (constructor 0x140061270, timer 0x140061f50, paint 0x1400613d0):
  - dB = 20 log10(peak), or −60 when the peak ≤ 0.001; fill = dB / 60 + 1. The over light turns on at dB ≥ 0.
  - Top strip topH = 10 when h < 128, else h × 0.078125. Meter height meterH = h − topH − 4. Two bars w/2 − 2 wide, the right one at x w/2 + 2.
  - Bars shade from VU Meter Upper (darkened by the fill) down to VU Meter Lower.
  - Lines at topH + meterH × 0, 0.1, 0.2, 0.4 and 0.8, labelled 0, 6, 12, 24 and 48dB. Label font 8 when w ≤ 64, w × 0.125 when w ≤ 192, else 24.
- **Output Toggle** (constructor 0x140060700): a DrawableButton with two pictures. The rebuild draws its own curve-and-dot picture scaled by min(w, h) / 48 (the picture is **estimated**).
- **File box** (JUCE FilenameComponent, constructor 0x1401d8470, set up at 0x1401ea3d0):
  - Wildcard "*.wav;*.aif" (0x1402813a0). An empty box shows "<no file loaded>" at alpha 0.5.
  - A "..." button with font min(15, h × 0.6), width trunc(text width + h), tip "click to browse for a different file" (0x140424a70).
  - It saves for the Looper and Recorder and opens for the File Player.
- **File Player** (0x14002a420):
  - File box (0, 0, w − 58, 24); Play (w − 54, 0, 24, 24), tip "Play/pause audio file"; Return to zero (w − 26, 0, 24, 24), tip "Return to the start of the audio file".
  - Display (0, 28, w − 2, h − 48).
  - Ticks: Sync (0, h − 23, 168, 24), tip "Sync file playback to the main transport"; Loop (176, h − 23, 56, 24), tip "Loop this file".
- **Audio Recorder** (0x140020040, resized 0x140020dc0): file box (0, 0, w − 28, 24); Record (w − 26, 0, 24, 24), tip "Record audio input"; lighter display (0, 28, w − 2, h − 48); Sync tick (0, h − 23, 168, 24) with no tip.
- **Metronome** (0x14000c5c0, resized 0x14000e360):
  - Play (0, 0, 24, 24), tip "Play/pause metronome".
  - The time signature fills y 24, height h − 48, at font max(h × 0.625, 14). The numerator is round(w × 0.4896) wide, the denominator starts at x round(w × 0.5104), and "/" sits at x round(w × 0.4301), round(w × 0.1399) wide.
  - Sync tick (0, h − 23, 168, 24), tip "Sync metronome playback to the main transport".
- **Looper** (0x1400060a0):
  - A disabled file box (0, 0, w − 84, 24). Play (w − 80), Return to zero (w − 52) and Record (w − 24), each 24 × 24 at y 0; Record's tip is "Record/stop recording a loop".
  - Display (0, 28, w − 2, h − 48).
  - Ticks at y h − 23: Sync (x 0, 168 wide); "Stop after bar" (x 176, 112 wide), tip "Stop recording after a bar has elapsed.".

## Editor windows ("e" button, 0x140073120)

- "e" asks the processor for its own editor (createEditorIfNeeded 0x1400f0df0). When there is none it builds the generic editor (NiallsGenericEditor 0x1400f6c10). Either one opens in a PluginEditorWindow (0x140075950), and the box's window is marked open. There is no "no editor" message.
- **The window around a plugin's editor** (PluginEditorWindow 0x140075950):
  - Filled with Window Background, with all three title-bar buttons (7). It is first set to 400 × 300, then takes the editor's size.
  - The person can drag it between 396 × 32 and 10000 × 10000 (setResizeLimits).
  - It can be resized only while "fixedSizeWindows" is off, and never for a plugin named "VAZPlusVSTi" (case-sensitive compare 0x1401382a0).
  - It stays on top while "WindowsOnTop" is on, and opens at the box's saved uiLastX/uiLastY (each at least 10).
- The built-in editors fill Window Background (paint 0x140060660) and refresh every 60 ms:

| Editor | Size | Constructor | Notes |
|---|---|---|---|
| Level | 192 × 192 | 0x14005faf0 | resized 0x1400600e0 |
| Output Toggle | 192 × 192 | 0x140060bf0 | resized 0x140061140 |
| VU Meter | 128 × 256 | 0x140062030 | resized 0x140062300 |
| File Player | 600 × 200 | 0x140060290 | its control inset 4 |
| Audio Recorder | 600 × 200 | 0x140062330 | its control inset 4 |
| Metronome | 400 × 200 | 0x1400626c0 | its control inset 4, with the sound pickers |
| Looper | 500 × 300 | 0x140008bf0 | resized 0x14000ba20 |

- **Level editor:** dial (0, 0, w, h − 20); a white value box (trunc(w/2) − 40, h − 20, 80, 20) showing the gain with 7 decimals at font 15.
- **Metronome editor:** Play moves to (w − 26, 0, 24, 24). "Click:" (0, 0, 48, 24) with a file box (56, 0, w − 84, 24), and "Accent:" (0, 24, 64, 24) with a file box (56, 24, w − 58, 24). The time signature fills y 48, height h − 71.
- **Looper editor:**
  - Its display is h − 155 high, and an "Autoplay" tick joins the row at (296, h − 23, 80, 24).
  - "Bar length:" (0, h − 127, 80, 24); numerator (8, h − 103, 80, 80); "/" (64, h − 111, 80, 80) in bold; denominator (112, h − 103, 80, 80). All three use font 105.
  - "Input level" with its dial at x w − 170 and "Loop level" with its dial at x w − 82. The labels are at y h − 127, 78 × 24; the dials are at y h − 103, 80 × 80.
- **Generic editor** (0x1400f6c10):
  - 400 wide and n × 25 high, kept between 25 and 400; it scrolls beyond that. An empty parameter name shows "Unnamed".
  - Each row is 25 high (0x1400f6790; PropertyComponent 0x1401fcc50) and refreshes every 100 ms. The row background (0, 0, w, h − 1) is 0x66FFFFFF (0x1401ef580). The name is black at font min(h, 24) × 0.65 = 15.6 (0x1401ef5f0).
  - Name width textW = min(200, w / 3); the slider is at (textW, 1, w − textW − 1, h − 3) (0x1401ef710).
  - The slider (ParamSlider 0x1400f6a10, vtable 0x14041d448) is a bar from 0 to 1. Its text can't be typed into, and it ignores the mouse wheel, so the wheel scrolls the list. It shows the processor's own text for the value (0x1400f6b90) and sends changes as they happen (0x1400f6b10).
  - The bar is 0xFFBBBBFF from JUCE's colour table (0x140424680), drawn as (194, 194, 255) idle and (154, 154, 203) under the mouse or while dragging.
- Differences in the rebuild:
  - Built-in and generic editors open as separate desktop windows owned by the main window. In `--screenshot` mode they are drawn inside it. The rebuild preserves a built-in processor's saved editorX/Y/W/H values but does not restore editor positions or reopen editors when loading a patch.
  - VST2 plugins that give an empty parameter name show "Parameter N" instead of "Unnamed".
  - The original's editors poll their processor on timers (60 ms for built-in editors, 100 ms for generic editor rows). The rebuild redraws the whole window every 33 ms while audio runs, so values show at least as quickly.

## Mappings

- **MIDI CC to parameter, ccReceived (0x140052ba0):**
  - With latch on, a non-zero CC toggles the parameter.
  - Otherwise v = value / 127, and the parameter = lower + (upper − lower) × v.
- **Parameter index −1** is the bypass switch (0x140042290).
- **Mapping XML (0x140058ca0–0x1400595c0), kept inside the patch's `Mappings` element:**
  - `MidiMapping`: pluginId, parameterIndex, cc, latch, channel, lowerBound, upperBound.
  - `OscMapping`: pluginId, parameterIndex, address.
- **Application Mappings** lists the original File, Edit, Options, Help, Patch, Main Transport and Application commands. Click the plus beside a command to add a keyboard shortcut, a MIDI controller or an OSC address. Click an existing assignment to remove it. Reset to defaults restores the original shortcuts and clears custom assignments.
- **MIDI Learn** selects the next controller received from an enabled MIDI input. You can also choose any controller from 0 to 127. An assigned controller triggers its command at values above 64. The original listens on all channels. The rebuild also supports a specific channel from 1 to 16.
- **OSC assignments** accept a typed or previously received address and a zero-based float parameter index. A matching float above 0.5 triggers the command. Tap Tempo uses the arrival time of each input.
- **AppMappings.xml** stores keyboard command IDs in hexadecimal and MIDI and OSC command IDs in decimal. The rebuild imports the original file and older rebuild settings on first use, then saves edits in its own user data folder. The original behavior and file format are documented in the supplied ApplicationMappingsEditor, MidiCcAlertWindow, MidiMappingManager, OscMappingManager and App source files.
- **OSC:** the default port is 5678 (0x140081dc0). The default address for a parameter is "/<box name>/<parameter number>" (**estimated**).
- **Event log text** follows the original's strings, for example "MIDI CC message received: CC=.. val=.. chan=..".

## Patches and files

- **.pdl file save (0x14003b9c0) and load (0x14003c4f0):**
  - Root `Pedalboard2PatchFile`, which holds `Patch` elements with the attributes `name` (lowercase, string 0x14041080c) and `tempo`.
  - `DEVICESETUP` is written when "pdlAudioSettings" is on.
- **One patch: writer 0x14007d3f0, reader 0x14007dd40:**
  - A `FILTERGRAPH` of `FILTER` elements (uid, x, y, program, with `PLUGIN` and `STATE` children) and `CONNECTION` elements (srcFilter, srcChannel, dstFilter, dstChannel). MIDI/parameter channel = 0x1000.
  - `Mappings`, holding the `MidiMapping` and `OscMapping` elements.
  - `UserNames`, holding one `Name` element per renamed box: `id` = the box's uid, `val` = its name (strings 0x140417414 "Name", 0x14041741c "val", 0x140417330 "id"). The box title shows this name instead of the processor's.
  - With no `FILTERGRAPH`, the reader sets up the default boxes (0x14002cc30). The reader finds children by name, so their order doesn't matter.
  - The original has no "Rename" command for boxes (that string belongs only to presets, 0x14008a320 / 0x14008b450). The rebuild renames a box by double-clicking its title, the usual JUCE editable-label gesture: Enter or a click elsewhere keeps the name, Escape cancels, an empty name restores the processor's own (**estimated**).
- **Plugin state:** stored as JUCE base64 ("<size>." followed by 6-bit little-endian groups). XML state uses copyXmlToBinary (magic 0x21324356).
- **Patch switching:** the patch list ends with a "<new patch>" item, so with P patches it holds P + 1 items.
  - Next (0x14003930a) moves to the following patch. From the last patch it wraps to the first only when "LoopPatches" is on (read at 0x1400393a3); otherwise it stays.
  - Previous (0x1400393c1) moves to the patch before. From the first patch it wraps to the last only when "LoopPatches" is on (read at 0x1400393e9).
  - **MIDI program change:**
    - It is only acted on while "Program Change messages switch patches" is on (read at 0x140054c59).
    - The MIDI thread queues the number, and a 30 ms timer (timer 1, 0x140039d00) takes it off the queue (0x14003a141).
    - If 0 ≤ n < P, it selects patch index n, so program 0 is the first patch, and hides the notice.
    - Otherwise (0x14003a185) it stores n, shows the notice (or repaints it if already shown) and starts timer 2 for 5000 ms (0x14003a1aa). Timer 2 hides the notice.
    - Another out-of-range number restarts the 5 seconds.
  - **The notice:** a call-out box pointing at the patch list.
    - It is created in the MainPanel constructor (0x140034aa4–0x140034b08).
    - Its content is 250 × 150 (0x1400315fd), painted by 0x140031640.
    - "Out of bounds", "MIDI Program Change" and "received:" are in font 24, in 50-px rows at y 0, 24 and 48. The number is in bold 48 at y 88. Every line is centred.
    - The box's fill, outline, shadow and arrow size are **estimated**: the rebuild uses the window colours and the stock JUCE call-out geometry (20 px border, arrow at the patch list's top).
  - MMC stop/pause, play and rewind drive the main transport while "mmcTransport" is on (read at 0x140054b73). The mapping of messages to actions is **estimated**.

## Patch Organiser (constructor 0x14005c440, resized 0x14005d760, paint 0x14005d660, buttonClicked 0x14005d870)

- **Opening** (menu command 0x1400384e4): the "Patch Organiser" menu item (string 0x1404112d0, help text "Opens the patch organiser." at 0x140410c58).
  - The constructor asks for 600 × 400 (0x14005d499), but the menu command makes the content 400 × 300 (bytes at 0x1400384d6, setBounds 0x1401abc10) before showing it.
  - The dialog helper 0x140030940 shows it: titled "Patch Organiser" (used at 0x140038533), on "Window Background", centred, with a resizable border (0x14021d340, 0x1401e00d0). Escape closes it. It's only kept on top when the program isn't running stand-alone, so not here.
  - It runs modally (runModalLoop 0x1401acc50) and is built new each time, so it always opens with nothing selected.
  - **Rebuild:** a resizable window that opens centred at 400 × 300 with nothing selected.
    - It leaves the main window usable (**rebuild's own**). Escape closes it, as in the original (1 at 0x140038563), unless the person is renaming a patch, when Escape only cancels the rename.
    - It can't be made smaller than 200 × 248, which keeps Move Down in view. The original sets no limit (**estimated**).
- **Patch list** ("patchList", 0x1404143d0, created at 0x14005c57d):
  - It sits at (8, 8, w − 106, h − 15) (resized) on "Dialog Inner Background".
    - Its 1 px outline (constructor) is JUCE's ListBox outline colour 0xb2808080 (colour table 0x140424648).
    - Several rows can be selected at once (constructor).
  - **Rows** are 22 px high (ListBox constructor 0x140202670, value at 0x140202755). Each is a JUCE Label (constructor 0x1401fffb0):
    - 15 px text (0x14043394c), centred left;
    - 5 px in from the sides and 1 px from the top and bottom;
    - squeezed sideways down to 70% when the name is too long.
  - **Row text** is the patch's "name" attribute (0x14041080c, refreshComponentForRow 0x14005f020), with no number.
    - The original names a new patch "N - <untitled>" (comboBoxChanged 0x140035df0).
    - The rebuild stores new patches as "<untitled>" and shows them here as "N - <untitled>".
  - **Painting** (paintListBoxItem 0x14005ee30):
    - Every second row (1, 3, 5… counting from 0) is tinted black at alpha 0x10 (0x10000000, at 0x14005efdd).
    - A selected row is filled top to bottom with a gradient (0x1401470f0). It runs from "List Selected Colour" brightened by 0.4 (0x14042f6b8) to the same colour darkened by 0.125 (0x14043e348).
      - Colour::brighter (0x140146b90) gives 255 − (255 − c) ÷ 1.4.
      - Colour::darker (0x140146c30) gives c ÷ 1.125.
    - The text is white on selected rows and black on the others (Label text colour, id 0x1000281).
  - The scroll bar is JUCE's usual 18 px (**estimated**).
- **Clicks** (listBoxItemClicked 0x14005f2c0, backgroundClicked 0x14005f370):
  - A click selects only that row.
  - With Ctrl the row joins the selection. Ctrl can't take a row out again, because the handler selects the row right after JUCE flips it.
  - Shift works like a plain click, and a right-click selects too.
  - Clicking the empty space below the rows clears the selection (deselectAllRows 0x1402030e0).
  - Selecting never switches the playing patch.
  - The rebuild doesn't move the selection with the arrow keys (**rebuild's own**).
- **Renaming in place:** double-clicking a row opens a text box over it. The Label edits on double-click only (Label::showEditor 0x140200d40, createEditorComponent 0x1402012d0).
  - All the text is selected and the box takes the keyboard.
  - A click anywhere else keeps the new name (showEditor's modal state; losing the keyboard doesn't discard it, refreshComponentForRow). Return keeps it too and Escape drops it, as JUCE's Label does (**estimated**).
  - **The box** (TextEditor constructor 0x14020c050) has a 1 px border (3 px on the right) and a 4 px indent, so its text starts 5 px in from the left and the top.
    - The Label gives it black text on a clear background, so the row's fill shows through.
  - **While it has the keyboard** (drawTextEditorOutline 0x1401e7b60), it gets:
    - a 2 px outline in "Menu Selection Colour" (the original's look, 0x1400211e0);
    - a 4 px inner shadow of 0x38000000 (0x14042458c) at 0.75 strength (0x140433888).
    - The shadow is drawBevel (0x1401f0d10): ring i of 4 at strength (4 − i) ÷ 4, its sides at 0.75 of that. It's drawn 2 px taller than the box, so most of its bottom edge falls outside.
  - Selected text sits on "Button Highlight" (0x1400211e0). The caret and the selected text keep the default black (colour table 0x140424594 and 0x140424574).
  - egui places text on whole pixels, so the rebuild's text moves down 1 px when the box opens, where the original's moves 1.5 px. egui's selection band is also a little taller than the 15 px line (**estimated**).
  - labelTextChanged (0x14005f380) applies the new name: it renames the patch and its entry in the main window's patch list.
    - The original doesn't mark the file changed.
    - The rebuild does, so a rename isn't lost when the program closes (**rebuild's own**).
- **Buttons:** 82 × 24 at x = w − 90 (resized). The labels and tooltips are set in the constructor.

  | Button | Label | Tooltip | y in the original | y in the rebuild |
  |---|---|---|---|---|
  | Add | "Add" | "Add a new patch" (0x140414500) | 8 | 8 |
  | Copy | 0x140414420 | "Duplicate selected patch" (0x140414520) | 40 | 40 |
  | Remove | 0x140414450 | "Delete selected patch" (0x140414558) | 72 | 72 |
  | Import... | 0x1404144e8 | "Import patch from another .pdl file" (0x1404145d0) | 104 | 104 |
  | Export... | rebuild's own | "Export selected patch to a .pdl file" (**estimated**) | none | 136 |
  | Move Up | 0x140414480 | "Move patch up" (0x140414588) | 152 | 184 |
  | Move Down | 0x1404144b0 | "Move patch down" (0x1404145a8) | 184 | 216 |

- **Separator:** a line at (w − 89, 140, 80, 1) in black at alpha 0x40 separates Import... from the move buttons (paint).
  - The rebuild draws it at y 172 and moves both move buttons 32 px down to make room for Export... (**estimated**).
- **Button text** (drawButtonText 0x140021bb0):
  - "Text Colour", at half strength when the button is greyed out;
  - 8 px in from each side of an 82 × 24 button;
  - 1 px right and down while the button is held.
  - The original allows two lines. The rebuild keeps one line, squeezed sideways when it doesn't fit, in its own 14 px button font (**estimated**).
- **Button rules (buttonClicked 0x14005d870):**
  - **Add** picks "<new patch>" in the main window's patch list (comboBoxChanged 0x140035df0): a new patch at the end, which starts playing.
  - **Copy** saves the playing patch first, then adds each selected row, lowest first, to the end of the list as "<name> (copy)" and marks the file changed (0x14003cf90). The selection and the playing patch stay.
  - **Remove** takes the selected rows out, last first, while more than one patch is left. With fewer than 2 patches it does nothing (the test at the start of its branch).
  - **Move Up and Move Down** swap the lowest selected row with its neighbour and leave that row as the only one selected. Move Up needs a row above it, and Move Down a row below it.
  - **Not copied from the original:**
    - **After Remove**, the original switches to patch 1 (setSelectedId(1)) and saves the old patch into a row number that has already shifted.
      - The rebuild keeps the playing patch.
      - If that patch is removed, the one above it plays (the one below for row 0).
    - **After a move**, the original switches to patch 1 and loses unsaved changes to the playing patch.
      - It sets its "don't save" flag first (MainPanel + 0x5030, also used at 0x140033d2c, 0x140039765, 0x140039772, 0x14005d72d, 0x14005df86 and 0x14005e240).
      - It doesn't mark the file changed.
      - The rebuild keeps the playing patch wherever it moves to, and marks the file changed.
    - **Move Down with nothing selected** blanks patch 1 in the original. The rebuild greys it out.
  - **Greyed-out buttons:** the rebuild greys out buttons that would do nothing. The original's handler just ignores those clicks (**estimated**, appearance only).
    - Copy, Remove and Export... need a selection.
    - Remove needs at least 2 patches.
    - The move buttons need room to move.
- **Import...** (the last branch of buttonClicked):
  1. **Choosing the file:** a file chooser titled "Select file to import patch from..." (0x140414630, used at 0x14005e2ab) with the filter "*.pdl" and no starting file (File()), so Windows picks the folder.
     - The rebuild starts in the open file's folder (**estimated**).
  2. **Reading it:** the file is read as XML (XmlDocument::getDocumentElement).
     - When that fails, the original does nothing.
     - The rebuild says "Could not import a patch from …" (**rebuild's own**).
  3. **Listing the patches:** every child of the root whose tag is "Patch" (0x140411484, compared with _stricmp, so case doesn't matter) is listed by its "name" attribute (0x14041080c).
     - The root's own tag isn't checked.
     - A patch with no name is listed blank in the original; the rebuild shows "<untitled>" (the original's own placeholder, 0x1404107f0) (**estimated**).
  4. **"Select patch..."** (0x140414690, used at 0x14005e948) is an AlertWindow (0x140216750) with no icon. It holds:
     - the message "Patches in file: " (0x140414678) followed by the file's name (getFileName 0x140112380);
     - a combo box named "patchName" (0x1404146b0, addComboBox 0x140217340) with the first patch selected;
     - "OK" (0x1404146bc, return value 1, Return key 0x0d) and "Cancel" (0x1404146c8, return value 0, Escape key 0x1b), both added by addButton 0x140216e60.
     - It runs modally (runModalLoop 0x1401acc50). The rebuild blocks the rest of the window while it's open.
  5. **OK:** the original copies the first "Patch" element whose name equals the chosen one (0x14012be90, then 0x14012b940) and passes it to addPatch (0x14003cbd0). addPatch:
     - adds it at the end of the patch list;
     - renames the main window's "<new patch>" entry to the patch's name and adds a new "<new patch>" entry after it;
     - marks the file changed;
     - doesn't switch to the new patch.
  6. **Differences in the rebuild:**
     - It takes the chosen row itself. This only differs when two patches share a name, where the original always takes the first (**estimated**).
     - With no patches in the file, the original's OK would copy nothing; the rebuild greys OK out (**estimated**).
- **"Select patch..." layout (JUCE AlertWindow::updateLayout 0x140217bc0):**
  - **Fonts:** the message font is 15 px plain: Pedalboard2's own look-and-feel (BranchesLAF, table at 0x140283998) returns Font(15.0) from slot 0x70 (0x140023670, 15.0 at 0x14043394c). The title is that font × 1.1 (0x14043b888), in bold.
  - **First wrap width:** min(300 + 2·floor(√(15 × widest)), floor(0.7 × screen width)) (0.7 at 0x14041db74). "Widest" is the wider of the title and the message, both measured in the message font.
  - **Line balancing** (createLayoutWithBalancedLineLengths 0x140188e30, constants 0.5, 10.0 and 0.9) stops at once. It tests longest ÷ shortest line > 0.9, which always holds, so the text keeps the first wrap width.
  - **Text:** the title rows start 10 px down, then a blank message-font row, then the message. Text bottom = 40 + floor(text height).
  - **Width:** max(350, text width + 40), at most 0.7 × the screen width.
  - **Height:** text bottom + 50 per combo box + 20 + the button height, at most the screen height − 50.
  - **Combo box:** at (round(0.1 w), text bottom, round(0.8 w), 22) (0.1 at 0x14042f1fc, 0.8 at 0x140435d68).
  - **Buttons:** each is 28 high (BranchesLAF slot 0x68, 0x1401e47d0, read by addButton 0x140216e60) and its text width + 28 wide (changeWidthToFitText 0x1401baa00). They sit 16 apart, centred, with their bottoms at round(0.95 h) (0x14043ce98).
  - **Colours:** window background fill with a 1 px grey (0x66) outline (**estimated**).
- **Export...** (**rebuild's own**: the original organiser has no export):
  - **What it writes:** the selected patches, in list order, in a new patch file. The playing patch goes in with its latest changes.
    - The root is "Pedalboard2PatchFile", with one "Patch" element per patch (name and tempo as Save writes them) and no "DEVICESETUP".
    - Open and Import... both read it.
  - **The file chooser** is titled "Select file to export patch to...". It adds ".pdl" when no extension is typed. It suggests:
    - for one patch, its name (or "Untitled") with ".pdl";
    - for several, "Patches.pdl".
  - **Safety:** it refuses to write over the open patch file, which would lose that file's other patches.
  - **No side effects:** it doesn't change the open file, its changed flag or the recent file.

## User presets (PresetBar 0x1400865f0, UserPresetWindow 0x14008a320)

- **Where they live:** `<application data>\Pedalboard2\presets\<plugin>\<name>.fxp` (root 0x1400887b0, "presets" 0x140418240), one folder per plugin, named as the plugin list names the plugin (for an old VST2 plugin, its file name without the extension). The original uses that name as it is (getChildFile 0x140112600, under the user's application data, special location 3 at 0x140133b70); the rebuild replaces characters a folder name can't have (**rebuild's own**). The rebuild uses the same folder, so both programs see the same presets.
- **Listing** (0x140088480): every "*.fxp" file (0x140418264) in the plugin's folder, named without the extension. The rebuild sorts the names ignoring case, then by case.

### The "Presets:" bar (constructor 0x1400865f0, resized 0x140087310, paint 0x140087250)

- **Size:** 396 × 32 along the top of every plugin window. The window around the plugin (EditorWrapper 0x140076240) is the editor's size plus 32 in height, and never narrower than 396.
- **Parts** (x, y, w, h, from resized 0x140087310; made in the constructor 0x1400865f0):
  - label "Presets:" (0x140417fa8, named "presetsLabel") at (0, 4, 64, 24), its text at (5, 5, 54, 22): font 15 (0x14043394c), black (0xFF000000 at 0x1404b3520) on no background, centred on the left, not editable;
  - combo box "presetsComboBox" (0x140417f88) at (64, 4, 272, 24), its text editable and centred on the left (justification 0x21); with an empty list it says "(no choices)" (0x140284010);
  - Import button (340, 4, 24, 24), a folder icon (openButton.svg), tooltip "Import preset from .fxp file" (0x140418008);
  - Save button (368, 4, 24, 24), a disk icon (newButton.svg), tooltip "Save current preset" (0x140417fe0);
  - both are DrawableButtons showing their icon on a button (style 3) in the scheme's "Button Colour".
- **Look** (paint 0x140087250): filled 0xEEECE1, then a 1 px stroke (1.0 at 0x14042ee48) of 0x20000000 along y 32 (0x14043a9c8). The stroke's upper half falls on row 31 and comes out 0xDFDDD3 over the fill, which is the colour and row the rebuild draws. The paint also fills a path with no area in 0x2AA545, which draws nothing.
- **The list** (fill 0x1400879d0): the plugin's programs, then its presets, numbered from 1. A program without a name is listed as " " (0x1404180a4), because JUCE's combo box leaves empty items out. When the window opens, item getCurrentProgram() + 1 is ticked without sending a change, and its name is in the box.
- **Program names** are the plugin's own (VST2: effGetProgramNameIndexed, and effGetProgramName for the current one, trimmed). The rebuild makes none up; it used to show "Program N", a text the original doesn't have. The box's Programs menu shows an unnamed program as its number alone (**rebuild's own**; the menu is the rebuild's).
- **Picking** (comboBoxChanged 0x140087470):
  - a program is chosen, and when a preset went over it before, the state kept for it is put back and forgotten;
  - a preset is read whole from the plugin's folder, its name given the extension "fxp" (0x140088010, 0x140418250), and handed to the plugin as the current program's state (0x140087e50; JUCE's VST host reads the first block in it);
  - before a preset goes over a program, the program's state is kept on the box (PluginComponent+0x1e0, filled by 0x140073ba0), but only when the item picked before was a program (lastId − 1 < number of programs, so also when nothing was picked yet), and only for the first preset over it (std::map::insert 0x1400776e0 keeps the first). A preset picked after a preset keeps nothing, even when that preset moved the plugin to another program;
  - picking the ticked item again, with its own text in the box, does nothing (JUCE's combo box sends no change);
  - typing in the box (an item number below 0) renames the current program, and the typed name is the one Save uses.
- **Save** (buttonClicked 0x1400876e0, then 0x1400881a0): makes the presets folder (0x140087c30) and the plugin's folder when they're missing, then writes the current program's state (getCurrentProgramStateInformation) to the box's text with the extension ".fxp" (0x140418258, through File::withFileExtension 0x140113e60) in the plugin's folder. Then it fills the list again and ticks the item that was ticked (getSelectedId 0x1401fe830, setSelectedId 0x1401fe970), so the box shows that item's name as the plugin now gives it, or nothing when no item was ticked.
- **Import** (buttonClicked 0x1400876e0): the file chooser "Select an .fxp file to import..." (0x140418060) with "*.fxp" (0x140418048), starting nowhere in particular (File::nonexistent); the chosen file is read whole and loads over the current program (0x140087e50) without being added to the list, and the presets folder is made when it's missing (0x140087c30). The box then shows the current program's name (ComboBox::setText 0x1401fec80): the item with that text is ticked, or none. The last picked item and what's kept for Picking stay as they were.
- **The file** is what JUCE's VST host writes for one program: an 'FPCh' chunk for plugins that keep their state in a chunk, otherwise an 'FxCk' list of the program's parameters.
- **Differences in the rebuild:**
  - VST3 plugins (the original has none) keep their whole state in an 'FPCh' file, with the first 8 hex digits of the class id as the plugin id (**rebuild's own**).
  - A preset saved by a different plugin (another id in the file) is refused with "This preset was saved by a different plugin." (**rebuild's own** message).
  - Save checks the name first: an empty name, a reserved one (CON, NUL, COM1…) or one with \ / : * ? " < > | is refused (**rebuild's own**). The original checks nothing (0x1400881a0): an empty box saves `<plugin>.fxp` beside the plugin's folder, where no list finds it.
  - The original cuts a name at its last dot before adding ".fxp" (File::withFileExtension 0x140113e60): "v1.2" is saved as "v1.fxp", and a preset whose file name has a dot in it, copied in from elsewhere, can't be loaded from the bar. The rebuild keeps the whole name (a bug, fixed).
  - The original adds to the end of a file that's already there (FileOutputStream 0x140133440: CreateFileW with OPEN_ALWAYS, then SetFilePointer to the end), so saving over a preset leaves the old one first in the file, and that's what loads. The rebuild writes the file afresh (a bug, fixed).
  - After Save the rebuild keeps the same preset ticked by its name. The original keeps the item's number, so when the new preset sorts before the ticked one, the tick moves to its neighbour.
  - After Import the rebuild reads the current program's name again and ticks that program. The original ticks the last item with that name, which is a preset when one has the program's name, and when the file renamed the program its item keeps the old name, so nothing is ticked. With no programs the rebuild empties the box and ticks nothing, as the original does when the plugin gives its current program no name.
  - Typing: the original renames the current program even to an empty text, and puts the text on the item picked last, even a preset's. The rebuild ignores an empty text and changes only the program's item (**rebuild's own**).
  - With no programs, the original ticks the first preset without loading it when the window opens, so picking it then does nothing. The rebuild ticks nothing (a quirk, not copied).
  - Presets earlier versions of the rebuild kept in their own folder move into the shared folder as .fxp files when the app starts (never in screenshot mode) (**rebuild's own**).
  - The bar follows a program chosen anywhere else (a mapping, MIDI, the box's Programs menu or the plugin's own window), so such a program counts as the item picked before when a preset goes over it.
  - The built-in processors' windows get the bar too, saving their settings as JUCE binary XML (**rebuild's own**).

### User Preset Management (constructor 0x14008a320, resized 0x14008b360, paint 0x14008b2e0, buttonClicked 0x14008b450)

- **Opening:** Edit > User Preset Management (0x140410d10) or the box menu's "User presets...". It is 400 × 300 (0x1400385df–0x140038601) and goes through the same dialog helper as the organiser (0x140030940, called at 0x14003866b): centred on the active window (0x14021f700, see Dialog windows), resizable (0x14021d340), only a close button (4), Escape closes it (flag 1 at 0x140038658), and it blocks the main window until it's closed (runModalLoop 0x1401acc50). The original builds a new window each time; the rebuild brings an open one to the front.
- **Look** (paint 0x14008b2e0): the whole window 0xEEECE1, whatever the colour scheme, and a 0x40000000 shade 1 px larger than the tree all round.
- **The tree** "presetList" (0x1404185c0) at (8, 8, w − 106, h − 15):
  - its root is hidden (RootItem 0x140089fd0); under it, a folder per plugin (PluginItem 0x140089bc0, its presets listed with "*.fxp" when it opens 0x140089ce0, painted bold 0x140089e60) and its presets (PresetItem 0x140089870, painted plain and 4 px in, 0x140089980);
  - rows are 20 high (0x14001bc50), each level 24 px further in, names 16 px (0x14042ef7c);
  - the open/close box is 11 px (0x1401e6110), filled 0xE5FFFFFF with a 0x80000000 outline; the lines are 0x4C000000 (0x1404245c0, paint 0x140215a20);
  - a press on the box, or a double-click on a folder's name, opens or closes it (mouseDown 0x140212370, double-click 0x140215a70).
- **Buttons:** 82 × 24 at x w − 90, at y 8, 40, 72, 104 and 136 (resized 0x14008b360), always on:

| Button | Tooltip | String |
|---|---|---|
| Rename | Rename the selected preset | 0x140418750 |
| Copy | Duplicate the selected preset | 0x1404185d8 |
| Remove | Delete the selected preset | 0x140418618 |
| Import... | Import a preset from an .fxp file | 0x140418650 |
| Export... | Export the selected preset to an .fxp file | 0x1404186c0 |

- **What they do** (buttonClicked 0x14008b450). All but Import do nothing unless a preset is selected (a dynamic_cast to PresetItem).
  - **Copy** (0x14008b4ce): the box "Copy Preset" (0x1404187f0) asks "Enter a name for the duplicate preset:" (0x1404187a0) in an empty text box "presetName" (0x140418808), with Cancel (result 0, no key) and Ok (result 1, Return). Ok copies the file to `<text>.fxp` (0x140418258) in the same folder.
  - **Rename** (0x14008c926): the box "Rename Preset" (0x140418ae8) asks "Enter a new name for the selected preset:" (0x140418a90) the same way. Ok moves the file to `<text>` with the extension "fxp" (withFileExtension, 0x140418250; moveFileTo).
  - **Remove** (0x14008b8ec): deletes the file at once, without asking. When that fails: a warning "Preset Deletion Error" (0x1404188c0), "Could not delete preset from the filesystem. Check your permissions." (0x140418830). Then the plugin's folder is deleted when nothing is left in it (wildcard "*", 0x1404185b8).
  - **Import...:** the box "Import Preset" (0x140418958) asks "Select which plugin this preset is intended for:" (0x1404188f0) in a list "plugins" (0x140418978) with the first picked. After Ok, the file chooser "Import preset" (0x140418988, "*.fxp", flags 5 to open) and the file is copied into the plugin's folder. When the folder can't be made: a warning "Preset Import Error" (0x140418a40), "Could not create a directory for this plugin. Check your permissions." (0x1404189b0).
  - **Export...:** the file chooser "Export preset" (0x140418a68, "*.fxp", flags 0x86 to save and ask before writing over a file); the name gets the extension "fxp" and the preset is copied there.
  - After each, the tree is read again.
- **Differences in the rebuild:**
  - Copy, Rename and Import refuse to write over a preset with the same name ("A preset with that name already exists.") and check the name as Save does; the original writes over it (**rebuild's own**).
  - Import always copies the file. The original copies it only when the plugin's folder didn't exist yet, so importing a second preset for a plugin did nothing there (a bug, fixed).
  - Import's list holds the preset folder names of every plugin in the plugin list and of the built-in processors, sorted and without repeats; the original lists the plugin list's names in its own order (**estimated**).
  - Export adds ".fxp" to a name typed with another extension instead of replacing it, and suggests `<preset>.fxp`. If a file already has the name with ".fxp" added, Export leaves it alone and says so, because the chooser only asked about the name as typed.
  - After a change the tree keeps which folders are closed and what's selected (**estimated**); there's no keyboard control of the tree (**estimated**).
  - The window can't get smaller than 200 × 168, keeping Export... in view (**estimated**; the original has no limit). Like the rebuild's organiser, it doesn't block the main window (**rebuild's own**); Escape closes it, as in the original (1 at 0x140038658).
  - Escape in the Copy and Rename prompts: neither button has it, and AlertWindow's keys (0x1402188d0) only cancel on Escape when a box has no buttons, so it does nothing.

## Misc settings window (constructor 0x140081dc0, buttonClicked 0x140085980)

- **Size:** 560 × 530 (setSize at 0x140085077). The dialog helper makes the window resizable (0x14021d340); the controls keep the fixed places below. The rebuild's window can be resized too, and scrolls when it's smaller than its contents.
- **Painting (paint, 0x140085450):**
  - It fills the window with "Window Background".
  - The I/O panel is the rectangle (12, 132, width − 24, 82). It is filled with "Dialog Inner Background" and outlined 1 px in black at 25% alpha (0x40000000).
- **Fonts:**
  - Headings are bold 15: Font constructor 0x140183D70, bold style set at 0x140082513, 15.0 read from 0x14043394c.
  - Labels and the hint are plain 15 (Label::setFont 0x1402007B0).
  - Tick-box text is 15: drawToggleButton (0x140024cf0) uses min(15, height × 0.75), and these rows are 24 high.
- **Layout (resized, 0x140085670), as x, y, w, h:**

| Control | Bounds |
|---|---|
| **Open Sound Control Options** (heading) | 0, 8, 208, 24 |
| "OSC Port:" and its text box | 8, 40, 72, 24 and 80, 40, 64, 24 |
| "OSC Multicast Address:", its text box and "(leave blank for a one-to-one connection)" | 8, 72, 160, 24; 168, 72, 112, 24; 280, 72, 272, 24 |
| **Visible I/O Nodes** (heading) | 0, 104, 136, 24 |
| Audio Input, Midi Input, OSC Input (inside the panel) | 16, 136, 96, 24; 16, 160, 88, 24; 16, 184, 88, 24 |
| **Midi Options** (heading) | 0, 224, 104, 24 |
| Program Change messages switch patches | 16, 248, 288, 24 |
| Main transport responds to MMC | 16, 272, 232, 24 |
| **Other Options** (heading) | 0, 304, 150, 24 |
| Open mappings window on successful param connection | 16, 328, 376, 24 |
| Loop next/prev patch action | 16, 352, 208, 24 |
| Set plugin windows Always On Top | 16, 376, 256, 24 |
| Ignore plugin pin names | 16, 400, 176, 24 |
| Display tray icon (not OSX) | 16, 424, 200, 24 |
| Start in tray (not OSX) | 16, 448, 168, 24 |
| Force fixed-size plugin windows | 16, 472, 224, 24 |
| Save audio settings in .pdl files | 16, 496, 224, 24 |

- **Defaults:** the setting key, its default and where the code reads it. getBoolValue is 0x14010FB40, getValue 0x14010FA00 and PropertySet::setValue 0x14010FC00.

| Key | Default | Settings window | Other reader |
|---|---|---|---|
| AudioInput | on | 0x1400847d4 | 0x14002c372 |
| MidiInput | on | 0x14008484c | 0x14002c449 |
| OscInput | on | 0x1400848c4 | 0x140034938 |
| midiProgramChange | off | 0x140084907 | 0x140054c59 |
| mmcTransport | off | 0x14008497f | 0x140054b73 |
| AutoMappingsWindow | on | 0x1400849f7 | 0x140078341 |
| LoopPatches | on | 0x140084aa4 | 0x1400393a3 |
| WindowsOnTop | off | 0x140084ae7 | 0x140075bf4 |
| IgnorePinNames | off | 0x140084b5f | 0x140073ea1 |
| useTrayIcon | off | 0x140084cd9 | 0x140019bba |
| startInTray | off | 0x140084d64 | 0x140019c81 |
| fixedSizeWindows | on | 0x140084c0f | 0x140075d16 |
| pdlAudioSettings | off | 0x140084c58 | 0x14003bd1f |
| OSCPort | "5678" (text at 0x1404107A0) | 0x140081dc0 | 0x14003b190 |
| OSCMulticastAddress | "" (blank) | — | 0x14003b190 |

- **Text boxes:** the two text boxes apply on Return or when they lose focus (0x140086430, 0x140086540).
- **Rebuild's own rules (estimated):**
  - A port that isn't a number from 1 to 65535 goes back to the current one. The original would simply read it as a number.
  - "Start in tray" can only be ticked while the tray icon is on.

## Tray icon and closing the window (TrayIcon 0x1400891a0, closeButtonPressed 0x14001a970)

- **Start:** App::initialise (0x140019980) creates the main window hidden when "Display tray icon" and "Start in tray" are both on. With the icon on it also makes the TrayIcon (0x1400891a0) and keeps it at App+0x30.
- **The icon:** a picture stored in the program (a PNG at 0x14040b870, 0x211 bytes) with the tooltip "Pedalboard 2".
  - The original registers for Windows' `TaskbarCreated` message and adds the icon (0x140235e10). Its message handler adds the icon again when the taskbar is recreated (0x140236280).
  - A left double-click shows or hides the main window (mouseDoubleClick 0x140089830). A single click does nothing.
  - A right click (mouseDown 0x140089360) opens a menu: "Show Pedalboard" while the window is hidden, or "Hide Pedalboard", then a separator and "Quit" (0x140418450). Show/Hide toggles the window. Quit runs the close button's code.
- **Close button** (closeButtonPressed 0x14001a970, slot 46 of the main window's vtable 0x140282d68): with the icon on and the window showing, it only hides the window. Otherwise it asks to save (0x1402324e0, see Dialog windows) and quits (0x140222f80) unless that's cancelled.
- **File > Exit** (0x140038342) calls the same close button code, so with the icon on, Exit also only hides the window.
- **Minimise** is JUCE's own (slots 47 and 48 aren't overridden): it only minimises, icon or not.
- **Rebuild:**
  - On Windows, a fresh launch shows the tray icon and the main window, as requested. The original defaults both tray settings to off (0x140019980). Old rebuilds saved the old default even if the checkbox had never been touched. On upgrade, the icon is enabled once. A previously enabled hidden start is preserved when the old icon was enabled. An old disabled icon opens with the window showing. Later choices in Misc Settings persist.
  - The main window supplies its icon on its own thread. The tray thread handles registration and its popup menu, and posts commands back to the main window. When Windows Explorer restarts, the app registers the icon again. The user confirmed that the tray icon reappears on their desktop. If registration fails, it retries every 2000 ms (**estimated**, rebuild timing). Hiding on startup or close requires successful registration.
  - The icon uses the main window's own icon, the tooltip and menu say "Pedalboard Host" ("Show Pedalboard Host", "Hide Pedalboard Host"), and the menu opens when the right button is released (**rebuild's own**). A minimised window counts as hidden, so Show brings it up.
  - Quit always quits, asking to save first when there are changes. In the original it only hides a window that's showing (that quirk isn't kept, **rebuild's own**).
  - While the window is in the tray the app keeps working: MIDI program changes, MIDI and OSC mappings and the plugin windows' preset bars are handled at once, and the rest (plugins' chores, audio device checks) at the pace the window keeps while audio runs, every 33 ms. Windows doesn't paint a hidden window and a frame of the app waits for a paint, so the icon's thread posts the paints itself (**rebuild's own**; the original's timers don't depend on its window). A minimised window still gets its paints, so minimising needs nothing.
  - A dialog that opens while the window is in the tray (an audio device error, for example) waits until the window shows again.

## OSC input (0x140016fa0, 0x14003ab00, 0x14003ac90, 0x14003b190)

- **Socket (0x140016fa0):**
  - A UDP socket is bound to every network interface at the OSC port. A failure is logged with its error.
  - With a multicast address, the socket also joins that group on any interface (IP_ADD_MEMBERSHIP with the group and INADDR_ANY), so messages sent to the group arrive too.
- **Changing the port or address:** a new port (0x14003ab00) or multicast address (0x14003ac90) reopens the socket and saves the setting.
- **On and off:** OSC is only received while "OSC Input" is on (0x14003b190).
  - Turning it off stops the listener.
  - Turning it on reads OSCPort (blank means 5678) and OSCMulticastAddress, then starts listening again.
- **In the rebuild:** the old socket is closed before a new one opens. An address that isn't IPv4, or a group that can't be joined, is logged, and listening carries on without multicast.

## Rebuild-only design: low latency and VST3

These are new in the rebuild and are not taken from the original.

- **ASIO:**
  - The rebuild hosts the drivers listed under HKLM\SOFTWARE\ASIO directly over COM.
  - The driver's buffer-switch callback converts the samples in place and runs the patch at once.
  - There is no extra buffering, so the latency equals the driver's buffer.
- **Audio callback:**
  - It never allocates, waits on a lock or frees memory.
  - A new patch arrives through a lock-free queue as a precompiled plan (processing order, buffer pool, pointer tables).
  - Replaced plans go back to the window thread and are freed there.
  - Denormals are flushed to zero.
- **Windows Audio (WASAPI):** the fallback when no ASIO driver exists. The input ring is trimmed every callback so input latency can't build up.
- The audio thread applies **MIDI and OSC processor mappings**. The main window dispatches application commands using the mappings in `AppMappings.xml`.
- **Plugins:**
  - Both VST2 and VST3 load.
  - Plugin files are scanned in a separate short-lived process, so a crashing plugin can't take the host down. The original scans in its own process instead: it notes in a "RecentlyCrashedPluginsList" file next to its settings any plugin that crashed a scan, and scans those last the next time (plugin list window 0x1400319b0).

## Staying responsive: driver restarts, timer precision and plugin windows

### From the original

- **Timer precision: 1 ms.** At startup the original asks Windows for 1 ms timer precision with `timeBeginPeriod(1)` (function 0x140277130). The rebuild does the same at startup and releases it on exit.
- **How the original's timers run** (one shared "Juce Timer" thread made on first use at 0x1401412d0; startTimer 0x1401419c0; the thread's loop 0x1401414f0):
  - The thread reads the time to the millisecond (timeGetTime, at the 1 ms precision above) and counts every timer down. Between looks it waits until the first timer is due, at most 50 ms.
  - When a timer is due it posts one message (0xC400) to the window thread, which runs every due timer. It posts no other until that one has been handled, looking again every 4 ms, and posts it again if it hasn't been handled after 300 ms.
  - Posted messages reach the window thread ahead of Windows' own timer messages, so these timers keep their time to the millisecond.
- **Old-style (VST2) plugin windows** (VSTPluginWindow):
  - **Opening** (0x1400fdf40), only while the editor isn't open and the window exists:
    - It asks the plugin for its editor's size, opens the editor in the window, asks for the size again, then asks for the current program and whether the plugin wants keys.
    - The plugin's own window is the first child window (GetWindow GW_CHILD).
    - With no child window the editor counts as closed, the area becomes 300 × 150, and no timer starts.
    - Otherwise the area takes the child window's size. The exception: when the size the plugin reported is believable (each side 51–1999 px and both differ from the child's) or the child has no width or height yet, and it is more than 350 px off on either side, the child is set to the reported size and the area takes it. If the child won't take it, the plugin counts as refusing resizes from then on.
    - Each side is at least 32 px (0x1400fe280).
    - Its timer starts at 18 + a random 0–4 ms (JUCE Random, at 0x1400fe2ee): about 45–55 ticks a second.
  - **Each tick** (timerCallback 0x1400fde40): every 10th tick it checks the size (below). Then it sends the plugin its idle call (effEditIdle), unless one is already running in any plugin window (one shared flag, DAT_1404bc899).
  - **Size check** (0x1400fe480): while the window is showing, if the child window's size is above 0 each way and differs from the area, and the plugin doesn't refuse resizes, the area takes the child's size and the check runs again on the next tick.
  - **Moves and resizes** (componentMovedOrResized 0x1400fdbf0): the child window is laid over the whole area. Hiding the area closes the editor and showing it opens it again (0x1400fdce0); a new native window closes and reopens it (0x1400fdd30).
  - **Keys** (0x1400fe530): when the plugin doesn't want keys, key messages to its window are passed on to the window around it.
- **Opening an ASIO driver** (0x1400a4440):
  - With more than one clock source and none in use, it selects the first one and waits 20 ms.
  - It reads the driver's current rate and asks for the wanted rate **only if it differs**.
  - If the driver answers "no clock" (−995) and has clock sources, it waits 10 ms, selects the first one, waits 10 ms and asks once more.
- **Driver messages** (ASIO message callback at 0x1400a78b0; its jump table at 0x1400a7900 covers messages 1–8, anything else answers 0):
  - "Is message N supported?" (1): yes for 3, 2, 5, 6 and 9, tested in that order at 0x1400a78ce; no for the rest.
  - Engine version (2): answers 2 (0x1400a78f5).
  - Reset request (3), buffer-size change (4) and resync request (5): all go to the restart request at 0x1400a7920 (0x1400a78ed).
  - Latencies changed (6): answers 1 and nothing restarts (0x1400a78e7).
  - Time info (7) and time code (8): answer 0 (0x1400a78fb), so the driver keeps calling the plain buffer callback.
- **Restart request** (0x1400a7920): starts a 500 ms timer. Another request inside that time starts the wait over, so a burst of requests gives one restart.
- **Restart** (timer callback at 0x1400a5bb0): closes the device, opens it again with the same channels, rate and buffer size, and reconnects the audio. While the driver's own control panel is open, it waits another 100 ms and checks again.
  - In the rebuild the control-panel call holds the window thread until it returns, and restarts run on that thread, so a restart can't happen while the panel is open. That covers the 100 ms retry.
- **Sample-rate change notices are ignored** (the sampleRateDidChange callback at 0x1401adc20 is empty). A driver that needs a restart for a new rate sends a reset request as well.

### Rebuild-only rules (not in the original)

- **The reported crash** was Windows closing the program after its window stopped answering. The Windows log shows an "application hang" about 30 minutes into a session with the MOTU M Series ASIO driver (48 kHz, 128 samples) and two FabFilter VST3s loaded.
  - No crash dump exists, so the exact trigger is unknown.
  - Before this fix the rebuild reopened the device on the window thread as soon as a reset was flagged, with no delay and no limit, and it set the rate even when the driver was already running at it. A driver that answers every reopen with another request turns that into an endless loop.
  - The rebuild now follows the original's rules above, plus:
    - **Restart guard (estimated):** at most 4 automatic restarts in any 15 seconds. Later requests are held back until the older restarts are 15 seconds old. The first time this happens, a message asks the person to press Apply in Audio Settings if the sound stops. Opening the device by hand clears the count.
    - **A Windows Audio (WASAPI) failure** restarts the device once per failure, not on every frame.
- **Freeze and crash notes:** a watcher thread asks the window every second whether it is answering.
  - After 5 seconds without an answer, it writes a note and a memory snapshot to the program's settings folder. The note says what the program was doing: drawing, plugin idle, a VST2 plugin window's idle call, opening a plugin window, opening the audio device or loading a plugin.
  - It adds a line when the window answers again.
  - A crash also writes a note and a snapshot.
- **Plugin window frame rate:**
  - Every plugin in the person's list is a FabFilter VST3. These plugins draw on their own render thread, paced by Windows timers, so the host process's timer precision sets their frame rate.
  - Measured with audio running (Pro-Q 3 and Pro-L 2, audio fed in real time in 128-sample blocks at 48 kHz):
    - default 15.6 ms timer precision (before): about 13.5 pictures a second, about 78 ms apart (5 timer ticks);
    - 1 ms precision (the original's, now the rebuild's): about 49 a second, about 20.6 ms apart;
    - 0.5 ms precision (test only): 50.4 a second, still 20.6 ms apart.
  - About 50 a second is FabFilter's own pacing. Finer timers don't raise it, so the host can't take these plugins to 60.
  - With no audio playing, FabFilter redraws only about once a second, because nothing changes.
  - Windows 11 may ignore a timer-precision request from a program it thinks is hidden and silent. ASIO sound bypasses Windows, so the program looks silent. The rebuild therefore opts out of power throttling (execution speed and timer resolution) so the 1 ms precision holds.
  - **VST2 plugin windows** follow the original's rules above, except:
    - The idle call comes every 10 ms instead of every 18–22 ms, because the person asked for at least 60 pictures a second (**estimated**, not from the original).
    - Each plugin window keeps that time with a ticker thread of its own that works like the original's timer thread: one tick waiting at a time, a look every 4 ms while it waits, and a fresh post after 300 ms. Measured with nothing else running: about 99.7 idle calls a second. Windows' own 10 ms window timer gave about 63.5, because it fires only on Windows' 15.6 ms tick, whatever the timer precision.
    - Idle calls carry on when the plugin makes no window of its own, so a window it makes later is still picked up.
    - A size Windows won't give the window (smaller than its title bar allows, for example) is asked for once, not at every size check.
    - The window's frame is measured on the window itself, so the plugin's area is right at any display scaling.
    - Not carried over: the key hook (the rebuild's plugin windows have no shortcuts for the keys to reach), the "Opening VST UI" log line, the saved window position, and closing and reopening the editor when its window is hidden and shown (closing the window closes the editor).
  - **The main window doesn't wait for the screen refresh (vsync off).** Plugin windows share its thread, and in one measurement waiting for the refresh held that thread for 31 ms a frame.
