//! Heap measurements for `live-check` phases.
//!
//! [`heap_mark`] notes the allocator's bytes in use (glibc `mallinfo2`) and resident memory at a
//! named step. Built with `--features heap-profile` and run with `T3_HEAP_PROFILE=<dir>`, a
//! sampling heap profiler runs from the start: about one allocation per 32 KiB allocated keeps its
//! backtrace while it lives (the tcmalloc method, so GPUI keeps its frame rate). Each mark writes
//! `<dir>/<step>.txt`: estimated live bytes by call site and by owning `t3code_*` function, for
//! everything live and for what was allocated since the step `T3_HEAP_FROM` names.

#[cfg(feature = "heap-profile")]
mod sampler {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

    const MEAN: f64 = 32.0 * 1024.0;
    const DEPTH: usize = 64;

    pub static ENABLED: AtomicBool = AtomicBool::new(false);
    /// Bumped at each mark; a sample records the epoch it was taken in.
    pub static EPOCH: AtomicU64 = AtomicU64::new(0);
    static SAMPLES: AtomicUsize = AtomicUsize::new(0);
    static LIVE: Mutex<Option<HashMap<usize, Sample>>> = Mutex::new(None);

    thread_local! {
        static GUARD: Cell<bool> = const { Cell::new(false) };
        static UNTIL: Cell<f64> = const { Cell::new(MEAN) };
        static RNG: Cell<u64> = const { Cell::new(0) };
    }

    struct Sample {
        size: usize,
        epoch: u64,
        frames: Vec<usize>,
    }

    /// Runs `f` with this thread's allocations unsampled; `None` when already inside.
    fn guarded<R>(f: impl FnOnce() -> R) -> Option<R> {
        if GUARD.try_with(|g| g.replace(true)).unwrap_or(true) {
            return None;
        }
        let result = f();
        _ = GUARD.try_with(|g| g.set(false));
        Some(result)
    }

