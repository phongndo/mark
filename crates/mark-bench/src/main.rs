use std::{
    collections::{BTreeSet, HashSet},
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

type BenchResult<T> = Result<T, BenchError>;

#[derive(Debug)]
enum BenchError {
    Io(io::Error),
    Json(serde_json::Error),
    Mark(String),
    Git { command: String, stderr: String },
    Usage(String),
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Mark(error) => write!(formatter, "{error}"),
            Self::Git { command, stderr } => {
                if stderr.trim().is_empty() {
                    write!(formatter, "git command failed: {command}")
                } else {
                    write!(
                        formatter,
                        "git command failed: {command}: {}",
                        stderr.trim()
                    )
                }
            }
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for BenchError {}

impl From<io::Error> for BenchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BenchError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Parser)]
#[command(name = "mark-bench", about = "mark local benchmark utilities")]
struct Cli {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    #[command(about = "Generate deterministic diff benchmark fixtures")]
    Fixtures(FixturesArgs),
    #[command(about = "Measure patch loading, TUI rendering, and syntax highlighting")]
    Measure(MeasureArgs),
    #[command(about = "Measure a real Git repository diff")]
    MeasureRepo(MeasureRepoArgs),
    #[command(about = "Measure an existing patch file")]
    MeasurePatch(MeasurePatchArgs),
    #[command(about = "Compare raw Mark syntax highlighting with Shiki on identical files")]
    SyntaxCompare(SyntaxCompareArgs),
    #[command(about = "Compare full editor reload with path-scoped editor reload")]
    EditorReload(EditorReloadArgs),
}

#[derive(Debug, Parser)]
struct FixturesArgs {
    /// Output directory for generated fixture directories.
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    /// Scenario to generate. May be repeated. Defaults to the standard suite.
    #[arg(long, value_enum, value_name = "NAME")]
    scenario: Vec<ScenarioKind>,
    /// Generate the standard suite. This is also the default when no scenario is passed.
    #[arg(long)]
    all: bool,
    /// Include the larger stress scenario with --all or the default suite.
    #[arg(long)]
    stress: bool,
    /// Include syntax-oriented Rust fixture scenarios.
    #[arg(long)]
    syntax: bool,
    /// Remove an existing scenario output directory before writing it.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Parser)]
struct MeasureArgs {
    /// Directory containing generated benchmark fixture directories.
    #[arg(long, value_name = "DIR")]
    fixtures: PathBuf,
    /// Scenario to measure. May be repeated. Defaults to the standard suite.
    #[arg(long, value_enum, value_name = "NAME")]
    scenario: Vec<ScenarioKind>,
    /// Measure all standard scenarios. This is also the default when no scenario is passed.
    #[arg(long)]
    all: bool,
    /// Include the larger stress scenario with --all or the default suite.
    #[arg(long)]
    stress: bool,
    /// Include syntax-oriented Rust scenarios with --all or the default suite.
    #[arg(long)]
    syntax: bool,
    /// Language to enable for the syntax run. Repeat to enable several languages.
    #[arg(long = "syntax-language", value_name = "LANG")]
    syntax_languages: Vec<String>,
    /// Terminal width used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 160)]
    width: usize,
    /// Visible rows used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 40)]
    viewport_rows: usize,
    /// Row delta between measured scroll positions.
    #[arg(long, default_value_t = 20)]
    scroll_step: usize,
    /// Maximum measured scroll positions per scenario and mode.
    #[arg(long, default_value_t = 200)]
    max_scroll_steps: usize,
    /// Emit JSON instead of a human table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct MeasureRepoArgs {
    /// Repository whose worktree diff should be measured.
    #[arg(long, value_name = "DIR")]
    repo: PathBuf,
    /// Exclude untracked files from the measured diff.
    #[arg(long)]
    no_untracked: bool,
    /// Language to enable for the syntax run. Repeat to enable several languages.
    #[arg(long = "syntax-language", value_name = "LANG")]
    syntax_languages: Vec<String>,
    /// Terminal width used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 160)]
    width: usize,
    /// Visible rows used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 40)]
    viewport_rows: usize,
    /// Row delta between measured scroll positions.
    #[arg(long, default_value_t = 20)]
    scroll_step: usize,
    /// Maximum measured scroll positions per mode.
    #[arg(long, default_value_t = 200)]
    max_scroll_steps: usize,
    /// Emit JSON instead of a human table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct MeasurePatchArgs {
    /// Unified diff patch file to measure.
    #[arg(value_name = "PATCH")]
    patch: PathBuf,
    /// Language to enable for the syntax run. Repeat to enable several languages.
    #[arg(long = "syntax-language", value_name = "LANG")]
    syntax_languages: Vec<String>,
    /// Terminal width used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 160)]
    width: usize,
    /// Visible rows used by the synthetic TUI renderer.
    #[arg(long, default_value_t = 40)]
    viewport_rows: usize,
    /// Row delta between measured scroll positions.
    #[arg(long, default_value_t = 20)]
    scroll_step: usize,
    /// Maximum measured scroll positions per mode.
    #[arg(long, default_value_t = 200)]
    max_scroll_steps: usize,
    /// Emit JSON instead of a human table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct EditorReloadArgs {
    /// Directory containing generated benchmark fixture directories.
    #[arg(long, value_name = "DIR")]
    fixtures: PathBuf,
    /// Scenario to measure.
    #[arg(long, value_enum, value_name = "NAME")]
    scenario: ScenarioKind,
    /// Repo-relative path to reload. Defaults to the first changed file.
    #[arg(long, value_name = "PATH")]
    path: Option<PathBuf>,
    /// Number of measured iterations for each reload strategy.
    #[arg(long, default_value_t = 5)]
    iterations: usize,
    /// Emit JSON instead of a human line.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct SyntaxCompareArgs {
    /// Repository whose changed files should be highlighted. Uses `git diff HEAD`.
    #[arg(long, value_name = "DIR")]
    repo: Option<PathBuf>,
    /// Explicit file to highlight. May be repeated.
    #[arg(long = "file", value_name = "PATH")]
    files: Vec<PathBuf>,
    /// Restrict to languages. Values use Mark canonical language names or aliases.
    #[arg(long = "language", value_name = "LANG")]
    languages: Vec<String>,
    /// Maximum files to include after filtering.
    #[arg(long, default_value_t = 512)]
    max_files: usize,
    /// Maximum total source bytes to include after filtering.
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_bytes: usize,
    /// Number of measured highlight passes.
    #[arg(long, default_value_t = 1)]
    iterations: usize,
    /// Skip the separate untimed diagnostic-counter pass.
    #[arg(long)]
    skip_counters: bool,
    /// Also run Shiki via local target npm install.
    #[arg(long)]
    shiki: bool,
    /// Shiki package version used when --shiki is set.
    #[arg(long, default_value = "4.3.0")]
    shiki_version: String,
    /// Directory used for the local Shiki package and generated scripts.
    #[arg(long, default_value = "target/shiki-syntax-compare")]
    shiki_dir: PathBuf,
    /// Emit JSON instead of a human summary.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum ScenarioKind {
    ManySmallFiles,
    BalancedChangeset,
    LargeSingleFile,
    ManyUntrackedSmall,
    FewUntrackedLarge,
    MinifiedOneLine,
    BinaryFiles,
    HugeMixedStress,
    SyntaxManySmallRust,
    SyntaxLargeRust,
    SyntaxMinifiedRust,
}

impl ScenarioKind {
    fn name(self) -> &'static str {
        match self {
            Self::ManySmallFiles => "many-small-files",
            Self::BalancedChangeset => "balanced-changeset",
            Self::LargeSingleFile => "large-single-file",
            Self::ManyUntrackedSmall => "many-untracked-small",
            Self::FewUntrackedLarge => "few-untracked-large",
            Self::MinifiedOneLine => "minified-one-line",
            Self::BinaryFiles => "binary-files",
            Self::HugeMixedStress => "huge-mixed-stress",
            Self::SyntaxManySmallRust => "syntax-many-small-rust",
            Self::SyntaxLargeRust => "syntax-large-rust",
            Self::SyntaxMinifiedRust => "syntax-minified-rust",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ManySmallFiles => "Many small tracked files with localized edits.",
            Self::BalancedChangeset => "Medium file count with larger per-file edits.",
            Self::LargeSingleFile => "One large tracked file with a large changed region.",
            Self::ManyUntrackedSmall => "Tracked edits plus many small untracked files.",
            Self::FewUntrackedLarge => "Tracked edits plus a few large untracked files.",
            Self::MinifiedOneLine => "A pathological single-line minified file edit.",
            Self::BinaryFiles => "Binary modified and untracked files plus a small text edit.",
            Self::HugeMixedStress => "Large opt-in stress case for max-size and memory testing.",
            Self::SyntaxManySmallRust => "Rust many-small-file diff for syntax-enabled runs.",
            Self::SyntaxLargeRust => "Rust large-single-file diff for syntax-enabled runs.",
            Self::SyntaxMinifiedRust => {
                "Generated one-line Rust file that should hit fallback caps."
            }
        }
    }

