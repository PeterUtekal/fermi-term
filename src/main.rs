//! fermi-term — a fast, dependency-minimal terminal emulator written in Rust.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  main.rs  —  event loop, keyboard input, frame timing   │
//! │                                                         │
//! │  ┌──────────────┐        ┌───────────────────────────┐  │
//! │  │ terminal.rs  │        │      renderer.rs          │  │
//! │  │              │        │                           │  │
//! │  │  Grid        │◄──────►│  fontdue rasterisation    │  │
//! │  │  Cell        │        │  pixel-buffer compositing │  │
//! │  │  VTE Perform │        │                           │  │
//! │  └──────┬───────┘        └───────────────────────────┘  │
//! │         │                                               │
//! │  ┌──────▼───────┐                                       │
//! │  │  portable-pty│  ←  shell process (bash/zsh/fish)     │
//! │  └──────────────┘                                       │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! The PTY reader runs on a background thread and feeds bytes into the
//! VTE parser, which updates the shared [`terminal::Grid`] via a `Mutex`.
//! The main thread owns the minifb window, polls for key events, writes
//! input bytes to the PTY master, and re-renders the grid every frame.

mod terminal;
mod renderer;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use minifb::{Key, KeyRepeat, Window, WindowOptions};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use vte::Parser;

use terminal::Grid;
use renderer::Renderer;

const WIN_W: usize = 1200;
const WIN_H: usize = 800;
const TARGET_FPS: u64 = 60;
const FRAME_DURATION: Duration = Duration::from_nanos(1_000_000_000 / TARGET_FPS);

fn main() {
    // --- Initialize renderer first to get cell dimensions ---
    let renderer = Renderer::new();
    let cell_w = renderer.cell_w;
    let cell_h = renderer.cell_h;

    let cols = WIN_W / cell_w;
    let rows = WIN_H / cell_h;

    eprintln!(
        "[fermi-term] Grid: {}x{} cells (cell size: {}x{})",
        cols, rows, cell_w, cell_h
    );

    // --- Initialize terminal grid ---
    let grid = Arc::new(Mutex::new(Grid::new(cols, rows)));

    // --- Spawn PTY ---
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    // Determine shell
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let _child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn shell");

    // Get PTY reader (clone before taking writer)
    let mut pty_reader = pty_pair
        .master
        .try_clone_reader()
        .expect("Failed to clone PTY reader");

    // Take the writer (can only be called once)
    let pty_writer_box = pty_pair
        .master
        .take_writer()
        .expect("Failed to take PTY writer");

    // PTY writer shared with main thread for keyboard input
    let pty_writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pty_writer_box));

    // --- Background thread: read PTY output, parse VTE, update grid ---
    let grid_clone = Arc::clone(&grid);
    std::thread::spawn(move || {
        let mut parser = Parser::new();
        let mut buf = [0u8; 4096];

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => break, // EOF — shell exited
                Ok(n) => {
                    let mut g = grid_clone.lock().unwrap();
                    for &byte in &buf[..n] {
                        parser.advance(&mut *g, byte);
                    }
                }
                Err(e) => {
                    eprintln!("[fermi-term] PTY read error: {}", e);
                    break;
                }
            }
        }

        eprintln!("[fermi-term] Shell exited. Close the window to quit.");
    });

    // --- Create minifb window ---
    let mut window = Window::new(
        "fermi-term",
        WIN_W,
        WIN_H,
        WindowOptions {
            resize: false,
            ..WindowOptions::default()
        },
    )
    .expect("Failed to create window");

    window.limit_update_rate(None); // We manage our own frame rate

    let mut pixel_buffer = vec![0u32; WIN_W * WIN_H];

    // --- Main event loop ---
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let frame_start = Instant::now();

        // --- Keyboard input ---
        let ctrl_down = window.is_key_down(Key::LeftCtrl) || window.is_key_down(Key::RightCtrl);
        let shift_down = window.is_key_down(Key::LeftShift) || window.is_key_down(Key::RightShift);

        let keys = window.get_keys_pressed(KeyRepeat::Yes);
        let mut input_bytes: Vec<u8> = Vec::new();

        for key in keys {
            let bytes = key_to_bytes(key, ctrl_down, shift_down);
            input_bytes.extend_from_slice(&bytes);
        }

        if !input_bytes.is_empty() {
            if let Ok(mut writer) = pty_writer.lock() {
                let _ = writer.write_all(&input_bytes);
            }
        }

        // --- Render ---
        {
            let g = grid.lock().unwrap();
            renderer.render(&g, &mut pixel_buffer, WIN_W, WIN_H);
        }

        window
            .update_with_buffer(&pixel_buffer, WIN_W, WIN_H)
            .expect("Failed to update window buffer");

        // --- Frame rate cap ---
        let elapsed = frame_start.elapsed();
        if elapsed < FRAME_DURATION {
            std::thread::sleep(FRAME_DURATION - elapsed);
        }
    }
}

