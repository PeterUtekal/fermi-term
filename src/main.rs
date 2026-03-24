//! fermi-term entry point.
//!
//! Initialises the PTY, spawns the shell, creates the window via winit,
//! and drives the render + input loop until the window is closed.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  main.rs  —  winit event loop, PTY orchestration            │
//! │                                                             │
//! │  ┌──────────────┐        ┌──────────────────────────────┐   │
//! │  │ terminal.rs  │        │      renderer.rs             │   │
//! │  │              │        │                              │   │
//! │  │  Grid        │◄──────►│  wgpu GPU renderer           │   │
//! │  │  Cell        │        │  fontdue glyph atlas         │   │
//! │  │  VTE Perform │        │  instanced draw calls        │   │
//! │  └──────┬───────┘        └──────────────────────────────┘   │
//! │         │                                                   │
//! │  ┌──────▼───────┐                                           │
//! │  │  portable-pty│  ←  shell process (bash/zsh/fish)         │
//! │  └──────────────┘                                           │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! The PTY reader runs on a background thread and feeds bytes into the
//! VTE parser, which updates the shared [`terminal::Grid`] via a `Mutex`.
//! The main thread owns the winit/wgpu window, polls for key events, writes
//! input bytes to the PTY master, and re-renders the grid every frame.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use winit::event::{ElementState, Event, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::WindowBuilder;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use vte::Parser;

mod config;
mod renderer;
mod terminal;

use config::Config;
use terminal::Grid;

fn main() {
    let config = Config::load();

    // ── Set up PTY ────────────────────────────────────────────────────
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("Failed to open PTY");

    let mut cmd = CommandBuilder::new(&config.shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let _child = pty_pair
        .slave
        .spawn_command(cmd)
        .expect("Failed to spawn shell");

    let pty_reader = pty_pair
        .master
        .try_clone_reader()
        .expect("Failed to clone PTY reader");
    let pty_writer: Box<dyn Write + Send> = pty_pair
        .master
        .take_writer()
        .expect("Failed to take PTY writer");
    let master: Box<dyn portable_pty::MasterPty + Send> = pty_pair.master;

    let grid = Arc::new(Mutex::new(Grid::new(
        80,
        24,
        config.scrollback_lines,
        config.fg,
        config.bg,
    )));

    // ── Background thread: PTY → VTE → Grid ──────────────────────────
    let grid_clone = Arc::clone(&grid);
    std::thread::spawn(move || {
        let mut parser = Parser::new();
        let mut buf = [0u8; 4096];
        let mut reader = pty_reader;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut g = grid_clone.lock().unwrap();
                    for &byte in &buf[..n] {
                        parser.advance(&mut *g, byte);
                    }
                }
            }
        }
        eprintln!("[fermi-term] Shell exited.");
    });

    // ── Create window & renderer ──────────────────────────────────────
    let event_loop = EventLoop::new().expect("Failed to create event loop");

    let window = WindowBuilder::new()
        .with_title("fermi-term ⚡")
        .with_inner_size(winit::dpi::PhysicalSize::new(
            config.window_width,
            config.window_height,
        ))
        .build(&event_loop)
        .expect("Failed to create window");
    let window = Arc::new(window);

    let mut renderer_instance =
        pollster::block_on(renderer::Renderer::new(Arc::clone(&window), &config));

    let cell_w = renderer_instance.cell_w;
    let cell_h = renderer_instance.cell_h;

    // Resize grid to actual window size
    {
        let size = window.inner_size();
        let cols = (size.width as usize / cell_w).max(1);
        let rows = (size.height as usize / cell_h).max(1);
        let mut g = grid.lock().unwrap();
        g.resize(cols, rows);
    }

    let pty_writer_shared: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(pty_writer));
    let master_shared: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>> =
        Arc::new(Mutex::new(master));

    let mut modifiers = ModifiersState::default();

    // ── Event loop (winit 0.29 API) ───────────────────────────────────
    let _ = event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }

                WindowEvent::Resized(size) => {
                    renderer_instance.resize(size.width, size.height);
                    let new_cols = (size.width as usize / cell_w).max(1);
                    let new_rows = (size.height as usize / cell_h).max(1);
                    {
                        let mut g = grid.lock().unwrap();
                        g.resize(new_cols, new_rows);
                    }
                    let _ = master_shared.lock().unwrap().resize(PtySize {
                        rows: new_rows as u16,
                        cols: new_cols as u16,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }

                WindowEvent::ModifiersChanged(mods) => {
                    modifiers = mods.state();
                }

                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            state: ElementState::Pressed,
                            logical_key,
                            ..
                        },
                    ..
                } => {
                    let bytes = key_to_bytes(&logical_key, modifiers);
                    if !bytes.is_empty() {
                        grid.lock().unwrap().scroll_to_bottom();
                        let _ = pty_writer_shared.lock().unwrap().write_all(&bytes);
                    }
                }

                WindowEvent::MouseWheel { delta, .. } => {
                    let lines: isize = match delta {
                        MouseScrollDelta::LineDelta(_, y) => (-(y as isize)) * 3,
                        MouseScrollDelta::PixelDelta(pos) => -(pos.y as isize) / 10,
                    };
                    grid.lock().unwrap().scroll_view(lines);
                    window.request_redraw();
                }

                WindowEvent::RedrawRequested => {
                    let g = grid.lock().unwrap();
                    renderer_instance.render(&g);
                }

                _ => {}
            },

            Event::AboutToWait => {
                window.request_redraw();
            }

            _ => {}
        }
    });
}

