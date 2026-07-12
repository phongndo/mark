use std::{
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::OnceLock,
};

const MAX_CPU_THREADS: usize = 8;

static CPU_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();

/// Returns the process-wide pool for divisible CPU-bound work.
///
/// The pool is constructed lazily. `MARK_CPU_THREADS` is therefore read only
/// once, when the pool is first requested. Values zero and one both select a
/// single worker; larger values are capped at eight.
pub fn cpu_pool() -> &'static rayon::ThreadPool {
    CPU_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(cpu_thread_count())
            .thread_name(|index| format!("mark-cpu-{index}"))
            .build()
            .expect("shared CPU pool should start")
    })
}

/// Runs CPU-bound work on the shared pool without blocking the async caller.
pub async fn run_cpu<F, R>(function: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    cpu_pool().spawn_fifo(move || {
        let result = catch_unwind(AssertUnwindSafe(function));
        let _ = sender.send(result);
    });

    match receiver
        .await
        .expect("shared CPU pool stopped before returning its result")
    {
        Ok(result) => result,
        Err(panic) => resume_unwind(panic),
    }
}

/// Reports whether the lazy shared CPU pool has been constructed.
pub fn is_cpu_pool_started() -> bool {
    CPU_POOL.get().is_some()
}

fn cpu_thread_count() -> usize {
    let override_value = std::env::var("MARK_CPU_THREADS").ok();
    cpu_thread_count_from(override_value.as_deref(), num_cpus::get_physical())
}

fn cpu_thread_count_from(override_value: Option<&str>, physical_threads: usize) -> usize {
    override_value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|threads| threads.clamp(1, MAX_CPU_THREADS))
        .unwrap_or_else(|| physical_threads.clamp(1, MAX_CPU_THREADS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_pool_is_bounded_and_named() {
        let pool = cpu_pool();
        assert!((1..=MAX_CPU_THREADS).contains(&pool.current_num_threads()));

        let (sender, receiver) = std::sync::mpsc::channel();
        pool.spawn(move || {
            let name = std::thread::current().name().map(str::to_owned);
            sender.send(name).expect("thread name should be sent");
        });

        assert!(
            receiver
                .recv()
                .expect("pool task should finish")
                .is_some_and(|name| name.starts_with("mark-cpu-"))
        );
        assert!(is_cpu_pool_started());
    }

    #[test]
    fn run_cpu_returns_pool_result() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime should start");
        let result = runtime.block_on(run_cpu(|| 20 + 22));

        assert_eq!(result, 42);
    }

    #[test]
    fn thread_count_override_is_serial_at_zero_or_one_and_capped_at_eight() {
        assert_eq!(cpu_thread_count_from(Some("0"), 4), 1);
        assert_eq!(cpu_thread_count_from(Some(" 1 "), 4), 1);
        assert_eq!(cpu_thread_count_from(Some("4"), 2), 4);
        assert_eq!(cpu_thread_count_from(Some("99"), 16), 8);
        assert_eq!(cpu_thread_count_from(Some("invalid"), 6), 6);
        assert_eq!(cpu_thread_count_from(None, 0), 1);
        assert_eq!(cpu_thread_count_from(None, 16), 8);
    }
}
