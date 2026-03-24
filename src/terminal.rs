/// A single character cell in the terminal grid.
///
/// Each cell stores the displayed character along with its foreground
/// and background colours and the bold attribute. The `bold` field is
/// stored so that future renderers (e.g. a GPU path with a bold font
/// variant) can use it without a schema migration.
#[derive(Clone, Debug)]
pub struct Cell {
    /// The Unicode scalar displayed in this cell.
    pub c: char,
    /// Foreground colour as `[R, G, B]`.
    pub fg: [u8; 3],
    /// Background colour as `[R, G, B]`.
    pub bg: [u8; 3],
    /// Whether the cell should be rendered in bold weight.
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
///
/// `cells` holds the full scrollback history. The visible region is always
/// the last `rows` entries. When output scrolls the screen, new rows are
/// appended and old rows are trimmed only when `cells.len() > rows + max_scrollback`.
pub struct Grid {
    /// Number of columns (characters per line).
    pub cols: usize,
    /// Number of visible rows.
    pub rows: usize,
    /// Full history including scrollback. Index 0 = oldest line.
    /// Visible region is `cells[visible_start()..]` for `rows` lines.
    pub cells: Vec<Vec<Cell>>,
    /// Current cursor column (0-indexed).
    pub cursor_x: usize,
    /// Cursor row relative to the visible region (not absolute history index).
    pub cursor_y: usize,
    /// Lines scrolled back from the live view (0 = showing latest output).
    pub scroll_offset: usize,
    /// Maximum history lines retained above the visible area.
    pub max_scrollback: usize,
    /// Default foreground colour.
    pub default_fg: [u8; 3],
    /// Default background colour.
    pub default_bg: [u8; 3],

    // ── Current SGR (Select Graphic Rendition) state ──────────────────
    cur_fg: [u8; 3],
    cur_bg: [u8; 3],
    cur_bold: bool,
}

impl Grid {
    pub fn new(
        cols: usize,
        rows: usize,
        max_scrollback: usize,
        default_fg: [u8; 3],
        default_bg: [u8; 3],
    ) -> Self {
        let cells = (0..rows)
            .map(|_| {
                vec![
                    Cell {
                        c: ' ',
                        fg: default_fg,
                        bg: default_bg,
                        bold: false,
                    };
                    cols
                ]
            })
            .collect();
        Grid {
            cols,
            rows,
            cells,
            cursor_x: 0,
            cursor_y: 0,
            scroll_offset: 0,
            max_scrollback,
            default_fg,
            default_bg,
            cur_fg: default_fg,
            cur_bg: default_bg,
            cur_bold: false,
        }
    }

    /// Index of the first visible row in `self.cells`.
    pub fn visible_start(&self) -> usize {
        let total = self.cells.len();
        if total <= self.rows {
            0
        } else {
            let live_start = total - self.rows;
            live_start.saturating_sub(self.scroll_offset)
        }
    }

    /// Scroll the view by `delta` lines (positive = scroll up / back in history).
    pub fn scroll_view(&mut self, delta: isize) {
        let total = self.cells.len();
        let max_offset = total.saturating_sub(self.rows);
        let new_offset = (self.scroll_offset as isize + delta)
            .max(0)
            .min(max_offset as isize) as usize;
        self.scroll_offset = new_offset;
    }

    /// Jump to the live (bottom) view.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    #[allow(dead_code)]
    fn blank_row(&self) -> Vec<Cell> {
        vec![
            Cell {
                c: ' ',
                fg: self.default_fg,
                bg: self.default_bg,
                bold: false,
            };
            self.cols
        ]
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
        // Append a blank row using current SGR colors
        let blank_row = vec![self.current_cell(); self.cols];
        self.cells.push(blank_row);
        let max_total = self.rows + self.max_scrollback;
        if self.cells.len() > max_total {
            self.cells.drain(0..1);
        }
    }

    /// Get a mutable reference to a cell in the live (visible) region.
    fn visible_cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        let live_start = self.cells.len().saturating_sub(self.rows);
        self.cells.get_mut(live_start + row)?.get_mut(col)
    }