fn key_to_bytes(key: &Key, mods: ModifiersState) -> Vec<u8> {
    let ctrl = mods.control_key();
    let shift = mods.shift_key();

    // Ctrl shortcuts
    if ctrl {
        if let Key::Character(s) = key {
            return match s.as_str() {
                "c" | "C" => vec![0x03],
                "d" | "D" => vec![0x04],
                "l" | "L" => vec![0x0c],
                "z" | "Z" => vec![0x1a],
                "a" | "A" => vec![0x01],
                "e" | "E" => vec![0x05],
                "k" | "K" => vec![0x0b],
                "u" | "U" => vec![0x15],
                "w" | "W" => vec![0x17],
                "r" | "R" => vec![0x12],
                _ => vec![],
            };
        }
    }

    match key {
        Key::Named(NamedKey::Enter) => vec![b'\r'],
        Key::Named(NamedKey::Backspace) => vec![0x7f],
        Key::Named(NamedKey::Tab) => vec![b'\t'],
        Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
        Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
        Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
        Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
        Key::Named(NamedKey::Home) => b"\x1b[H".to_vec(),
        Key::Named(NamedKey::End) => b"\x1b[F".to_vec(),
        Key::Named(NamedKey::PageUp) => b"\x1b[5~".to_vec(),
        Key::Named(NamedKey::PageDown) => b"\x1b[6~".to_vec(),
        Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
        Key::Named(NamedKey::Insert) => b"\x1b[2~".to_vec(),
        Key::Named(NamedKey::Escape) => vec![0x1b],
        Key::Named(NamedKey::F1) => b"\x1bOP".to_vec(),
        Key::Named(NamedKey::F2) => b"\x1bOQ".to_vec(),
        Key::Named(NamedKey::F3) => b"\x1bOR".to_vec(),
        Key::Named(NamedKey::F4) => b"\x1bOS".to_vec(),
        Key::Named(NamedKey::F5) => b"\x1b[15~".to_vec(),
        Key::Named(NamedKey::F6) => b"\x1b[17~".to_vec(),
        Key::Named(NamedKey::F7) => b"\x1b[18~".to_vec(),
        Key::Named(NamedKey::F8) => b"\x1b[19~".to_vec(),
        Key::Named(NamedKey::F9) => b"\x1b[20~".to_vec(),
        Key::Named(NamedKey::F10) => b"\x1b[21~".to_vec(),
        Key::Named(NamedKey::F11) => b"\x1b[23~".to_vec(),
        Key::Named(NamedKey::F12) => b"\x1b[24~".to_vec(),
        Key::Named(NamedKey::Space) => vec![b' '],
        Key::Character(s) => {
            let s = if shift {
                s.to_uppercase().to_string()
            } else {
                s.to_string()
            };
            s.into_bytes()
        }
        _ => vec![],
    }
}
