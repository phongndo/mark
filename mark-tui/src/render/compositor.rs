use mark_core::MarkResult;
use ratatui::{Frame, layout::Rect};

use crate::app::DiffApp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComponentEventResult {
    Ignored,
    Consumed,
    Quit,
}

pub(crate) type EventHandler<E> = fn(E, &mut DiffApp) -> MarkResult<ComponentEventResult>;

#[derive(Clone, Copy)]
pub(crate) struct EventLayer<E> {
    id: ComponentId,
    handle: EventHandler<E>,
}

impl<E> EventLayer<E> {
    pub(crate) const fn new(id: ComponentId, handle: EventHandler<E>) -> Self {
        Self { id, handle }
    }
}

pub(crate) fn route_event_through_layers<E: Copy>(
    layers: &[EventLayer<E>],
    event: E,
    app: &mut DiffApp,
) -> MarkResult<ComponentEventResult> {
    for layer in layers.iter().rev() {
        let _component_id = layer.id;
        let result = (layer.handle)(event, app)?;
        if !matches!(result, ComponentEventResult::Ignored) {
            return Ok(result);
        }
    }
    Ok(ComponentEventResult::Ignored)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComponentId {
    Header,
    FileSidebar,
    FilterBar,
    ErrorLogPanel,
    Toasts,
    AnnotationDraftBindings,
    QuitKey,
    MouseScrollReset,
    FilterInput,
    AnnotationInput,
    HelpMenu,
    BranchMenu,
    CommitMenu,
    ReviewInput,
    DiffMenu,
    ColorSchemePicker,
    OptionsMenu,
    ErrorLog,
    Prefix,
    GlobalAction,
    ErrorLogResize,
    Navigation,
    OpenMenuScroll,
    FileSidebarResize,
    DiffView,
}

pub(crate) struct AppCtx<'a> {
    app: &'a mut DiffApp,
}

impl<'a> AppCtx<'a> {
    pub(crate) fn new(app: &'a mut DiffApp) -> Self {
        Self { app }
    }

    pub(crate) fn app(&mut self) -> &mut DiffApp {
        self.app
    }
}

pub(crate) trait UiComponent {
    fn id(&self) -> ComponentId;

    fn render(&mut self, _frame: &mut Frame<'_>, _ctx: &mut AppCtx<'_>) {}
}

pub(crate) struct Compositor<'a> {
    layers: Vec<Box<dyn UiComponent + 'a>>,
}

impl<'a> Compositor<'a> {
    pub(crate) fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub(crate) fn push(&mut self, layer: impl UiComponent + 'a) {
        self.layers.push(Box::new(layer));
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>, app: &mut DiffApp) {
        for layer in &mut self.layers {
            let _component_id = layer.id();
            let mut ctx = AppCtx::new(app);
            layer.render(frame, &mut ctx);
        }
    }
}

pub(crate) struct RectComponent {
    id: ComponentId,
    area: Rect,
    render: fn(&mut Frame<'_>, &mut DiffApp, Rect),
}

impl RectComponent {
    pub(crate) fn new(
        id: ComponentId,
        area: Rect,
        render: fn(&mut Frame<'_>, &mut DiffApp, Rect),
    ) -> Self {
        Self { id, area, render }
    }
}

impl UiComponent for RectComponent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn render(&mut self, frame: &mut Frame<'_>, ctx: &mut AppCtx<'_>) {
        (self.render)(frame, ctx.app(), self.area);
    }
}
