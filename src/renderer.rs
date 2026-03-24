use fontdue::{Font, FontSettings};
use crate::terminal::Grid;

pub struct Renderer {
    pub font: Font,
    pub font_size: f32,
    pub cell_w: usize,
    pub cell_h: usize,
}

impl Renderer {
    pub fn new() -> Self {
        let font_size = 14.0f32;
        let font = Self::load_font();

        // Calculate cell dimensions using typical character metrics
        // Use 'M' as a reference character for width
        let (metrics, _) = font.rasterize('M', font_size);
        let (metrics_h, _) = font.rasterize('|', font_size);

        // Cell width: advance width from metrics
        let cell_w = metrics.advance_width.ceil() as usize;
        // Cell height: use line metrics
        let line_metrics = font.horizontal_line_metrics(font_size);
        let cell_h = if let Some(lm) = line_metrics {
            (lm.ascent - lm.descent + lm.line_gap).ceil() as usize
        } else {
            (metrics_h.height + 4).max(18)
        };

        // Ensure minimum reasonable cell size
        let cell_w = cell_w.max(8);
        let cell_h = cell_h.max(14);

        Renderer { font, font_size, cell_w, cell_h }
    }

    fn load_font() -> Font {
        let font_paths: &[&str] = &[
            // macOS
            "/System/Library/Fonts/Menlo.ttc",
            "/Library/Fonts/Courier New.ttf",
            "/System/Library/Fonts/Monaco.ttf",
            // Linux
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
        ];

        for path in font_paths {
            if let Ok(bytes) = std::fs::read(path) {
                // Try fontdue with collection index 0
                let settings = FontSettings {
                    collection_index: 0,
                    scale: 40.0,
                    ..FontSettings::default()
                };
                if let Ok(font) = Font::from_bytes(bytes.as_slice(), settings) {
                    eprintln!("[fermi-term] Loaded font from: {}", path);
                    return font;
                }
            }
        }

        panic!(
            "fermi-term: Could not load any font. Tried:\n{}\n\
             Please install a monospace font (e.g. DejaVu Sans Mono on Linux).",
            font_paths.join("\n")
        );
    }

    /// Render the full grid into the pixel buffer.
    /// Buffer format: minifb `u32` as `0x00RRGGBB`.
    pub fn render(&self, grid: &Grid, buffer: &mut Vec<u32>, buf_w: usize, buf_h: usize) {
        // Clear buffer to default background
        let default_bg = rgb_to_u32(14, 14, 26);
        for px in buffer.iter_mut() {
            *px = default_bg;
        }

        let line_metrics = self.font.horizontal_line_metrics(self.font_size);
        let ascent = if let Some(lm) = line_metrics {
            lm.ascent.ceil() as i32
        } else {
            self.cell_h as i32 - 2
        };

        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let cell = &grid.cells[row][col];
                let px_x = col * self.cell_w;
                let px_y = row * self.cell_h;

                if px_x >= buf_w || px_y >= buf_h {
                    continue;
                }

                // Fill background rectangle
                let bg = rgb_to_u32(cell.bg[0], cell.bg[1], cell.bg[2]);
                let cell_right = (px_x + self.cell_w).min(buf_w);
                let cell_bottom = (px_y + self.cell_h).min(buf_h);

                for y in px_y..cell_bottom {
                    for x in px_x..cell_right {
                        buffer[y * buf_w + x] = bg;
                    }
                }

                // Draw cursor block
                if col == grid.cursor_x && row == grid.cursor_y {
                    let cursor_color = rgb_to_u32(200, 200, 200);
                    for y in px_y..cell_bottom {
                        for x in px_x..cell_right {
                            buffer[y * buf_w + x] = cursor_color;
                        }
                    }
                }

                // Skip rendering space (already filled bg)
                if cell.c == ' ' || cell.c == '\0' {
                    continue;
                }

                // Rasterize glyph
                let (metrics, bitmap) = self.font.rasterize(cell.c, self.font_size);

                if bitmap.is_empty() {
                    continue;
                }

                // Cursor inversion: if this is cursor cell, draw char inverted
                let (fg_r, fg_g, fg_b) = if col == grid.cursor_x && row == grid.cursor_y {
                    (cell.bg[0], cell.bg[1], cell.bg[2])
                } else {
                    (cell.fg[0], cell.fg[1], cell.fg[2])
                };

                let (bg_r, bg_g, bg_b) = if col == grid.cursor_x && row == grid.cursor_y {
                    (cell.fg[0], cell.fg[1], cell.fg[2])
                } else {
                    (cell.bg[0], cell.bg[1], cell.bg[2])
                };

                // Glyph top offset relative to cell top
                let glyph_top = ascent - metrics.ymin - metrics.height as i32;
                let glyph_top = glyph_top.max(0) as usize;

                for gy in 0..metrics.height {
                    let buf_row = px_y + glyph_top + gy;
                    if buf_row >= buf_h {
                        break;
                    }

                    for gx in 0..metrics.width {
                        let buf_col = px_x + metrics.xmin as usize + gx;
                        if buf_col >= buf_w {
                            break;
                        }

                        let alpha = bitmap[gy * metrics.width + gx] as u32;
                        if alpha == 0 {
                            continue;
                        }

                        // Alpha blend: out = fg * alpha/255 + bg * (1 - alpha/255)
                        let r = blend(fg_r, bg_r, alpha);
                        let g = blend(fg_g, bg_g, alpha);
                        let b = blend(fg_b, bg_b, alpha);

                        buffer[buf_row * buf_w + buf_col] = rgb_to_u32(r, g, b);
                    }
                }
            }
        }
    }
}

#[inline]
fn rgb_to_u32(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
fn blend(fg: u8, bg: u8, alpha: u32) -> u8 {
    let fg = fg as u32;
    let bg = bg as u32;
    ((fg * alpha + bg * (255 - alpha)) / 255) as u8
}
