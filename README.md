# fermi-term ⚡

**The fastest, most easy-to-use terminal emulator. Written in Rust.**

> GPU-accelerated. Scrollback. Configurable. Zero bloat.

---

## What is it?

`fermi-term` is a lightweight terminal emulator written entirely in Rust. It spawns a real shell via a PTY, renders text on the GPU using wgpu with an instanced glyph atlas, and handles ANSI/VTE escape codes — all in under 1000 lines of code.

No Electron. No bloat. No config hell. Just a terminal that starts instantly and gets out of your way.

---

## Prerequisites

- **Rust** (stable): [rustup.rs](https://rustup.rs)
- **macOS** or **Linux** (Vulkan, Metal, or OpenGL support)
- A monospace font installed:
  - macOS: `Menlo` (ships with macOS) ✅
  - Linux: `DejaVu Sans Mono` (`apt install fonts-dejavu` or `pacman -S ttf-dejavu`)

On macOS you may also need Xcode Command Line Tools:
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

## Configuration

fermi-term loads config from `~/.config/fermi-term/config.toml`. All fields are optional — sane defaults are used for anything missing.

```toml
# Font size in points
font_size = 14.0

# Shell to spawn (defaults to $SHELL)
shell = "/bin/zsh"

# Default foreground colour [R, G, B]
fg = [200, 200, 200]

# Default background colour [R, G, B]
bg = [14, 14, 26]

# Cursor colour [R, G, B]
cursor_color = [220, 220, 100]

# Maximum scrollback lines (default: 10000)
scrollback_lines = 10000

# Initial window size in pixels
window_width = 1200
window_height = 800
```

---

## Features

- ✅ **GPU rendering** — wgpu + instanced draw calls, glyph atlas on a 2048×2048 R8 texture
- ✅ **Scrollback buffer** — 10,000 lines by default, mouse wheel to scroll
- ✅ **Window resize** — reconfigures GPU surface, PTY, and grid dynamically
- ✅ **TOML configuration** — fonts, colors, shell, window size
- ✅ **Real PTY** — spawns `$SHELL` (bash, zsh, fish, etc.)
- ✅ **ANSI/VTE escape codes**:
  - Cursor movement (A/B/C/D, H/f, G, E/F)
  - Erase in Display (`J`) and Line (`K`)
  - SGR colors: 8-color, 16-color, 256-color, true color (24-bit)
  - Bold text, insert/delete characters, scroll up/down
- ✅ **Full keyboard input** — Ctrl shortcuts, arrow keys, function keys, special keys
- ✅ **Pure Rust** — `wgpu` for GPU, `winit` for windowing, `fontdue` for rasterization, `portable-pty` for PTY, `vte` for parsing

---

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| Enter | Send `\r` |
| Backspace | Send `\x7f` |
| Tab | Send `\t` |
| Arrow keys | Send `ESC[A/B/C/D` |
| Ctrl+C | Interrupt (`\x03`) |
| Ctrl+D | EOF (`\x04`) |
| Ctrl+L | Clear screen (`\x0c`) |
| Ctrl+Z | Suspend (`\x1a`) |
| Ctrl+A/E/K/U/W/R | Standard readline shortcuts |
| Mouse wheel | Scroll through history |
| F1–F12 | Standard escape sequences |
| Home/End | Cursor to start/end |
| Page Up/Down | Send page up/down |

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│  main.rs  —  winit event loop, PTY orchestration            │
│                                                             │
│  ┌──────────────┐        ┌──────────────────────────────┐   │
│  │ terminal.rs  │        │      renderer.rs             │   │
│  │              │        │                              │   │
│  │  Grid        │◄──────►│  wgpu GPU renderer           │   │
│  │  Cell        │        │  fontdue glyph atlas         │   │
│  │  VTE Perform │        │  instanced draw calls        │   │
│  └──────┬───────┘        └──────────────────────────────┘   │
│         │                                                   │
│  ┌──────▼───────┐        ┌──────────────────────────────┐   │
│  │  portable-pty│  ←     │  config.rs                   │   │
│  │  shell proc  │        │  TOML config loading         │   │
│  └──────────────┘        └──────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

| File | Purpose |
|------|---------|
| `main.rs` | winit event loop, keyboard input, PTY orchestration |
| `terminal.rs` | Grid struct, Cell, scrollback buffer, VTE Perform (ANSI parser) |
| `renderer.rs` | wgpu GPU renderer, glyph atlas, instanced draw pipeline |
| `config.rs` | TOML config loading from `~/.config/fermi-term/config.toml` |

---

## Roadmap

- [x] GPU rendering (wgpu) for zero CPU overhead
- [x] Config file (TOML) — font, colors, shell, window size
- [x] Scrollback buffer with mouse wheel
- [x] Window resize support
- [ ] Tabs and splits
- [ ] Mouse support (SGR mouse protocol)
- [ ] URL detection & click-to-open
- [ ] IME / Unicode input
- [ ] Sixel graphics
- [ ] Ligatures
- [ ] Selection & copy/paste

---

## License

MIT

---

*Built with ❤️ and an unhealthy obsession with fast software.*
