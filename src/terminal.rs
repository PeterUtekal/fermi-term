/// A single character cell in the terminal grid.
#[derive(Clone, Debug)]
pub struct Cell {
    pub c: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: [200, 200, 200],
            bg: [14, 14, 26],
            bold: false,
        }
    }
}

/// The terminal grid holding the cell matrix and cursor state.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<Cell>>,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub scrollback_offset: usize,

    // Current SGR state
    cur_fg: [u8; 3],
    cur_bg: [u8; 3],
    cur_bold: bool,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cells = vec![vec![Cell::default(); cols]; rows];
        Grid {
            cols,
            rows,
            cells,
            cursor_x: 0,
            cursor_y: 0,
            scrollback_offset: 0,
            cur_fg: [200, 200, 200],
            cur_bg: [14, 14, 26],
            cur_bold: false,
        }
    }

    fn current_cell(&self) -> Cell {
        Cell {
            c: ' ',
            fg: self.cur_fg,
            bg: self.cur_bg,
            bold: self.cur_bold,
        }
    }

    fn scroll_up(&mut self) {
        self.cells.remove(0);
        let blank_row = vec![self.current_cell(); self.cols];
        self.cells.push(blank_row);
    }

    fn advance_cursor(&mut self) {
        self.cursor_x += 1;
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.cursor_y += 1;
            if self.cursor_y >= self.rows {
                self.cursor_y = self.rows - 1;
                self.scroll_up();
            }
        }
    }

    fn clamp_cursor(&mut self) {
        if self.cursor_x >= self.cols {
            self.cursor_x = self.cols.saturating_sub(1);
        }
        if self.cursor_y >= self.rows {
            self.cursor_y = self.rows.saturating_sub(1);
        }
    }
}

