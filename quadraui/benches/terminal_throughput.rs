//! Throughput benchmark for [`quadraui::terminal_engine::TerminalSession`]
//! (issue #340).
//!
//! "Simple and fast" is the terminal engine's product thesis but was
//! unmeasured until this benchmark. It targets the two functions the
//! issue called out as plausible bottlenecks:
//!
//! - `build_rows` (private; reached here via the public
//!   [`TerminalSession::to_terminal`]) — rebuilds the entire cell-grid
//!   snapshot from scratch every frame.
//! - `process_with_capture` (private; reached here via the
//!   `#[doc(hidden)]` [`TerminalSession::feed_for_bench`] hook added by
//!   this issue) — the ingest path `poll()` uses, including the
//!   `set_scrollback(n)` → read → `set_scrollback(0)` capture dance for
//!   rows that scroll off the live screen into history.
//!
//! `feed_for_bench` exists because `process_with_capture` is normally only
//! reachable through `poll()`, which drains a channel fed by a real PTY
//! reader thread — not something a repeatable, deterministic benchmark
//! wants to depend on. `feed_for_bench` drives the *exact same* private
//! ingest code with synthetic bytes instead, "no real PTY needed" per the
//! issue. A `TerminalSession` is still spawned once per `bench_function`
//! (real PTY, real shell) because that is the only way to construct one —
//! the shell is left idle and its own output is simply never read; all
//! measured work happens through `feed_for_bench`/`resize`/`to_terminal`.
//!
//! Four workloads, matching the issue's list:
//!
//! 1. `cat_storm` — a large-file dump: thousands of newline-terminated
//!    lines, far more than fit on screen, so every chunk boundary also
//!    exercises `capture_scrolled_rows`'s scrollback dance.
//! 2. `yes_flood` — minimal per-line content at maximum line rate (the
//!    `capture_scrolled_rows` call count is maximised relative to bytes
//!    processed).
//! 3. `resize_storm` — repeated `resize()` calls alternating width (the
//!    expensive `reflow_screen` snapshot→resize→replay path, not the
//!    cheap height-only `set_size`), modelling a tmux/vim-style drag.
//! 4. `sgr_heavy` — a color-churn stream: an SGR 256-color escape before
//!    nearly every character, stressing attribute bookkeeping rather than
//!    raw text throughput.
//!
//! Plus a dedicated `snapshot_build` benchmark isolating `to_terminal`
//! (i.e. `build_rows`) on a screen already full of history + live content,
//! and a one-shot (non-criterion) allocation count printed to stderr for
//! both `process_with_capture` and `build_rows`, addressing the issue's
//! "allocations per frame" ask — criterion itself has no allocation-count
//! metric, so this runs its own before/after delta around a fixed
//! iteration count using a counting `#[global_allocator]`.
//!
//! Run: `cargo bench --bench terminal_throughput --features terminal`
//! (`cargo bench --bench terminal_throughput --features terminal -- --quick`
//! for a fast sanity pass).
//!
//! ## Baseline (one worker run, debug-grade dev box, `--sample-size 30
//! --measurement-time 2`; re-run before trusting absolute numbers — only
//! the *relative* shape across workloads is the load-bearing finding)
//!
//! | Benchmark | Time | Throughput | Allocs/call | Bytes/call |
//! |---|---|---|---|---|
//! | `cat_storm` (4 000 lines, 284 000 B, 4 000 captured rows) | ~32 ms | ~8.4 MiB/s | ~490 000 | ~44 MB |
//! | `yes_flood` (20 000 lines, 40 000 B, 20 000 captured rows) | ~144 ms | ~270 KiB/s | — | — |
//! | `sgr_heavy` (500 lines × 120 cols SGR, 631 000 B, 500 captured rows) | ~8 ms | ~83 MiB/s | — | — |
//! | `resize_storm` (width-change `resize()`) | ~80 ms/call | ~12.5 elem/s | — | — |
//! | `snapshot_build` (`to_terminal`, 120×50 grid, history populated) | ~346 µs/call | ~17.3 Melem/s | ~6 052 | ~295 KB |
//!
//! **Takeaways — "fast" does not hold uniformly, and it is the
//! scrollback-capture dance in `process_with_capture`/`capture_scrolled_rows`
//! that dominates, not `build_rows`:**
//!
//! - **`process_with_capture` throughput is driven by captured-row count,
//!   not byte count.** `yes_flood` has 7× *fewer* bytes than `cat_storm`
//!   but is ~4.5× *slower* in wall time (31× worse MiB/s) — it has 5×
//!   more newlines, i.e. 5× more rows pushed through
//!   `capture_scrolled_rows`'s `set_scrollback(n)` → per-cell read →
//!   `set_scrollback(0)` dance. `sgr_heavy` confirms the other direction:
//!   2.2× `cat_storm`'s bytes but only 500 captured rows (one per line,
//!   same as `cat_storm`'s 4 000... scaled down) runs ~4× faster in
//!   MiB/s than `cat_storm` and ~300× faster than `yes_flood`.
//! - **The per-cell allocation count in `process_with_capture` is the
//!   smoking gun.** `cat_storm`'s 4 000 captured rows × 120 cols ≈ 480 000
//!   cells, and the call measures ~490 000 allocations — essentially one
//!   `String` allocation per captured cell
//!   (`capture_scrolled_rows`'s `HistCell { text: ... .to_string(), ... }`
//!   per `(row, col)`), ~44 MB/call. A sustained flood (CI log tail,
//!   build output) pays this on every captured row, forever.
//! - **`resize_storm` is the other outlier**: ~80 ms per width-changing
//!   `resize()` call is well above a single frame budget (16.6 ms @
//!   60 fps) — consistent with `reflow_screen`'s
//!   snapshot→resize→replay (`contents_formatted` dump + full reparse)
//!   being expensive, exactly the drag-resize scenario the issue named.
//! - **`build_rows` (via `to_terminal`) is comparatively cheap in wall
//!   time** — ~346 µs is ~2% of a 60fps frame budget — but still
//!   allocates ~6 000 times and ~295 KB *every single frame* for a
//!   120×50 grid (one `String`-per-cell `TerminalCell`, rebuilt from
//!   scratch regardless of what changed). Not the dominant cost measured
//!   here, but real allocation churn a dirty-row/diff approach would
//!   remove.
//!
//! Per the issue's acceptance bar, this should be filed as a follow-up
//! optimization: dirty-row tracking (or at least capping/batching
//! `capture_scrolled_rows`'s per-cell `String` allocation) to fix the
//! `process_with_capture` outlier, and investigating whether
//! `reflow_screen` can avoid a full dump+replay on every `resize()` call
//! during a drag — both ahead of `build_rows`, which is a smaller, steadier
//! cost by comparison.

