use mark_core::MarkResult;
use ratatui::{Frame, layout::Rect};

use crate::app::AppEffect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComponentEventResult {
    Ignored,
    Consumed,
    Effect(AppEffect),
    Quit,
}

pub(crate) trait EventComponent<E, Ctx: ?Sized>: Sync {
    fn id(&self) -> ComponentId;

    fn handle_event(&self, event: E, ctx: &mut Ctx) -> MarkResult<ComponentEventResult>;
}

pub(crate) fn route_event_through_layers<E: Copy, Ctx: ?Sized, C>(
    layers: &[C],
    event: E,
    ctx: &mut Ctx,
) -> MarkResult<ComponentEventResult>
where
    C: EventComponent<E, Ctx>,
{
    for layer in layers.iter().rev() {
        let _component_id = layer.id();
        let result = layer.handle_event(event, ctx)?;
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
    AnnotationTarget,
    AnnotationDraftBindings,
    QuitKey,
    EditorShortcut,
    MouseScrollReset,
    FilterInput,
    AnnotationInput,
    HelpMenu,
    MarksConfirm,
    BranchMenu,
    CommitMenu,
    ReviewInput,
    DiffMenu,
    ColorSchemePicker,
    OptionsMenu,
    AnnotationMenu,
    ErrorLog,
    Prefix,
    GlobalAction,
    OpenMenuKey,
    ErrorLogResize,
    Navigation,
    OpenMenuScroll,
    FileSidebarResize,
    DiffView,
}

pub(crate) trait RenderContext {
    fn render_rect_component(&mut self, frame: &mut Frame<'_>, id: ComponentId, area: Rect);
}

pub(crate) struct Compositor {
    layers: Vec<RectComponent>,
}

impl Compositor {
    pub(crate) fn new() -> Self {
        Self {
            layers: Vec::with_capacity(8),
        }
    }

    pub(crate) fn push(&mut self, layer: RectComponent) {
        self.layers.push(layer);
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, ctx: &mut impl RenderContext) {
        for layer in &self.layers {
            ctx.render_rect_component(frame, layer.id, layer.area);
        }
    }
}

pub(crate) struct RectComponent {
    id: ComponentId,
    area: Rect,
}

impl RectComponent {
    pub(crate) fn new(id: ComponentId, area: Rect) -> Self {
        Self { id, area }
    }
}