    fn standard() -> &'static [Self] {
        &[
            Self::ManySmallFiles,
            Self::BalancedChangeset,
            Self::LargeSingleFile,
            Self::ManyUntrackedSmall,
            Self::FewUntrackedLarge,
            Self::MinifiedOneLine,
            Self::BinaryFiles,
        ]
    }

    fn syntax_suite() -> &'static [Self] {
        &[
            Self::SyntaxManySmallRust,
            Self::SyntaxLargeRust,
            Self::SyntaxMinifiedRust,
        ]
    }
}

trait ScenarioSelection {
    fn scenarios(&self) -> &[ScenarioKind];
    fn all(&self) -> bool;
    fn stress(&self) -> bool;
    fn syntax(&self) -> bool;
}

impl ScenarioSelection for FixturesArgs {
    fn scenarios(&self) -> &[ScenarioKind] {
        &self.scenario
    }

    fn all(&self) -> bool {
        self.all
    }

    fn stress(&self) -> bool {
        self.stress
    }

    fn syntax(&self) -> bool {
        self.syntax
    }
}

impl ScenarioSelection for MeasureArgs {
    fn scenarios(&self) -> &[ScenarioKind] {
        &self.scenario
    }

    fn all(&self) -> bool {
        self.all
    }

    fn stress(&self) -> bool {
        self.stress
    }

    fn syntax(&self) -> bool {
        self.syntax
    }
}

