use super::{AppEffect, DiffApp, LiveReloadStatus};
use crate::controls::CrosstermTerminal;
use crate::event_reader::TerminalEventReader;
use crate::live_diff::{LiveDiff, LiveDiffReload, live_diff_supported};
use crate::render::draw;
use crate::theme::MAX_READY_EVENTS_PER_FRAME;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use mark_core::MarkResult;
use ratatui::layout::Rect;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;

const MAX_SCROLL_EVENTS_DRAIN_PER_FRAME: usize = 2048;
const MOUSE_SCROLL_CONTEXT_QUIET_PERIOD: Duration = Duration::from_millis(150);

pub(crate) async fn run_loop(
    terminal: &mut CrosstermTerminal,
    app: &mut DiffApp,
    live_updates: bool,
    live_diff: &mut Option<LiveDiff>,
) -> MarkResult<()> {
    let mut events = TerminalEventReader::start("mark-diff-events")?;
    let mut scroll_fence = MouseScrollContextFence::default();

    loop {
        app.expire_toasts(Instant::now());
        drain_live_diff_invalidation(app, live_diff.as_ref());
        sync_live_diff(live_diff, app, live_updates);
        drain_live_reloads(
            app,
            live_diff.as_mut().map(|live_diff| &mut live_diff.reload_rx),
        );
        app.drain_pending_diff_load();
        app.drain_diff_prefetch();
        app.start_due_filter_apply();
        app.drain_filter_worker();
        app.drain_syntax();
        if app.runtime.dirty {
            if app.runtime.terminal_clear_requested {
                terminal.clear()?;
                app.runtime.terminal_clear_requested = false;
            }
            terminal.draw(|frame| draw(frame, app))?;
            app.runtime.dirty = false;
            app.start_diff_prefetches();
        }
        app.start_pending_editor_reload();
        if app.drain_editor_reload() {
            continue;
        }

        if let Some(event) = events.read_timeout(app.event_poll()).await?
            && handle_ready_events(app, live_diff, event, &mut events, &mut scroll_fence)?
        {
            break;
        }
    }

    Ok(())
}

fn handle_ready_events(
    app: &mut DiffApp,
    live_diff: &mut Option<LiveDiff>,
    first_event: Event,
    events: &mut TerminalEventReader,
    scroll_fence: &mut MouseScrollContextFence,
) -> MarkResult<bool> {
    let mut pending = Some(first_event);
    let mut handled = 0usize;

    'events: while handled < MAX_READY_EVENTS_PER_FRAME {
        let event = if let Some(event) = pending.take() {
            Some(event)
        } else {
            events.try_read()?
        };
        let Some(event) = event else {
            break;
        };

        if is_mouse_scroll_event(&event) {
            let drained = drain_mouse_scroll_run(event, events)?;
            if let Some(quit) = drained.quit_event {
                return handle_event(app, quit, live_diff, events);
            }

            if let Some(next_event) = drained.next_event {
                // A newer non-scroll input means the scroll burst sitting in
                // front of it is stale. Prefer the user's latest intent over
                // faithfully replaying old wheel ticks after they have already
                // moved on to keyboard/mouse navigation.
                scroll_fence.observe_scroll(Instant::now());
                if handle_non_scroll_event(app, next_event, live_diff, events, scroll_fence)? {
                    return Ok(true);
                }
                handled += 1;
                if app.runtime.dirty {
                    break 'events;
                }
                continue;
            }

            if scroll_fence.should_suppress_scroll(Instant::now()) {
                handled += 1;
                continue;
            }

            for (mouse, ticks) in mouse_scroll_runs(drained.scroll_events) {
                if handle_mouse_scroll_burst(app, mouse, ticks, live_diff, events)? {
                    return Ok(true);
                }
            }
            handled += 1;

            if app.runtime.dirty {
                break 'events;
            }
            continue;
        }

        if handle_non_scroll_event(app, event, live_diff, events, scroll_fence)? {
            return Ok(true);
        }
        handled += 1;
        if app.runtime.dirty {
            break;
        }
    }

    Ok(false)
}

