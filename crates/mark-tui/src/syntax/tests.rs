use super::*;
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc as std_mpsc,
    thread,
    time::Duration,
};

use mark_syntax::{SyntaxLanguageSet, SyntaxLimits};
use tokio::sync::mpsc;

use crate::theme::{SYNTAX_THEME_ID, SyntaxBenchmarkReport};

#[test]
fn drop_closes_full_result_channel_before_joining_worker() {
    let queue = SyntaxWorkerQueue::new(1, 0, usize::MAX);
    let (result_tx, result_rx) = mpsc::channel(1);
    result_tx
        .try_send(SyntaxResult {
            key: syntax_key(0),
            language: "rust".to_owned(),
            source_kind: SyntaxSourceKind::HunkSide { hunk: 0 },
            priority: SyntaxPriority::Visible,
            queue_latency_micros: 0,
            run_latency_micros: 0,
            side: Err(SyntaxJobFailure::HighlightError),
        })
        .expect("result channel should be prefilled");

    let (started_tx, started_rx) = std_mpsc::channel();
    let worker = thread::spawn(move || {
        started_tx
            .send(())
            .expect("worker start signal should send");
        let _ = result_tx.blocking_send(SyntaxResult {
            key: syntax_key(1),
            language: "rust".to_owned(),
            source_kind: SyntaxSourceKind::HunkSide { hunk: 0 },
            priority: SyntaxPriority::Visible,
            queue_latency_micros: 0,
            run_latency_micros: 0,
            side: Err(SyntaxJobFailure::HighlightError),
        });
    });
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("worker should start");

    let syntax = SyntaxRuntime {
        languages: SyntaxLanguageSet::from_enabled_languages(&[]),
        limits: SyntaxLimits::default(),
        result_rx,
        queue,
        cache: LruCache::new(8),
        pending: HashSet::new(),
        source_keys: HashMap::new(),
        position_keys: HashMap::new(),
        line_maps: HashMap::new(),
        skipped: HashMap::new(),
        skipped_sources: HashSet::new(),
        unavailable_full_files: HashSet::new(),
        failed: HashSet::new(),
        stats: SyntaxBenchmarkReport::default(),
        workers: vec![worker],
    };
    let (done_tx, done_rx) = std_mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(syntax);
        done_tx.send(()).expect("drop signal should send");
    });

    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("syntax runtime drop should not wait on a full result channel");
    dropper.join().expect("dropper thread should finish");
}

#[test]
fn drain_records_syntax_latency_buckets() {
    let key = syntax_key(0);
    let (result_tx, result_rx) = mpsc::channel(1);
    result_tx
        .try_send(SyntaxResult {
            key,
            language: "rust".to_owned(),
            source_kind: SyntaxSourceKind::HunkSide { hunk: 0 },
            priority: SyntaxPriority::Visible,
            queue_latency_micros: 7,
            run_latency_micros: 11,
            side: Err(SyntaxJobFailure::HighlightError),
        })
        .expect("result channel should accept latency fixture");

    let mut syntax = SyntaxRuntime {
        languages: SyntaxLanguageSet::from_enabled_languages(&[]),
        limits: SyntaxLimits::default(),
        result_rx,
        queue: SyntaxWorkerQueue::new(1, 0, usize::MAX),
        cache: LruCache::new(8),
        pending: HashSet::from([key]),
        source_keys: HashMap::new(),
        position_keys: HashMap::new(),
        line_maps: HashMap::new(),
        skipped: HashMap::new(),
        skipped_sources: HashSet::new(),
        unavailable_full_files: HashSet::new(),
        failed: HashSet::new(),
        stats: SyntaxBenchmarkReport::default(),
        workers: Vec::new(),
    };

    syntax.drain(0, 1);

    assert_eq!(syntax.stats.first_visible_latency_micros, Some(18));
    assert_eq!(syntax.stats.latency_buckets.len(), 1);
    let bucket = &syntax.stats.latency_buckets[0];
    assert_eq!(bucket.language, "rust");
    assert_eq!(bucket.source_kind, "hunk");
    assert_eq!(bucket.jobs, 1);
    assert_eq!(bucket.queue_latency_total_micros, 7);
    assert_eq!(bucket.run_latency_total_micros, 11);
}

fn syntax_key(file: usize) -> SyntaxKey {
    SyntaxKey {
        source: SyntaxSourceId {
            generation: 0,
            file,
            side: DiffSide::New,
            kind: SyntaxSourceKind::HunkSide { hunk: 0 },
        },
        language_hash: 1,
        theme_id: SYNTAX_THEME_ID,
    }
}
