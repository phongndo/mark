pub(crate) mod annotations;
pub(crate) mod compositor;
pub(crate) mod diff;
pub(crate) mod grep;
pub(crate) mod headers;
pub(crate) mod menus;
pub(crate) mod screen_layout;
pub(crate) mod selector_menu;
pub(crate) mod sidebar;
pub(crate) mod snapshot;
pub(crate) mod statusline;
pub(crate) mod style;
pub(crate) mod text;
pub(crate) mod toast;
pub(crate) mod viewport_plan;

use crate::app::DiffApp;
use ratatui::{Frame, layout::Rect};

use self::{
    compositor::{ComponentId, Compositor, RectComponent},
    diff::draw_diff,
    menus::{
        branch_menu_area, branch_menu_list_visible_rows, color_scheme_picker_area,
        commit_menu_area, commit_menu_list_visible_rows, diff_menu_area, draw_branch_menu,
        draw_color_scheme_picker, draw_commit_menu, draw_diff_menu, draw_help_menu,
        draw_options_menu, draw_review_input, help_menu_list_visible_rows, options_menu_area,
        options_menu_block, review_input_area,
    },
    screen_layout::ScreenLayout,
    selector_menu::selector_menu_list_rows,
    sidebar::draw_file_sidebar,
    snapshot::{HitMap, OverlayLayer, RenderPlan, RenderSnapshot, RenderStatePlan},
    statusline::{draw_error_log, draw_filter_bar, draw_header},
    toast::draw_toasts,
};

type LayerRenderer = fn(&mut Frame<'_>, &mut DiffApp, Rect);

fn overlay_component_id(layer: OverlayLayer) -> ComponentId {
    match layer {
        OverlayLayer::DiffMenu => ComponentId::DiffMenu,
        OverlayLayer::ReviewInput => ComponentId::ReviewInput,
        OverlayLayer::OptionsMenu => ComponentId::OptionsMenu,
        OverlayLayer::ColorSchemePicker => ComponentId::ColorSchemePicker,
        OverlayLayer::BranchMenu => ComponentId::BranchMenu,
        OverlayLayer::CommitMenu => ComponentId::CommitMenu,
        OverlayLayer::HelpMenu => ComponentId::HelpMenu,
    }
}

fn overlay_renderer(layer: OverlayLayer) -> LayerRenderer {
    match layer {
        OverlayLayer::DiffMenu => draw_diff_menu,
        OverlayLayer::ReviewInput => draw_review_input,
        OverlayLayer::OptionsMenu => draw_options_menu,
        OverlayLayer::ColorSchemePicker => draw_color_scheme_picker,
        OverlayLayer::BranchMenu => draw_branch_menu,
        OverlayLayer::CommitMenu => draw_commit_menu,
        OverlayLayer::HelpMenu => draw_help_menu,
    }
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut DiffApp) {
    let area = frame.area();
    if area.height == 0 {
        return;
    }

    let snapshot = RenderSnapshot::from_app(app);
    let layout = ScreenLayout::build(app, &snapshot, area);
    let layout_snapshot = layout.snapshot();
    let render_plan = RenderPlan {
        layout: layout_snapshot,
        hit_map: build_hit_map(app, &layout, area),
        state: build_render_state_plan(app, &layout, area),
    };
    app.apply_render_plan(render_plan);

    let mut compositor = Compositor::new();
    compositor.push(RectComponent::new(
        ComponentId::Header,
        layout.header,
        draw_header_component,
    ));
    if let Some(sidebar_area) = layout.sidebar {
        compositor.push(RectComponent::new(
            ComponentId::FileSidebar,
            sidebar_area,
            draw_file_sidebar,
        ));
    }
    compositor.push(RectComponent::new(
        ComponentId::DiffView,
        layout.diff,
        draw_diff,
    ));
    if let Some(filter_bar_area) = layout.filter_bar {
        compositor.push(RectComponent::new(
            ComponentId::FilterBar,
            filter_bar_area,
            draw_filter_bar_component,
        ));
    }
    if let Some(error_log_area) = layout.error_log {
        compositor.push(RectComponent::new(
            ComponentId::ErrorLogPanel,
            error_log_area,
            draw_error_log_component,
        ));
    }
    compositor.push(RectComponent::new(
        ComponentId::Toasts,
        render_plan.layout.body,
        draw_toasts_component,
    ));
    for layer in snapshot.overlay_layers {
        compositor.push(RectComponent::new(
            overlay_component_id(layer),
            render_plan.layout.root,
            overlay_renderer(layer),
        ));
    }
    compositor.render(frame, app);
}

fn build_render_state_plan(app: &DiffApp, layout: &ScreenLayout, root: Rect) -> RenderStatePlan {
    let option_items = app.filtered_options_menu_items();
    let options_menu_visible_rows = options_menu_area(app, root, &option_items).map(|area| {
        let inner = options_menu_block(app.config.theme).inner(area);
        selector_menu_list_rows(inner.height, 0)
    });

    RenderStatePlan {
        terminal_area: root,
        file_sidebar_render_width: layout.sidebar_width,
        viewport_rows: layout.diff.height as usize,
        viewport_width: layout.diff.width as usize,
        options_menu_visible_rows,
        branch_menu_visible_rows: branch_menu_list_visible_rows(app, root),
        commit_menu_visible_rows: commit_menu_list_visible_rows(app, root),
        help_menu_visible_rows: help_menu_list_visible_rows(app, root),
    }
}

fn build_hit_map(app: &DiffApp, layout: &ScreenLayout, root: Rect) -> HitMap {
    let diff_choices = app.filtered_diff_choices();
    let option_items = app.filtered_options_menu_items();
    HitMap {
        diff_area: Some(layout.diff),
        diff_menu_area: diff_menu_area(app, root, &diff_choices),
        branch_menu_area: branch_menu_area(app, root),
        commit_menu_area: commit_menu_area(app, root),
        options_menu_area: options_menu_area(app, root, &option_items),
        review_input_area: review_input_area(app, root),
        color_scheme_picker_area: color_scheme_picker_area(app, root),
        error_log_separator_row: layout.error_log.map(|area| area.y),
    }
}

fn draw_header_component(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    draw_header(frame, app, area);
}

fn draw_filter_bar_component(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    draw_filter_bar(frame, app, area);
}

fn draw_error_log_component(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    draw_error_log(frame, app, area);
}

fn draw_toasts_component(frame: &mut Frame<'_>, app: &mut DiffApp, area: Rect) {
    draw_toasts(frame, app, area);
}
