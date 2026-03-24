# fermi-term ⚡

**The fastest, simplest terminal emulator in the world. Written in Rust.**

> MVP status — but it works.

![Screenshot placeholder](./screenshot.png)

---

## What is it?

`fermi-term` is a lightweight, dependency-minimal terminal emulator written entirely in Rust. It spawns a real shell via a PTY, renders text using a pure-Rust font rasterizer, and handles ANSI/VTE escape codes — all in a simple pixel-buffer window.

No Electron. No bloat. No config hell. Just a terminal that starts instantly and gets out of your way.

---

## Prerequisites

- **Rust** (stable): [rustup.rs](https://rustup.rs)
- **macOS** or **Linux**
- A monospace font installed:
  - macOS: `Menlo` (ships with macOS) ✅
  - Linux: `DejaVu Sans Mono` (`apt install fonts-dejavu` or `pacman -S ttf-dejavu`)

On macOS you may also need Xcode Command Line Tools for the build:
```bash
xcode-select --install
```

---

## Build & Run

```bash
git clone https://github.com/PeterUtekal/fermi-term
cd fermi-term
cargo run --release
```

That's it. A window opens with your default shell (`$SHELL`).

---

## Current Features

- ✅ Real PTY — spawns `$SHELL` (bash, zsh, fish, etc.)
- ✅ ANSI/VTE escape code support:
  - Cursor movement (A/B/C/D, H/f, G, E/F)
  - Erase in Display (`J`) and Line (`K`)
  - SGR colors: 8-color, 16-color, 256-color, true color (24-bit)
  - Bold text
- ✅ Keyboard input with modifier support (Ctrl+C, Ctrl+D, Ctrl+L, Ctrl+Z, arrows, etc.)
- ✅ Scrolling (terminal scroll when shell outputs past bottom)
- ✅ ~60fps render loop
- ✅ Pure Rust — `fontdue` for font rendering, `minifb` for windowing, `portable-pty` for PTY, `vte` for parsing

---

## Keyboard Shortcuts

| Key | Sends |
|-----|-------|
| Enter | `\r` |
| Backspace | `\x7f` |
| Tab | `\t` |
| Arrow keys | `ESC[A/B/C/D` |
| Ctrl+C | Interrupt (`\x03`) |
| Ctrl+D | EOF (`\x04`) |
| Ctrl+L | Clear screen (`\x0c`) |
| Ctrl+Z | Suspend (`\x1a`) |
| Escape | Quit fermi-term |

---

## Roadmap

- [ ] GPU rendering (wgpu) for zero CPU overhead
- [ ] Tabs and splits
- [ ] Config file (TOML) — font, colors, keybinds
- [ ] Scrollback buffer with mouse wheel
- [ ] Mouse support (SGR mouse protocol)
- [ ] URL detection & click-to-open
- [ ] Window resize support
- [ ] IME / Unicode input
- [ ] Sixel graphics
- [ ] ligatures

---

## Architecture

```
main.rs        → event loop, keyboard, frame timing
terminal.rs    → Grid struct, VTE Perform implementation (ANSI parser)
renderer.rs    → fontdue glyph rasterization, pixel buffer compositing
```

---

## License

MIT

---

*Built with ❤️ and an unhealthy obsession with fast software.*
