use crate::app::controllers::navigation::NavigationContext;

use super::KeyEventCtx;

impl NavigationContext for KeyEventCtx<'_> {
    fn filters_active(&self) -> bool {
        self.app.filters.active()
    }

    fn grep_filter_active(&self) -> bool {
        self.app.filters.grep_active()
    }

    fn clear_all_filters(&mut self) {
        self.app.clear_all_filters();
    }

    fn scroll_or_focus_hunk(&mut self, delta: isize) {
        self.app.navigate_diff_vertically(delta);
    }

    fn navigate_vertical_page(&mut self, delta: isize) {
        self.app.navigate_diff_by_visual_page(delta);
    }

    fn scroll_horizontally_by(&mut self, delta: isize) {
        self.app.scroll_horizontally_by(delta);
    }

    fn set_scroll(&mut self, scroll: usize) {
        self.app.set_scroll(scroll);
    }

    fn max_scroll(&self) -> usize {
        self.app.max_scroll()
    }

    fn navigate_to_boundary(&mut self, last: bool) {
        self.app.navigate_diff_to_boundary(last);
    }

    fn vertical_page_delta(&self, full_page: bool) -> isize {
        self.app.vertical_page_delta(full_page)
    }

    fn move_grep_match(&mut self, delta: isize) {
        self.app.move_grep_match(delta);
    }
}
