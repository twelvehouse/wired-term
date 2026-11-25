# Third-party notices

wired is MIT licensed. It is built from, links or ships the third-party components below.
Keep this file up to date whenever a dependency, a bundled file or adapted code is added.

The release zip, made by `scripts/package.ps1`, contains next to `wired.exe`:
- wired's own `LICENSE` and this file.
- `licenses\`: the license texts of the components in sections 1 to 3.
- `THIRD-PARTY-LICENSES.html`: the license text of every Rust crate (section 5).
- `themes\LICENSE.txt`: the license of the color themes.

## 1. Linked into wired.exe (not Rust crates)

### libghostty-vt (Ghostty), statically linked
- Source: https://github.com/twelvehouse/ghostty-wired (fork of https://github.com/ghostty-org/ghostty, tag `vt-1.0.1` plus our patches listed in its `WIRED.md`).
- License: MIT, `LICENSE` in that repository.
- It contains these packages. The list comes from Ghostty's own report (`zig build lib-vt-packages -Demit-lib-vt`, and `deps/libghostty-vt-third-party.json`, entries with `"use": "linked"`):

| Package | Version | License | Source |
|---|---|---|---|
| highway | 1.2.0 | Apache-2.0 OR BSD-3-Clause | https://github.com/google/highway |
| simdutf | 9.0.0 | Apache-2.0 OR MIT | https://github.com/simdutf/simdutf |
| uucode | 0.2.0 | MIT AND Unicode-3.0 | https://github.com/jacobsandlund/uucode |
| wuffs | 0.4.0-alpha.10 | Apache-2.0 OR MIT | https://github.com/google/wuffs |
| translate_c | 2.0.0 | MIT | https://codeberg.org/ziglang/translate-c |
| Zig standard library | 0.17.1 | MIT | https://github.com/ziglang/zig |

Re-check this list when the Ghostty fork is updated. Their license files are in `licenses\`.

### gpui-pre and gpui-pre-windows (patched): gpui-pre-wired
- Source: https://github.com/twelvehouse/gpui-pre-wired. It holds crates.io `gpui-pre` 0.3.12 (Zed's gpui) and `gpui-pre-windows` 0.3.12 (its Windows platform code), each in its own folder.
- License: Apache-2.0 (`LICENSE-APACHE` in each folder). Our changes are listed in its `WIRED.md`, and every patched place is marked `wired patch`, as Apache-2.0 section 4 requires.
- Adapted code inside `gpui-pre-windows`:
  - Skia's mask gamma (`SkMaskGamma.cpp` / `.h`) and its RGB-to-A8 conversion in `src/shaders.hlsl` and `src/direct_write.rs`: BSD-3-Clause, Copyright Google Inc. The license is in `LICENSE-SKIA`.
  - `src/alpha_correction.hlsl`, adapted from Windows Terminal's `dwrite.hlsl` (already in upstream gpui): MIT, Copyright (c) Microsoft Corporation.

## 2. Shipped as separate files

### ConPTY (`conpty.dll`, `OpenConsole.exe`)
- NuGet package `Microsoft.Windows.Console.ConPTY` 1.25.260930003 from https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY, built from https://github.com/microsoft/terminal.
- License: MIT, Copyright (c) Microsoft Corporation. The package carries only the SPDX expression, so the MIT text from https://github.com/microsoft/terminal/blob/main/LICENSE is kept in `licenses\conpty-LICENSE.txt`.
- Downloaded at build time with a pinned SHA-256 and shipped unmodified.

### Color themes (`themes\`)
- The iTerm2-Color-Schemes collection, https://github.com/mbadolato/iTerm2-Color-Schemes, converted to Ghostty's theme format by the Ghostty project. The archive is the one Ghostty pins: `https://deps.files.ghostty.org/ghostty-themes-release-20261102-150615-3d8a6a1.tgz`.
- License: MIT, Copyright (c) 2011 to Present Mark Badolato. The copyright of each theme belongs to its author. The license text is shipped as `themes\LICENSE.txt` (from `crates/wired-app/iterm2-color-schemes-LICENSE.txt`).
- Downloaded at build time with a pinned SHA-256 and shipped unmodified. File names that hold `:` get `_` instead, as Windows file names cannot hold `:`.