use criterion::{Criterion, Throughput};
use quadraui::terminal_engine::TerminalSession;
use quadraui::WidgetId;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── Allocation counting (for the "allocations per frame" ask) ──────────────

struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarding to `System` with the same layout contract
        // `GlobalAlloc::alloc` requires of its caller.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarding to `System` with the same layout/ptr contract
        // `GlobalAlloc::dealloc` requires of its caller.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn alloc_snapshot() -> (usize, usize) {
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

// ── Fixture generation ──────────────────────────────────────────────────────

const COLS: u16 = 120;
const ROWS: u16 = 50;

/// `cat`-of-a-large-file: thousands of newline-terminated lines, far more
/// than the 50-row screen, so processing it exercises `capture_scrolled_rows`
/// repeatedly, not just once.
fn cat_storm_bytes(lines: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    for i in 0..lines {
        buf.extend_from_slice(
            format!("line {i:06} the quick brown fox jumps over the lazy dog 0123456789 end\n")
                .as_bytes(),
        );
    }
    buf
}

/// `yes`-style flood: minimal per-line content, maximum line rate.
fn yes_flood_bytes(lines: usize) -> Vec<u8> {
    "y\n".repeat(lines).into_bytes()
}

/// SGR-heavy / wide-output stream: a 256-color escape before nearly every
/// character, cycling through the palette, across many lines.
fn sgr_heavy_bytes(lines: usize, cols: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    for l in 0..lines {
        for c in 0..cols {
            let color = (l * cols + c) % 256;
            buf.extend_from_slice(format!("\x1b[38;5;{color}mX").as_bytes());
        }
        buf.extend_from_slice(b"\x1b[0m\n");
    }
    buf
}

fn spawn_idle_session(cols: u16, rows: u16, history_capacity: usize) -> TerminalSession {
    let cwd = std::env::temp_dir();
    // `/bin/sh` left idle at its own prompt — we never call `poll()`, so its
    // output (if any) just sits unread in the reader thread's channel. All
    // measured work below goes through `feed_for_bench`/`resize`/
    // `to_terminal`, not through the shell's own output.
    TerminalSession::spawn(cols, rows, "/bin/sh", Path::new(&cwd), history_capacity)
        .expect("failed to spawn /bin/sh for benchmark fixture")
}

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_cat_storm(c: &mut Criterion) {
    let data = cat_storm_bytes(4_000);
    let mut session = spawn_idle_session(COLS, ROWS, 2_000);
    let mut group = c.benchmark_group("terminal_cat_storm");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("process_with_capture", |b| {
        b.iter(|| session.feed_for_bench(black_box(&data)));
    });
    group.finish();
}

