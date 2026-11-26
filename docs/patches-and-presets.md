# Patch and preset management

Pedalboard Host separates **patches** from **presets**:

- A patch (`.pdl`) is a complete board state, including its processors, connections, names and tempo. A `.pdl` file can contain multiple patches.
- A preset stores one processor's sound or settings. Plugin presets use `.fxp` files and are kept in a library organised by plugin.

## Organise patches

Choose **Edit → Organise patches** to open the patch organiser. Select one or more rows, then use the buttons to:

- **Add** a new patch.
- **Copy** selected patches to the end of the list.
- **Remove** selected patches (at least one patch is kept).
- **Move Up / Move Down** to reorder the selected row.
- **Import...** to read patches from another `.pdl` file, then choose which patch to add. The imported patch is appended; the current patch remains active.
- **Export...** to save selected patches to a separate `.pdl` file. Exporting over the currently open patch file is refused to protect its other patches.

Double-click a patch name to rename it in place. Press **Enter** or click elsewhere to keep the edit; press **Escape** to cancel it. A new patch is shown as `<number> - <untitled>` until it is renamed.

## Use the processor preset bar

Open a plugin or built-in processor's editor to see its **Presets** bar. The list contains the plugin's own programs followed by its saved presets.

- Choose a program or preset from the list to load it.
- Type a name in the editable name field and click **Save** to save the current sound/settings. Saving under an existing name replaces that preset file.
- Click the folder button (**Import**) to load an `.fxp` file directly into the current processor. This does not add the file to the saved-preset list.

The bar is available for built-in processors as well as VST2 and VST3 plugins. The contents of an `.fxp` file depend on the target: plugin presets store plugin state, while built-in presets store the processor's settings in the host's preset format. A preset identified as belonging to a different plugin is not loaded.

## Manage the preset library

Open **Edit → User Preset Management**, or choose **User presets...** from a processor's context menu. Select a preset in the tree to enable its actions:

- **Rename** changes the preset's name.
- **Copy** creates a second preset in the same plugin folder.
- **Remove** deletes the preset.
- **Import...** asks which plugin or built-in processor the preset is for, then copies an `.fxp` file into that target's library.
- **Export...** copies the selected preset to a chosen `.fxp` file.

The manager's import is different from the preset bar's import: the manager adds a preset to the library, while the bar loads a file immediately without adding it to the list. Copying, renaming or importing with a name already in use is refused rather than overwriting the existing library entry.

Presets are shared with the original Pedalboard2 and live under:

```text
%APPDATA%\Pedalboard2\presets\<plugin>\<preset>.fxp
```

Each plugin has its own folder. The host migrates presets created by older versions of this rebuild into the shared library when it starts; an existing shared preset is left unchanged.

For implementation details, compatibility notes and measured UI behavior, see [`app/MECHANICS.md`](../app/MECHANICS.md).