#[derive(Debug, Clone, Copy)]
struct TextShape {
    file_count: usize,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
    extension: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct UntrackedShape {
    file_count: usize,
    lines: usize,
    extension: &'static str,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FixtureCounts {
    tracked_files: usize,
    untracked_files: usize,
    binary_files: usize,
    expected_text_additions: usize,
    expected_text_deletions: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct FixturePaths {
    repo: String,
    patch: String,
    head_patch: String,
    pair_before: String,
    pair_after: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct FixtureManifest {
    version: u8,
    scenario: String,
    description: String,
    paths: FixturePaths,
    counts: FixtureCounts,
    patch_bytes: u64,
    head_patch_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum SourceVariant {
    Baseline,
    ChangedA,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        BenchCommand::Fixtures(args) => generate_fixtures(args)?,
        BenchCommand::Measure(args) => measure_fixtures(args)?,
        BenchCommand::MeasureRepo(args) => measure_repo(args)?,
        BenchCommand::MeasurePatch(args) => measure_patch(args)?,
        BenchCommand::SyntaxCompare(args) => syntax_compare(args)?,
        BenchCommand::EditorReload(args) => measure_editor_reload(args)?,
    }
    Ok(())
}

fn generate_fixtures(args: FixturesArgs) -> BenchResult<()> {
    let scenarios = select_scenarios(&args);
    fs::create_dir_all(&args.out)?;

    for scenario in scenarios {
        let manifest = generate_scenario(&args.out, scenario, args.force)?;
        println!(
            "generated {}: {} files, {} untracked, {} bytes patch",
            manifest.scenario,
            manifest.counts.tracked_files,
            manifest.counts.untracked_files,
            manifest.patch_bytes
        );
    }

    Ok(())
}

fn select_scenarios(selection: &impl ScenarioSelection) -> Vec<ScenarioKind> {
    let mut selected = Vec::new();
    if selection.all() || selection.scenarios().is_empty() {
        selected.extend_from_slice(ScenarioKind::standard());
    }
    selected.extend(selection.scenarios().iter().copied());

    if selection.stress() && !selected.contains(&ScenarioKind::HugeMixedStress) {
        selected.push(ScenarioKind::HugeMixedStress);
    }
    if selection.syntax() {
        selected.extend_from_slice(ScenarioKind::syntax_suite());
    }

    let mut seen = HashSet::new();
    selected.retain(|scenario| seen.insert(*scenario));
    selected
}

#[derive(Debug, Serialize)]
struct MeasureSuiteReport {
    version: u8,
    fixture_root: String,
    options: MeasureOptionsReport,
    runs: Vec<MeasureRunReport>,
}

#[derive(Debug, Serialize)]
struct MeasureOptionsReport {
    width: usize,
    viewport_rows: usize,
    scroll_step: usize,
    max_scroll_steps: usize,
    syntax_languages: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MeasureRunReport {
    scenario: String,
    mode: String,
    patch_bytes: u64,
    load_micros: u128,
    rss_before_bytes: Option<u64>,
    rss_after_bytes: Option<u64>,
    rss_delta_bytes: Option<i128>,
    tui: TuiMeasureReport,
}

#[derive(Debug, Serialize)]
struct TuiMeasureReport {
    syntax_enabled: bool,
    row_count: usize,
    file_count: usize,
    hunk_count: usize,
    open_micros: u128,
    file_filter_micros: u128,
    legacy_file_filter_micros: u128,
    grep_filter_micros: u128,
    legacy_grep_filter_micros: u128,
    file_filter_apply_micros: u128,
    grep_filter_apply_micros: u128,
    hunk_navigation_steps: usize,
    hunk_navigation_total_micros: u128,
    hunk_navigation_max_micros: u128,
    initial_render_micros: u128,
    cold_scroll_steps: usize,
    cold_scroll_total_micros: u128,
    cold_scroll_max_micros: u128,
    cold_scroll_avg_micros: u128,
    syntax_settle_micros: Option<u128>,
    warm_scroll_steps: usize,
    warm_scroll_total_micros: u128,
    warm_scroll_max_micros: u128,
    warm_scroll_avg_micros: u128,
    warm_cache_hits: u64,
    warm_cache_misses: u64,
    warm_cache_hit_rate: Option<f64>,
    syntax: SyntaxMeasureReport,
}

#[derive(Debug, Serialize)]
struct SyntaxMeasureReport {
    queue_requests: u64,
    jobs_queued: u64,
    jobs_completed: u64,
    jobs_failed: u64,
    jobs_skipped: u64,
    jobs_rejected: u64,
    jobs_evicted: u64,
    stale_results: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_entries_peak: usize,
    queue_depth_peak: usize,
    source_bytes_queued: u64,
    source_lines_queued: u64,
    estimated_memory_peak_bytes: u64,
}

#[derive(Debug, Serialize)]
struct EditorReloadReport {
    scenario: &'static str,
    path: String,
    iterations: usize,
    full_avg_micros: u128,
    scoped_avg_micros: u128,
    speedup: Option<f64>,
}

#[derive(Debug, Serialize)]
struct SyntaxCompareReport {
    version: u8,
    source: SyntaxCompareSourceReport,
    input: SyntaxCompareInputReport,
    mark: RawSyntaxEngineReport,
    shiki: Option<RawSyntaxEngineReport>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SyntaxCompareSourceReport {
    repo: Option<String>,
    explicit_files: usize,
    languages: Vec<String>,
    max_files: usize,
    max_bytes: usize,
    iterations: usize,
}

#[derive(Debug, Serialize)]
struct SyntaxCompareInputReport {
    files: usize,
    bytes: u64,
    lines: u64,
    languages: Vec<SyntaxCompareLanguageReport>,
}

#[derive(Debug, Serialize)]
struct SyntaxCompareLanguageReport {
    language: String,
    files: usize,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SyntaxCompareInput {
    path: PathBuf,
    display_path: String,
    language: String,
    shiki_language: String,
    bytes: u64,
    lines: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawSyntaxEngineReport {
    engine: String,
    iterations: usize,
    files: usize,
    bytes: u64,
    lines: u64,
    tokens: u64,
    setup_micros: u128,
    highlight_micros: u128,
    bytes_per_second: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    counters: Option<mark_syntax::engine::counters::EngineCounters>,
}

trait DiffBenchmarkSelection {
    fn syntax_languages(&self) -> &[String];
    fn width(&self) -> usize;
    fn viewport_rows(&self) -> usize;
    fn scroll_step(&self) -> usize;
    fn max_scroll_steps(&self) -> usize;
    fn json(&self) -> bool;
}

impl DiffBenchmarkSelection for MeasureRepoArgs {
    fn syntax_languages(&self) -> &[String] {
        &self.syntax_languages
    }

    fn width(&self) -> usize {
        self.width
    }

    fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    fn scroll_step(&self) -> usize {
        self.scroll_step
    }

    fn max_scroll_steps(&self) -> usize {
        self.max_scroll_steps
    }

    fn json(&self) -> bool {
        self.json
    }
}

impl DiffBenchmarkSelection for MeasurePatchArgs {
    fn syntax_languages(&self) -> &[String] {
        &self.syntax_languages
    }

    fn width(&self) -> usize {
        self.width
    }

    fn viewport_rows(&self) -> usize {
        self.viewport_rows
    }

    fn scroll_step(&self) -> usize {
        self.scroll_step
    }

    fn max_scroll_steps(&self) -> usize {
        self.max_scroll_steps
    }

    fn json(&self) -> bool {
        self.json
    }
}

fn measure_fixtures(args: MeasureArgs) -> BenchResult<()> {
    let scenarios = select_scenarios(&args);
    let syntax_languages = if args.syntax_languages.is_empty() && args.syntax {
        vec!["rust".to_owned()]
    } else {
        args.syntax_languages.clone()
    };
    let options = mark_tui::DiffBenchmarkOptions {
        width: args.width,
        viewport_rows: args.viewport_rows,
        scroll_step: args.scroll_step,
        max_scroll_steps: args.max_scroll_steps,
    };
    let mut runs = Vec::new();

    for scenario in scenarios {
        let scenario_dir = args.fixtures.join(scenario.name());
        let manifest = load_manifest(&scenario_dir)?;
        runs.push(measure_fixture_run(
            scenario,
            "plain",
            &scenario_dir,
            &manifest,
            None,
            options,
        )?);
        if !syntax_languages.is_empty() {
            runs.push(measure_fixture_run(
                scenario,
                "syntax",
                &scenario_dir,
                &manifest,
                Some(syntax_languages.clone()),
                options,
            )?);
        }
    }

    let report = MeasureSuiteReport {
        version: 1,
        fixture_root: args.fixtures.display().to_string(),
        options: MeasureOptionsReport {
            width: options.width,
            viewport_rows: options.viewport_rows,
            scroll_step: options.scroll_step,
            max_scroll_steps: options.max_scroll_steps,
            syntax_languages,
        },
        runs,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_measure_report(&report);
    }

    Ok(())
}

fn measure_editor_reload(args: EditorReloadArgs) -> BenchResult<()> {
    if args.iterations == 0 {
        return Err(BenchError::Usage(
            "--iterations must be greater than zero".to_owned(),
        ));
    }

    let scenario_dir = args.fixtures.join(args.scenario.name());
    let manifest = load_manifest(&scenario_dir)?;
    let repo = scenario_dir.join(&manifest.paths.repo);
    let options = mark_diff::DiffOptions {
        repo: Some(repo.into()),
        ..mark_diff::DiffOptions::default()
    };
    let path = match args.path {
        Some(path) => path,
        None => mark_diff::load_review_ref(&options)
            .map_err(|error| BenchError::Mark(error.to_string()))?
            .files
            .first()
            .map(|file| PathBuf::from(file.display_path()))
            .ok_or_else(|| BenchError::Usage("scenario has no changed files".to_owned()))?,
    };

    let mut full_total = 0u128;
    for _ in 0..args.iterations {
        let start = Instant::now();
        let _ = mark_diff::load_review_ref(&options)
            .map_err(|error| BenchError::Mark(error.to_string()))?;
        full_total = full_total.saturating_add(start.elapsed().as_micros());
    }

    let mut scoped_total = 0u128;
    for _ in 0..args.iterations {
        let start = Instant::now();
        let _ = mark_diff::load_review_ref_path(&options, &path)
            .map_err(|error| BenchError::Mark(error.to_string()))?;
        scoped_total = scoped_total.saturating_add(start.elapsed().as_micros());
    }

    let full_avg = average_micros(full_total, args.iterations);
    let scoped_avg = average_micros(scoped_total, args.iterations);
    let report = EditorReloadReport {
        scenario: args.scenario.name(),
        path: path.display().to_string(),
        iterations: args.iterations,
        full_avg_micros: full_avg,
        scoped_avg_micros: scoped_avg,
        speedup: (scoped_avg > 0).then(|| full_avg as f64 / scoped_avg as f64),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} path={} iterations={} full_avg={}µs scoped_avg={}µs speedup={}",
            report.scenario,
            report.path,
            report.iterations,
            report.full_avg_micros,
            report.scoped_avg_micros,
            report
                .speedup
                .map(|speedup| format!("{speedup:.2}x"))
                .unwrap_or_else(|| "n/a".to_owned())
        );
    }

    Ok(())
}

fn measure_repo(args: MeasureRepoArgs) -> BenchResult<()> {
    let repo = args.repo.clone();
    let diff_options = mark_diff::DiffOptions {
        repo: Some(repo.clone().into()),
        local_untracked: mark_diff::UntrackedMode::from_include(!args.no_untracked),
        ..mark_diff::DiffOptions::default()
    };
    measure_one_diff_source(
        format!("repo:{}", repo.display()),
        repo.display().to_string(),
        None,
        diff_options,
        &args,
    )
}

fn measure_patch(args: MeasurePatchArgs) -> BenchResult<()> {
    let patch = args.patch.clone();
    let patch_bytes = fs::metadata(&patch).ok().map(|metadata| metadata.len());
    let diff_options = mark_diff::DiffOptions {
        repo: None,
        source: mark_diff::DiffSource::Patch(mark_diff::PatchSource::File(patch.clone())),
        local_untracked: mark_diff::UntrackedMode::Exclude,
        output: mark_diff::DiffOutput::Patch,
    };
    measure_one_diff_source(
        format!("patch:{}", patch.display()),
        patch.display().to_string(),
        patch_bytes,
        diff_options,
        &args,
    )
}

fn syntax_compare(args: SyntaxCompareArgs) -> BenchResult<()> {
    if args.iterations == 0 {
        return Err(BenchError::Usage(
            "--iterations must be greater than zero".to_owned(),
        ));
    }
    if args.repo.is_none() && args.files.is_empty() {
        return Err(BenchError::Usage(
            "provide --repo or at least one --file".to_owned(),
        ));
    }

    let language_filter = canonical_language_filter(&args.languages)?;
    let inputs = collect_syntax_compare_inputs(&args, &language_filter)?;
    if inputs.is_empty() {
        return Err(BenchError::Usage(
            "no supported syntax inputs after filtering".to_owned(),
        ));
    }

    let input_report = syntax_compare_input_report(&inputs);
    let mark = measure_mark_syntax_raw(&inputs, args.iterations, !args.skip_counters)?;
    let shiki = if args.shiki {
        Some(measure_shiki_raw(
            &inputs,
            args.iterations,
            &args.shiki_dir,
            &args.shiki_version,
        )?)
    } else {
        None
    };
    let mut notes = vec![
        "apples-to-apples raw comparison: both engines highlight the same full source files"
            .to_owned(),
        "timings exclude source collection; Shiki npm install and highlighter construction are reported as setup, not highlight time"
            .to_owned(),
    ];
    if args.max_files < usize::MAX || args.max_bytes < usize::MAX {
        notes.push("input may be capped by --max-files/--max-bytes".to_owned());
    }
    let report = SyntaxCompareReport {
        version: 1,
        source: SyntaxCompareSourceReport {
            repo: args.repo.as_ref().map(|repo| repo.display().to_string()),
            explicit_files: args.files.len(),
            languages: language_filter.iter().cloned().collect(),
            max_files: args.max_files,
            max_bytes: args.max_bytes,
            iterations: args.iterations,
        },
        input: input_report,
        mark,
        shiki,
        notes,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_syntax_compare_report(&report);
    }

    Ok(())
}

fn canonical_language_filter(languages: &[String]) -> BenchResult<BTreeSet<String>> {
    let mut filter = BTreeSet::new();
    for language in languages {
        let canonical = mark_syntax::canonical_language(language).ok_or_else(|| {
            BenchError::Usage(format!("unsupported syntax language filter: {language}"))
        })?;
        filter.insert(canonical);
    }
    Ok(filter)
}

fn collect_syntax_compare_inputs(
    args: &SyntaxCompareArgs,
    language_filter: &BTreeSet<String>,
) -> BenchResult<Vec<SyntaxCompareInput>> {
    let mut candidates = Vec::new();
    if let Some(repo) = &args.repo {
        candidates.extend(changed_repo_files(repo)?);
    }
    candidates.extend(args.files.iter().map(|file| (None, file.clone())));

    let mut seen = HashSet::new();
    let mut inputs = Vec::new();
    let mut total_bytes = 0usize;

    for (root, path) in candidates {
        if inputs.len() >= args.max_files || total_bytes >= args.max_bytes {
            break;
        }
        let full_path = root
            .as_ref()
            .map_or_else(|| path.clone(), |root| root.join(&path));
        let dedupe_key = full_path.clone();
        if !seen.insert(dedupe_key) || !full_path.is_file() {
            continue;
        }
        let display_path = path.display().to_string();
        let Some(language) = mark_syntax::detect_language_from_path(&display_path) else {
            continue;
        };
        if !language_filter.is_empty() && !language_filter.contains(&language) {
            continue;
        }
        let shiki_language = match shiki_language_name(&language) {
            Some(shiki_language) => shiki_language.to_owned(),
            None if args.shiki => continue,
            None => language.clone(),
        };
        let metadata = fs::metadata(&full_path)?;
        let bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if bytes == 0 || total_bytes.saturating_add(bytes) > args.max_bytes {
            continue;
        }
        let source = fs::read_to_string(&full_path).map_err(|error| {
            BenchError::Usage(format!(
                "failed to read UTF-8 source {}: {error}",
                full_path.display()
            ))
        })?;
        let lines = source.lines().count().max(1) as u64;
        total_bytes = total_bytes.saturating_add(source.len());
        inputs.push(SyntaxCompareInput {
            path: full_path,
            display_path,
            language,
            shiki_language,
            bytes: source.len() as u64,
            lines,
        });
    }

    Ok(inputs)
}

fn changed_repo_files(repo: &Path) -> BenchResult<Vec<(Option<PathBuf>, PathBuf)>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-only", "--diff-filter=ACMR", "HEAD", "--"])
        .output()?;
    if !output.status.success() {
        return Err(BenchError::Git {
            command: format!("git -C {} diff --name-only HEAD", repo.display()),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| (Some(repo.to_path_buf()), PathBuf::from(line)))
        .collect())
}

fn shiki_language_name(language: &str) -> Option<&'static str> {
    match language {
        "abap" => Some("abap"),
        "agda" => Some("agda"),
        "angular-html" => Some("angular-html"),
        "angular-ts" => Some("angular-ts"),
        "apex" => Some("apex"),
        "apl" => Some("apl"),
        "ara" => Some("ara"),
        "astro" => Some("astro"),
        "ballerina" => Some("ballerina"),
        "beancount" => Some("beancount"),
        "berry" => Some("berry"),
        "bicep" => Some("bicep"),
        "bird2" => Some("bird2"),
        "bash" => Some("bash"),
        "blade" => Some("blade"),
        "bsl" => Some("bsl"),
        "asm" | "arm-assembly" | "x86-64-assembly" => Some("asm"),
        "c" => Some("c"),
        "c3" => Some("c3"),
        "cadence" => Some("cadence"),
        "cairo" => Some("cairo"),
        "chapel" => Some("chapel"),
        "clarity" => Some("clarity"),
        "codeql" => Some("codeql"),
        "codeowners" => Some("codeowners"),
        "cobol" => Some("cobol"),
        "common-lisp" => Some("common-lisp"),
        "coq" => Some("coq"),
        "cpp" => Some("cpp"),
        "csharp" => Some("csharp"),
        "css" => Some("css"),
        "crystal" => Some("crystal"),
        "clojure" => Some("clojure"),
        "cmake" => Some("cmake"),
        "cuda" => Some("cuda"),
        "cue" => Some("cue"),
        "cypher" => Some("cypher"),
        "dart" => Some("dart"),
        "dax" => Some("dax"),
        "dhall" => Some("dhall"),
        "dream-maker" => Some("dream-maker"),
        "dockerfile" => Some("dockerfile"),
        "dockerfile-with-bash" => Some("dockerfile"),
        "elixir" => Some("elixir"),
        "elm" => Some("elm"),
        "erlang" => Some("erlang"),
        "emacs-lisp" => Some("emacs-lisp"),
        "edge" => Some("edge"),
        "ejs" => Some("erb"),
        "fennel" => Some("fennel"),
        "fluent" => Some("fluent"),
        "forth" => Some("forth"),
        "gdresource" => Some("gdresource"),
        "gdshader" => Some("gdshader"),
        "genie" => Some("genie"),
        "gherkin" => Some("gherkin"),
        "gleam" => Some("gleam"),
        "glimmer-js" => Some("glimmer-js"),
        "glimmer-ts" => Some("glimmer-ts"),
        "gn" => Some("gn"),
        "glsl" => Some("glsl"),
        "go" => Some("go"),
        "graphql" => Some("graphql"),
        "haskell" => Some("haskell"),
        "hlsl" => Some("hlsl"),
        "haxe" => Some("haxe"),
        "hurl" => Some("hurl"),
        "hxml" => Some("hxml"),
        "hy" => Some("hy"),
        "hack" => Some("hack"),
        "handlebars" => Some("handlebars"),
        "html" => Some("html"),
        "html-jinja2" => Some("jinja"),
        "html-rails" => Some("erb"),
        "html-twig" => Some("twig"),
        "imba" => Some("imba"),
        "jison" => Some("jison"),
        "java" => Some("java"),
        "javascript" => Some("javascript"),
        "json" => Some("json"),
        "jssm" => Some("jssm"),
        "just" => Some("just"),
        "kdl" => Some("kdl"),
        "kotlin" => Some("kotlin"),
        "kusto" => Some("kusto"),
        "latex" => Some("latex"),
        "lean-4" => Some("lean"),
        "lisp" => Some("common-lisp"),
        "liquid" => Some("liquid"),
        "llvm" => Some("llvm"),
        "logo" => Some("logo"),
        "lua" => Some("lua"),
        "luau" => Some("luau"),
        "make" => Some("make"),
        "markdown" => Some("markdown"),
        "marko" => Some("marko"),
        "mdc" => Some("mdc"),
        "mdx" => Some("mdx"),
        "mermaid" => Some("mermaid"),
        "meson" => Some("meson"),
        "metal" => Some("metal"),
        "mipsasm" => Some("mipsasm"),
        "mojo" => Some("mojo"),
        "moonbit" => Some("moonbit"),
        "move" => Some("move"),
        "narrat" => Some("narrat"),
        "nextflow" => Some("nextflow"),
        "nextflow-groovy" => Some("nextflow-groovy"),
        "nim" => Some("nim"),
        "nix" => Some("nix"),
        "nushell" => Some("nushell"),
        "objective-c" => Some("objective-c"),
        "objective-c++" => Some("objective-cpp"),
        "odin" => Some("odin"),
        "openscad" => Some("openscad"),
        "orgmode" => Some("org"),
        "perl" => Some("perl"),
        "php" => Some("php"),
        "pkl" => Some("pkl"),
        "po" => Some("po"),
        "polar" => Some("polar"),
        "pony" => Some("pony"),
        "powerquery" => Some("powerquery"),
        "prisma" => Some("prisma"),
        "python" => Some("python"),
        "prolog" => Some("prolog"),
        "pug" => Some("pug"),
        "qmldir" => Some("qmldir"),
        "qml" => Some("qml"),
        "qss" => Some("qss"),
        "r" => Some("r"),
        "racket" => Some("racket"),
        "raku" => Some("raku"),
        "razor" => Some("razor"),
        "rel" => Some("rel"),
        "riscv" => Some("riscv"),
        "ron" => Some("ron"),
        "rosmsg" => Some("rosmsg"),
        "ruby" => Some("ruby"),
        "ruby-haml" => Some("haml"),
        "rust" => Some("rust"),
        "scala" => Some("scala"),
        "scheme" => Some("scheme"),
        "sas" => Some("sas"),
        "sdbl" => Some("sdbl"),
        "shaderlab" => Some("shaderlab"),
        "solidity" => Some("solidity"),
        "soy" => Some("soy"),
        "sparql" => Some("sparql"),
        "sql" => Some("sql"),
        "splunk" => Some("splunk"),
        "stata" => Some("stata"),
        "surrealql" => Some("surrealql"),
        "svelte" => Some("svelte"),
        "swift" => Some("swift"),
        "systemd" => Some("systemd"),
        "systemverilog" => Some("system-verilog"),
        "talonscript" => Some("talonscript"),
        "tasl" => Some("tasl"),
        "templ" => Some("templ"),
        "terraform" => Some("terraform"),
        "tex" => Some("tex"),
        "toml" => Some("toml"),
        "tsx" => Some("tsx"),
        "twig" => Some("twig"),
        "ts-tags" => Some("ts-tags"),
        "turtle" => Some("turtle"),
        "typescript" => Some("typescript"),
        "typespec" => Some("typespec"),
        "v" => Some("v"),
        "vala" => Some("vala"),
        "vue-component" => Some("vue"),
        "verilog" => Some("verilog"),
        "vhdl" => Some("vhdl"),
        "wasm" => Some("wasm"),
        "wgsl" => Some("wgsl"),
        "wenyan" => Some("wenyan"),
        "wit" => Some("wit"),
        "wolfram" => Some("wolfram"),
        "xsl" => Some("xsl"),
        "yaml" => Some("yaml"),
        "zenscript" => Some("zenscript"),
        "zig" => Some("zig"),
        _ => None,
    }
}

fn syntax_compare_input_report(inputs: &[SyntaxCompareInput]) -> SyntaxCompareInputReport {
    let mut languages = BTreeSet::<String>::new();
    for input in inputs {
        languages.insert(input.language.clone());
    }
    let language_reports = languages
        .into_iter()
        .map(|language| {
            let matching = inputs.iter().filter(|input| input.language == language);
            let mut files = 0usize;
            let mut bytes = 0u64;
            for input in matching {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(input.bytes);
            }
            SyntaxCompareLanguageReport {
                language,
                files,
                bytes,
            }
        })
        .collect::<Vec<_>>();
    SyntaxCompareInputReport {
        files: inputs.len(),
        bytes: inputs.iter().map(|input| input.bytes).sum(),
        lines: inputs.iter().map(|input| input.lines).sum(),
        languages: language_reports,
    }
}

fn measure_mark_syntax_raw(
    inputs: &[SyntaxCompareInput],
    iterations: usize,
    collect_counters: bool,
) -> BenchResult<RawSyntaxEngineReport> {
    let setup_start = Instant::now();
    let _ = mark_syntax::installed_languages();
    let mut highlighter = mark_syntax::SyntaxHighlighter::new();
    let setup_micros = setup_start.elapsed().as_micros();

    let mut sources = Vec::with_capacity(inputs.len());
    for input in inputs {
        sources.push(fs::read_to_string(&input.path)?);
    }

    let start = Instant::now();
    let mut tokens = 0u64;
    for _ in 0..iterations {
        for (input, source) in inputs.iter().zip(&sources) {
            let highlighted = highlighter
                .highlight(&input.language, source)
                .map_err(|error| BenchError::Mark(error.to_string()))?;
            tokens = tokens.saturating_add(
                highlighted
                    .lines
                    .iter()
                    .map(|line| line.segments.len() as u64)
                    .sum::<u64>(),
            );
        }
    }
    let highlight_micros = start.elapsed().as_micros();
    let bytes = inputs.iter().map(|input| input.bytes).sum::<u64>();
    let lines = inputs.iter().map(|input| input.lines).sum::<u64>();
    // Run diagnostics separately: counters must not distort timed throughput,
    // and a fresh engine avoids reporting only warm line-cache hits. Iterative
    // A/B performance experiments can skip this comparatively expensive pass.
    let counters = if collect_counters {
        let mut probe = mark_syntax::SyntaxHighlighter::new();
        probe.set_engine_counters_enabled(true);
        for (input, source) in inputs.iter().zip(&sources) {
            let _ = probe
                .highlight(&input.language, source)
                .map_err(|error| BenchError::Mark(error.to_string()))?;
        }
        Some(probe.take_engine_counters())
    } else {
        None
    };
    Ok(RawSyntaxEngineReport {
        engine: "mark-syntax".to_owned(),
        iterations,
        files: inputs.len(),
        bytes,
        lines,
        tokens,
        setup_micros,
        highlight_micros,
        bytes_per_second: bytes_per_second(bytes, iterations, highlight_micros),
        counters,
    })
}

fn measure_shiki_raw(
    inputs: &[SyntaxCompareInput],
    iterations: usize,
    shiki_dir: &Path,
    shiki_version: &str,
) -> BenchResult<RawSyntaxEngineReport> {
    fs::create_dir_all(shiki_dir)?;
    let manifest_path = shiki_dir.join("inputs.json");
    let script_path = shiki_dir.join("syntax-compare.mjs");
    fs::write(&manifest_path, serde_json::to_vec_pretty(inputs)?)?;
    fs::write(&script_path, SHIKI_COMPARE_SCRIPT)?;

    let package_spec = format!("shiki@{shiki_version}");
    let install = Command::new("npm")
        .args(["install", "--silent", "--no-audit", "--no-fund", "--prefix"])
        .arg(shiki_dir)
        .arg(package_spec)
        .output()?;
    if !install.status.success() {
        return Err(BenchError::Usage(format!(
            "npm install shiki failed: {}",
            String::from_utf8_lossy(&install.stderr).trim()
        )));
    }

    let output = Command::new("node")
        .arg(&script_path)
        .arg(&manifest_path)
        .arg(iterations.to_string())
        .output()?;
    if !output.status.success() {
        return Err(BenchError::Usage(format!(
            "shiki syntax compare failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout).map_err(BenchError::Json)
}

fn bytes_per_second(bytes: u64, iterations: usize, micros: u128) -> f64 {
    if micros == 0 {
        return f64::INFINITY;
    }
    (bytes as f64 * iterations as f64) / (micros as f64 / 1_000_000.0)
}

fn print_syntax_compare_report(report: &SyntaxCompareReport) {
    println!(
        "inputs: {} files, {}, {} lines",
        report.input.files,
        human_bytes(report.input.bytes),
        report.input.lines
    );
    print_raw_syntax_engine(&report.mark);
    if let Some(shiki) = &report.shiki {
        print_raw_syntax_engine(shiki);
        if shiki.highlight_micros > 0 && report.mark.highlight_micros > 0 {
            println!(
                "mark/shiki highlight speedup: {:.2}x",
                shiki.highlight_micros as f64 / report.mark.highlight_micros as f64
            );
        }
    }
}

fn print_raw_syntax_engine(report: &RawSyntaxEngineReport) {
    println!(
        "{:<14} setup={}µs highlight={}µs throughput={}/s tokens={}",
        report.engine,
        report.setup_micros,
        report.highlight_micros,
        human_bytes(report.bytes_per_second as u64),
        report.tokens
    );
}

const SHIKI_COMPARE_SCRIPT: &str = r#"
import fs from 'node:fs'
import { createHighlighter } from 'shiki'

const [manifestPath, iterationsText] = process.argv.slice(2)
const iterations = Number(iterationsText || '1')
const inputs = JSON.parse(fs.readFileSync(manifestPath, 'utf8'))
const langs = [...new Set(inputs.map(input => input.shiki_language))]

const setupStart = process.hrtime.bigint()
const highlighter = await createHighlighter({ themes: ['github-dark'], langs })
const setupMicros = Number((process.hrtime.bigint() - setupStart) / 1000n)
const sources = inputs.map(input => fs.readFileSync(input.path, 'utf8'))

let tokens = 0
const start = process.hrtime.bigint()
for (let iteration = 0; iteration < iterations; iteration++) {
  for (let index = 0; index < inputs.length; index++) {
    const result = highlighter.codeToTokens(sources[index], {
      lang: inputs[index].shiki_language,
      theme: 'github-dark',
    })
    for (const line of result.tokens) tokens += line.length
  }
}
const highlightMicros = Number((process.hrtime.bigint() - start) / 1000n)
const bytes = inputs.reduce((sum, input) => sum + input.bytes, 0)
const lines = inputs.reduce((sum, input) => sum + input.lines, 0)
const bytesPerSecond = highlightMicros === 0
  ? Number.POSITIVE_INFINITY
  : (bytes * iterations) / (highlightMicros / 1_000_000)

console.log(JSON.stringify({
  engine: 'shiki',
  iterations,
  files: inputs.length,
  bytes,
  lines,
  tokens,
  setup_micros: setupMicros,
  highlight_micros: highlightMicros,
  bytes_per_second: bytesPerSecond,
}))
"#;

fn measure_one_diff_source(
    scenario: String,
    fixture_root: String,
    patch_bytes_hint: Option<u64>,
    diff_options: mark_diff::DiffOptions,
    selection: &impl DiffBenchmarkSelection,
) -> BenchResult<()> {
    let options = mark_tui::DiffBenchmarkOptions {
        width: selection.width(),
        viewport_rows: selection.viewport_rows(),
        scroll_step: selection.scroll_step(),
        max_scroll_steps: selection.max_scroll_steps(),
    };
    let syntax_languages = selection.syntax_languages().to_vec();
    let mut runs = Vec::new();
    runs.push(measure_diff_options_run(
        scenario.clone(),
        "plain",
        &diff_options,
        None,
        patch_bytes_hint,
        options,
    )?);
    if !syntax_languages.is_empty() {
        runs.push(measure_diff_options_run(
            scenario,
            "syntax",
            &diff_options,
            Some(syntax_languages.clone()),
            patch_bytes_hint,
            options,
        )?);
    }

    let report = MeasureSuiteReport {
        version: 1,
        fixture_root,
        options: MeasureOptionsReport {
            width: options.width,
            viewport_rows: options.viewport_rows,
            scroll_step: options.scroll_step,
            max_scroll_steps: options.max_scroll_steps,
            syntax_languages,
        },
        runs,
    };

    if selection.json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_measure_report(&report);
    }
    Ok(())
}

fn load_manifest(scenario_dir: &Path) -> BenchResult<FixtureManifest> {
    let bytes = fs::read(scenario_dir.join("manifest.json"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn measure_fixture_run(
    scenario: ScenarioKind,
    mode: &'static str,
    scenario_dir: &Path,
    manifest: &FixtureManifest,
    syntax_languages: Option<Vec<String>>,
    options: mark_tui::DiffBenchmarkOptions,
) -> BenchResult<MeasureRunReport> {
    let patch = scenario_dir.join(&manifest.paths.patch);
    let diff_options = mark_diff::DiffOptions {
        repo: None,
        source: mark_diff::DiffSource::Patch(mark_diff::PatchSource::File(patch)),
        local_untracked: mark_diff::UntrackedMode::Exclude,
        output: mark_diff::DiffOutput::Patch,
    };
    measure_diff_options_run(
        scenario.name().to_owned(),
        mode,
        &diff_options,
        syntax_languages,
        Some(manifest.patch_bytes),
        options,
    )
}

fn measure_diff_options_run(
    scenario: String,
    mode: &str,
    diff_options: &mark_diff::DiffOptions,
    syntax_languages: Option<Vec<String>>,
    patch_bytes_hint: Option<u64>,
    options: mark_tui::DiffBenchmarkOptions,
) -> BenchResult<MeasureRunReport> {
    let load_start = Instant::now();
    let (changeset, patch_bytes) = load_benchmark_changeset(diff_options, patch_bytes_hint)?;
    let load_micros = load_start.elapsed().as_micros();

    let rss_before = current_rss_bytes();
    let tui = mark_tui::benchmark_diff_view(changeset, syntax_languages, options);
    let rss_after = current_rss_bytes();

    Ok(MeasureRunReport {
        scenario,
        mode: mode.to_owned(),
        patch_bytes,
        load_micros,
        rss_before_bytes: rss_before,
        rss_after_bytes: rss_after,
        rss_delta_bytes: rss_before
            .zip(rss_after)
            .map(|(before, after)| after as i128 - before as i128),
        tui: tui_report(tui),
    })
}

fn load_benchmark_changeset(
    diff_options: &mark_diff::DiffOptions,
    patch_bytes_hint: Option<u64>,
) -> BenchResult<(mark_diff::Changeset, u64)> {
    match patch_bytes_hint {
        Some(patch_bytes) => {
            let changeset = mark_diff::load_review_ref(diff_options)
                .map_err(|error| BenchError::Mark(error.to_string()))?;
            Ok((changeset, patch_bytes))
        }
        None => mark_diff::load_review_ref_with_patch_bytes(diff_options)
            .map_err(|error| BenchError::Mark(error.to_string())),
    }
}

fn tui_report(report: mark_tui::DiffBenchmarkReport) -> TuiMeasureReport {
    let warm_cache_total = report
        .warm_cache_hits
        .saturating_add(report.warm_cache_misses);
    TuiMeasureReport {
        syntax_enabled: report.syntax_enabled,
        row_count: report.row_count,
        file_count: report.file_count,
        hunk_count: report.hunk_count,
        open_micros: report.open_micros,
        file_filter_micros: report.file_filter_micros,
        legacy_file_filter_micros: report.legacy_file_filter_micros,
        grep_filter_micros: report.grep_filter_micros,
        legacy_grep_filter_micros: report.legacy_grep_filter_micros,
        file_filter_apply_micros: report.file_filter_apply_micros,
        grep_filter_apply_micros: report.grep_filter_apply_micros,
        hunk_navigation_steps: report.hunk_navigation_steps,
        hunk_navigation_total_micros: report.hunk_navigation_total_micros,
        hunk_navigation_max_micros: report.hunk_navigation_max_micros,
        initial_render_micros: report.initial_render_micros,
        cold_scroll_steps: report.cold_scroll_steps,
        cold_scroll_total_micros: report.cold_scroll_total_micros,
        cold_scroll_max_micros: report.cold_scroll_max_micros,
        cold_scroll_avg_micros: average_micros(
            report.cold_scroll_total_micros,
            report.cold_scroll_steps,
        ),
        syntax_settle_micros: report.syntax_settle_micros,
        warm_scroll_steps: report.warm_scroll_steps,
        warm_scroll_total_micros: report.warm_scroll_total_micros,
        warm_scroll_max_micros: report.warm_scroll_max_micros,
        warm_scroll_avg_micros: average_micros(
            report.warm_scroll_total_micros,
            report.warm_scroll_steps,
        ),
        warm_cache_hits: report.warm_cache_hits,
        warm_cache_misses: report.warm_cache_misses,
        warm_cache_hit_rate: (warm_cache_total > 0)
            .then(|| report.warm_cache_hits as f64 / warm_cache_total as f64),
        syntax: syntax_report(report.syntax),
    }
}

fn syntax_report(report: mark_tui::SyntaxBenchmarkReport) -> SyntaxMeasureReport {
    SyntaxMeasureReport {
        queue_requests: report.queue_requests,
        jobs_queued: report.jobs_queued,
        jobs_completed: report.jobs_completed,
        jobs_failed: report.jobs_failed,
        jobs_skipped: report.jobs_skipped,
        jobs_rejected: report.jobs_rejected,
        jobs_evicted: report.jobs_evicted,
        stale_results: report.stale_results,
        cache_hits: report.cache_hits,
        cache_misses: report.cache_misses,
        cache_entries_peak: report.cache_entries_peak,
        queue_depth_peak: report.queue_depth_peak,
        source_bytes_queued: report.source_bytes_queued,
        source_lines_queued: report.source_lines_queued,
        estimated_memory_peak_bytes: report.estimated_memory_peak_bytes,
    }
}

fn average_micros(total: u128, count: usize) -> u128 {
    if count == 0 { 0 } else { total / count as u128 }
}

/// Returns the current process RSS in bytes on Unix-like hosts.
///
/// The benchmark runner uses the `ps` command because it is already available
/// in the supported development and CI environments. Non-Unix hosts return
/// `None` instead of shelling out to a platform-specific equivalent.
#[cfg(unix)]
fn current_rss_bytes() -> Option<u64> {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", pid.as_str()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kb| kb.saturating_mul(1024))
}

#[cfg(not(unix))]
fn current_rss_bytes() -> Option<u64> {
    None
}

fn print_measure_report(report: &MeasureSuiteReport) {
    println!(
        "{:<24} {:<7} {:>7} {:>8} {:>8} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8} {:>8} {:>8} {:>9} {:>8}",
        "scenario",
        "mode",
        "rows",
        "loadµs",
        "openµs",
        "filterµs",
        "grepµs",
        "hunkµs",
        "coldµs",
        "warmµs",
        "hit%",
        "qpeak",
        "cache",
        "synmem",
        "rssΔ"
    );
    for run in &report.runs {
        println!(
            "{:<24} {:<7} {:>7} {:>8} {:>8} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8} {:>8} {:>8} {:>9} {:>8}",
            run.scenario,
            run.mode,
            run.tui.row_count,
            run.load_micros,
            run.tui.open_micros,
            run.tui.file_filter_micros,
            run.tui.grep_filter_micros,
            run.tui.hunk_navigation_total_micros,
            run.tui.cold_scroll_avg_micros,
            run.tui.warm_scroll_avg_micros,
            percent(run.tui.warm_cache_hit_rate),
            run.tui.syntax.queue_depth_peak,
            run.tui.syntax.cache_entries_peak,
            human_bytes(run.tui.syntax.estimated_memory_peak_bytes),
            run.rss_delta_bytes
                .map(human_signed_bytes)
                .unwrap_or_else(|| "n/a".to_owned())
        );
    }
}

fn percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:.1}", value * 100.0))
        .unwrap_or_else(|| "n/a".to_owned())
}

fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        bytes.to_string()
    }
}

fn human_signed_bytes(bytes: i128) -> String {
    if bytes < 0 {
        format!("-{}", human_bytes(bytes.unsigned_abs() as u64))
    } else {
        human_bytes(bytes as u64)
    }
}

fn generate_scenario(
    output_root: &Path,
    scenario: ScenarioKind,
    force: bool,
) -> BenchResult<FixtureManifest> {
    let scenario_dir = output_root.join(scenario.name());
    prepare_output_dir(&scenario_dir, force)?;

    let manifest = match scenario {
        ScenarioKind::ManySmallFiles => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 240,
                lines: 72,
                changed_start: None,
                changed_lines: 12,
                extension: "ts",
            },
        )?,
        ScenarioKind::BalancedChangeset => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 96,
                lines: 420,
                changed_start: None,
                changed_lines: 96,
                extension: "ts",
            },
        )?,
        ScenarioKind::LargeSingleFile => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 1,
                lines: 32_000,
                changed_start: Some(8_000),
                changed_lines: 16_000,
                extension: "ts",
            },
        )?,
        ScenarioKind::ManyUntrackedSmall => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 16,
                lines: 72,
                changed_start: None,
                changed_lines: 12,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 120,
                lines: 36,
                extension: "ts",
            },
        )?,
        ScenarioKind::FewUntrackedLarge => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 8,
                lines: 80,
                changed_start: None,
                changed_lines: 16,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 6,
                lines: 5_000,
                extension: "ts",
            },
        )?,
        ScenarioKind::MinifiedOneLine => {
            generate_minified_one_line_scenario(&scenario_dir, scenario, 45_000)?
        }
        ScenarioKind::BinaryFiles => generate_binary_scenario(&scenario_dir, scenario)?,
        ScenarioKind::HugeMixedStress => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 1_000,
                lines: 600,
                changed_start: None,
                changed_lines: 120,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 500,
                lines: 160,
                extension: "ts",
            },
        )?,
        ScenarioKind::SyntaxManySmallRust => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 240,
                lines: 72,
                changed_start: None,
                changed_lines: 12,
                extension: "rs",
            },
        )?,
        ScenarioKind::SyntaxLargeRust => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 1,
                lines: 32_000,
                changed_start: Some(8_000),
                changed_lines: 16_000,
                extension: "rs",
            },
        )?,
        ScenarioKind::SyntaxMinifiedRust => {
            generate_minified_rust_scenario(&scenario_dir, scenario, 45_000)?
        }
    };

    write_manifest(&scenario_dir, &manifest)?;
    Ok(manifest)
}

