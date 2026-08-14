use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::NavigationContext;
use crate::{app::is_plain_char_key, theme::HORIZONTAL_SCROLL_STEP};

pub(in crate::app) struct NavigationController;

impl NavigationController {
    pub(in crate::app) fn handle_key<C: NavigationContext + ?Sized>(
        ctx: &mut C,
        key: KeyEvent,
    ) -> bool {
        if let KeyCode::Char(character) = key.code
            && is_plain_char_key(key, character)
            && character.is_ascii_digit()
            && ctx.push_vim_motion_digit(character.to_digit(10).unwrap_or_default())
        {
            return true;
        }

        let explicit_count = ctx.take_vim_motion_count();
        let count = explicit_count.unwrap_or(1).min(isize::MAX as usize) as isize;
        let horizontal_delta = (HORIZONTAL_SCROLL_STEP as isize).saturating_mul(count);

        let handled = match key.code {
            KeyCode::Esc if ctx.filters_active() => {
                ctx.clear_all_filters();
                true
            }
            KeyCode::Esc if explicit_count.is_some() => true,
            KeyCode::Down | KeyCode::Char('j') => {
                ctx.scroll_or_focus_hunk(count);
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ctx.scroll_or_focus_hunk(-count);
                true
            }
            KeyCode::Char('+') => {
                ctx.scroll_or_focus_hunk(count);
                true
            }
            KeyCode::Char('-') => {
                ctx.scroll_or_focus_hunk(-count);
                true
            }
            KeyCode::Left | KeyCode::Char('h') => {
                ctx.scroll_horizontally_by(-horizontal_delta);
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                ctx.scroll_horizontally_by(horizontal_delta);
                true
            }
            KeyCode::Char('0') | KeyCode::Char('^') if is_plain_key(key) => {
                ctx.set_horizontal_scroll_to_boundary(false);
                true
            }
            KeyCode::Char('$') if is_plain_key(key) => {
                ctx.set_horizontal_scroll_to_boundary(true);
                true
            }
            KeyCode::PageDown => {
                ctx.navigate_vertical_page(ctx.vertical_page_delta(true));
                true
            }
            KeyCode::Char('d') if is_plain_char_key(key, 'd') => {
                ctx.navigate_vertical_page(ctx.vertical_page_delta(false));
                true
            }
            KeyCode::PageUp => {
                ctx.navigate_vertical_page(-ctx.vertical_page_delta(true));
                true
            }
            KeyCode::Char('u') if is_plain_char_key(key, 'u') => {
                ctx.navigate_vertical_page(-ctx.vertical_page_delta(false));
                true
            }
            KeyCode::Home => {
                ctx.navigate_to_boundary(false);
                true
            }
            KeyCode::Char('g') if is_plain_key(key) => {
                ctx.navigate_to_boundary(false);
                true
            }
            KeyCode::End => {
                ctx.navigate_to_boundary(true);
                true
            }
            KeyCode::Char('G') if is_plain_key(key) => {
                ctx.navigate_to_boundary(true);
                true
            }
            KeyCode::Char('H') if is_plain_key(key) => {
                ctx.navigate_to_viewport_position(-1, count as usize);
                true
            }
            KeyCode::Char('M') if is_plain_key(key) => {
                ctx.navigate_to_viewport_position(0, count as usize);
                true
            }
            KeyCode::Char('L') if is_plain_key(key) => {
                ctx.navigate_to_viewport_position(1, count as usize);
                true
            }
            KeyCode::Char('n') if ctx.grep_filter_active() => {
                ctx.move_grep_match(count);
                true
            }
            KeyCode::Char('p') | KeyCode::Char('N') if ctx.grep_filter_active() => {
                ctx.move_grep_match(-count);
                true
            }
            _ => false,
        };

        if !handled {
            ctx.clear_vim_motion();
        }
        handled
    }
}

fn is_plain_key(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}
