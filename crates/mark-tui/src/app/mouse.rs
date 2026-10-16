use crossterm::event::{MouseEvent, MouseEventKind};
use mark_core::MarkResult;

use super::{ActionOutcome, DiffApp};
use crate::render::compositor::{ComponentEventResult, EventComponent, route_event_through_layers};
use crate::theme::HORIZONTAL_SCROLL_STEP;

mod annotation_clicks;
mod click;
mod cursor;
mod event_context;
mod scroll;
mod sidebar;

pub(crate) use scroll::{MouseScroll, MouseScrollDirection};

use event_context::{MouseEventContext, MouseEventCtx};

#[derive(Clone, Copy)]
enum MouseComponent {
    Diff,
    ErrorLogResize,
    OptionsMenu,
    ColorSchemePicker,
    FileSidebarResize,
    HelpMenu,
    OpenMenuScroll,
}

impl<C: MouseEventContext> EventComponent<MouseEvent, C> for MouseComponent {
    fn handle_event(&self, mouse: MouseEvent, ctx: &mut C) -> MarkResult<ComponentEventResult> {
        let consumed = match self {
            Self::Diff => ctx.handle_diff_mouse(mouse),
            Self::ErrorLogResize => ctx.handle_error_log_resize_mouse(mouse),
            Self::OptionsMenu => ctx.handle_options_menu_mouse(mouse),
            Self::ColorSchemePicker => ctx.handle_color_scheme_picker_mouse(mouse),
            Self::FileSidebarResize => ctx.handle_file_sidebar_resize_mouse(mouse),
            Self::HelpMenu => ctx.handle_help_menu_mouse(mouse),
            Self::OpenMenuScroll => ctx.handle_open_menu_scroll(mouse.kind),
        };
        Ok(ComponentEventResult::consumed_if(consumed))
    }
}

const MOUSE_LAYERS: &[MouseComponent] = &[
    MouseComponent::Diff,
    MouseComponent::ErrorLogResize,
    MouseComponent::OptionsMenu,
    MouseComponent::ColorSchemePicker,
    MouseComponent::FileSidebarResize,
    MouseComponent::HelpMenu,
    MouseComponent::OpenMenuScroll,
];

fn route_mouse_through_layers(
    app: &mut DiffApp,
    mouse: MouseEvent,
) -> MarkResult<ComponentEventResult> {
    let mut ctx = MouseEventCtx::new(app);
    route_event_through_layers(MOUSE_LAYERS, mouse, &mut ctx)
}

impl DiffApp {
    pub(crate) fn handle_mouse_with_effects(
        &mut self,
        mouse: MouseEvent,
    ) -> MarkResult<ActionOutcome> {
        self.input.clear_vim_motion();
        let mut outcome =
            ActionOutcome::from_component_event_result(route_mouse_through_layers(self, mouse)?);
        outcome.extend_effects(self.take_queued_effects());
        Ok(outcome)
    }

    pub(crate) fn handle_mouse_scroll_burst_with_effects(
        &mut self,
        mouse: MouseEvent,
        ticks: usize,
    ) -> MarkResult<ActionOutcome> {
        self.input.clear_vim_motion();
        if ticks <= 1 || !is_scroll_kind(mouse.kind) {
            return self.handle_mouse_with_effects(mouse);
        }

        if self.handle_open_menu_mouse_scroll_ticks(mouse.kind, ticks) {
            return Ok(self.mouse_burst_outcome());
        }
        if self.overlays.help_menu_is_open() {
            self.input.reset_mouse_scroll();
            return Ok(self.mouse_burst_outcome());
        }

        self.clear_code_selection();
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.input.reset_mouse_scroll();
                    self.scroll_file_sidebar_by(ticks.min(isize::MAX as usize) as isize);
                } else {
                    self.mouse_scroll_or_focus_hunk_ticks(MouseScrollDirection::Down, ticks);
                }
                Ok(self.mouse_burst_outcome())
            }
            MouseEventKind::ScrollUp => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.input.reset_mouse_scroll();
                    self.scroll_file_sidebar_by(-(ticks.min(isize::MAX as usize) as isize));
                } else {
                    self.mouse_scroll_or_focus_hunk_ticks(MouseScrollDirection::Up, ticks);
                }
                Ok(self.mouse_burst_outcome())
            }
            MouseEventKind::ScrollLeft => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.input.reset_mouse_scroll();
                } else {
                    let delta = HORIZONTAL_SCROLL_STEP
                        .saturating_mul(ticks)
                        .min(isize::MAX as usize) as isize;
                    self.scroll_horizontally_by(-delta);
                }
                Ok(self.mouse_burst_outcome())
            }
            MouseEventKind::ScrollRight => {
                if self.is_file_sidebar_position(mouse.column, mouse.row) {
                    self.input.reset_mouse_scroll();
                } else {
                    let delta = HORIZONTAL_SCROLL_STEP
                        .saturating_mul(ticks)
                        .min(isize::MAX as usize) as isize;
                    self.scroll_horizontally_by(delta);
                }
                Ok(self.mouse_burst_outcome())
            }
            _ => self.handle_mouse_with_effects(mouse),
        }
    }

    fn mouse_burst_outcome(&mut self) -> ActionOutcome {
        let mut outcome = ActionOutcome::consumed();
        outcome.extend_effects(self.take_queued_effects());
        outcome
    }

    #[cfg(test)]
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> MarkResult<()> {
        let outcome = self.handle_mouse_with_effects(mouse)?;
        self.run_effects(outcome.into_effects())
    }
}

fn is_scroll_kind(kind: MouseEventKind) -> bool {
    matches!(
        kind,
        MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    )
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    use super::*;

    #[derive(Default)]
    struct FakeMouseCtx {
        open_menu_scroll: bool,
        help_menu: bool,
        file_sidebar_resize: bool,
        color_scheme_picker: bool,
        options_menu: bool,
        error_log_resize: bool,
        diff: bool,
        diff_calls: usize,
    }

    impl MouseEventContext for FakeMouseCtx {
        fn handle_open_menu_scroll(&mut self, _kind: MouseEventKind) -> bool {
            self.open_menu_scroll
        }

        fn handle_help_menu_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.help_menu
        }

        fn handle_file_sidebar_resize_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.file_sidebar_resize
        }

        fn handle_color_scheme_picker_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.color_scheme_picker
        }

        fn handle_options_menu_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.options_menu
        }

        fn handle_error_log_resize_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.error_log_resize
        }

        fn handle_diff_mouse(&mut self, _mouse: MouseEvent) -> bool {
            self.diff_calls += 1;
            self.diff
        }
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn open_menu_scroll_preempts_diff_mouse_layer() {
        let mut ctx = FakeMouseCtx {
            open_menu_scroll: true,
            diff: true,
            ..Default::default()
        };

        let result =
            route_event_through_layers(MOUSE_LAYERS, mouse(MouseEventKind::ScrollDown), &mut ctx)
                .expect("route mouse");

        assert_eq!(result, ComponentEventResult::Consumed);
        assert_eq!(ctx.diff_calls, 0);
    }

    #[test]
    fn diff_layer_handles_mouse_when_overlays_ignore_it() {
        let mut ctx = FakeMouseCtx {
            diff: true,
            ..Default::default()
        };

        let result =
            route_event_through_layers(MOUSE_LAYERS, mouse(MouseEventKind::Moved), &mut ctx)
                .expect("route mouse");

        assert_eq!(result, ComponentEventResult::Consumed);
        assert_eq!(ctx.diff_calls, 1);
    }
}