### UDEV Gothic 35NFLG and NFLG (bundled fonts, `fonts\`)
- Source: https://github.com/yuru7/udev-gothic, release asset `UDEVGothic_NF_v2.2.0.zip` (pinned SHA-256), the `UDEVGothic35NFLG-*.ttf` and `UDEVGothicNFLG-*.ttf` files, shipped unmodified.
- License: SIL Open Font License 1.1, Copyright (c) 2022 Yuko OTAWARA, Reserved Font Name "UDEV Gothic". The release asset holds no license file, so the texts from the repository at tag `v2.2.0` are kept in `licenses\`: `udev-gothic-LICENSE.txt` and, for the fonts it is built from, `udev-gothic-LICENSE_BIZUDGothic.txt` (BIZ UDGothic, OFL 1.1), `udev-gothic-LICENSE_JetBrainsMono.txt` (JetBrains Mono, OFL 1.1) and `udev-gothic-LICENSE_NerdFonts.txt` (Nerd Fonts glyphs).

## 3. Assets compiled in through crates
- Lucide icons (title bar and tab icons), via the `gpui-kit-assets` crate: ISC License, Copyright (c) Lucide Icons and Contributors (`LICENSE-LUCIDE` in that crate).
- Profile icons (`crates/wired-ui/assets/profiles/*.svg`), copied unchanged from Tabby's `tabby-electron/src/icons`: MIT License, Copyright (c) 2017 Tabby Developers (`crates/wired-ui/assets/profiles/LICENSE-TABBY`). `cmd.svg` and `clink.svg` are the Font Awesome Free "windows" brand icon (CC BY 4.0, Fonticons, Inc.), recolored. The logos are trademarks of their owners (Microsoft, Git, Canonical, Debian, SUSE, Oracle and the other distributions) and identify the programs they start.

## 4. Behavior adapted from other projects
These are rewritten in Rust, not copied, but they follow these projects' source closely, so the projects are credited here.
- Windows Terminal (MIT, Microsoft): the rule for trimming single-line pastes (`Utils::TrimPaste`); right-click copies the selection instead of pasting (`ControlInteractivity::PointerPressed`); the dot size of the shade characters ░▒▓ (`src/renderer/atlas/BackendD3D.cpp`).
- Ghostty (MIT): rules for routing mouse reports and selection, and the cursor-blink rule (DEC mode 12 is ignored when the default cursor blinks); the geometry of box drawing and block elements drawn without the font (`crates/wired-terminal-view/src/custom_glyphs.rs`, after `src/font/sprite/draw`).
- Tabby (MIT, Copyright (c) 2017 Tabby Developers, https://github.com/Eugeny/tabby): pane messages written into the terminal screen with badge styles (`crates/wired-core/src/notice.rs`, after `tabby-ssh`'s session messages); focus after closing a pane or tab; shell detection and the choice of profile icons (`crates/wired-platform/src/shells.rs`, after `tabby-electron/src/shells`).
- Chromium (BSD-3-Clause, The Chromium Authors): IME candidate window placement with an exclusion rectangle (`IMM32Manager::MoveImeWindow`), in gpui-pre-wired.

## 5. Rust crates
About 450 crates end up in wired.exe; the list comes from `cargo metadata`, normal dependencies of `wired-app` for `x86_64-pc-windows-msvc`. All of them are permissively licensed: MIT and/or Apache-2.0 for most, the rest under BSD-2/3-Clause, Zlib, ISC, 0BSD, CC0-1.0, Unlicense, BSL-1.0 or Unicode-3.0.
- One exception, file-level copyleft: `option-ext` (MPL-2.0, through `dirs`). It is used unmodified; its source is at https://crates.io/crates/option-ext.
- `THIRD-PARTY-LICENSES.html` is generated with `cargo about`, which fails on any license not accepted in `about.toml`.

Not shipped and so not listed: system fonts such as Segoe UI and Segoe Fluent Icons, and build-only tools (Zig, bindgen, the Windows SDK).
