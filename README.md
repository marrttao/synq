<div align="center">

# synq

**A terminal music player with live lyrics, GIFs and a sound visualizer**

Rust · Windows and Linux · runs entirely in the terminal

English · [Русский](README.ru.md)

</div>

---

synq plays your music and shows synced lyrics (karaoke-style, with the current word highlighted, or, if you like, just the word being sung in big letters). It finds lyrics on its own and draws a GIF and a sound spectrum next to them. Everything is configurable: the screen layout, colors, keys, and global hotkeys.

## Features

- **Library:** tags, search, playlists, favorites; new files are picked up on the fly (`F5`). Formats: mp3, flac, wav, ogg, m4a, aac.
- **Lyrics:** a `.lrc` file next to the track, embedded tags, [LRCLIB](https://lrclib.net), and if nothing is found, local Whisper speech recognition. Two display modes: all lines, or one word at a time in a large font.
- **GIFs and pictures:** as real pixels (sixel, kitty, iTerm2), as blocks, or as ASCII; fit, fill and stretch modes; a separate GIF for each playlist.
- **Sound visualizer:** even bars with smoothing and peak marks, two styles.
- **Layout editor:** panels are moved and resized with the mouse or the arrow keys, removed and added back, and can be stacked on top of each other.
- **Colors:** palettes of 2-12 colors and a color editor.
- **Keys:** rebindable; on Windows any key can be made global, so you can skip a track from a full-screen game.
- **Remembers everything:** the view, volume, layout, palette and keys between runs.
- **Never touches your music:** the player only reads the `music/` folder.

## Quick start

```powershell
git clone https://github.com/marrttao/synq.git
cd synq
.\install.ps1
setx PLAYER_HOME "$PWD"
```

Then open a new terminal window, put your music into the `music/` folder (and pictures into `gifs/`) and run:

```
synq
```

`F10` opens a menu with everything the player can do, and `?` shows all the keys. On Linux, install the dependencies (below) and run `cargo run --release`.

## Requirements for building

You need Rust (stable, 1.86 or newer): <https://rustup.rs>.

### Windows

1. Rust via `rustup` (or `winget install Rustlang.Rustup`).
2. **Visual Studio Build Tools** with the "Desktop development with C++" workload (MSVC and the Windows SDK). CMake comes with it.
3. For Whisper recognition (on by default):
   - **CMake** (part of Visual Studio, or `winget install Kitware.CMake`); it must be in `PATH`;
   - **LLVM / libclang** (`winget install LLVM.LLVM`). If `libclang.dll` lies in a non-standard place, set the `LIBCLANG_PATH` variable to its folder.

Terminal: Windows Terminal (recommended) or any other one with Unicode and color support.

### Linux (Debian / Ubuntu)

```bash
sudo apt install build-essential pkg-config cmake libclang-dev libasound2-dev
```

Fedora: `sudo dnf install gcc gcc-c++ make cmake clang-devel alsa-lib-devel`. Arch: `sudo pacman -S base-devel cmake clang alsa-lib`.

### Without Whisper

If you do not need speech recognition, CMake and libclang are not required:

```bash
cargo run --release --no-default-features
```

The player works fully, it just cannot transcribe lyrics by itself.

## Installing the `synq` command

In PowerShell, from the project folder:

```powershell
.\install.ps1
```

The script finds CMake and libclang for Whisper, builds the player and installs the `synq` command (into the Cargo bin folder, `~/.cargo/bin`). `.\install.ps1 -NoWhisper` builds without Whisper. A running `synq` must be closed first. To make the player use the music and settings of the project folder, run `setx PLAYER_HOME "C:/path/to/the/project"` once and open a new terminal window.

## Running

```bash
cargo run --release
```

The first build with Whisper is slow (whisper.cpp is compiled), the next ones are fast. The ready binary is at `target/release/synq` (`synq.exe` on Windows).

## What is remembered

Everything you choose while the player runs is still there at the next start: the picture mode (`v`), fitting and cropping (`V`, `z`/`Z`), the lyrics view (`w`, font `W`), the visualizer style (`x`), whether lyrics are shown (`l`), volume, shuffle and repeat. Layouts, palettes, keys and the chosen GIF are remembered too. Values in `config.toml` only set the very first start: a remembered choice wins.

## Folders

The working folder (BASE) is found like this: the `PLAYER_HOME` environment variable; otherwise the folder of the program if it contains `music/`; otherwise the current folder. Missing folders are created automatically.

```
BASE/
  music/            your music (mp3, flac, wav, ogg, m4a, aac); the player only reads it
  gifs/             your pictures: gif, png, jpg
  fonts/            your own fonts for the one-word lyrics (.ttf, .otf)
  playlists/<name>/ hard links to the tracks of a playlist (copies on another disk)
  favorites/        the same for favorites
  data/
    library.db      library, playlists, settings
    lyrics/         cache of found and recognized lyrics
    models/         Whisper models
    logs/           logs (synq.log)
  config.toml       settings
```

New files in `music/` are picked up on the fly. Files you deleted from `music/` by hand simply disappear from the lists (the database keeps a record of them in case the file comes back). Playlists and favorites can be browsed in the file manager: they are ordinary folders with tracks. The tracks there are hard links, so they take no extra disk space.

## Keys

**There is nothing to memorize:** `F10` (or `Ctrl+P`) opens a menu with everything the player can do and the key next to each item. `?` or `F1` shows all the keys. There is always a hint at the bottom. Keys also work in the Russian keyboard layout (each letter acts as the Latin one in the same place).

The highlighted row in a list always matches the playing track: when the track changes, the highlight follows it. `*` (favorite) and `a` (to a playlist) act on the highlighted track, and the player names it in its message.

The keys below can be rebound in `S`:

| Key | Action |
|---|---|
| `Space` | pause / play (if nothing is playing, starts the selected track) |
| `n` / `p` | next / previous track |
| `←` / `→` | seek 5 seconds |
| `Shift+←` / `Shift+→` | seek 30 seconds |
| `+` / `-` | volume |
| `s` | shuffle |
| `r` | repeat: off / all / one |
| `*` | add to favorites and back |
| `l` | show / hide lyrics |
| `w` | lyrics: all lines / only the word being sung (large) |
| `W` | font of the "word" mode: Lobster / Oswald / your own from the `fonts` folder (`.ttf`, `.otf`; read at start) |
| `[` / `]` | shift lyrics timing 0.1 s earlier / later |
| `f` | full screen: only lyrics and the GIF (leave with `f` or `Esc`) |
| `x` | sound visualizer: bars / mirror |
| `v` | GIF: pixels / blocks / ASCII |
| `V` | GIF: whole picture (fit) / fill with cropping (fill) / stretch (stretch) |
| `z` / `Z` | GIF larger / smaller: how many percent fit mode may crop |
| `{` / `}` | previous / next screen layout |
| `(` / `)` | previous / next color palette |
| `F5` | **refresh**: re-read `music/`, playlists and `gifs/` |

The other keys (not rebindable):

| Key | Action |
|---|---|
| `F10` / `Ctrl+P` | menu |
| `S` | key settings |
| `E` | layout editor |
| `C` | color editor |
| `a` | add to a playlist |
| `/` | search (Enter keeps the filter, Esc clears it) |
| `g` | choose a GIF (`Tab` in the window: for the playlist or global) |
| `t` | recognize lyrics again |
| `Tab` | Library / Playlists / Favorites |
| `Enter` | play, open a playlist |
| `c` / `e` / `d` | playlists: create / rename / delete |
| `Esc` | back, clear the filter |
| `Ctrl+↑` / `Ctrl+↓` | scroll lyrics without timings |
| `? / F1` | help (arrows scroll) |
| `q` | quit |

## Layouts and the editor

Three ready-made layouts: **Classic** (as before; narrow windows fold the panels away on their own), **Lyrics** (lyrics and a big picture, no list) and **Cinema** (the picture on top, lyrics under it). Switch with `}` and `{` (the keys can be changed in `S`).

`E` opens the editor. The panels are shown as labeled blocks (Library, Lyrics, Visual), the selected one has a bold frame, and a hint in plain words is at the bottom.

- **With the mouse:** drag a panel to move it; drag its bottom right corner (the ◢ mark) to resize it. The mouse is on only in the editor, so the rest of the time you can select text in the terminal as usual.
- **With the keyboard:**

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | select a panel: Library, Lyrics, Visual, Sound, Player bar |
| arrows | move the panel (5% steps) |
| `Shift`+arrows (or `Ctrl`+arrows) | change width and height |
| `+` / `-` | scale along the diagonal |
| `Space` | show / hide the selected panel (a hidden one stays as an outline so you can bring it back) |
| `Delete` | **remove** the selected panel from the screen |
| `Insert` | **add** a removed panel: a list of removed ones opens, `Enter` brings back the chosen one |
| `1`, `2`, `3` | start over from a ready-made layout (Classic, Lyrics, Cinema) |
| `Enter` | save; editing a ready-made layout does not change it but creates "Custom N" |
| `F3` | save as a new one, the old one stays |
| `F4` | delete your own layout |
| `Esc` | cancel |

Panels can be laid on top of each other; the order from bottom to top is: Visual (the picture), Library, Sound, Lyrics. This way lyrics lie over the GIF like subtitles.

Over a picture, the Lyrics panel has a dark backdrop: what is under it is darkened towards the theme background. How strongly is set by `lyrics_backdrop` in `[ui]` (0 no backdrop, 100 solid, 80 by default).

Sizes are percentages of the area above the bottom bar. The player bar always stands at the bottom with a fixed height and can only be hidden. Your own layouts are stored in `data/library.db` and remembered together with the chosen one.

## Sound visualizer

The **Sound** panel shows the spectrum of the playing sound. Low frequencies are on the left, high ones on the right.

- **Even bars:** they fill the whole width of the panel with nothing left over; the gaps and the bar widths are the same at any window size.
- **Loudness adapts:** the scale follows the loudness of the music, so both quiet and loud songs fill the panel the same way (the player's volume does not affect it). High frequencies are lifted, so the bars on the right do not lie at the bottom.
- **Smooth:** neighbouring frequencies are blended, bars rise fast and fall slowly, and thin peak marks above them hold and then fall slowly. On pause everything sinks smoothly.
- **Color** follows the gradient of your palette (the accent and extra colors) along the spectrum and with height.
- **Two styles**, key `x`: **bars** (bars grow from the bottom) and **mirror** (they grow up and down from the middle line). Default: `[ui] sound_style = "mirror"` in `config.toml`.

The panel is part of all three ready-made layouts (Classic and Lyrics: under the lyrics, Cinema: bottom right) and is in the layout editor as an ordinary panel. Layouts you saved earlier get it switched off; it can be added with `Insert` in the editor.

## Colors: palettes of N colors

`)` and `(` switch palettes forward and back: the ready-made ones (default, nord, gruvbox, light, mono), then yours. The choice is remembered and wins over `[theme]` in `config.toml`.

`C` opens the color editor, where the player is recolored at once. There can be from 2 to 12 colors. In order, they mean:

| № | Used for |
|---|---|
| 1 | background |
| 2 | text |
| 3 | accent (title frames, the selected row, progress) |
| 4 | dim text |
| 5 | borders |
| 6-12 | extra: together with the accent they color, in turn, the current lyric line (and karaoke words) and the panel titles |

If there are fewer than five colors, the missing ones are mixed from the background and text. Use as many colors as you like.

Keys in the editor: `↑`/`↓` choose a color; just start typing the value (3 or 6 hex digits, for example `ff79c6`) and press `Enter`; `←`/`→` make the color darker and lighter; `+` adds a color, `-` removes the selected one; `F2` saves (editing a ready-made palette creates "Mine N"), `F3` saves as a new one, `F4` deletes your own, `Esc` cancels.

## Key settings and global hotkeys

`S` opens a window: next to each action you see its key and a "Works everywhere" checkbox.

- `Enter`: assign a new key (press the one you want; `Esc` cancels). `Ctrl` and `Alt` are allowed, for example `Ctrl+Alt+N`. If the key is taken by another action, the player says which one.
- `Space`: switch "works everywhere" on or off.
- `Backspace`: restore the default key.

A key with the checkbox works in any program: you are playing a full-screen game, press `Ctrl+Alt+N` and the track changes. This is **Windows only** (system hotkeys). So that the player does not intercept ordinary typing in all programs, only a key with `Ctrl` or `Alt` (or `F1`..`F24`) can be made global. If the combination is already taken by another program, a message appears at the bottom. Global keys are not supported on Linux; the ordinary ones work everywhere inside the player.

The choice is stored in `data/library.db` and kept between runs.

## Lyrics

Search order: a `.lrc` file next to the track, then lyrics embedded in the tags, then the cache, then [LRCLIB](https://lrclib.net) (if enabled), and finally Whisper. What is found is cached in `data/lyrics/`, so each track is searched or recognized only once.

### Lyrics modes

`w` switches how lyrics are shown: **all lines** (as before, the current one highlighted, karaoke words) and **one word**: the Lyrics panel shows, large, exactly the word that is being sung right now, in a nice font (the letters are drawn with quadrant characters, so they are smooth). If the lyrics have per-word timings (karaoke or Whisper), they are used; if there are only per-line ones, the time of the line is divided between the words by their length. The color of the word changes along the palette. The font changes with `W`: **Lobster** (handwritten) and **Oswald** (strict); both support Cyrillic. In word mode it is handy to make the Lyrics panel large (layout editor `E`) and lay it over the picture in blocks mode.

**Your own fonts.** Put `.ttf` or `.otf` files into the `fonts` folder (it is created at the first start, next to `music/`) and restart the player: they join the `W` cycle after Lobster and Oswald, under the file name. If a font has no Cyrillic letters, Russian words will come out empty: that is a limit of the font.

Lobster and Oswald are distributed under the SIL Open Font License (`assets/fonts/`).

In `config.toml`: `[lyrics] display = "word"` and `word_font = "oswald"`.

### Whisper

When lyrics are found nowhere and a track has been playing for 3 seconds, the player offers to recognize them. The first time it **asks permission** to download a model from Hugging Face (tiny ≈ 75 MB, base ≈ 142 MB, small ≈ 466 MB, medium ≈ 1.5 GB) and shows its size. The model is saved in `data/models/` and downloaded once. Recognition runs in the background on the processor, music does not slow down; progress is shown at the bottom.

On singing over loud music, `tiny` and `base` give modest quality; for a better result set `model = "small"` or `"medium"`. A filter drops typical "hallucinations" (`[Music]`, "Thanks for watching", looped repeats).

## GIFs

Put gif / png / jpg files into `gifs/` and press `g`. One GIF is global, and each playlist can have its own (it is shown when the track was started from that playlist). A GIF is animated with its own frame delays and pauses together with the music. The GIF panel is visible when the window is wider than 110 columns; in full screen (`f`) it is always shown.

The `V` key switches how the picture fits the panel: **fit** (default: the picture is shown with the right proportions, and the free space is filled with a blurred dark copy of it, like in video players), **fill** (fill the panel, cropping the edges) and **stretch** (stretch it, the proportions are distorted). In fit mode you can allow some of the picture to be cropped so that the GIF itself is larger and the blurred background smaller: `z` raises the crop allowance by 10% (a larger picture), `Z` lowers it (a smaller, more complete picture). The default allowance is 35%; it can be set in `[visuals]`: `max_crop = 35` (0 means crop nothing, 90 is almost like fill). If no GIF is chosen, the first one from `gifs/` is shown; choosing "(none)" turns the picture off. The `v` key switches three display modes:

- **picture**: real pixels (sixel, kitty, iTerm2), full quality. But a terminal has a hard time redrawing a big picture on every frame, so the animation may be jerky, and text cannot be drawn over such a picture. In a terminal without graphics this mode draws with blocks by itself;
- **blocks**: the picture is made of quadrant characters (2×2 pixels with two colors in each cell), smooth, and other panels can be laid over it;
- **ascii**: colored characters by brightness (a long scale of 70 characters, contrast adjusted to the picture, the cell background darkened with the pixel's color).

If another panel lies over the picture, the picture mode draws with blocks by itself so that the text is visible. The default mode is `mode = "blocks"` (or `"ascii"`) in `[visuals]`.

New files in `gifs/` and `music/` are picked up with `F5`.

The terminal is detected automatically: kitty, WezTerm, iTerm2 and other terminals with graphics show the real picture, the rest (including ordinary Linux terminals and Windows Terminal) draw with half blocks. If the picture looks strange, set `protocol = "halfblocks"`.

## Settings (`config.toml`)

Created at the first start with comments. Any line can be deleted, then the default is used. A mistake in the file does not stop the start: a message is shown and the defaults are used.

```toml
[ui]
animations = true          # smooth appearing and highlights
lyrics_backdrop = 80       # dark backdrop under lyrics over a picture, 0-100
sound_style = "bars"       # sound visualizer: bars / mirror (key x)

[theme]
preset = "default"         # default / nord / gruvbox / light / mono
# background = "#14141c"   # any color can be overridden
# foreground = "#e6e6ef"
# dim = "#6c6c80"
# accent = "#ff79c6"
# border = "#44475a"

[audio]
volume = 0.8               # volume at start, 0.0 - 1.0
seek_step_secs = 5         # seek step

[lyrics]
online = true              # look up lyrics on LRCLIB

[whisper]
enabled = true             # recognize lyrics that are missing
model = "base"             # tiny / base / small / medium
language = "auto"          # "en", "ru", ... or auto

[visuals]
protocol = "auto"          # auto / kitty / sixel / iterm2 / halfblocks
mode = "picture"           # picture / blocks / ascii (key v)
fit = "fit"                # fit / fill / stretch (key V)
max_crop = 35              # how many % of the picture fit may crop, 0-90 (keys z / Z)
pause_with_music = true    # the GIF stands still when the music does
```

## Troubleshooting

- **No sound.** Check the default output device in the system. An error opening the device is shown at the bottom. On Linux, ALSA (`libasound2`) is needed.
- **No tracks visible.** The format must be one from the list above and the file must lie in `music/`. Broken files are skipped and described in `data/logs/`.
- **The build with Whisper fails** on `libclang` or `cmake`: install them (see above) or build with `--no-default-features`.
- **Lyrics are not found online.** Check the internet or turn `online` off; lyrics can always be put as a `.lrc` file next to the track (the same file name).
- Details are always in `data/logs/synq.log`.

## Development

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test                       # with Whisper
cargo test --no-default-features # without Whisper
```

A real recognition test (downloads a model, skipped by default):

```bash
SYNQ_TEST_AUDIO=speech.wav cargo test real_model -- --ignored --nocapture
```