fn bench_yes_flood(c: &mut Criterion) {
    let data = yes_flood_bytes(20_000);
    let mut session = spawn_idle_session(COLS, ROWS, 2_000);
    let mut group = c.benchmark_group("terminal_yes_flood");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("process_with_capture", |b| {
        b.iter(|| session.feed_for_bench(black_box(&data)));
    });
    group.finish();
}

fn bench_sgr_heavy(c: &mut Criterion) {
    let data = sgr_heavy_bytes(500, COLS as usize);
    let mut session = spawn_idle_session(COLS, ROWS, 2_000);
    let mut group = c.benchmark_group("terminal_sgr_heavy");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("process_with_capture", |b| {
        b.iter(|| session.feed_for_bench(black_box(&data)));
    });
    group.finish();
}

fn bench_resize_storm(c: &mut Criterion) {
    let mut session = spawn_idle_session(COLS, ROWS, 2_000);
    // Prime the screen with real content first — a resize storm against an
    // empty screen would never exercise `reflow_screen`'s replay path.
    session.feed_for_bench(&cat_storm_bytes(200));
    let sizes = [(COLS, ROWS), (COLS + 40, ROWS - 10)];
    let mut i = 0usize;
    let mut group = c.benchmark_group("terminal_resize_storm");
    group.throughput(Throughput::Elements(1));
    group.bench_function("resize_width_change", |b| {
        b.iter(|| {
            let (cols, rows) = sizes[i % 2];
            i += 1;
            session.resize(black_box(cols), black_box(rows));
        });
    });
    group.finish();
}

fn bench_snapshot_build(c: &mut Criterion) {
    let mut session = spawn_idle_session(COLS, ROWS, 2_000);
    // Fill both history and the live screen so `build_rows` has real work
    // to do blending the two, not just painting blanks.
    session.feed_for_bench(&cat_storm_bytes(4_000));
    let mut group = c.benchmark_group("terminal_snapshot_build");
    group.throughput(Throughput::Elements((COLS as u64) * (ROWS as u64)));
    group.bench_function("to_terminal", |b| {
        b.iter(|| black_box(session.to_terminal(WidgetId::new("bench-terminal"), None)));
    });
    group.finish();
}

/// Not a criterion-measured benchmark — a one-shot diagnostic printed to
/// stderr. Criterion has no "allocations per iteration" metric, so this
/// takes an allocator snapshot, runs a fixed number of iterations of each
/// hot path, and reports the delta divided by iteration count.
fn report_allocations_per_frame() {
    const ITERS: usize = 200;

    // `process_with_capture`, via a representative cat-storm chunk.
    {
        let data = cat_storm_bytes(4_000);
        let mut session = spawn_idle_session(COLS, ROWS, 2_000);
        session.feed_for_bench(&data); // warm up (first-touch allocations)
        let (count0, bytes0) = alloc_snapshot();
        for _ in 0..ITERS {
            session.feed_for_bench(black_box(&data));
        }
        let (count1, bytes1) = alloc_snapshot();
        eprintln!(
            "terminal_throughput: process_with_capture ({} bytes/call): \
             {:.1} allocs/call, {:.1} bytes/call",
            data.len(),
            (count1 - count0) as f64 / ITERS as f64,
            (bytes1 - bytes0) as f64 / ITERS as f64,
        );
    }

    // `build_rows`, via `to_terminal`.
    {
        let mut session = spawn_idle_session(COLS, ROWS, 2_000);
        session.feed_for_bench(&cat_storm_bytes(4_000));
        let _ = black_box(session.to_terminal(WidgetId::new("warm"), None)); // warm up
        let (count0, bytes0) = alloc_snapshot();
        for _ in 0..ITERS {
            black_box(session.to_terminal(WidgetId::new("bench-terminal"), None));
        }
        let (count1, bytes1) = alloc_snapshot();
        eprintln!(
            "terminal_throughput: build_rows (to_terminal, {COLS}x{ROWS} grid): \
             {:.1} allocs/call, {:.1} bytes/call",
            (count1 - count0) as f64 / ITERS as f64,
            (bytes1 - bytes0) as f64 / ITERS as f64,
        );
    }
}

fn main() {
    report_allocations_per_frame();

    let mut criterion = Criterion::default().configure_from_args();
    bench_cat_storm(&mut criterion);
    bench_yes_flood(&mut criterion);
    bench_sgr_heavy(&mut criterion);
    bench_resize_storm(&mut criterion);
    bench_snapshot_build(&mut criterion);
    criterion.final_summary();
}
