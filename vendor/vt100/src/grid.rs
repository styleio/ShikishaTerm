use crate::term::BufWrite as _;

#[derive(Clone, Debug)]
pub struct Grid {
    size: Size,
    pos: Pos,
    saved_pos: Pos,
    rows: Vec<crate::row::Row>,
    scroll_top: u16,
    scroll_bottom: u16,
    origin_mode: bool,
    saved_origin_mode: bool,
    scrollback: std::collections::VecDeque<crate::row::Row>,
    scrollback_len: usize,
    scrollback_offset: usize,
}

impl Grid {
    pub fn new(size: Size, scrollback_len: usize) -> Self {
        Self {
            size,
            pos: Pos::default(),
            saved_pos: Pos::default(),
            rows: vec![],
            scroll_top: 0,
            scroll_bottom: size.rows - 1,
            origin_mode: false,
            saved_origin_mode: false,
            scrollback: std::collections::VecDeque::new(),
            scrollback_len,
            scrollback_offset: 0,
        }
    }

    pub fn allocate_rows(&mut self) {
        if self.rows.is_empty() {
            self.rows.extend(
                std::iter::repeat_with(|| {
                    crate::row::Row::new(self.size.cols)
                })
                .take(usize::from(self.size.rows)),
            );
        }
    }

    fn new_row(&self) -> crate::row::Row {
        crate::row::Row::new(self.size.cols)
    }