    /// Resize the visible grid. Extends rows with blank lines, trims if smaller.
    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        // Extend rows if needed
        while self.cells.len() < new_rows {
            let row = vec![
                Cell {
                    c: ' ',
                    fg: self.default_fg,
                    bg: self.default_bg,
                    bold: false,
                };
                new_cols
            ];
            self.cells.push(row);
        }
        // Resize each row
        for row in self.cells.iter_mut() {
            row.resize(
                new_cols,
                Cell {
                    c: ' ',
                    fg: self.default_fg,
                    bg: self.default_bg,
                    bold: false,
                },
            );
        }
        // Clamp cursor
        self.cursor_x = self.cursor_x.min(new_cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(new_rows.saturating_sub(1));
        self.cols = new_cols;
        self.rows = new_rows;
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
        let cur_fg = self.cur_fg;
        let cur_bg = self.cur_bg;
        let cur_bold = self.cur_bold;
        if let Some(cell) = self.visible_cell_mut(self.cursor_y, self.cursor_x) {
            cell.c = c;
            cell.fg = cur_fg;
            cell.bg = cur_bg;
            cell.bold = cur_bold;
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

    fn hook(
        &mut self,
        _params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }
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
        let p0 = ps.first().copied().unwrap_or(0);

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
                let blank = self.current_cell();
                let cx = self.cursor_x;
                let cy = self.cursor_y;
                let live_start = self.cells.len().saturating_sub(self.rows);
                match p0 {
                    0 => {
                        // Erase from cursor to end of screen
                        for x in cx..self.cols {
                            if let Some(row) = self.cells.get_mut(live_start + cy) {
                                if let Some(cell) = row.get_mut(x) {
                                    *cell = blank.clone();
                                }
                            }
                        }
                        for y in (cy + 1)..self.rows {
                            for x in 0..self.cols {
                                if let Some(row) = self.cells.get_mut(live_start + y) {
                                    if let Some(cell) = row.get_mut(x) {
                                        *cell = blank.clone();
                                    }
                                }
                            }
                        }
                    }
                    1 => {
                        // Erase from start to cursor
                        for y in 0..cy {
                            for x in 0..self.cols {
                                if let Some(row) = self.cells.get_mut(live_start + y) {
                                    if let Some(cell) = row.get_mut(x) {
                                        *cell = blank.clone();
                                    }
                                }
                            }
                        }
                        for x in 0..=cx.min(self.cols - 1) {
                            if let Some(row) = self.cells.get_mut(live_start + cy) {
                                if let Some(cell) = row.get_mut(x) {
                                    *cell = blank.clone();
                                }
                            }
                        }
                    }
                    2 | 3 => {
                        // Erase entire display
                        for y in 0..self.rows {
                            for x in 0..self.cols {
                                if let Some(row) = self.cells.get_mut(live_start + y) {
                                    if let Some(cell) = row.get_mut(x) {
                                        *cell = blank.clone();
                                    }
                                }
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
                let live_start = self.cells.len().saturating_sub(self.rows);
                match p0 {
                    0 => {
                        // Erase from cursor to end of line
                        if let Some(row) = self.cells.get_mut(live_start + cy) {
                            for x in cx..self.cols {
                                if let Some(cell) = row.get_mut(x) {
                                    *cell = blank.clone();
                                }
                            }
                        }
                    }
                    1 => {
                        // Erase from start of line to cursor
                        if let Some(row) = self.cells.get_mut(live_start + cy) {
                            for x in 0..=cx.min(self.cols - 1) {
                                if let Some(cell) = row.get_mut(x) {
                                    *cell = blank.clone();
                                }
                            }
                        }
                    }
                    2 => {
                        // Erase entire line
                        if let Some(row) = self.cells.get_mut(live_start + cy) {
                            for x in 0..self.cols {
                                if let Some(cell) = row.get_mut(x) {
                                    *cell = blank.clone();
                                }
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
                let blank = self.current_cell();
                let live_start = self.cells.len().saturating_sub(self.rows);
                if let Some(row) = self.cells.get_mut(live_start + cy) {
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
                let live_start = self.cells.len().saturating_sub(self.rows);
                for _ in 0..n {
                    // insert blank line at live view top
                    let blank_row = vec![self.current_cell(); self.cols];
                    self.cells.insert(live_start, blank_row);
                    // remove the last row if we've gone over `rows`
                    if self.cells.len() > self.rows + self.max_scrollback {
                        self.cells.pop();
                    }
                }
            }
            // Insert blank characters
            '@' => {
                let n = p0.max(1) as usize;
                let cy = self.cursor_y;
                let cx = self.cursor_x;
                let blank = self.current_cell();
                let live_start = self.cells.len().saturating_sub(self.rows);
                if let Some(row) = self.cells.get_mut(live_start + cy) {
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
            [0, 0, 0],       // 0  Black
            [170, 0, 0],     // 1  Red
            [0, 170, 0],     // 2  Green
            [170, 170, 0],   // 3  Yellow
            [0, 0, 170],     // 4  Blue
            [170, 0, 170],   // 5  Magenta
            [0, 170, 170],   // 6  Cyan
            [170, 170, 170], // 7  White
            [85, 85, 85],    // 8  Bright Black (Gray)
            [255, 85, 85],   // 9  Bright Red
            [85, 255, 85],   // 10 Bright Green
            [255, 255, 85],  // 11 Bright Yellow
            [85, 85, 255],   // 12 Bright Blue
            [255, 85, 255],  // 13 Bright Magenta
            [85, 255, 255],  // 14 Bright Cyan
            [255, 255, 255], // 15 Bright White
        ];

        if params.is_empty() {
            self.cur_fg = self.default_fg;
            self.cur_bg = self.default_bg;
            self.cur_bold = false;
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.cur_fg = self.default_fg;
                    self.cur_bg = self.default_bg;
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
                39 => self.cur_fg = self.default_fg, // Default fg
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
                49 => self.cur_bg = self.default_bg, // Default bg
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
        [0, 0, 0],
        [170, 0, 0],
        [0, 170, 0],
        [170, 170, 0],
        [0, 0, 170],
        [170, 0, 170],
        [0, 170, 170],
        [170, 170, 170],
        [85, 85, 85],
        [255, 85, 85],
        [85, 255, 85],
        [255, 255, 85],
        [85, 85, 255],
        [255, 85, 255],
        [85, 255, 255],
        [255, 255, 255],
    ];

    if n < 16 {
        ANSI_16[n]
    } else if n < 232 {
        let n = n - 16;
        let r = (n / 36) as u8;
        let g = ((n % 36) / 6) as u8;
        let b = (n % 6) as u8;
        let to_byte = |v: u8| {
            if v == 0 {
                0
            } else {
                55 + v * 40
            }
        };
        [to_byte(r), to_byte(g), to_byte(b)]
    } else {
        // Grayscale
        let v = 8 + (n - 232) as u8 * 10;
        [v, v, v]
    }
}
