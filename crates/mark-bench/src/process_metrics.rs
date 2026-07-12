use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

pub(crate) struct PeakThreadSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}

impl PeakThreadSampler {
    pub(crate) fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicUsize::new(current_thread_count().unwrap_or_default()));
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let worker_stop = Arc::clone(&stop);
        let worker_peak = Arc::clone(&peak);
        let worker = thread::Builder::new()
            .name("mark-bench-census".to_owned())
            .spawn(move || {
                sample(&worker_peak, true);
                let _ = ready_tx.send(());
                while !worker_stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(1));
                    sample(&worker_peak, true);
                }
            })
            .ok();
        if worker.is_some() {
            let _ = ready_rx.recv();
        }
        Self { stop, peak, worker }
    }

    pub(crate) fn finish(mut self) -> Option<usize> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let peak = self.peak.load(Ordering::Relaxed);
        (peak > 0).then_some(peak)
    }
}

impl Drop for PeakThreadSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn sample(peak: &AtomicUsize, exclude_sampler: bool) {
    if let Some(mut count) = current_thread_count() {
        if exclude_sampler {
            count = count.saturating_sub(1);
        }
        peak.fetch_max(count, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
fn current_thread_count() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))?
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn current_thread_count() -> Option<usize> {
    use std::{ffi::c_int, mem::MaybeUninit};

    const PROC_PIDTASKINFO: c_int = 4;

    #[repr(C)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: c_int,
        faults: c_int,
        pageins: c_int,
        cow_faults: c_int,
        messages_sent: c_int,
        messages_received: c_int,
        syscalls_mach: c_int,
        syscalls_unix: c_int,
        csw: c_int,
        threadnum: c_int,
        numrunning: c_int,
        priority: c_int,
    }

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let mut info = MaybeUninit::<ProcTaskInfo>::uninit();
    let expected = c_int::try_from(std::mem::size_of::<ProcTaskInfo>()).ok()?;
    // SAFETY: `info` points to a writable buffer of exactly `expected` bytes,
    // and it is only assumed initialized when proc_pidinfo reports a full write.
    let written = unsafe {
        proc_pidinfo(
            c_int::try_from(std::process::id()).ok()?,
            PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            expected,
        )
    };
    if written != expected {
        return None;
    }
    // SAFETY: a full successful write initialized every field in the C struct.
    let info = unsafe { info.assume_init() };
    usize::try_from(info.threadnum).ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn current_thread_count() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_has_at_least_one_thread() {
        if let Some(count) = current_thread_count() {
            assert!(count >= 1);
        }
    }

    #[test]
    fn sampler_reports_the_process_peak() {
        let sampler = PeakThreadSampler::start();
        let worker = thread::spawn(|| thread::sleep(Duration::from_millis(10)));
        thread::sleep(Duration::from_millis(3));
        worker.join().unwrap();
        if let Some(peak) = sampler.finish() {
            assert!(peak >= 2);
        }
    }
}