/// Prevents one physical wheel/trackpad gesture from being routed to two
/// different UI layers when a modal opens or closes mid-gesture.
#[derive(Debug, Default)]
struct MouseScrollContextFence {
    last_scroll: Option<Instant>,
    context_fenced: bool,
}

impl MouseScrollContextFence {
    fn observe_scroll(&mut self, now: Instant) {
        self.last_scroll = Some(now);
    }

    fn context_changed(&mut self, now: Instant) {
        self.context_fenced = self.last_scroll.is_some_and(|last_scroll| {
            now.saturating_duration_since(last_scroll) <= MOUSE_SCROLL_CONTEXT_QUIET_PERIOD
        });
    }

    fn should_suppress_scroll(&mut self, now: Instant) -> bool {
        let continues_previous_gesture = self.last_scroll.is_some_and(|last_scroll| {
            now.saturating_duration_since(last_scroll) <= MOUSE_SCROLL_CONTEXT_QUIET_PERIOD
        });
        self.last_scroll = Some(now);

        if self.context_fenced && continues_previous_gesture {
            return true;
        }

        self.context_fenced = false;
        false
    }
}

fn handle_non_scroll_event(
    app: &mut DiffApp,
    event: Event,
    live_diff: &mut Option<LiveDiff>,
    events: &mut TerminalEventReader,
    scroll_fence: &mut MouseScrollContextFence,
) -> MarkResult<bool> {
    let previous_context = app.mouse_scroll_context();
    let should_quit = handle_event(app, event, live_diff, events)?;
    if app.mouse_scroll_context() != previous_context {
        scroll_fence.context_changed(Instant::now());
    }
    Ok(should_quit)
}

struct DrainedMouseScrollRun {
    scroll_events: Vec<Event>,
    next_event: Option<Event>,
    quit_event: Option<Event>,
}

fn drain_mouse_scroll_run(
    first_event: Event,
    events: &mut TerminalEventReader,
) -> MarkResult<DrainedMouseScrollRun> {
    let mut scroll_events = vec![first_event];
    let mut next_event = None;
    let mut quit_event = None;

    for _ in 0..MAX_SCROLL_EVENTS_DRAIN_PER_FRAME {
        let Some(event) = events.try_read()? else {
            break;
        };

        if is_quit_event(&event) {
            quit_event = Some(event);
            break;
        }

        if is_mouse_scroll_event(&event) {
            scroll_events.push(event);
        } else {
            next_event = Some(event);
            break;
        }
    }

    Ok(DrainedMouseScrollRun {
        scroll_events,
        next_event,
        quit_event,
    })
}

fn mouse_scroll_runs(events: Vec<Event>) -> Vec<(MouseEvent, usize)> {
    let mut runs: Vec<(MouseEvent, usize)> = Vec::new();
    for event in events {
        let Event::Mouse(mouse) = event else {
            continue;
        };
        if !is_mouse_scroll_kind(mouse.kind) {
            continue;
        }
        if let Some((last, ticks)) = runs.last_mut()
            && same_mouse_scroll(*last, mouse)
        {
            *ticks = ticks.saturating_add(1);
            continue;
        }
        runs.push((mouse, 1));
    }
    runs
}

fn same_mouse_scroll(left: MouseEvent, right: MouseEvent) -> bool {
    left.kind == right.kind
        && left.column == right.column
        && left.row == right.row
        && left.modifiers == right.modifiers
}

fn handle_mouse_scroll_burst(
    app: &mut DiffApp,
    mouse: MouseEvent,
    ticks: usize,
    live_diff: &mut Option<LiveDiff>,
    events: &mut TerminalEventReader,
) -> MarkResult<bool> {
    let outcome = app.handle_mouse_scroll_burst_with_effects(mouse, ticks)?;
    let should_quit = outcome.handled_quit_request().unwrap_or(false);
    run_event_effects(app, outcome.into_effects(), live_diff, events)?;
    Ok(should_quit)
}

