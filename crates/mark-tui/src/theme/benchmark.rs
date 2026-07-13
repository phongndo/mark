#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffBenchmarkOptions {
    pub width: usize,
    pub viewport_rows: usize,
    pub scroll_step: usize,
    pub max_scroll_steps: usize,
}

impl Default for DiffBenchmarkOptions {
    fn default() -> Self {
        Self {
            width: 160,
            viewport_rows: 40,
            scroll_step: 20,
            max_scroll_steps: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBenchmarkReport {
    pub syntax_enabled: bool,
    pub row_count: usize,
    pub file_count: usize,
    pub hunk_count: usize,
    pub changeset_estimated_memory_bytes: usize,
    pub ui_model_estimated_memory_bytes: usize,
    pub search_index_estimated_memory_bytes: usize,
    pub inline_cache_entries: usize,
    pub diff_cache_entries: usize,
    pub syntax_cache_estimated_memory_bytes: usize,
    pub scope_stack_count: usize,
    pub scope_table_bytes: usize,
    pub open_micros: u128,
    pub file_filter_micros: u128,
    pub legacy_file_filter_micros: u128,
    pub grep_filter_micros: u128,
    pub legacy_grep_filter_micros: u128,
    pub file_filter_apply_micros: u128,
    pub grep_filter_apply_micros: u128,
    pub hunk_navigation_steps: usize,
    pub hunk_navigation_total_micros: u128,
    pub hunk_navigation_max_micros: u128,
    pub initial_render_micros: u128,
    pub cold_scroll_steps: usize,
    pub cold_scroll_total_micros: u128,
    pub cold_scroll_max_micros: u128,
    pub syntax_settle_micros: Option<u128>,
    pub warm_scroll_steps: usize,
    pub warm_scroll_total_micros: u128,
    pub warm_scroll_max_micros: u128,
    pub random_scroll_steps: usize,
    pub random_scroll_total_micros: u128,
    pub random_scroll_max_micros: u128,
    pub warm_cache_hits: u64,
    pub warm_cache_misses: u64,
    pub warm_theme_cache_hits: u64,
    pub warm_theme_cache_misses: u64,
    pub channel_send_timeouts: u64,
    pub syntax: SyntaxBenchmarkReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxBenchmarkReport {
    pub queue_requests: u64,
    pub jobs_queued: u64,
    pub hunk_jobs_queued: u64,
    pub full_file_jobs_queued: u64,
    pub jobs_completed: u64,
    pub jobs_failed: u64,
    pub jobs_skipped: u64,
    pub skips_invalid_position: u64,
    pub skips_no_path: u64,
    pub skips_no_language: u64,
    pub skips_no_source: u64,
    pub skips_too_large: u64,
    pub skips_queue_closed: u64,
    pub skips_highlight_error: u64,
    pub jobs_rejected: u64,
    pub jobs_evicted: u64,
    pub stale_results: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_entries_peak: usize,
    pub queue_depth_peak: usize,
    pub source_bytes_queued: u64,
    pub source_lines_queued: u64,
    pub estimated_memory_peak_bytes: u64,
    pub first_visible_latency_micros: Option<u128>,
    pub latency_buckets: Vec<SyntaxLatencyBucket>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaxLatencyBucket {
    pub language: String,
    pub source_kind: String,
    pub jobs: u64,
    pub queue_latency_total_micros: u128,
    pub queue_latency_max_micros: u128,
    pub run_latency_total_micros: u128,
    pub run_latency_max_micros: u128,
}