fn prepare_output_dir(path: &Path, force: bool) -> BenchResult<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        return Ok(());
    }

    if !force {
        return Err(BenchError::Usage(format!(
            "fixture output already exists: {} (pass --force to replace it)",
            path.display()
        )));
    }

    fs::remove_dir_all(path)?;
    fs::create_dir_all(path)?;
    Ok(())
}

fn generate_tracked_text_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    shape: TextShape,
) -> BenchResult<FixtureManifest> {
    let repo = create_text_repo(scenario_dir, shape)?;
    write_pair_fixture(scenario_dir, shape, 9_999)?;
    let counts = FixtureCounts {
        tracked_files: shape.file_count,
        expected_text_additions: shape.file_count * shape.changed_lines,
        expected_text_deletions: shape.file_count * shape.changed_lines,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &[])
}

fn generate_untracked_text_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    tracked: TextShape,
    untracked: UntrackedShape,
) -> BenchResult<FixtureManifest> {
    let repo = create_text_repo(scenario_dir, tracked)?;
    let untracked_paths = add_untracked_text_files(&repo, untracked)?;
    write_pair_fixture(scenario_dir, tracked, 9_999)?;

    let counts = FixtureCounts {
        tracked_files: tracked.file_count,
        untracked_files: untracked.file_count,
        expected_text_additions: tracked.file_count * tracked.changed_lines
            + untracked.file_count * untracked.lines,
        expected_text_deletions: tracked.file_count * tracked.changed_lines,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &untracked_paths)
}