fn is_quit_event(event: &Event) -> bool {
    matches!(event, Event::Key(key) if is_quit_key(*key))
}

fn is_mouse_scroll_event(event: &Event) -> bool {
    matches!(event, Event::Mouse(mouse) if is_mouse_scroll_kind(mouse.kind))
}

fn is_mouse_scroll_kind(kind: MouseEventKind) -> bool {
    matches!(
        kind,
        MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    )
}

pub(crate) fn drain_live_diff_invalidation(app: &mut DiffApp, live_diff: Option<&LiveDiff>) {
    if live_diff.is_some_and(|live_diff| live_diff.take_invalidated()) {
        app.mark_live_reload_invalidated();
    }
}

pub(crate) fn sync_live_diff(
    live_diff: &mut Option<LiveDiff>,
    app: &mut DiffApp,
    live_updates: bool,
) {
    if !live_updates
        || !app.jobs.live_updates.allowed()
        || !app.jobs.live_updates.enabled()
        || !live_diff_supported(&app.document.options)
    {
        *live_diff = None;
        app.jobs.live_diff_failed_options = None;
        app.jobs.live_updates.reset_reload();
        app.clear_cached_diff_choices();
        return;
    }

    if live_diff
        .as_ref()
        .is_some_and(|live_diff| live_diff.options == app.document.options)
    {
        return;
    }
    if app.jobs.live_diff_failed_options.as_ref() == Some(&app.document.options) {
        return;
    }

    match LiveDiff::start(app.document.options.clone(), &app.document.changeset.repo) {
        Ok(next_live_diff) => {
            app.jobs.live_diff_failed_options = None;
            app.jobs.live_updates.reset_reload();
            *live_diff = Some(next_live_diff);
        }
        Err(error) => {
            *live_diff = None;
            app.jobs.live_diff_failed_options = Some(app.document.options.clone());
            app.jobs.live_updates.reset_reload();
            app.clear_cached_diff_choices();
            app.set_error_log(format!("live reload unavailable: {error}"));
        }
    }
}

pub(crate) fn drain_live_reloads(
    app: &mut DiffApp,
    live_reload_rx: Option<&mut Receiver<LiveDiffReload>>,
) {
    let Some(live_reload_rx) = live_reload_rx else {
        return;
    };

    while let Ok(reload) = live_reload_rx.try_recv() {
        match reload {
            LiveDiffReload::Started => {
                if app.jobs.live_updates.status() != Some(LiveReloadStatus::Pending) {
                    app.mark_live_reload_pending();
                }
            }
            LiveDiffReload::Loaded(Ok(changeset)) => app.replace_changeset(changeset),
            LiveDiffReload::Loaded(Err(error)) => {
                app.jobs.live_updates.reset_reload();
                app.set_error_log(format!("live reload failed: {error}"));
            }
        }
    }
}

