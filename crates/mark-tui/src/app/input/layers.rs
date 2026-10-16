use super::super::{
    AppEffect, DiffApp,
    controllers::{
        filter::FilterController,
        menu::{MenuController, MenuRouteResult},
        navigation::NavigationController,
    },
    is_quit_key,
};
use super::event_context::{KeyEventContext, KeyEventCtx};
use crate::render::compositor::{ComponentEventResult, EventComponent, route_event_through_layers};
use crossterm::event::KeyEvent;
use mark_core::MarkResult;

#[derive(Clone, Copy)]
enum KeyComponent {
    Navigation,
    ErrorLogResize,
    PrefixStart,
    SingleGlobal,
    PendingPrefix,
    ErrorLogClose,
    OpenMenu,
    AnnotationInput,
    SubmitMarks,
    FilterInput,
    MouseScrollReset,
    EditorShortcut,
    Quit,
    AnnotationDraftBindings,
    AnnotationTarget,
}

impl<C: KeyEventContext> EventComponent<KeyEvent, C> for KeyComponent {
    fn handle_event(&self, key: KeyEvent, ctx: &mut C) -> MarkResult<ComponentEventResult> {
        Ok(match self {
            Self::Navigation => {
                ComponentEventResult::consumed_if(NavigationController::handle_key(ctx, key))
            }
            Self::ErrorLogResize => {
                ComponentEventResult::consumed_if(ctx.handle_error_log_resize_key(key))
            }
            Self::PrefixStart => {
                ComponentEventResult::consumed_if(ctx.begin_prefix_if_matches(key))
            }
            Self::SingleGlobal => key_route_result(ctx.handle_single_global_key(key)?),
            Self::PendingPrefix => key_route_result(ctx.handle_pending_prefix_key(key)?),
            Self::ErrorLogClose => {
                ComponentEventResult::consumed_if(ctx.close_error_log_on_key(key))
            }
            Self::OpenMenu => menu_route_result(MenuController::route_open_menu(ctx, key)?),
            Self::AnnotationInput => {
                ComponentEventResult::consumed_if(ctx.handle_annotation_input_key_if_open(key))
            }
            Self::SubmitMarks => key_route_result(ctx.handle_submit_marks_key(key)?),
            Self::FilterInput => {
                ComponentEventResult::consumed_if(FilterController::handle_input_key(ctx, key))
            }
            Self::MouseScrollReset => {
                ctx.reset_mouse_scroll();
                ComponentEventResult::Ignored
            }
            Self::EditorShortcut if ctx.editor_shortcut_requested(key) => {
                ComponentEventResult::Effect(AppEffect::OpenEditorShortcut)
            }
            Self::Quit if is_quit_key(key) => ComponentEventResult::Quit,
            Self::EditorShortcut | Self::Quit => ComponentEventResult::Ignored,
            Self::AnnotationDraftBindings => {
                ComponentEventResult::consumed_if(ctx.handle_annotation_save_or_cancel_key(key))
            }
            Self::AnnotationTarget => {
                ComponentEventResult::consumed_if(ctx.handle_annotation_target_key_if_open(key))
            }
        })
    }
}

// The compositor traverses this list in reverse, so editable/modal layers near
// the end preempt global actions and navigation near the beginning.
const KEY_LAYERS: &[KeyComponent] = &[
    KeyComponent::Navigation,
    KeyComponent::ErrorLogResize,
    KeyComponent::PrefixStart,
    KeyComponent::SingleGlobal,
    KeyComponent::PendingPrefix,
    KeyComponent::ErrorLogClose,
    KeyComponent::OpenMenu,
    KeyComponent::AnnotationInput,
    KeyComponent::FilterInput,
    KeyComponent::MouseScrollReset,
    KeyComponent::EditorShortcut,
    KeyComponent::Quit,
    KeyComponent::AnnotationTarget,
    KeyComponent::SubmitMarks,
    KeyComponent::AnnotationDraftBindings,
];

/// Maps a handler's `Some(should_quit)` / `None` result to a layer result.
fn key_route_result(handled: Option<bool>) -> ComponentEventResult {
    match handled {
        Some(true) => ComponentEventResult::Quit,
        Some(false) => ComponentEventResult::Consumed,
        None => ComponentEventResult::Ignored,
    }
}

fn menu_route_result(result: MenuRouteResult) -> ComponentEventResult {
    match result {
        MenuRouteResult::Ignored => ComponentEventResult::Ignored,
        MenuRouteResult::Consumed => ComponentEventResult::Consumed,
        MenuRouteResult::Quit => ComponentEventResult::Quit,
    }
}

pub(super) fn route_key_through_layers(
    app: &mut DiffApp,
    key: KeyEvent,
) -> MarkResult<ComponentEventResult> {
    let mut ctx = KeyEventCtx { app };
    route_event_through_layers(KEY_LAYERS, key, &mut ctx)
}

#[cfg(test)]
mod tests;