fn generate_minified_one_line_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    tokens: usize,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    let path = repo.join("src/bundle.min.js");
    write_file(
        &path,
        minified_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;
    write_file(
        &path,
        minified_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let pair = scenario_dir.join("pair");
    write_file(
        &pair.join("before.js"),
        minified_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    write_file(
        &pair.join("after.js"),
        minified_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let counts = FixtureCounts {
        tracked_files: 1,
        expected_text_additions: 1,
        expected_text_deletions: 1,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &[])
}

fn generate_minified_rust_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    tokens: usize,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    let path = repo.join("src/generated.rs");
    write_file(
        &path,
        minified_rust_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;
    write_file(
        &path,
        minified_rust_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let pair = scenario_dir.join("pair");
    write_file(
        &pair.join("before.rs"),
        minified_rust_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    write_file(
        &pair.join("after.rs"),
        minified_rust_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let counts = FixtureCounts {
        tracked_files: 1,
        expected_text_additions: 1,
        expected_text_deletions: 1,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &[])
}

fn minified_rust_source(tokens: usize, variant: SourceVariant) -> String {
    let mut text = String::from("pub static MARK_BENCH_GENERATED: &[&str] = &[");
    for index in 0..tokens {
        if index > 0 {
            text.push(',');
        }
        match variant {
            SourceVariant::Baseline => text.push_str(&format!("\"token_{index}\"")),
            SourceVariant::ChangedA => text.push_str(&format!("\"token_{index}_changed\"")),
        }
    }
    text.push_str("];\n");
    text
}

fn generate_binary_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    write_file(
        &repo.join("src/readme.txt"),
        synthetic_source(1, SourceVariant::Baseline, 24, None, 6).as_bytes(),
    )?;
    write_file(&repo.join("bin/blob.dat"), &binary_blob(32 * 1024, 17))?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;

    write_file(
        &repo.join("src/readme.txt"),
        synthetic_source(1, SourceVariant::ChangedA, 24, None, 6).as_bytes(),
    )?;
    write_file(&repo.join("bin/blob.dat"), &binary_blob(32 * 1024, 91))?;
    write_file(
        &repo.join("bin/new-untracked.dat"),
        &binary_blob(64 * 1024, 143),
    )?;

    let pair = scenario_dir.join("pair");
    write_file(&pair.join("before.bin"), &binary_blob(8 * 1024, 1))?;
    write_file(&pair.join("after.bin"), &binary_blob(8 * 1024, 2))?;

    let counts = FixtureCounts {
        tracked_files: 2,
        untracked_files: 1,
        binary_files: 2,
        expected_text_additions: 6,
        expected_text_deletions: 6,
    };

    finish_manifest(
        scenario_dir,
        scenario,
        counts,
        &repo,
        &[PathBuf::from("bin/new-untracked.dat")],
    )
}

fn create_text_repo(scenario_dir: &Path, shape: TextShape) -> BenchResult<PathBuf> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    for index in 1..=shape.file_count {
        let relative = text_file_path(index, shape.extension);
        write_file(
            &repo.join(&relative),
            synthetic_source_for_extension(
                index,
                SourceVariant::Baseline,
                shape.lines,
                shape.changed_start,
                shape.changed_lines,
                shape.extension,
            )
            .as_bytes(),
        )?;
    }

    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;

    for index in 1..=shape.file_count {
        let relative = text_file_path(index, shape.extension);
        write_file(
            &repo.join(&relative),
            synthetic_source_for_extension(
                index,
                SourceVariant::ChangedA,
                shape.lines,
                shape.changed_start,
                shape.changed_lines,
                shape.extension,
            )
            .as_bytes(),
        )?;
    }

    Ok(repo)
}

fn add_untracked_text_files(repo: &Path, shape: UntrackedShape) -> BenchResult<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(shape.file_count);
    for index in 1..=shape.file_count {
        let relative = PathBuf::from(format!("untracked/new{index}.{}", shape.extension));
        write_file(
            &repo.join(&relative),
            synthetic_source_for_extension(
                index,
                SourceVariant::ChangedA,
                shape.lines,
                None,
                shape.lines / 4,
                shape.extension,
            )
            .as_bytes(),
        )?;
        paths.push(relative);
    }
    Ok(paths)
}

fn write_pair_fixture(scenario_dir: &Path, shape: TextShape, file_index: usize) -> BenchResult<()> {
    let pair = scenario_dir.join("pair");
    write_file(
        &pair.join(format!("before.{}", shape.extension)),
        synthetic_source_for_extension(
            file_index,
            SourceVariant::Baseline,
            shape.lines,
            shape.changed_start,
            shape.changed_lines,
            shape.extension,
        )
        .as_bytes(),
    )?;
    write_file(
        &pair.join(format!("after.{}", shape.extension)),
        synthetic_source_for_extension(
            file_index,
            SourceVariant::ChangedA,
            shape.lines,
            shape.changed_start,
            shape.changed_lines,
            shape.extension,
        )
        .as_bytes(),
    )?;
    Ok(())
}

fn finish_manifest(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    counts: FixtureCounts,
    repo: &Path,
    untracked_paths: &[PathBuf],
) -> BenchResult<FixtureManifest> {
    let head_patch = append_untracked_patches(
        git_diff(
            repo,
            &[
                "diff",
                "HEAD",
                "--binary",
                "--no-ext-diff",
                "--no-color",
                "--find-renames",
            ],
        )?,
        repo,
        untracked_paths,
    )?;
    write_file(&scenario_dir.join("patch.diff"), head_patch.as_bytes())?;
    write_file(&scenario_dir.join("head.patch"), head_patch.as_bytes())?;

    Ok(FixtureManifest {
        version: 1,
        scenario: scenario.name().to_owned(),
        description: scenario.description().to_owned(),
        paths: FixturePaths {
            repo: "repo".to_owned(),
            patch: "patch.diff".to_owned(),
            head_patch: "head.patch".to_owned(),
            pair_before: pair_before_path(scenario_dir),
            pair_after: pair_after_path(scenario_dir),
        },
        counts,
        patch_bytes: head_patch.len() as u64,
        head_patch_bytes: head_patch.len() as u64,
    })
}

fn pair_before_path(scenario_dir: &Path) -> String {
    pair_file_path(scenario_dir, "before")
}

fn pair_after_path(scenario_dir: &Path) -> String {
    pair_file_path(scenario_dir, "after")
}

fn pair_file_path(scenario_dir: &Path, prefix: &str) -> String {
    let pair = scenario_dir.join("pair");
    let Ok(entries) = fs::read_dir(pair) else {
        return format!("pair/{prefix}.ts");
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) {
            return format!("pair/{name}");
        }
    }

    format!("pair/{prefix}.ts")
}

fn write_manifest(scenario_dir: &Path, manifest: &FixtureManifest) -> BenchResult<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    write_file(&scenario_dir.join("manifest.json"), &bytes)
}

fn append_untracked_patches(
    mut patch: String,
    repo: &Path,
    untracked_paths: &[PathBuf],
) -> BenchResult<String> {
    for relative in untracked_paths {
        let path = repo.join(relative);
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            append_separator(&mut patch);
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n",
                path = patch_path(relative)
            ));
            continue;
        }

        let text = String::from_utf8_lossy(&bytes);
        append_separator(&mut patch);
        patch.push_str(&new_file_patch(relative, &text));
    }
    Ok(patch)
}