    fn next_gap() -> f64 {
        let mut x = RNG.try_with(Cell::get).unwrap_or(0);
        if x == 0 {
            x = (&x as *const u64 as u64) | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        _ = RNG.try_with(|r| r.set(x));
        let u = ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
        -u.ln() * MEAN
    }

    fn record(ptr: *mut u8, size: usize) {
        if !ENABLED.load(Relaxed) {
            return;
        }
        let Ok(left) = UNTIL.try_with(|u| {
            let left = u.get() - size as f64;
            u.set(left);
            left
        }) else {
            return;
        };
        if left > 0.0 {
            return;
        }
        guarded(|| {
            _ = UNTIL.try_with(|u| u.set(next_gap()));
            let mut frames = Vec::with_capacity(DEPTH);
            backtrace::trace(|frame| {
                frames.push(frame.ip() as usize);
                frames.len() < DEPTH
            });
            let sample = Sample { size, epoch: EPOCH.load(Relaxed), frames };
            let replaced = LIVE.lock().unwrap().get_or_insert_with(HashMap::new).insert(ptr as usize, sample);
            if replaced.is_none() {
                SAMPLES.fetch_add(1, Relaxed);
            }
        });
    }

    fn forget(ptr: *mut u8) {
        if SAMPLES.load(Relaxed) == 0 {
            return;
        }
        guarded(|| {
            let removed = LIVE.lock().unwrap().as_mut().and_then(|live| live.remove(&(ptr as usize)));
            if removed.is_some() {
                SAMPLES.fetch_sub(1, Relaxed);
            }
            removed
        });
    }

    /// The system allocator, sampling while [`ENABLED`].
    pub struct Sampler;

    unsafe impl GlobalAlloc for Sampler {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                record(ptr, layout.size());
            }
            ptr
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                record(ptr, layout.size());
            }
            ptr
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            forget(ptr);
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            forget(ptr);
            let next = unsafe { System.realloc(ptr, layout, new_size) };
            if !next.is_null() {
                record(next, new_size);
            }
            next
        }
    }

    /// The bytes a sample stands for.
    fn weight(size: usize) -> f64 {
        let size = size as f64;
        size / (1.0 - (-size / MEAN).exp())
    }

    fn symbols(ip: usize, cache: &mut HashMap<usize, Vec<String>>) -> Vec<String> {
        cache
            .entry(ip)
            .or_insert_with(|| {
                let mut names = Vec::new();
                backtrace::resolve(ip as *mut std::ffi::c_void, |symbol| {
                    let name = symbol.name().map(|n| format!("{n:#}")).unwrap_or_else(|| format!("{ip:#x}"));
                    let at = match (symbol.filename(), symbol.lineno()) {
                        (Some(file), Some(line)) => {
                            let file = file.to_string_lossy();
                            // Our crates keep their path from the repo root; others from the crate.
                            let short = ["/apps/", "/packages/", "/patches/", "/registry/src/"]
                                .iter()
                                .find_map(|root| file.rsplit_once(root).map(|(_, rest)| rest.split_once('/').filter(|_| *root == "/registry/src/").map_or(rest, |(_, r)| r)))
                                .unwrap_or(file.as_ref())
                                .to_owned();
                            let ours = ["/apps/", "/packages/"].iter().any(|root| file.contains(root)) && !file.contains("/.cargo/");
                            format!(" ({}{short}:{line})", if ours { "@" } else { "" })
                        }
                        _ => String::new(),
                    };
                    names.push(format!("{name}{at}"));
                });
                names
            })
            .clone()
    }

    fn noise(name: &str) -> bool {
        ["live_check_heap", "backtrace::", "alloc::alloc", "__rust", "std::alloc", "core::alloc", "<alloc::", "alloc::raw_vec", "hashbrown::raw"].iter().any(|n| name.contains(n))
    }

    /// Writes the report for the samples taken since epoch `since` (and for all live ones).
    pub fn report(path: &std::path::Path, step: &str, since: u64) {
        guarded(|| {
            let samples: Vec<(f64, u64, Vec<usize>)> =
                LIVE.lock().unwrap().as_ref().map(|live| live.values().map(|s| (weight(s.size), s.epoch, s.frames.clone())).collect()).unwrap_or_default();
            let mut cache = HashMap::new();
            let mut out = String::new();
            for (title, from) in [(format!("allocated since mark epoch {since}, still live"), since), ("everything live".to_owned(), 0)] {
                let chosen: Vec<&(f64, u64, Vec<usize>)> = samples.iter().filter(|s| s.1 >= from).collect();
                let total: f64 = chosen.iter().map(|s| s.0).sum();
                _ = writeln!(out, "== {step}: {title}: {:.1} MiB in {} samples\n", total / 1048576.0, chosen.len());
                let mut by_owner: HashMap<String, f64> = HashMap::new();
                let mut by_stack: HashMap<(String, Vec<String>), f64> = HashMap::new();
                let mut scheduler = 0.0;
                for (bytes, _, frames) in &chosen {
                    let names: Vec<String> = frames.iter().flat_map(|ip| symbols(*ip, &mut cache)).filter(|n| !noise(n)).collect();
                    // The headless test scheduler resolves a backtrace when a foreign thread wakes
                    // a task; with line tables that loads the binary's DWARF. The app never does.
                    if names.iter().any(|n| n.starts_with("assert_correct_thread ")) {
                        scheduler += bytes;
                        continue;
                    }
                    let owner = names.iter().find(|n| n.contains(" (@") && !n.contains("live_check")).cloned().unwrap_or_else(|| names.first().cloned().unwrap_or_default());
                    *by_owner.entry(owner.clone()).or_default() += bytes;
                    let stack: Vec<String> = names.into_iter().take(64).collect();
                    *by_stack.entry((owner, stack)).or_default() += bytes;
                }
                let mut owners: Vec<_> = by_owner.into_iter().collect();
                owners.sort_by(|a, b| b.1.total_cmp(&a.1));
                _ = writeln!(out, "headless test scheduler backtraces (not the app): {:.1} MiB; the app: {:.1} MiB\n", scheduler / 1048576.0, (total - scheduler) / 1048576.0);
                _ = writeln!(out, "-- by owning t3code function");
                for (owner, bytes) in owners.iter().take(40) {
                    _ = writeln!(out, "{:9.2} MiB  {owner}", bytes / 1048576.0);
                }
                let mut stacks: Vec<_> = by_stack.into_iter().collect();
                stacks.sort_by(|a, b| b.1.total_cmp(&a.1));
                _ = writeln!(out, "\n-- top stacks of the 15 largest owners");
                for (owner, bytes) in owners.iter().take(15) {
                    _ = writeln!(out, "{:9.2} MiB  {owner}", bytes / 1048576.0);
                    for ((_, stack), bytes) in stacks.iter().filter(|((o, _), _)| o == owner).take(2) {
                        _ = writeln!(out, "  {:9.2} MiB", bytes / 1048576.0);
                        for frame in stack.iter().take_while(|frame| *frame != owner) {
                            _ = writeln!(out, "        {frame}");
                        }
                    }
                }
                _ = writeln!(out);
            }
            _ = std::fs::write(path, out);
        });
    }
}

