mod annotation;
mod app;
mod controls;
mod editor;
mod event_reader;
mod integration;
mod keymap;
pub use integration::refresh_integrated_review;
mod live_diff;
mod model;
mod render;
mod review;
mod run;
mod runtime;
mod search;
mod selector;
mod session;
mod static_pager;
mod syntax;
mod terminal_input;
#[cfg(test)]
mod tests;
mod text_input;
mod theme;
mod toast;

pub use run::{
    DiffRunOptions, benchmark_diff_view, run_annotation_review, run_diff_with_options,
    run_integrated_review, run_pager_diff_with_options,
};
pub use static_pager::{
    StaticPagerLayout, StaticPagerOptions, render_static_changeset,
    render_static_changeset_to_writer, render_static_pager,
};
pub use theme::{
    AllocationBenchmarkStage, DecorationPreference, DiffBenchmarkOptions, DiffBenchmarkReport,
    SyntaxBenchmarkReport, SyntaxLatencyBucket,
};
