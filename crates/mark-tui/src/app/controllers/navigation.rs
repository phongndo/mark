mod vim;

pub(in crate::app) use vim::NavigationController;

pub(in crate::app) trait NavigationContext {
    fn filters_active(&self) -> bool;
    fn grep_filter_active(&self) -> bool;
    fn clear_all_filters(&mut self);
    fn scroll_or_focus_hunk(&mut self, delta: isize);
    fn navigate_vertical_page(&mut self, delta: isize) {
        self.scroll_or_focus_hunk(delta);
    }
    fn scroll_horizontally_by(&mut self, delta: isize);
    fn set_horizontal_scroll_to_boundary(&mut self, _last: bool) {}
    fn set_scroll(&mut self, scroll: usize);
    fn max_scroll(&self) -> usize;
    fn navigate_to_boundary(&mut self, last: bool) {
        self.set_scroll(if last { self.max_scroll() } else { 0 });
    }
    /// `position` is -1 (top), 0 (middle), or 1 (bottom) of the viewport.
    fn navigate_to_viewport_position(&mut self, _position: i8, _count: usize) {}
    fn vertical_page_delta(&self, _full_page: bool) -> isize {
        20
    }
    fn move_grep_match(&mut self, delta: isize);

    fn push_vim_motion_digit(&mut self, _digit: u32) -> bool {
        false
    }
    fn take_vim_motion_count(&mut self) -> Option<usize> {
        None
    }
    fn clear_vim_motion(&mut self) -> bool {
        false
    }
    fn cancel_visual_mode(&mut self) -> bool {
        false
    }
}