/// Map a minifb Key + modifier state to the bytes to send to the PTY.
fn key_to_bytes(key: Key, ctrl: bool, shift: bool) -> Vec<u8> {
    // Ctrl+key shortcuts
    if ctrl {
        match key {
            Key::C => return vec![0x03],
            Key::D => return vec![0x04],
            Key::L => return vec![0x0c],
            Key::Z => return vec![0x1a],
            Key::A => return vec![0x01],
            Key::E => return vec![0x05],
            Key::K => return vec![0x0b],
            Key::U => return vec![0x15],
            Key::W => return vec![0x17],
            Key::R => return vec![0x12],
            _ => {}
        }
    }

    // Special keys
    match key {
        Key::Enter | Key::NumPadEnter => return vec![b'\r'],
        Key::Backspace => return vec![0x7f],
        Key::Tab => return vec![b'\t'],
        Key::Up => return b"\x1b[A".to_vec(),
        Key::Down => return b"\x1b[B".to_vec(),
        Key::Right => return b"\x1b[C".to_vec(),
        Key::Left => return b"\x1b[D".to_vec(),
        Key::Home => return b"\x1b[H".to_vec(),
        Key::End => return b"\x1b[F".to_vec(),
        Key::PageUp => return b"\x1b[5~".to_vec(),
        Key::PageDown => return b"\x1b[6~".to_vec(),
        Key::Delete => return b"\x1b[3~".to_vec(),
        Key::Insert => return b"\x1b[2~".to_vec(),
        Key::F1 => return b"\x1bOP".to_vec(),
        Key::F2 => return b"\x1bOQ".to_vec(),
        Key::F3 => return b"\x1bOR".to_vec(),
        Key::F4 => return b"\x1bOS".to_vec(),
        Key::F5 => return b"\x1b[15~".to_vec(),
        Key::F6 => return b"\x1b[17~".to_vec(),
        Key::F7 => return b"\x1b[18~".to_vec(),
        Key::F8 => return b"\x1b[19~".to_vec(),
        Key::F9 => return b"\x1b[20~".to_vec(),
        Key::F10 => return b"\x1b[21~".to_vec(),
        Key::F11 => return b"\x1b[23~".to_vec(),
        Key::F12 => return b"\x1b[24~".to_vec(),
        _ => {}
    }

    // Printable characters
    let ch = key_to_char(key, shift);
    if let Some(c) = ch {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        return s.as_bytes().to_vec();
    }

    vec![]
}

/// Convert a Key to its character representation, accounting for shift.
fn key_to_char(key: Key, shift: bool) -> Option<char> {
    match key {
        Key::Space => Some(' '),
        Key::A => Some(if shift { 'A' } else { 'a' }),
        Key::B => Some(if shift { 'B' } else { 'b' }),
        Key::C => Some(if shift { 'C' } else { 'c' }),
        Key::D => Some(if shift { 'D' } else { 'd' }),
        Key::E => Some(if shift { 'E' } else { 'e' }),
        Key::F => Some(if shift { 'F' } else { 'f' }),
        Key::G => Some(if shift { 'G' } else { 'g' }),
        Key::H => Some(if shift { 'H' } else { 'h' }),
        Key::I => Some(if shift { 'I' } else { 'i' }),
        Key::J => Some(if shift { 'J' } else { 'j' }),
        Key::K => Some(if shift { 'K' } else { 'k' }),
        Key::L => Some(if shift { 'L' } else { 'l' }),
        Key::M => Some(if shift { 'M' } else { 'm' }),
        Key::N => Some(if shift { 'N' } else { 'n' }),
        Key::O => Some(if shift { 'O' } else { 'o' }),
        Key::P => Some(if shift { 'P' } else { 'p' }),
        Key::Q => Some(if shift { 'Q' } else { 'q' }),
        Key::R => Some(if shift { 'R' } else { 'r' }),
        Key::S => Some(if shift { 'S' } else { 's' }),
        Key::T => Some(if shift { 'T' } else { 't' }),
        Key::U => Some(if shift { 'U' } else { 'u' }),
        Key::V => Some(if shift { 'V' } else { 'v' }),
        Key::W => Some(if shift { 'W' } else { 'w' }),
        Key::X => Some(if shift { 'X' } else { 'x' }),
        Key::Y => Some(if shift { 'Y' } else { 'y' }),
        Key::Z => Some(if shift { 'Z' } else { 'z' }),
        Key::Key0 => Some(if shift { ')' } else { '0' }),
        Key::Key1 => Some(if shift { '!' } else { '1' }),
        Key::Key2 => Some(if shift { '@' } else { '2' }),
        Key::Key3 => Some(if shift { '#' } else { '3' }),
        Key::Key4 => Some(if shift { '$' } else { '4' }),
        Key::Key5 => Some(if shift { '%' } else { '5' }),
        Key::Key6 => Some(if shift { '^' } else { '6' }),
        Key::Key7 => Some(if shift { '&' } else { '7' }),
        Key::Key8 => Some(if shift { '*' } else { '8' }),
        Key::Key9 => Some(if shift { '(' } else { '9' }),
        Key::Minus => Some(if shift { '_' } else { '-' }),
        Key::Equal => Some(if shift { '+' } else { '=' }),
        Key::LeftBracket => Some(if shift { '{' } else { '[' }),
        Key::RightBracket => Some(if shift { '}' } else { ']' }),
        Key::Backslash => Some(if shift { '|' } else { '\\' }),
        Key::Semicolon => Some(if shift { ':' } else { ';' }),
        Key::Apostrophe => Some(if shift { '"' } else { '\'' }),
        Key::Comma => Some(if shift { '<' } else { ',' }),
        Key::Period => Some(if shift { '>' } else { '.' }),
        Key::Slash => Some(if shift { '?' } else { '/' }),
        Key::Backquote => Some(if shift { '~' } else { '`' }),
        Key::NumPad0 => Some('0'),
        Key::NumPad1 => Some('1'),
        Key::NumPad2 => Some('2'),
        Key::NumPad3 => Some('3'),
        Key::NumPad4 => Some('4'),
        Key::NumPad5 => Some('5'),
        Key::NumPad6 => Some('6'),
        Key::NumPad7 => Some('7'),
        Key::NumPad8 => Some('8'),
        Key::NumPad9 => Some('9'),
        Key::NumPadDot => Some('.'),
        Key::NumPadSlash => Some('/'),
        Key::NumPadAsterisk => Some('*'),
        Key::NumPadMinus => Some('-'),
        Key::NumPadPlus => Some('+'),
        _ => None,
    }
}