fn append_separator(patch: &mut String) {
    if !patch.is_empty() && !patch.ends_with('\n') {
        patch.push('\n');
    }
}

fn new_file_patch(relative: &Path, contents: &str) -> String {
    let path = patch_path(relative);
    let lines: Vec<&str> = contents.lines().collect();
    let mut patch = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

fn patch_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn initialize_repo(path: &Path) -> BenchResult<()> {
    fs::create_dir_all(path)?;
    git(path, &["init"])?;
    git(path, &["config", "core.autocrlf", "false"])?;
    git(path, &["config", "core.eol", "lf"])?;
    git(path, &["config", "commit.gpgsign", "false"])?;
    git(path, &["config", "user.name", "Benchmark User"])?;
    git(path, &["config", "user.email", "benchmark@example.com"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> BenchResult<String> {
    git_with_program(cwd, "git", args)
}

fn git_diff(cwd: &Path, args: &[&str]) -> BenchResult<String> {
    git_with_program(cwd, "git", args)
}

fn git_with_program(cwd: &Path, program: &str, args: &[&str]) -> BenchResult<String> {
    let output = Command::new(program).current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(BenchError::Git {
            command: format!("{program} {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn text_file_path(index: usize, extension: &str) -> PathBuf {
    PathBuf::from(format!("src/bench{index}.{extension}"))
}

fn write_file(path: &Path, bytes: &[u8]) -> BenchResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn synthetic_source(
    file_index: usize,
    variant: SourceVariant,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
) -> String {
    let start = changed_start.unwrap_or(lines / 3).min(lines);
    let end = (start + changed_lines).min(lines);
    let mut text = String::new();

    for line_index in 0..lines {
        let line = line_index + 1;
        let in_changed_region = line_index >= start && line_index < end;
        if in_changed_region {
            match variant {
                SourceVariant::Baseline => text.push_str(&format!(
                    "export function bench{file_index}_{line}(value: number) {{ return value + {line}; }}\n"
                )),
                SourceVariant::ChangedA => text.push_str(&format!(
                    "export function bench{file_index}_{line}(value: number) {{ return value * {line} + {file_index}; }}\n"
                )),
            }
        } else {
            text.push_str(&format!(
                "export function bench{file_index}_{line}(value: number) {{ return value + {line}; }}\n"
            ));
        }
    }

    text
}

fn synthetic_source_for_extension(
    file_index: usize,
    variant: SourceVariant,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
    extension: &str,
) -> String {
    match extension {
        "rs" => synthetic_rust_source(file_index, variant, lines, changed_start, changed_lines),
        _ => synthetic_source(file_index, variant, lines, changed_start, changed_lines),
    }
}

fn synthetic_rust_source(
    file_index: usize,
    variant: SourceVariant,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
) -> String {
    let start = changed_start.unwrap_or(lines / 3).min(lines);
    let end = (start + changed_lines).min(lines);
    let mut text = String::new();

    for line_index in 0..lines {
        let line = line_index + 1;
        let in_changed_region = line_index >= start && line_index < end;
        if in_changed_region {
            match variant {
                SourceVariant::Baseline => text.push_str(&format!(
                    "pub fn bench_{file_index}_{line}(value: i64) -> i64 {{ value + {line} }}\n"
                )),
                SourceVariant::ChangedA => text.push_str(&format!(
                    "pub fn bench_{file_index}_{line}(value: i64) -> i64 {{ value * {line} + {file_index} }}\n"
                )),
            }
        } else {
            text.push_str(&format!(
                "pub fn bench_{file_index}_{line}(value: i64) -> i64 {{ value + {line} }}\n"
            ));
        }
    }

    text
}

fn minified_source(tokens: usize, variant: SourceVariant) -> String {
    let mut text = String::from("const markBenchBundle=[");
    for index in 0..tokens {
        if index > 0 {
            text.push(',');
        }
        match variant {
            SourceVariant::Baseline => text.push_str(&format!("\"token_{index}\"")),
            SourceVariant::ChangedA => text.push_str(&format!("\"token_{index}_changed\"")),
        }
    }
    text.push_str("];console.log(markBenchBundle.length);");
    text
}

fn binary_blob(size: usize, seed: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size);
    for index in 0..size {
        bytes.push(seed.wrapping_add((index % 251) as u8));
    }
    if !bytes.is_empty() {
        bytes[0] = 0;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scenario_names_are_unique() {
        let scenarios = [
            ScenarioKind::ManySmallFiles,
            ScenarioKind::BalancedChangeset,
            ScenarioKind::LargeSingleFile,
            ScenarioKind::ManyUntrackedSmall,
            ScenarioKind::FewUntrackedLarge,
            ScenarioKind::MinifiedOneLine,
            ScenarioKind::BinaryFiles,
            ScenarioKind::HugeMixedStress,
            ScenarioKind::SyntaxManySmallRust,
            ScenarioKind::SyntaxLargeRust,
            ScenarioKind::SyntaxMinifiedRust,
        ];
        let mut names = HashSet::new();
        for scenario in scenarios {
            assert!(names.insert(scenario.name()));
        }
    }

    #[test]
    fn synthetic_source_changes_only_requested_region() {
        let baseline = synthetic_source(1, SourceVariant::Baseline, 10, Some(3), 2);
        let changed = synthetic_source(1, SourceVariant::ChangedA, 10, Some(3), 2);
        let baseline_lines: Vec<_> = baseline.lines().collect();
        let changed_lines: Vec<_> = changed.lines().collect();

        for index in [0, 1, 2, 5, 6, 7, 8, 9] {
            assert_eq!(baseline_lines[index], changed_lines[index]);
        }
        assert_ne!(baseline_lines[3], changed_lines[3]);
        assert_ne!(baseline_lines[4], changed_lines[4]);
    }

    #[test]
    fn new_file_patch_uses_git_paths_and_addition_lines() {
        let patch = new_file_patch(Path::new("dir/file.ts"), "one\ntwo\n");
        assert!(patch.contains("diff --git a/dir/file.ts b/dir/file.ts"));
        assert!(patch.contains("@@ -0,0 +1,2 @@"));
        assert!(patch.contains("+one\n+two\n"));
    }

    #[test]
    fn all_scenarios_include_explicit_stress_selection() {
        let args = FixturesArgs {
            out: PathBuf::from("fixtures"),
            scenario: vec![ScenarioKind::HugeMixedStress],
            all: true,
            stress: false,
            syntax: false,
            force: false,
        };

        let selected = select_scenarios(&args);

        assert!(selected.starts_with(ScenarioKind::standard()));
        assert_eq!(selected.last(), Some(&ScenarioKind::HugeMixedStress));
    }

    #[test]
    fn syntax_flag_includes_syntax_fixture_suite() {
        let args = FixturesArgs {
            out: PathBuf::from("fixtures"),
            scenario: Vec::new(),
            all: false,
            stress: false,
            syntax: true,
            force: false,
        };

        let selected = select_scenarios(&args);

        assert!(selected.contains(&ScenarioKind::SyntaxManySmallRust));
        assert!(selected.contains(&ScenarioKind::SyntaxLargeRust));
        assert!(selected.contains(&ScenarioKind::SyntaxMinifiedRust));
    }

    #[test]
    fn rust_syntax_fixture_source_uses_rust_extension_and_shape() {
        let source =
            synthetic_source_for_extension(7, SourceVariant::ChangedA, 4, Some(1), 2, "rs");

        assert!(source.contains("pub fn bench_7_2(value: i64) -> i64"));
        assert!(source.contains("value * 2 + 7"));
    }

    #[test]
    fn syntax_compare_keeps_mark_only_inputs_without_shiki() {
        let dir = temp_test_dir("syntax-compare-mark-only");
        fs::create_dir_all(&dir).expect("test directory should be created");
        let file = dir.join(".gitignore");
        fs::write(&file, "target/\n").expect("test input should be written");

        let args = syntax_compare_test_args(file.clone(), false);
        let inputs = collect_syntax_compare_inputs(&args, &BTreeSet::new())
            .expect("syntax inputs should collect");

        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].path, file);
        assert_eq!(inputs[0].language, "git-ignore");
        assert_eq!(inputs[0].shiki_language, "git-ignore");

        fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn syntax_compare_skips_mark_only_inputs_when_shiki_requested() {
        let dir = temp_test_dir("syntax-compare-shiki-only");
        fs::create_dir_all(&dir).expect("test directory should be created");
        let file = dir.join(".gitignore");
        fs::write(&file, "target/\n").expect("test input should be written");

        let args = syntax_compare_test_args(file, true);
        let inputs = collect_syntax_compare_inputs(&args, &BTreeSet::new())
            .expect("syntax inputs should collect");

        assert!(inputs.is_empty());

        fs::remove_dir_all(dir).expect("test directory should be removed");
    }

    #[test]
    fn initialize_repo_pins_deterministic_git_config() {
        let repo = temp_test_dir("git-config");

        initialize_repo(&repo).expect("repo should initialize");

        assert_eq!(
            git(&repo, &["config", "core.autocrlf"]).unwrap().trim(),
            "false"
        );
        assert_eq!(git(&repo, &["config", "core.eol"]).unwrap().trim(), "lf");
        assert_eq!(
            git(&repo, &["config", "commit.gpgsign"]).unwrap().trim(),
            "false"
        );

        fs::remove_dir_all(repo).expect("test repo should be removed");
    }

    #[test]
    fn load_benchmark_changeset_counts_patch_bytes_without_hint() {
        let patch = Arc::<[u8]>::from(
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
                .as_slice(),
        );
        let options = mark_diff::DiffOptions {
            source: mark_diff::DiffSource::Patch(mark_diff::PatchSource::Stdin(patch.clone())),
            local_untracked: mark_diff::UntrackedMode::Exclude,
            ..mark_diff::DiffOptions::default()
        };

        let (changeset, patch_bytes) =
            load_benchmark_changeset(&options, None).expect("changeset should load");

        assert_eq!(patch_bytes, u64::try_from(patch.len()).unwrap());
        assert_eq!(changeset.files.len(), 1);
        assert!(changeset.raw_patch.is_empty());
    }

    fn syntax_compare_test_args(file: PathBuf, shiki: bool) -> SyntaxCompareArgs {
        SyntaxCompareArgs {
            repo: None,
            files: vec![file],
            languages: Vec::new(),
            max_files: usize::MAX,
            max_bytes: usize::MAX,
            iterations: 1,
            skip_counters: false,
            shiki,
            shiki_version: "4.3.0".to_owned(),
            shiki_dir: PathBuf::from("target/shiki-syntax-compare-test"),
            json: false,
        }
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mark-bench-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ))
    }
}
