use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::theme::HORIZONTAL_SCROLL_STEP;

use super::super::is_plain_char_key;

pub(in crate::app) trait NavigationContext {
    fn filters_active(&self) -> bool;
    fn grep_filter_active(&self) -> bool;
    fn clear_all_filters(&mut self);
    fn scroll_or_focus_hunk(&mut self, delta: isize);
    fn navigate_vertical_page(&mut self, delta: isize) {
        self.scroll_or_focus_hunk(delta);
    }
    fn scroll_horizontally_by(&mut self, delta: isize);
    fn set_scroll(&mut self, scroll: usize);
    fn max_scroll(&self) -> usize;
    fn navigate_to_boundary(&mut self, last: bool) {
        self.set_scroll(if last { self.max_scroll() } else { 0 });
    }
    fn vertical_page_delta(&self, _full_page: bool) -> isize {
        20
    }
    fn move_grep_match(&mut self, delta: isize);
}

pub(in crate::app) struct NavigationController;

impl NavigationController {
    pub(in crate::app) fn handle_key<C: NavigationContext + ?Sized>(
        ctx: &mut C,
        key: KeyEvent,
    ) -> bool {
        match key.code {
            KeyCode::Esc if ctx.filters_active() => ctx.clear_all_filters(),
            KeyCode::Down | KeyCode::Char('j') => ctx.scroll_or_focus_hunk(1),
            KeyCode::Up | KeyCode::Char('k') => ctx.scroll_or_focus_hunk(-1),
            KeyCode::Left | KeyCode::Char('h') => {
                ctx.scroll_horizontally_by(-(HORIZONTAL_SCROLL_STEP as isize));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                ctx.scroll_horizontally_by(HORIZONTAL_SCROLL_STEP as isize);
            }
            KeyCode::PageDown => ctx.navigate_vertical_page(ctx.vertical_page_delta(true)),
            KeyCode::Char('d') if is_plain_char_key(key, 'd') => {
                ctx.navigate_vertical_page(ctx.vertical_page_delta(false));
            }
            KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                ctx.navigate_vertical_page(ctx.vertical_page_delta(false));
            }
            KeyCode::PageUp => ctx.navigate_vertical_page(-ctx.vertical_page_delta(true)),
            KeyCode::Char('u') if is_plain_char_key(key, 'u') => {
                ctx.navigate_vertical_page(-ctx.vertical_page_delta(false));
            }
            KeyCode::Home => ctx.navigate_to_boundary(false),
            KeyCode::Char('g') if is_plain_char_key(key, 'g') => ctx.navigate_to_boundary(false),
            KeyCode::End | KeyCode::Char('G') => ctx.navigate_to_boundary(true),
            KeyCode::Char('n') if ctx.grep_filter_active() => ctx.move_grep_match(1),
            KeyCode::Char('p') | KeyCode::Char('N') if ctx.grep_filter_active() => {
                ctx.move_grep_match(-1);
            }
            _ => return false,
        }

        true
    }
}
