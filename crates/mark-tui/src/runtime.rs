use std::{
    future::Future,
    io,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::mpsc,
};

const RUNTIME_WORKER_THREADS: usize = 2;
const RUNTIME_MAX_BLOCKING_THREADS: usize = 8;
const CHANNEL_SEND_TIMEOUT: Duration = Duration::from_millis(10);
static CHANNEL_SEND_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn build_runtime() -> io::Result<Runtime> {
    Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKER_THREADS)
        .max_blocking_threads(RUNTIME_MAX_BLOCKING_THREADS)
        .thread_name("mark-tokio")
        .enable_time()
        .build()
}

pub(crate) fn block_on<F, R>(future: F) -> io::Result<R>
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    if Handle::try_current().is_ok() {
        let handle = thread::Builder::new()
            .name("mark-runtime".to_owned())
            .spawn(move || Ok(global_runtime().block_on(future)))?;
        return handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    }

    Ok(global_runtime().block_on(future))
}

pub(crate) fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(future)
    } else {
        global_runtime().spawn(future)
    }
}

pub(crate) fn spawn_blocking<F, R>(function: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn_blocking(function)
    } else {
        global_runtime().spawn_blocking(function)
    }
}

pub(crate) async fn run_blocking<F, R>(function: F) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    spawn_blocking(function).await
}

pub(crate) fn send_with_timeout<T>(sender: &mpsc::Sender<T>, mut value: T) -> bool {
    match sender.try_send(value) {
        Ok(()) => return true,
        Err(mpsc::error::TrySendError::Full(next_value)) => value = next_value,
        Err(mpsc::error::TrySendError::Closed(_)) => return false,
    }

    let start = std::time::Instant::now();
    loop {
        if start.elapsed() >= CHANNEL_SEND_TIMEOUT {
            CHANNEL_SEND_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        thread::sleep(Duration::from_millis(1));
        match sender.try_send(value) {
            Ok(()) => return true,
            Err(mpsc::error::TrySendError::Full(next_value)) => value = next_value,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
        }
    }
}

pub(crate) fn channel_send_timeout_count() -> u64 {
    CHANNEL_SEND_TIMEOUTS.load(Ordering::Relaxed)
}

fn global_runtime() -> &'static Runtime {
    // This runtime intentionally lives for the process lifetime. Static values are not dropped at
    // process exit, so blocked `spawn_blocking` work cannot delay CLI shutdown.
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| build_runtime().expect("tokio runtime should start"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;

    #[test]
    fn block_on_runs_without_current_tokio_runtime() {
        let value = block_on(async { 42 }).expect("runtime should start");

        assert_eq!(value, 42);
    }

    #[test]
    fn block_on_runs_inside_current_tokio_runtime() {
        let runtime = build_runtime().expect("runtime should start");
        let value = runtime.block_on(async {
            block_on(async { 42 }).expect("nested runtime helper should run on a thread")
        });

        assert_eq!(value, 42);
    }

    #[test]
    fn shutdown_timeout_does_not_wait_for_blocked_worker() {
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        let (stopped_tx, stopped_rx) = std_mpsc::channel();
        let runtime = build_runtime().expect("runtime should start");
        runtime.spawn_blocking(move || {
            started_tx.send(()).expect("start signal should send");
            let _ = release_rx.recv();
            stopped_tx.send(()).expect("stop signal should send");
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking worker should start");

        let shutdown_started = std::time::Instant::now();
        runtime.shutdown_timeout(Duration::from_millis(250));
        assert!(
            shutdown_started.elapsed() <= Duration::from_millis(300),
            "runtime shutdown exceeded the 300 ms exit budget"
        );

        let _ = release_tx.send(());
        stopped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking worker should stop after release");
    }

    #[test]
    fn send_timeout_is_counted() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(1)
            .expect("channel should have initial capacity");
        let before = channel_send_timeout_count();

        assert!(!send_with_timeout(&tx, 2));
        assert!(channel_send_timeout_count() > before);
    }
}
