#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Navigation {
    #[default]
    Clamp,
    Wrap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub cursor: usize,
    pub offset: usize,
}

impl Viewport {
    pub fn move_by(&mut self, amount: isize, len: usize, navigation: Navigation) {
        if len == 0 {
            self.cursor = 0;
            self.offset = 0;
            return;
        }
        self.cursor = match navigation {
            Navigation::Wrap => {
                (self.cursor as i128 + amount as i128).rem_euclid(len as i128) as usize
            }
            Navigation::Clamp => self.cursor.saturating_add_signed(amount).min(len - 1),
        };
    }
    pub fn settle(&mut self, len: usize, height: usize) -> bool {
        let before = *self;
        if len == 0 {
            *self = Self::default();
            return before != *self;
        }
        self.cursor = self.cursor.min(len - 1);
        let height = height.max(1);
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset.saturating_add(height) {
            self.offset = self.cursor + 1 - height;
        }
        self.offset = self.offset.min(len.saturating_sub(height));
        before != *self
    }
    pub fn visible(&self, len: usize, height: usize) -> std::ops::Range<usize> {
        self.offset.min(len)..self.offset.saturating_add(height).min(len)
    }
}