pub(crate) fn handle_event(
    app: &mut DiffApp,
    event: Event,
    live_diff: &mut Option<LiveDiff>,
    events: &mut TerminalEventReader,
) -> MarkResult<bool> {
    drain_live_diff_invalidation(app, live_diff.as_ref());
    if app.debug_notifications_enabled() {
        app.set_debug_notice(format!("event: {}", event_label(&event)));
    }

    match event {
        Event::Key(key) if app.ignore_post_editor_quit_key(key, Instant::now()) => Ok(false),
        Event::Key(key) => handle_key_event(app, key, live_diff, events),
        Event::Mouse(mouse) => {
            let outcome = app.handle_mouse_with_effects(mouse)?;
            let should_quit = outcome.handled_quit_request().unwrap_or(false);
            run_event_effects(app, outcome.into_effects(), live_diff, events)?;
            Ok(should_quit)
        }
        Event::FocusLost => {
            app.clear_diff_mouse_hover();
            Ok(false)
        }
        Event::Resize(width, height) => {
            app.clear_diff_mouse_hover();
            app.set_terminal_area(Rect {
                x: 0,
                y: 0,
                width,
                height,
            });
            app.apply_responsive_layout(width);
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn handle_key_event(
    app: &mut DiffApp,
    key: KeyEvent,
    live_diff: &mut Option<LiveDiff>,
    events: &mut TerminalEventReader,
) -> MarkResult<bool> {
    let outcome = app.handle_key_with_effects(key)?;
    let should_quit = outcome.handled_quit_request().unwrap_or(false);
    run_event_effects(app, outcome.into_effects(), live_diff, events)?;
    Ok(should_quit)
}

fn run_event_effects(
    app: &mut DiffApp,
    effects: Vec<AppEffect>,
    live_diff: &mut Option<LiveDiff>,
    events: &mut TerminalEventReader,
) -> MarkResult<()> {
    for effect in effects {
        match effect {
            AppEffect::OpenEditorShortcut => {
                if app.annotations_state.annotation_draft.is_some() {
                    let paused_events = events.pause();
                    app.open_annotation_draft_in_editor();
                    paused_events.resume()?;
                } else if let Some(editor) = app.prepare_focused_hunk_editor() {
                    let paused_events = events.pause();
                    app.open_prepared_hunk_in_editor(editor, Some(live_diff));
                    paused_events.resume()?;
                }
            }
            effect => app.run_effect(effect)?,
        }
    }
    Ok(())
}

fn event_label(event: &Event) -> String {
    match event {
        Event::Key(key) => format!("key {:?}", key.code),
        Event::Mouse(mouse) => format!("mouse {:?}", mouse.kind),
        Event::Resize(width, height) => format!("resize {width}x{height}"),
        Event::FocusGained => "focus gained".to_owned(),
        Event::FocusLost => "focus lost".to_owned(),
        Event::Paste(text) => format!("paste {} bytes", text.len()),
    }
}

pub(crate) fn is_quit_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::SHIFT)
        && key.code == KeyCode::Char('c')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseEvent};
    use tokio::sync::mpsc;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn scroll_context_fence_drops_the_tail_of_an_active_gesture() {
        let start = Instant::now();
        let mut fence = MouseScrollContextFence::default();

        fence.observe_scroll(start);
        fence.context_changed(start + Duration::from_millis(1));
        assert!(fence.should_suppress_scroll(start + Duration::from_millis(2)));
        assert!(fence.should_suppress_scroll(start + Duration::from_millis(100)));

        assert!(!fence.should_suppress_scroll(
            start
                + Duration::from_millis(100)
                + MOUSE_SCROLL_CONTEXT_QUIET_PERIOD
                + Duration::from_millis(1)
        ));
    }

    #[test]
    fn scroll_context_fence_does_not_block_a_new_gesture() {
        let start = Instant::now();
        let mut fence = MouseScrollContextFence::default();

        fence.context_changed(start);
        assert!(!fence.should_suppress_scroll(start));

        fence.observe_scroll(start);
        fence.context_changed(start + MOUSE_SCROLL_CONTEXT_QUIET_PERIOD + Duration::from_millis(1));
        assert!(!fence.should_suppress_scroll(
            start + MOUSE_SCROLL_CONTEXT_QUIET_PERIOD + Duration::from_millis(2)
        ));
    }

    #[test]
    fn scroll_run_drain_surfaces_quit_before_more_scroll() {
        let (tx, rx) = mpsc::channel(4);
        tx.try_send(Ok(mouse(MouseEventKind::ScrollDown, 4, 5)))
            .unwrap();
        tx.try_send(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ))))
        .unwrap();
        tx.try_send(Ok(mouse(MouseEventKind::ScrollDown, 4, 5)))
            .unwrap();
        let mut reader = TerminalEventReader::from_receiver(rx);

        let drained = drain_mouse_scroll_run(mouse(MouseEventKind::ScrollDown, 4, 5), &mut reader)
            .expect("drain scroll run");

        assert_eq!(drained.scroll_events.len(), 2);
        assert!(matches!(drained.quit_event, Some(Event::Key(key)) if is_quit_key(key)));
        assert!(drained.next_event.is_none());
    }
}
