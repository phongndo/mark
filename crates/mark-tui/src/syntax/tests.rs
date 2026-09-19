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

#[test]
fn rejected_visible_job_preserves_pending_prefetch_jobs() {
    // The visible jobs alone can prevent admission, even after evicting all
    // prefetch jobs. A failed push must not silently lose a pending key.
    let (_, result_rx) = mpsc::channel(1);
    let mut syntax = SyntaxRuntime {
        languages: SyntaxLanguageSet::from_enabled_languages(&[]),
        limits: SyntaxLimits::default(),
        result_rx,
        queue: SyntaxWorkerQueue::new(4, 0, 10),
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
        workers: Vec::new(),
    };
    let source = |bytes| {
        SyntaxJobSource::Hunk(HunkSource {
            text: "x".repeat(bytes),
            line_map: vec![Some(0)],
            source_lines: 1,
        })
    };
    assert!(syntax.queue_job(
        syntax_key(0),
        "rust".into(),
        source(8),
        SyntaxPriority::Visible,
        None
    ));
    assert!(syntax.queue_job(
        syntax_key(1),
        "rust".into(),
        source(2),
        SyntaxPriority::Prefetch,
        None
    ));
    assert!(!syntax.queue_job(
        syntax_key(2),
        "rust".into(),
        source(3),
        SyntaxPriority::Visible,
        None
    ));

    assert_eq!(
        syntax.pending,
        HashSet::from([syntax_key(0), syntax_key(1)])
    );
    assert_eq!(
        syntax.queue.len(),
        2,
        "rejection must not orphan pending jobs"
    );
    assert_eq!(syntax.queue.pop().unwrap().key, syntax_key(0));
    assert_eq!(syntax.queue.pop().unwrap().key, syntax_key(1));
}

#[test]
fn oversized_visible_job_does_not_evict_prefetch() {
    let queue = SyntaxWorkerQueue::new(2, 0, 2);
    let job = |file, bytes, priority| SyntaxJob {
        key: syntax_key(file),
        language: "rust".into(),
        source: SyntaxJobSource::Hunk(HunkSource {
            text: "x".repeat(bytes),
            line_map: vec![Some(0)],
            source_lines: 1,
        }),
        limits: SyntaxLimits::default(),
        queued_source_bytes: bytes as u64,
        priority,
        queued_at: std::time::Instant::now(),
    };
    queue
        .try_push(
            job(0, 1, SyntaxPriority::Prefetch),
            SyntaxPriority::Prefetch,
        )
        .unwrap();
    assert_eq!(
        queue.try_push(job(1, 3, SyntaxPriority::Visible), SyntaxPriority::Visible),
        Err(SyntaxQueueError::Full)
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.pop().unwrap().key, syntax_key(0));
    assert_eq!(queue.inner.state.lock().unwrap().queued_bytes, 0);
}

#[test]
fn promoted_job_reports_visible_priority() {
    let queue = SyntaxWorkerQueue::new(2, 0, 10);
    let job = SyntaxJob {
        key: syntax_key(0),
        language: "rust".into(),
        source: SyntaxJobSource::Hunk(HunkSource {
            text: "x".into(),
            line_map: vec![Some(0)],
            source_lines: 1,
        }),
        limits: SyntaxLimits::default(),
        queued_source_bytes: 1,
        priority: SyntaxPriority::Prefetch,
        queued_at: std::time::Instant::now(),
    };
    queue.try_push(job, SyntaxPriority::Prefetch).unwrap();
    assert!(queue.promote(syntax_key(0)));
    assert_eq!(queue.pop().unwrap().priority, SyntaxPriority::Visible);
}

#[test]
fn queue_admission_preserves_budget_and_reports_every_eviction() {
    for capacity in 0..=4 {
        for budget in 0..=8 {
            for incoming_bytes in 0..=10 {
                let queue = SyntaxWorkerQueue::new(capacity, 0, budget);
                let make_job = |file, bytes, priority| SyntaxJob {
                    key: syntax_key(file),
                    language: "rust".into(),
                    source: SyntaxJobSource::Hunk(HunkSource {
                        text: "x".repeat(bytes),
                        line_map: Vec::new(),
                        source_lines: 1,
                    }),
                    limits: SyntaxLimits::default(),
                    queued_source_bytes: bytes as u64,
                    priority,
                    queued_at: std::time::Instant::now(),
                };
                for (file, bytes, priority) in [
                    (0, 2, SyntaxPriority::Visible),
                    (1, 1, SyntaxPriority::Prefetch),
                    (2, 3, SyntaxPriority::Prefetch),
                ] {
                    let _ = queue.try_push(make_job(file, bytes, priority), priority);
                }
                let snapshot = || {
                    let state = queue.inner.state.lock().unwrap();
                    (
                        state
                            .visible
                            .iter()
                            .chain(&state.prefetch)
                            .map(|job| job.key)
                            .collect::<Vec<_>>(),
                        state.queued_bytes,
                    )
                };
                let before = snapshot();
                let result = queue.try_push(
                    make_job(3, incoming_bytes, SyntaxPriority::Visible),
                    SyntaxPriority::Visible,
                );
                let after = snapshot();
                match result {
                    Err(SyntaxQueueError::Full) => assert_eq!(after, before),
                    Err(error) => panic!("unexpected rejection: {error:?}"),
                    Ok(push) => {
                        let evicted = push
                            .dropped
                            .into_iter()
                            .chain(push.dropped_more)
                            .collect::<HashSet<_>>();
                        let missing = before
                            .0
                            .iter()
                            .copied()
                            .filter(|key| !after.0.contains(key))
                            .collect::<HashSet<_>>();
                        assert_eq!(evicted, missing);
                        assert!(after.0.contains(&syntax_key(3)));
                        assert_eq!(push.depth, after.0.len());
                        assert!(after.0.len() <= capacity);
                        assert!(after.1 <= budget as u64);
                    }
                }
                queue.set_generation(1);
                assert_eq!(snapshot(), (Vec::new(), 0));
                assert_eq!(
                    queue.try_push(
                        make_job(4, 0, SyntaxPriority::Visible),
                        SyntaxPriority::Visible
                    ),
                    Err(SyntaxQueueError::Stale)
                );
                queue.close();
                assert!(queue.pop().is_none());
            }
        }
    }
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
