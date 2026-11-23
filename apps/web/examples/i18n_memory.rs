//! Measures what the translation catalogs cost: embedded bytes, live heap and RSS
//! after each locale loads, peak heap while it inflates, load time, and whether a
//! `t!` lookup with nothing to interpolate allocates.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Instant,
};

use t3code_web::i18n::{Locale, catalog::Catalog, runtime::activate_locale_blocking, t};

/// Counts live bytes, the peak, and calls, so the numbers are exact heap use.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn rss_kib() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn kib(bytes: usize) -> String {
    format!("{:.0} KiB", bytes as f64 / 1024.0)
}

fn main() {
    println!("embedded deflated catalogs (9 locales, web + desktop): {}", kib(Catalog::embedded_size()));
    let base = LIVE.load(Relaxed);
    println!("baseline: heap {}, rss {} KiB", kib(base), rss_kib());
    println!("locale\tkeys\tload ms\tlive heap\tpeak heap during load\trss KiB");
    for locale in Locale::ALL.into_iter().skip(1) {
        activate_locale_blocking(Locale::En);
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        let start = Instant::now();
        activate_locale_blocking(locale);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let keys = Catalog::load(locale).map_or(0, |c| c.len());
        println!(
            "{}\t{keys}\t{ms:.1}\t{}\t{}\t{}",
            locale.tag(),
            kib(LIVE.load(Relaxed) - base),
            kib(PEAK.load(Relaxed) - base),
            rss_kib()
        );
    }

    activate_locale_blocking(Locale::De);
    let probe = || {
        let before = ALLOCATIONS.load(Relaxed);
        let start = Instant::now();
        let mut total = 0;
        for _ in 0..100_000 {
            total += t!("Open settings").len();
        }
        let ns = start.elapsed().as_nanos() as f64 / 100_000.0;
        (ALLOCATIONS.load(Relaxed) - before, ns, total)
    };
    let (allocations, ns, _) = probe();
    println!("de: 100000 x t!(\"Open settings\") -> {:?}: {allocations} allocations, {ns:.0} ns each", t!("Open settings"));
    activate_locale_blocking(Locale::En);
    println!("after switching back to English: heap {}", kib(LIVE.load(Relaxed) - base));
    let (allocations, ns, _) = probe();
    println!("en: 100000 x t!(\"Open settings\") -> {:?}: {allocations} allocations, {ns:.0} ns each", t!("Open settings"));
}