    pub fn clear(&mut self) {
        self.pos = Pos::default();
        self.saved_pos = Pos::default();
        for row in self.drawing_rows_mut() {
            row.clear(crate::attrs::Attrs::default());
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows - 1;
        self.origin_mode = false;
        self.saved_origin_mode = false;
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// NOTE (vendored patch): resizes the way a terminal whose width can
    /// change under running text has to -- the lines that ran past the edge
    /// are laid out again at the new width, rather than cut at it.
    ///
    /// Upstream truncates every row to the new width and forgets which rows
    /// ran on. A pseudo console does not draw the screen again after a
    /// resize (it leaves the terminal to reflow, as terminals with scrollback
    /// do), so whatever lay past the new edge was simply gone: an address
    /// printed at one width and read after the window narrowed came back with
    /// characters missing at every row break. Used for the main screen only;
    /// a program on the alternate screen draws itself again at the new size
    pub fn set_size_reflowing(&mut self, size: Size) {
        if size.cols == self.size.cols
            || size.cols == 0
            || size.rows == 0
            || self.rows.is_empty()
        {
            self.set_size(size);
            return;
        }
        self.reflow(size);
    }

    fn reflow(&mut self, size: Size) {
        let cols = usize::from(size.cols);
        let screen_rows = usize::from(size.rows);
        let above = self.scrollback.len();
        // Rows under the last one in use -- the cursor's, or the last with
        // anything written on it -- are blank, and are made again at the end
        let written = self
            .rows
            .iter()
            .rposition(crate::row::Row::is_written)
            .map_or(0, |i| i + 1);
        let used = written
            .max(usize::from(self.pos.row) + 1)
            .min(self.rows.len());
        let cursor_row = above + usize::from(self.pos.row);
        let cursor_col = usize::from(self.pos.col);

        let mut old: Vec<crate::row::Row> = self.scrollback.drain(..).collect();
        old.extend(self.rows.drain(..).take(used));

        let mut laid: Vec<crate::row::Row> = Vec::with_capacity(old.len());
        let mut cursor: Option<(usize, usize)> = None;
        let mut line: Vec<crate::Cell> = Vec::new();
        let mut cursor_in_line: Option<usize> = None;
        let last = old.len().saturating_sub(1);
        for (i, row) in old.into_iter().enumerate() {
            let runs_on = row.wrapped() && i < last;
            if i == cursor_row {
                cursor_in_line = Some(line.len() + cursor_col);
            }
            let mut cells = row.into_cells();
            // A row that runs on ends in something written, unless a wide
            // character did not fit in its last column: that column is
            // nothing, and is not carried into the line
            if runs_on
                && cells
                    .last()
                    .is_some_and(|c| !c.has_contents() && !c.is_wide_continuation())
            {
                cells.pop();
            }
            line.extend(cells);
            if !runs_on {
                let at = lay_out(
                    std::mem::take(&mut line),
                    cursor_in_line.take(),
                    cols,
                    &mut laid,
                );
                if at.is_some() {
                    cursor = at;
                }
            }
        }

        let (cursor_at, cursor_col) =
            cursor.unwrap_or((laid.len().saturating_sub(1), 0));
        // The screen shows the last rows laid out, as it showed the last
        // rows before; everything above goes back into the scrollback
        let mut top = laid.len().saturating_sub(screen_rows);
        if cursor_at < top {
            top = cursor_at;
        }
        let mut shown: Vec<crate::row::Row> = laid.split_off(top);
        shown.truncate(screen_rows);
        while shown.len() < screen_rows {
            shown.push(crate::row::Row::new(size.cols));
        }
        let keep = laid.len().saturating_sub(self.scrollback_len);
        self.scrollback = laid.into_iter().skip(keep).collect();
        self.scrollback_offset = 0;
        self.rows = shown;

        // A scroll region follows the height as a plain resize has it follow
        if self.scroll_bottom == self.size.rows - 1 {
            self.scroll_bottom = size.rows - 1;
        }
        self.size = size;
        if self.scroll_bottom >= size.rows {
            self.scroll_bottom = size.rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }
        self.pos = Pos {
            row: u16::try_from(cursor_at - top).unwrap_or(size.rows - 1),
            col: u16::try_from(cursor_col).unwrap_or(size.cols - 1),
        };
        self.row_clamp_bottom(false);
        self.col_clamp();
        if self.saved_pos.row > self.size.rows - 1 {
            self.saved_pos.row = self.size.rows - 1;
        }
        if self.saved_pos.col > self.size.cols - 1 {
            self.saved_pos.col = self.size.cols - 1;
        }
    }

    pub fn set_size(&mut self, size: Size) {
        if size.cols != self.size.cols {
            for row in &mut self.rows {
                row.wrap(false);
            }
        }

        if self.scroll_bottom == self.size.rows - 1 {
            self.scroll_bottom = size.rows - 1;
        }

        self.size = size;
        for row in &mut self.rows {
            row.resize(size.cols, crate::Cell::new());
        }
        self.rows.resize(usize::from(size.rows), self.new_row());

        if self.scroll_bottom >= size.rows {
            self.scroll_bottom = size.rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }

        self.row_clamp_top(false);
        self.row_clamp_bottom(false);
        self.col_clamp();

        if self.saved_pos.row > self.size.rows - 1 {
            self.saved_pos.row = self.size.rows - 1;
        }
        if self.saved_pos.col > self.size.cols - 1 {
            self.saved_pos.col = self.size.cols - 1;
        }
    }

    pub fn pos(&self) -> Pos {
        self.pos
    }

    pub fn set_pos(&mut self, mut pos: Pos) {
        if self.origin_mode {
            pos.row = pos.row.saturating_add(self.scroll_top);
        }
        self.pos = pos;
        self.row_clamp_top(self.origin_mode);
        self.row_clamp_bottom(self.origin_mode);
        self.col_clamp();
    }

    pub fn save_cursor(&mut self) {
        self.saved_pos = self.pos;
        self.saved_origin_mode = self.origin_mode;
    }

    pub fn restore_cursor(&mut self) {
        self.pos = self.saved_pos;
        self.origin_mode = self.saved_origin_mode;
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        let scrollback_len = self.scrollback.len();
        let rows_len = self.rows.len();
        self.scrollback
            .iter()
            .skip(scrollback_len - self.scrollback_offset)
            // when scrollback_offset > rows_len (e.g. rows = 3,
            // scrollback_len = 10, offset = 9) the skip(10 - 9)
            // will take 9 rows instead of 3. we need to set
            // the upper bound to rows_len (e.g. 3)
            .take(rows_len)
            // same for rows_len - scrollback_offset (e.g. 3 - 9).
            // it'll panic with overflow. we have to saturate the subtraction.
            .chain(
                self.rows
                    .iter()
                    .take(rows_len.saturating_sub(self.scrollback_offset)),
            )
    }

    pub fn drawing_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        self.rows.iter()
    }

    pub fn drawing_rows_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut crate::row::Row> {
        self.rows.iter_mut()
    }

    pub fn visible_row(&self, row: u16) -> Option<&crate::row::Row> {
        self.visible_rows().nth(usize::from(row))
    }

    pub fn drawing_row(&self, row: u16) -> Option<&crate::row::Row> {
        self.drawing_rows().nth(usize::from(row))
    }

    pub fn drawing_row_mut(
        &mut self,
        row: u16,
    ) -> Option<&mut crate::row::Row> {
        self.drawing_rows_mut().nth(usize::from(row))
    }

    pub fn current_row_mut(&mut self) -> &mut crate::row::Row {
        self.drawing_row_mut(self.pos.row)
            // we assume self.pos.row is always valid
            .unwrap()
    }

    pub fn visible_cell(&self, pos: Pos) -> Option<&crate::Cell> {
        self.visible_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell(&self, pos: Pos) -> Option<&crate::Cell> {
        self.drawing_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell_mut(&mut self, pos: Pos) -> Option<&mut crate::Cell> {
        self.drawing_row_mut(pos.row)
            .and_then(|r| r.get_mut(pos.col))
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn scrollback(&self) -> usize {
        self.scrollback_offset
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        self.scrollback_offset = rows.min(self.scrollback.len());
    }

    pub fn write_contents(&self, contents: &mut String) {
        let mut wrapping = false;
        for row in self.visible_rows() {
            row.write_contents(contents, 0, self.size.cols, wrapping);
            if !row.wrapped() {
                contents.push('\n');
            }
            wrapping = row.wrapped();
        }

        while contents.ends_with('\n') {
            contents.truncate(contents.len() - 1);
        }
    }

    pub fn write_contents_formatted(
        &self,
        contents: &mut Vec<u8>,
    ) -> crate::attrs::Attrs {
        crate::term::ClearAttrs.write_buf(contents);
        crate::term::ClearScreen.write_buf(contents);

        let mut prev_attrs = crate::attrs::Attrs::default();
        let mut prev_pos = Pos::default();
        let mut wrapping = false;
        for (i, row) in self.visible_rows().enumerate() {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_formatted(
                contents,
                0,
                self.size.cols,
                i,
                wrapping,
                Some(prev_pos),
                Some(prev_attrs),
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
        }

        self.write_cursor_position_formatted(
            contents,
            Some(prev_pos),
            Some(prev_attrs),
        );

        prev_attrs
    }

    pub fn write_contents_diff(
        &self,
        contents: &mut Vec<u8>,
        prev: &Self,
        mut prev_attrs: crate::attrs::Attrs,
    ) -> crate::attrs::Attrs {
        let mut prev_pos = prev.pos;
        let mut wrapping = false;
        let mut prev_wrapping = false;
        for (i, (row, prev_row)) in
            self.visible_rows().zip(prev.visible_rows()).enumerate()
        {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_diff(
                contents,
                prev_row,
                0,
                self.size.cols,
                i,
                wrapping,
                prev_wrapping,
                prev_pos,
                prev_attrs,
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
            prev_wrapping = prev_row.wrapped();
        }

        self.write_cursor_position_formatted(
            contents,
            Some(prev_pos),
            Some(prev_attrs),
        );

        prev_attrs
    }

    pub fn write_cursor_position_formatted(
        &self,
        contents: &mut Vec<u8>,
        prev_pos: Option<Pos>,
        prev_attrs: Option<crate::attrs::Attrs>,
    ) {
        let prev_attrs = prev_attrs.unwrap_or_default();
        // writing a character to the last column of a row doesn't wrap the
        // cursor immediately - it waits until the next character is actually
        // drawn. it is only possible for the cursor to have this kind of
        // position after drawing a character though, so if we end in this
        // position, we need to redraw the character at the end of the row.
        if prev_pos != Some(self.pos) && self.pos.col >= self.size.cols {
            let mut pos = Pos {
                row: self.pos.row,
                col: self.size.cols - 1,
            };
            if self
                .drawing_cell(pos)
                // we assume self.pos.row is always valid, and self.size.cols
                // - 1 is always a valid column
                .unwrap()
                .is_wide_continuation()
            {
                pos.col = self.size.cols - 2;
            }
            let cell =
                // we assume self.pos.row is always valid, and self.size.cols
                // - 2 must be a valid column because self.size.cols - 1 is
                // always valid and we just checked that the cell at
                // self.size.cols - 1 is a wide continuation character, which
                // means that the first half of the wide character must be
                // before it
                self.drawing_cell(pos).unwrap();
            if cell.has_contents() {
                if let Some(prev_pos) = prev_pos {
                    crate::term::MoveFromTo::new(prev_pos, pos)
                        .write_buf(contents);
                } else {
                    crate::term::MoveTo::new(pos).write_buf(contents);
                }
                cell.attrs().write_escape_code_diff(contents, &prev_attrs);
                contents.extend(cell.contents().as_bytes());
                prev_attrs.write_escape_code_diff(contents, cell.attrs());
            } else {
                // if the cell doesn't have contents, we can't have gotten
                // here by drawing a character in the last column. this means
                // that as far as i'm aware, we have to have reached here from
                // a newline when we were already after the end of an earlier
                // row. in the case where we are already after the end of an
                // earlier row, we can just write a few newlines, otherwise we
                // also need to do the same as above to get ourselves to after
                // the end of a row.
                let mut found = false;
                for i in (0..self.pos.row).rev() {
                    pos.row = i;
                    pos.col = self.size.cols - 1;
                    if self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 1 is always a valid col.
                        .unwrap()
                        .is_wide_continuation()
                    {
                        pos.col = self.size.cols - 2;
                    }
                    let cell = self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 2 is valid because self.size.cols
                        // - 1 is always valid, and col gets set to
                        // self.size.cols - 2 when the cell at self.size.cols
                        // - 1 is a wide continuation character, meaning that
                        // the first half of the wide character must be before
                        // it
                        .unwrap();
                    if cell.has_contents() {
                        if let Some(prev_pos) = prev_pos {
                            if prev_pos.row != i
                                || prev_pos.col < self.size.cols
                            {
                                crate::term::MoveFromTo::new(prev_pos, pos)
                                    .write_buf(contents);
                                cell.attrs().write_escape_code_diff(
                                    contents,
                                    &prev_attrs,
                                );
                                contents.extend(cell.contents().as_bytes());
                                prev_attrs.write_escape_code_diff(
                                    contents,
                                    cell.attrs(),
                                );
                            }
                        } else {
                            crate::term::MoveTo::new(pos).write_buf(contents);
                            cell.attrs().write_escape_code_diff(
                                contents,
                                &prev_attrs,
                            );
                            contents.extend(cell.contents().as_bytes());
                            prev_attrs.write_escape_code_diff(
                                contents,
                                cell.attrs(),
                            );
                        }
                        contents.extend(
                            "\n".repeat(usize::from(self.pos.row - i))
                                .as_bytes(),
                        );
                        found = true;
                        break;
                    }
                }

                // this can happen if you get the cursor off the end of a row,
                // and then do something to clear the end of the current row
                // without moving the cursor (IL, DL, ED, EL, etc). we know
                // there can't be something in the last column because we
                // would have caught that above, so it should be safe to
                // overwrite it.
                if !found {
                    pos = Pos {
                        row: self.pos.row,
                        col: self.size.cols - 1,
                    };
                    if let Some(prev_pos) = prev_pos {
                        crate::term::MoveFromTo::new(prev_pos, pos)
                            .write_buf(contents);
                    } else {
                        crate::term::MoveTo::new(pos).write_buf(contents);
                    }
                    contents.push(b' ');
                    // we know that the cell has no contents, but it still may
                    // have drawing attributes (background color, etc)
                    let end_cell = self
                        .drawing_cell(pos)
                        // we assume self.pos.row is always valid, and
                        // self.size.cols - 1 is always a valid column
                        .unwrap();
                    end_cell
                        .attrs()
                        .write_escape_code_diff(contents, &prev_attrs);
                    crate::term::SaveCursor.write_buf(contents);
                    crate::term::Backspace.write_buf(contents);
                    crate::term::EraseChar::new(1).write_buf(contents);
                    crate::term::RestoreCursor.write_buf(contents);
                    prev_attrs
                        .write_escape_code_diff(contents, end_cell.attrs());
                }
            }
        } else if let Some(prev_pos) = prev_pos {
            crate::term::MoveFromTo::new(prev_pos, self.pos)
                .write_buf(contents);
        } else {
            crate::term::MoveTo::new(self.pos).write_buf(contents);
        }
    }

    pub fn erase_all(&mut self, attrs: crate::attrs::Attrs) {
        for row in self.drawing_rows_mut() {
            row.clear(attrs);
        }
    }

    pub fn erase_all_forward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().skip(usize::from(pos.row) + 1) {
            row.clear(attrs);
        }

        self.erase_row_forward(attrs);
    }

    pub fn erase_all_backward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().take(usize::from(pos.row)) {
            row.clear(attrs);
        }

        self.erase_row_backward(attrs);
    }

    pub fn erase_row(&mut self, attrs: crate::attrs::Attrs) {
        self.current_row_mut().clear(attrs);
    }

    pub fn erase_row_forward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..size.cols {
            row.erase(col, attrs);
        }
    }

    pub fn erase_row_backward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in 0..=pos.col.min(size.cols - 1) {
            row.erase(col, attrs);
        }
    }

    pub fn insert_cells(&mut self, count: u16) {
        let size = self.size;
        let pos = self.pos;
        // NOTE (vendored patch): the cell lookups can miss after a resize;
        // treat a missing cell as "not a continuation" and skip the flag work
        let wide = pos.col < size.cols
            && self
                .drawing_cell(pos)
                .is_some_and(crate::cell::Cell::is_wide_continuation);
        let row = self.current_row_mut();
        for _ in 0..count {
            if wide {
                if let Some(c) = row.get_mut(pos.col) {
                    c.set_wide_continuation(false);
                }
            }
            row.insert(pos.col, crate::Cell::new());
            if wide {
                if let Some(c) = row.get_mut(pos.col) {
                    c.set_wide_continuation(true);
                }
            }
        }
        row.truncate(size.cols);
    }

    pub fn delete_cells(&mut self, count: u16) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        // NOTE (vendored patch): `cols - pos.col` underflows when a resize
        // left the cursor past the right edge
        for _ in 0..(count.min(size.cols.saturating_sub(pos.col))) {
            row.remove(pos.col);
        }
        row.resize(size.cols, crate::Cell::new());
    }

    pub fn erase_cells(&mut self, count: u16, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..((pos.col.saturating_add(count)).min(size.cols)) {
            row.erase(col, attrs);
        }
    }

    pub fn insert_lines(&mut self, count: u16) {
        for _ in 0..count {
            self.rows.remove(usize::from(self.scroll_bottom));
            self.rows.insert(usize::from(self.pos.row), self.new_row());
            // self.scroll_bottom is maintained to always be a valid row
            self.rows[usize::from(self.scroll_bottom)].wrap(false);
        }
    }

    pub fn delete_lines(&mut self, count: u16) {
        for _ in 0..(count.min(self.size.rows - self.pos.row)) {
            self.rows
                .insert(usize::from(self.scroll_bottom) + 1, self.new_row());
            self.rows.remove(usize::from(self.pos.row));
        }
    }

    pub fn scroll_up(&mut self, count: u16) {
        for _ in 0..(count.min(self.size.rows - self.scroll_top)) {
            self.rows
                .insert(usize::from(self.scroll_bottom) + 1, self.new_row());
            let removed = self.rows.remove(usize::from(self.scroll_top));
            if self.scrollback_len > 0 && !self.scroll_region_active() {
                self.scrollback.push_back(removed);
                while self.scrollback.len() > self.scrollback_len {
                    self.scrollback.pop_front();
                }
                if self.scrollback_offset > 0 {
                    self.scrollback_offset =
                        self.scrollback.len().min(self.scrollback_offset + 1);
                }
            }
        }
    }

    pub fn scroll_down(&mut self, count: u16) {
        for _ in 0..count {
            self.rows.remove(usize::from(self.scroll_bottom));
            self.rows
                .insert(usize::from(self.scroll_top), self.new_row());
            // self.scroll_bottom is maintained to always be a valid row
            self.rows[usize::from(self.scroll_bottom)].wrap(false);
        }
    }

    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let bottom = bottom.min(self.size().rows - 1);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = self.size().rows - 1;
        }
        self.pos.row = self.scroll_top;
        self.pos.col = 0;
    }

    fn in_scroll_region(&self) -> bool {
        self.pos.row >= self.scroll_top && self.pos.row <= self.scroll_bottom
    }

    fn scroll_region_active(&self) -> bool {
        self.scroll_top != 0 || self.scroll_bottom != self.size.rows - 1
    }

    pub fn set_origin_mode(&mut self, mode: bool) {
        self.origin_mode = mode;
        self.set_pos(Pos { row: 0, col: 0 });
    }

    pub fn row_inc_clamp(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_add(count);
        self.row_clamp_bottom(in_scroll_region);
    }

    pub fn row_inc_scroll(&mut self, count: u16) -> u16 {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_add(count);
        let lines = self.row_clamp_bottom(in_scroll_region);
        if in_scroll_region {
            self.scroll_up(lines);
            lines
        } else {
            0
        }
    }

    pub fn row_dec_clamp(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_sub(count);
        self.row_clamp_top(in_scroll_region);
    }

    pub fn row_dec_scroll(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        // need to account for clamping by both row_clamp_top and by
        // saturating_sub
        let extra_lines = count.saturating_sub(self.pos.row);
        self.pos.row = self.pos.row.saturating_sub(count);
        let lines = self.row_clamp_top(in_scroll_region);
        self.scroll_down(lines + extra_lines);
    }

    pub fn row_set(&mut self, i: u16) {
        self.pos.row = i;
        self.row_clamp();
    }

    pub fn col_inc(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_add(count);
    }

    pub fn col_inc_clamp(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_add(count);
        self.col_clamp();
    }

    pub fn col_dec(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_sub(count);
    }

    pub fn col_tab(&mut self) {
        self.pos.col -= self.pos.col % 8;
        self.pos.col += 8;
        self.col_clamp();
    }

    pub fn col_set(&mut self, i: u16) {
        self.pos.col = i;
        self.col_clamp();
    }

    pub fn col_wrap(&mut self, width: u16, wrap: bool) {
        // NOTE (vendored patch): `cols - width` underflows on a screen
        // narrower than the character, and the row lookup can miss after a
        // resize; both degrade to "no room, no wrap" instead of panicking
        if width <= self.size.cols && self.pos.col > self.size.cols - width {
            let mut prev_pos = self.pos;
            self.pos.col = 0;
            let scrolled = self.row_inc_scroll(1);
            prev_pos.row = prev_pos.row.saturating_sub(scrolled);
            let new_pos = self.pos;
            if let Some(row) = self.drawing_row_mut(prev_pos.row) {
                row.wrap(wrap && prev_pos.row + 1 == new_pos.row);
            }
        }
    }

    fn row_clamp_top(&mut self, limit_to_scroll_region: bool) -> u16 {
        if limit_to_scroll_region && self.pos.row < self.scroll_top {
            let rows = self.scroll_top - self.pos.row;
            self.pos.row = self.scroll_top;
            rows
        } else {
            0
        }
    }

    fn row_clamp_bottom(&mut self, limit_to_scroll_region: bool) -> u16 {
        let bottom = if limit_to_scroll_region {
            self.scroll_bottom
        } else {
            self.size.rows - 1
        };
        if self.pos.row > bottom {
            let rows = self.pos.row - bottom;
            self.pos.row = bottom;
            rows
        } else {
            0
        }
    }

    fn row_clamp(&mut self) {
        if self.pos.row > self.size.rows - 1 {
            self.pos.row = self.size.rows - 1;
        }
    }

    fn col_clamp(&mut self) {
        if self.pos.col > self.size.cols - 1 {
            self.pos.col = self.size.cols - 1;
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: u16,
    pub col: u16,
}

/// NOTE (vendored patch): one line -- a row and every row it ran on into --
/// laid out again `cols` wide onto the end of `out`. `cursor` is how many
/// cells into the line the cursor stood, when it stood on this line; the
/// answer is where it stands now (row in `out`, column)
fn lay_out(
    mut line: Vec<crate::Cell>,
    cursor: Option<usize>,
    cols: usize,
    out: &mut Vec<crate::row::Row>,
) -> Option<(usize, usize)> {
    fn close(
        row: &mut Vec<crate::Cell>,
        runs_on: bool,
        cols: usize,
        out: &mut Vec<crate::row::Row>,
    ) {
        row.resize(cols, crate::Cell::new());
        out.push(crate::row::Row::from_cells(std::mem::take(row), runs_on));
    }
    // What lies after the last thing written is where nothing was
    while line
        .last()
        .is_some_and(|c| !c.has_contents() && !c.is_wide_continuation())
    {
        line.pop();
    }
    let mut row: Vec<crate::Cell> = Vec::with_capacity(cols);
    let mut found = None;
    let mut i = 0;
    while i < line.len() {
        // A wide character's right half travels with it
        if line[i].is_wide_continuation() {
            i += 1;
            continue;
        }
        let wide = line[i].is_wide();
        let width = if wide { 2 } else { 1 };
        let half = wide && line.get(i + 1).is_some_and(crate::Cell::is_wide_continuation);
        let step = if half { 2 } else { 1 };
        // Too wide for any row of this screen: it cannot be drawn at all
        if width > cols {
            i += step;
            continue;
        }
        if row.len() + width > cols {
            close(&mut row, true, cols, out);
        }
        if cursor == Some(i) {
            found = Some((out.len(), row.len()));
        } else if half && cursor == Some(i + 1) {
            found = Some((out.len(), row.len() + 1));
        }
        row.push(line[i].clone());
        if wide {
            let right = if half {
                line[i + 1].clone()
            } else {
                let mut c = crate::Cell::new();
                c.set_wide_continuation(true);
                c
            };
            row.push(right);
        }
        i += step;
    }
    // A cursor past the written text: as far past it as it was, on the
    // row the text ends on, never over the edge
    if let Some(at) = cursor {
        if found.is_none() {
            let past = at.saturating_sub(line.len());
            let col = (row.len() + past).min(cols.saturating_sub(1));
            found = Some((out.len(), col));
        }
    }
    close(&mut row, false, cols, out);
    found
}

// NOTE (vendored patch): written out and read back (see `snapshot`).
// At most `scrollback_most` lines of scrollback go, the newest; where the
// view was scrolled back to does not (a new window starts at the bottom)
impl Grid {
    pub(crate) fn write_state(&self, out: &mut Vec<u8>, scrollback_most: usize) {
        use crate::snapshot::{put_bool, put_u16, put_u32};
        put_u16(out, self.size.rows);
        put_u16(out, self.size.cols);
        for p in [self.pos, self.saved_pos] {
            put_u16(out, p.row);
            put_u16(out, p.col);
        }
        put_u16(out, self.scroll_top);
        put_u16(out, self.scroll_bottom);
        put_bool(out, self.origin_mode);
        put_bool(out, self.saved_origin_mode);
        put_u32(out, u32::try_from(self.scrollback_len).unwrap_or(u32::MAX));
        put_u16(out, u16::try_from(self.rows.len()).unwrap_or(0));
        for row in &self.rows {
            row.write_state(out);
        }
        let sent = self.scrollback.len().min(scrollback_most);
        put_u32(out, u32::try_from(sent).unwrap_or(0));
        for row in self.scrollback.iter().skip(self.scrollback.len() - sent) {
            row.write_state(out);
        }
    }

    pub(crate) fn read_state(
        r: &mut crate::snapshot::Reader<'_>,
    ) -> Result<Self, crate::snapshot::SnapshotError> {
        use crate::snapshot::SnapshotError::Invalid;
        let size = Size { rows: r.u16()?, cols: r.u16()? };
        if size.rows == 0 || size.cols == 0 {
            return Err(Invalid("size"));
        }
        let pos = Pos { row: r.u16()?, col: r.u16()? };
        let saved_pos = Pos { row: r.u16()?, col: r.u16()? };
        // A column one past the last is where a line that has just filled
        // up waits for its next character
        let inside = |p: Pos| p.row < size.rows && p.col <= size.cols;
        if !inside(pos) || !inside(saved_pos) {
            return Err(Invalid("cursor"));
        }
        let scroll_top = r.u16()?;
        let scroll_bottom = r.u16()?;
        if scroll_top > scroll_bottom || scroll_bottom >= size.rows {
            return Err(Invalid("scroll region"));
        }
        let origin_mode = r.bool()?;
        let saved_origin_mode = r.bool()?;
        let scrollback_len = usize::try_from(r.u32()?).map_err(|_| Invalid("scrollback"))?;
        let count = r.u16()?;
        // An alternate screen never shown has no rows yet
        if count != 0 && count != size.rows {
            return Err(Invalid("row count"));
        }
        let rows = (0..count)
            .map(|_| crate::row::Row::read_state(r, size.cols))
            .collect::<Result<Vec<_>, _>>()?;
        let kept = r.u32()?;
        if usize::try_from(kept).map_or(true, |k| k > scrollback_len) {
            return Err(Invalid("scrollback"));
        }
        let scrollback = (0..kept)
            .map(|_| crate::row::Row::read_state(r, size.cols))
            .collect::<Result<std::collections::VecDeque<_>, _>>()?;
        Ok(Self {
            size,
            pos,
            saved_pos,
            rows,
            scroll_top,
            scroll_bottom,
            origin_mode,
            saved_origin_mode,
            scrollback,
            scrollback_len,
            scrollback_offset: 0,
        })
    }
}