/// VTE Performer implementation for Grid.
impl vte::Perform for Grid {
    fn print(&mut self, c: char) {
        if self.cursor_y < self.rows && self.cursor_x < self.cols {
            self.cells[self.cursor_y][self.cursor_x] = Cell {
                c,
                fg: self.cur_fg,
                bg: self.cur_bg,
                bold: self.cur_bold,
            };
        }
        self.advance_cursor();
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                // Newline: move down, scroll if at bottom
                self.cursor_y += 1;
                if self.cursor_y >= self.rows {
                    self.cursor_y = self.rows - 1;
                    self.scroll_up();
                }
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\t' => {
                // Tab: advance to next tab stop (every 8 cols)
                let next_tab = (self.cursor_x / 8 + 1) * 8;
                self.cursor_x = next_tab.min(self.cols - 1);
            }
            0x08 => {
                // Backspace
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Collect params into a Vec<u16>
        let ps: Vec<u16> = params.iter().map(|p| p[0]).collect();
        let p0 = ps.get(0).copied().unwrap_or(0);

        match action {
            // Cursor Up
            'A' => {
                let n = p0.max(1) as usize;
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            // Cursor Down
            'B' => {
                let n = p0.max(1) as usize;
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
            }
            // Cursor Forward (Right)
            'C' => {
                let n = p0.max(1) as usize;
                self.cursor_x = (self.cursor_x + n).min(self.cols - 1);
            }
            // Cursor Backward (Left)
            'D' => {
                let n = p0.max(1) as usize;
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            // Cursor Position (H or f)
            'H' | 'f' => {
                let row = p0.saturating_sub(1) as usize;
                let col = ps.get(1).copied().unwrap_or(1).saturating_sub(1) as usize;
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
            }
            // Erase in Display
            'J' => {
                match p0 {
                    0 => {
                        // Erase from cursor to end of screen
                        let blank = self.current_cell();
                        let cx = self.cursor_x;
                        let cy = self.cursor_y;
                        for x in cx..self.cols {
                            if cy < self.rows {
                                self.cells[cy][x] = blank.clone();
                            }
                        }
                        for y in (cy + 1)..self.rows {
                            for x in 0..self.cols {
                                self.cells[y][x] = blank.clone();
                            }
                        }
                    }
                    1 => {
                        // Erase from start to cursor
                        let blank = self.current_cell();
                        let cx = self.cursor_x;
                        let cy = self.cursor_y;
                        for y in 0..cy {
                            for x in 0..self.cols {
                                self.cells[y][x] = blank.clone();
                            }
                        }
                        if cy < self.rows {
                            for x in 0..=cx.min(self.cols - 1) {
                                self.cells[cy][x] = blank.clone();
                            }
                        }
                    }
                    2 | 3 => {
                        // Erase entire display
                        let blank = self.current_cell();
                        for y in 0..self.rows {
                            for x in 0..self.cols {
                                self.cells[y][x] = blank.clone();
                            }
                        }
                        if p0 == 2 {
                            self.cursor_x = 0;
                            self.cursor_y = 0;
                        }
                    }
                    _ => {}
                }
            }
            // Erase in Line
            'K' => {
                let blank = self.current_cell();
                let cx = self.cursor_x;
                let cy = self.cursor_y;
                match p0 {
                    0 => {
                        // Erase from cursor to end of line
                        if cy < self.rows {
                            for x in cx..self.cols {
                                self.cells[cy][x] = blank.clone();
                            }
                        }
                    }
                    1 => {
                        // Erase from start of line to cursor
                        if cy < self.rows {
                            for x in 0..=cx.min(self.cols - 1) {
                                self.cells[cy][x] = blank.clone();
                            }
                        }
                    }
                    2 => {
                        // Erase entire line
                        if cy < self.rows {
                            for x in 0..self.cols {
                                self.cells[cy][x] = blank.clone();
                            }
                        }
                    }
                    _ => {}
                }
            }
            // SGR - Select Graphic Rendition
            'm' => {
                self.apply_sgr(&ps);
            }
            // Cursor Next Line
            'E' => {
                let n = p0.max(1) as usize;
                self.cursor_x = 0;
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
            }
            // Cursor Previous Line
            'F' => {
                let n = p0.max(1) as usize;
                self.cursor_x = 0;
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            // Cursor Horizontal Absolute
            'G' => {
                let col = p0.saturating_sub(1) as usize;
                self.cursor_x = col.min(self.cols - 1);
            }
            // Delete characters
            'P' => {
                let n = p0.max(1) as usize;
                let cy = self.cursor_y;
                let cx = self.cursor_x;
                if cy < self.rows {
                    let blank = self.current_cell();
                    let row = &mut self.cells[cy];
                    for i in cx..self.cols {
                        if i + n < self.cols {
                            row[i] = row[i + n].clone();
                        } else {
                            row[i] = blank.clone();
                        }
                    }
                }
            }
            // Scroll Up
            'S' => {
                let n = p0.max(1) as usize;
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            // Scroll Down
            'T' => {
                let n = p0.max(1) as usize;
                for _ in 0..n {
                    // insert blank line at top
                    let blank_row = vec![self.current_cell(); self.cols];
                    self.cells.insert(0, blank_row);
                    if self.cells.len() > self.rows {
                        self.cells.pop();
                    }
                }
            }
            // Insert blank characters
            '@' => {
                let n = p0.max(1) as usize;
                let cy = self.cursor_y;
                let cx = self.cursor_x;
                if cy < self.rows {
                    let blank = self.current_cell();
                    let row = &mut self.cells[cy];
                    for i in (cx..self.cols).rev() {
                        if i >= cx + n {
                            row[i] = row[i - n].clone();
                        } else {
                            row[i] = blank.clone();
                        }
                    }
                }
            }
            // Ignore other CSI sequences
            _ => {}
        }

        self.clamp_cursor();
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

impl Grid {
    fn apply_sgr(&mut self, params: &[u16]) {
        // Standard ANSI 256-color table (first 16 colors)
        const ANSI_COLORS: [[u8; 3]; 16] = [
            [0,   0,   0  ], // 0  Black
            [170, 0,   0  ], // 1  Red
            [0,   170, 0  ], // 2  Green
            [170, 170, 0  ], // 3  Yellow
            [0,   0,   170], // 4  Blue
            [170, 0,   170], // 5  Magenta
            [0,   170, 170], // 6  Cyan
            [170, 170, 170], // 7  White
            [85,  85,  85 ], // 8  Bright Black (Gray)
            [255, 85,  85 ], // 9  Bright Red
            [85,  255, 85 ], // 10 Bright Green
            [255, 255, 85 ], // 11 Bright Yellow
            [85,  85,  255], // 12 Bright Blue
            [255, 85,  255], // 13 Bright Magenta
            [85,  255, 255], // 14 Bright Cyan
            [255, 255, 255], // 15 Bright White
        ];

        if params.is_empty() {
            // Reset
            self.cur_fg = [200, 200, 200];
            self.cur_bg = [14, 14, 26];
            self.cur_bold = false;
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.cur_fg = [200, 200, 200];
                    self.cur_bg = [14, 14, 26];
                    self.cur_bold = false;
                }
                1 => self.cur_bold = true,
                2..=9 => {} // dim, italic, underline, etc. - ignore for MVP
                22 => self.cur_bold = false,
                // Normal foreground colors
                30..=37 => {
                    let idx = (params[i] - 30) as usize;
                    self.cur_fg = ANSI_COLORS[idx];
                }
                38 => {
                    // Extended foreground color
                    if i + 1 < params.len() && params[i + 1] == 2 {
                        // 38;2;r;g;b
                        if i + 4 < params.len() {
                            self.cur_fg = [
                                params[i + 2] as u8,
                                params[i + 3] as u8,
                                params[i + 4] as u8,
                            ];
                            i += 4;
                        }
                    } else if i + 1 < params.len() && params[i + 1] == 5 {
                        // 38;5;n (256-color)
                        if i + 2 < params.len() {
                            let n = params[i + 2] as usize;
                            self.cur_fg = ansi_256_color(n);
                            i += 2;
                        }
                    }
                }
                39 => self.cur_fg = [200, 200, 200], // Default fg
                // Normal background colors
                40..=47 => {
                    let idx = (params[i] - 40) as usize;
                    self.cur_bg = ANSI_COLORS[idx];
                }
                48 => {
                    // Extended background color
                    if i + 1 < params.len() && params[i + 1] == 2 {
                        // 48;2;r;g;b
                        if i + 4 < params.len() {
                            self.cur_bg = [
                                params[i + 2] as u8,
                                params[i + 3] as u8,
                                params[i + 4] as u8,
                            ];
                            i += 4;
                        }
                    } else if i + 1 < params.len() && params[i + 1] == 5 {
                        // 48;5;n (256-color)
                        if i + 2 < params.len() {
                            let n = params[i + 2] as usize;
                            self.cur_bg = ansi_256_color(n);
                            i += 2;
                        }
                    }
                }
                49 => self.cur_bg = [14, 14, 26], // Default bg
                // Bright foreground colors
                90..=97 => {
                    let idx = (params[i] - 90 + 8) as usize;
                    self.cur_fg = ANSI_COLORS[idx];
                }
                // Bright background colors
                100..=107 => {
                    let idx = (params[i] - 100 + 8) as usize;
                    self.cur_bg = ANSI_COLORS[idx];
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Convert a 256-color ANSI index to RGB.
fn ansi_256_color(n: usize) -> [u8; 3] {
    const ANSI_16: [[u8; 3]; 16] = [
        [0,   0,   0  ],
        [170, 0,   0  ],
        [0,   170, 0  ],
        [170, 170, 0  ],
        [0,   0,   170],
        [170, 0,   170],
        [0,   170, 170],
        [170, 170, 170],
        [85,  85,  85 ],
        [255, 85,  85 ],
        [85,  255, 85 ],
        [255, 255, 85 ],
        [85,  85,  255],
        [255, 85,  255],
        [85,  255, 255],
        [255, 255, 255],
    ];

    if n < 16 {
        ANSI_16[n]
    } else if n < 232 {
        let n = n - 16;
        let r = (n / 36) as u8;
        let g = ((n % 36) / 6) as u8;
        let b = (n % 6) as u8;
        let to_byte = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        [to_byte(r), to_byte(g), to_byte(b)]
    } else {
        // Grayscale
        let v = 8 + (n - 232) as u8 * 10;
        [v, v, v]
    }
}