#[cfg(feature = "heap-profile")]
#[global_allocator]
static ALLOCATOR: sampler::Sampler = sampler::Sampler;

/// Starts sampling when `T3_HEAP_PROFILE` names a directory.
pub(crate) fn heap_init() {
    #[cfg(feature = "heap-profile")]
    if let Some(dir) = std::env::var_os("T3_HEAP_PROFILE") {
        _ = std::fs::create_dir_all(dir);
        sampler::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

/// Bytes the allocator has handed out and not had back (small blocks plus mapped large ones),
/// and bytes it holds free in its arenas.
pub(crate) fn heap_in_use() -> (usize, usize) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: mallinfo2 only reads the allocator's counters.
        let info = unsafe { mallinfo2() };
        (info.uordblks + info.hblkhd, info.fordblks)
    }
    #[cfg(not(target_os = "linux"))]
    (0, 0)
}

/// Resident memory by mapping (`/proc/self/smaps`): anonymous, heap and each file, largest first.
fn resident_by_mapping() -> Vec<(String, u64)> {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let mut by: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut current = String::new();
    for line in smaps.lines() {
        let mut fields = line.split_whitespace();
        let first = fields.next().unwrap_or_default();
        if first.contains('-') && !first.ends_with(':') {
            let path = line.split_whitespace().nth(5).unwrap_or("[anon]");
            current = path.rsplit('/').next().unwrap_or(path).to_owned();
        } else if first == "Rss:" {
            *by.entry(current.clone()).or_default() += fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }
    let mut list: Vec<_> = by.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    list
}

/// Heap in use and resident memory at `step`, in MiB, and writes the profile for this step.
pub(crate) fn heap_mark(step: &str) -> (f64, f64) {
    let (used, free) = heap_in_use();
    let rss = super::rss_mib();
    println!("  heap mark {step}: in use {:.1} MiB, free in arenas {:.1} MiB, resident {rss:.1} MiB", used as f64 / 1048576.0, free as f64 / 1048576.0);
    let mappings: Vec<String> = resident_by_mapping().iter().take(8).map(|(name, kib)| format!("{name} {:.1}", *kib as f64 / 1024.0)).collect();
    println!("  heap mark {step}: resident MiB by mapping: {}", mappings.join(", "));
    #[cfg(feature = "heap-profile")]
    if let Some(dir) = std::env::var_os("T3_HEAP_PROFILE") {
        use std::sync::atomic::Ordering::Relaxed;
        static FROM: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
        let epoch = sampler::EPOCH.fetch_add(1, Relaxed) + 1;
        let mut from = FROM.lock().unwrap();
        if from.is_none() || std::env::var("T3_HEAP_FROM").is_ok_and(|named| named == step) {
            *from = Some(epoch);
        }
        sampler::report(&std::path::Path::new(&dir).join(format!("{step}.txt")), step, from.unwrap_or(0));
    }
    (used as f64 / 1048576.0, rss)
}
