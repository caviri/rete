//! `--progress`: a live view of a long `rete build`, `rete merge` or
//! `rete recard` on stderr — the step it is in and how many remain, how far
//! through the step, the speed, the running totals, memory and spill, the
//! elapsed time, and when it should finish.
//!
//! The builders publish where they are through [`rete_core::build_progress`]
//! (process-wide atomics, written only while a listener lives). This module
//! starts the listener and a renderer thread that samples them; the build's
//! own thread never formats or writes a progress line, so a build with
//! progress writes the same file as one without, at the same speed.
//!
//! Four renderings:
//!
//! - **bar** — one line redrawn in place a few times a second. On Unix the
//!   process's stderr is routed through a pipe the renderer reads, so the
//!   lines the build prints (`extbuild: …`, `merge: …`) land *above* the bar
//!   instead of through it.
//! - **plain** — one line every `--progress-interval` seconds, and one when a
//!   step begins: for Docker and CI logs.
//! - **json** — the same events as one JSON object per line (see
//!   `docs/cli.md`, "Progress"), for scripts.
//! - **off** — nothing.
//!
//! `auto`, the default, is `bar` when stderr is a terminal and `off`
//! otherwise, so logs do not change unless asked.
//!
//! **The ETA.** The step being run is timed by its own measured rate. The
//! steps still to come are priced by a per-step model in units of the parse
//! step's duration — the `rete estimate` model ("a build is 3–5× its parse")
//! split per step and fitted to recorded builds — so it recalibrates itself
//! with the speed the parse actually reaches. Until the parse is 5% done the
//! ETA is labelled an estimate.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rete_core::build_progress::{self as core, Snapshot, Unit};
use rete_core::build_trace::Phase;

// ---------------------------------------------------------------------------
// flags
// ---------------------------------------------------------------------------

/// `--progress` and `--progress-interval`, shared by `build`, `merge` and
/// `recard`.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct ProgressArgs {
    /// Show progress on stderr while the command runs: `auto` (default: a
    /// live bar when stderr is a terminal, nothing otherwise), `bar`, `plain`
    /// (one line every `--progress-interval` seconds, for logs), `json` (one
    /// JSON object per line, for scripts) or `off`. Read from the
    /// `RETE_PROGRESS` environment variable when not given. Stdout and the
    /// file written are the same with or without it.
    #[arg(long, value_name = "MODE", value_parser = ["auto", "bar", "plain", "json", "off"])]
    pub progress: Option<String>,
    /// Seconds between two `plain` or `json` progress lines.
    #[arg(
        long = "progress-interval",
        value_name = "SECONDS",
        default_value_t = 30.0
    )]
    pub progress_interval: f64,
}

/// What `--progress` resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    Bar,
    Plain,
    Json,
}

/// The mode for `flag` (the `--progress` value), else `env` (`RETE_PROGRESS`),
/// else `auto`; `auto` draws the bar only on a terminal.
pub(crate) fn resolve_mode(
    flag: Option<&str>,
    env: Option<&str>,
    tty: bool,
) -> anyhow::Result<Mode> {
    let (raw, from_env) = match flag {
        Some(f) => (f, false),
        None => (env.unwrap_or(""), true),
    };
    Ok(match raw.trim() {
        "" | "auto" => {
            if tty {
                Mode::Bar
            } else {
                Mode::Off
            }
        }
        "bar" => Mode::Bar,
        "plain" => Mode::Plain,
        "json" => Mode::Json,
        "off" => Mode::Off,
        other if from_env => {
            anyhow::bail!("RETE_PROGRESS={other}: expected auto, bar, plain, json or off")
        }
        other => anyhow::bail!("--progress {other}: expected auto, bar, plain, json or off"),
    })
}

impl ProgressArgs {
    /// Start showing progress for `command` (`build`, `merge`, `recard`); it
    /// stops, with a summary line, when the returned session is dropped.
    pub(crate) fn start(&self, command: &'static str) -> anyhow::Result<Session> {
        let env = std::env::var("RETE_PROGRESS").ok();
        let mode = resolve_mode(
            self.progress.as_deref(),
            env.as_deref(),
            std::io::stderr().is_terminal(),
        )?;
        let interval = self.progress_interval;
        if !(interval.is_finite() && interval > 0.0) {
            anyhow::bail!("--progress-interval {interval}: give a number of seconds above 0");
        }
        Ok(Session::start(
            mode,
            command,
            Duration::from_secs_f64(interval.max(0.05)),
        ))
    }
}

// ---------------------------------------------------------------------------
// what a command tells the renderer
// ---------------------------------------------------------------------------

/// Which build path runs — it decides the step model the ETA prices the
/// steps to come with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `rete build`, N-Triples/N-Quads files: two streaming parse passes.
    InRamStream,
    /// `rete build` from Turtle, TriG, RDF/XML or stdin, or with reasoning.
    InRamText,
    /// `rete build --memory-budget-mb`.
    Bounded,
    /// `rete merge`.
    Merge,
    /// `rete recard`.
    Recard,
}

/// The steps a run expects and how to price them.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub kind: Kind,
    /// The steps, in the order they run. A step that does not run after all
    /// is dropped from the count once a later one begins; one that runs
    /// unplanned is added.
    pub phases: Vec<Phase>,
    /// Index permutations written (3 or 6).
    pub perms: usize,
    /// `--label-index`: the label pass is a real cost in the bounded build.
    pub label_index: bool,
}

struct Shared {
    plan: Option<Plan>,
    summary: Option<Summary>,
    /// The bar is drawn: [`finish`] has it cleared before the command
    /// prints its result to stdout, which may be the same terminal.
    bar: bool,
    /// [`finish`] asked the bar to stand down, and the renderer has.
    hold: bool,
    held: bool,
}

/// What the final line reports.
#[derive(Clone, Debug)]
struct Summary {
    statements: u64,
    output: String,
    output_bytes: u64,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    plan: None,
    summary: None,
    bar: false,
    hold: false,
    held: false,
});
/// Signalled when the renderer has cleared the bar for [`finish`].
static HELD: std::sync::Condvar = std::sync::Condvar::new();

/// Tell the renderer which steps this run expects. A no-op without
/// `--progress`.
pub(crate) fn plan(plan: Plan) {
    if core::listening() {
        if let Ok(mut s) = SHARED.lock() {
            s.plan = Some(plan);
        }
    }
}

/// Record the result for the summary line — the statements written and the
/// output file — and take the bar down, so the command's own result line on
/// stdout does not land on it. Call it just before that line.
pub(crate) fn finish(statements: u64, output: &str) {
    if !core::listening() {
        return;
    }
    let output_bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    let Ok(mut s) = SHARED.lock() else {
        return;
    };
    s.summary = Some(Summary {
        statements,
        output: output.to_string(),
        output_bytes,
    });
    if s.bar {
        s.hold = true;
        // The renderer looks at least every BAR_TICK.
        let _ = HELD.wait_timeout_while(s, Duration::from_secs(1), |s| !s.held);
    }
}

/// The input bytes a parse step will read — its progress total — or `None`
/// when one of them is stdin.
pub(crate) fn input_bytes(inputs: &[String]) -> Option<u64> {
    let mut total = 0u64;
    for i in inputs {
        if i == "-" {
            return None;
        }
        total += std::fs::metadata(i).ok()?.len();
    }
    Some(total)
}

/// A reader that counts what it reads into the current step's progress.
pub(crate) struct CountingRead<R>(pub R);

impl<R: std::io::Read> std::io::Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        core::advance(n as u64);
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// the session
// ---------------------------------------------------------------------------

/// Shows progress while it lives.
pub(crate) struct Session {
    running: Option<Running>,
}

struct Running {
    stop: Arc<AtomicBool>,
    wake: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    _listener: core::Listener,
    /// The real stderr, put back as fd 2 when the bar stops (Unix, bar mode).
    #[cfg(unix)]
    restore: Option<std::os::fd::OwnedFd>,
}

impl Session {
    /// A session that shows nothing.
    pub(crate) fn off() -> Self {
        Session { running: None }
    }

    fn start(mode: Mode, command: &'static str, interval: Duration) -> Self {
        if mode == Mode::Off {
            return Session::off();
        }
        if let Ok(mut s) = SHARED.lock() {
            s.plan = None;
            s.summary = None;
            s.bar = mode == Mode::Bar;
            s.hold = false;
            s.held = false;
        }
        let listener = core::listen();
        let stop = Arc::new(AtomicBool::new(false));
        let (wake, wait) = std::sync::mpsc::channel::<()>();

        // Bar mode on Unix: route fd 2 through a pipe this thread reads, so
        // the build's own lines are printed above the bar.
        #[cfg(unix)]
        let (out, pipe, restore) = match (mode == Mode::Bar).then(intercept::start).flatten() {
            Some((out, pipe, saved)) => (out, Some(pipe), Some(saved)),
            None => (Output::Stderr, None, None),
        };
        #[cfg(not(unix))]
        let (out, pipe) = (Output::Stderr, None::<()>);

        let renderer = Renderer::new(mode, command, interval, out);
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("rete-progress".into())
            .spawn(move || renderer.run(thread_stop, wait, pipe))
            .ok();
        // No renderer, no reader of the pipe: put stderr back at once.
        #[cfg(unix)]
        let restore = match (thread.is_none(), restore) {
            (true, Some(saved)) => {
                let _ = rustix::stdio::dup2_stderr(&saved);
                None
            }
            (_, r) => r,
        };
        Session {
            running: Some(Running {
                stop,
                wake,
                thread,
                _listener: listener,
                #[cfg(unix)]
                restore,
            }),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(mut r) = self.running.take() else {
            return;
        };
        r.stop.store(true, Ordering::Release);
        // Put the real stderr back first: that closes the pipe's last write
        // end, so the renderer drains what is left and sees its end.
        #[cfg(unix)]
        if let Some(saved) = r.restore.take() {
            let _ = rustix::stdio::dup2_stderr(&saved);
        }
        let _ = r.wake.send(());
        if let Some(t) = r.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut s) = SHARED.lock() {
            s.plan = None;
            s.summary = None;
            s.bar = false;
            s.hold = false;
            s.held = false;
        }
    }
}

/// Where the renderer writes.
enum Output {
    /// The process's stderr.
    Stderr,
    /// The real stderr while fd 2 is the pipe (Unix bar mode).
    #[cfg(unix)]
    File(std::fs::File),
}

impl Output {
    fn write_str(&mut self, s: &str) {
        match self {
            Output::Stderr => {
                let mut e = std::io::stderr().lock();
                let _ = e.write_all(s.as_bytes());
                let _ = e.flush();
            }
            #[cfg(unix)]
            Output::File(f) => {
                let _ = f.write_all(s.as_bytes());
            }
        }
    }

    /// Terminal columns, when the output is a terminal that says.
    fn columns(&self) -> Option<usize> {
        #[cfg(unix)]
        {
            use std::os::fd::AsFd;
            let ws = match self {
                Output::Stderr => rustix::termios::tcgetwinsize(std::io::stderr().as_fd()),
                Output::File(f) => rustix::termios::tcgetwinsize(f.as_fd()),
            };
            if let Ok(ws) = ws {
                if ws.ws_col > 0 {
                    return Some(ws.ws_col as usize);
                }
            }
        }
        std::env::var("COLUMNS").ok()?.trim().parse().ok()
    }
}

#[cfg(unix)]
mod intercept {
    //! Route fd 2 through a pipe for the bar (safe `rustix` calls only).
    use super::Output;
    use std::os::fd::{AsFd, OwnedFd};

    /// The real stderr to write to, the pipe's read end, and the real stderr
    /// again to put back as fd 2 at the end. `None` (no interception, the
    /// bar draws over whatever else is printed) if any call fails.
    pub(super) fn start() -> Option<(Output, std::fs::File, OwnedFd)> {
        let stderr = std::io::stderr();
        let saved = rustix::io::dup(stderr.as_fd()).ok()?;
        let out = rustix::io::dup(&saved).ok()?;
        let (read, write) = rustix::pipe::pipe().ok()?;
        // Hold stderr's lock while swapping, so no line is half written.
        let lock = stderr.lock();
        rustix::stdio::dup2_stderr(&write).ok()?;
        drop(lock);
        drop(write);
        Some((
            Output::File(std::fs::File::from(out)),
            std::fs::File::from(read),
            saved,
        ))
    }

    /// Wait up to `timeout` for the pipe to have something to read.
    pub(super) fn readable(pipe: &std::fs::File, timeout: std::time::Duration) -> bool {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let t = Timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };
        let mut fds = [PollFd::new(pipe, PollFlags::IN | PollFlags::HUP)];
        matches!(poll(&mut fds, Some(&t)), Ok(n) if n > 0)
    }
}

// ---------------------------------------------------------------------------
// the renderer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Running,
    Done,
    Skipped,
}

#[derive(Clone, Debug)]
struct Step {
    phase: Phase,
    state: State,
    started: Option<Instant>,
    secs: Option<f64>,
}

/// The steps of a run as the samples reveal them — the plan, corrected by
/// what the builders actually begin.
#[derive(Default)]
struct Steps {
    steps: Vec<Step>,
    cur: Option<usize>,
    /// Entries of the builders' begin log already folded in.
    seen: usize,
}

impl Steps {
    fn from_plan(phases: &[Phase]) -> Self {
        Steps {
            steps: phases
                .iter()
                .map(|&phase| Step {
                    phase,
                    state: State::Pending,
                    started: None,
                    secs: None,
                })
                .collect(),
            cur: None,
            seen: 0,
        }
    }

    /// Fold in the builders' begin log (every phase begun, with when, in
    /// order — so a phase that came and went between two samples is still
    /// counted and timed). A planned step passed over is skipped: this run
    /// does not take it. A phase nobody planned is added after the current
    /// step. Returns the steps that ended, in order.
    fn observe(&mut self, log: &[(Phase, Instant)]) -> Vec<usize> {
        let mut ended = Vec::new();
        for &(phase, at) in log.iter().skip(self.seen) {
            if let Some(c) = self.cur {
                let step = &mut self.steps[c];
                step.state = State::Done;
                step.secs = step
                    .started
                    .map(|t| at.saturating_duration_since(t).as_secs_f64());
                ended.push(c);
            }
            let after = self.cur.map_or(0, |c| c + 1);
            let pending = |s: &Step| s.phase == phase && s.state == State::Pending;
            let i = match self.steps[after..].iter().position(pending) {
                Some(j) => {
                    for k in after..after + j {
                        if self.steps[k].state == State::Pending {
                            self.steps[k].state = State::Skipped;
                        }
                    }
                    after + j
                }
                None => {
                    self.steps.insert(
                        after,
                        Step {
                            phase,
                            state: State::Pending,
                            started: None,
                            secs: None,
                        },
                    );
                    after
                }
            };
            self.steps[i].state = State::Running;
            self.steps[i].started = Some(at);
            self.cur = Some(i);
        }
        self.seen = log.len();
        ended
    }

    /// End the running step (the run is over).
    fn close(&mut self, now: Instant) {
        if let Some(c) = self.cur {
            if self.steps[c].state == State::Running {
                self.steps[c].state = State::Done;
                self.steps[c].secs = self.steps[c].started.map(|t| (now - t).as_secs_f64());
            }
        }
    }

    /// `(k, n)`: the current step's 1-based position among the steps this
    /// run takes, and how many it takes.
    fn position(&self) -> (usize, usize) {
        let counted = |s: &&Step| s.state != State::Skipped;
        let n = self.steps.iter().filter(counted).count();
        let k = match self.cur {
            Some(c) => self.steps[..=c].iter().filter(counted).count(),
            None => 0,
        };
        (k, n.max(k))
    }

    fn get(&self, phase: Phase) -> Option<&Step> {
        self.steps.iter().find(|s| s.phase == phase)
    }
}

/// An exponentially weighted rate, in units per second. Until it has seen
/// its time constant's worth of samples it is the plain average since it
/// started, so it does not ramp up from its first sample.
#[derive(Default, Clone, Copy)]
struct Ewma {
    rate: Option<f64>,
    /// Seconds sampled so far.
    age: f64,
}

impl Ewma {
    /// Time constant: about the last ten seconds weigh most.
    const TAU: f64 = 10.0;

    fn sample(&mut self, delta: f64, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        let inst = delta / dt;
        let a = (dt / (self.age + dt)).max(1.0 - (-dt / Self::TAU).exp());
        self.rate = Some(match self.rate {
            None => inst,
            Some(r) => r + a * (inst - r),
        });
        self.age += dt;
    }
}

/// The figures one line shows.
#[derive(Clone, Debug, Default)]
struct View {
    step: usize,
    steps: usize,
    phase: Option<Phase>,
    done: u64,
    total: Option<u64>,
    unit: Unit,
    percent: Option<f64>,
    rate: Option<f64>,
    mb_per_s: Option<f64>,
    statements_per_s: Option<f64>,
    statements: u64,
    terms: u64,
    chunks: u64,
    rss: Option<u64>,
    peak_rss: Option<u64>,
    memory_limit: Option<u64>,
    spill: Option<u64>,
    spill_peak: Option<u64>,
    elapsed: f64,
    eta: Option<f64>,
    eta_rough: bool,
    finish_epoch: Option<i64>,
}

struct Renderer {
    mode: Mode,
    command: &'static str,
    interval: Duration,
    out: Output,
    started: Instant,
    started_epoch: f64,
    plan: Option<Plan>,
    steps: Steps,
    last_seq: u64,
    /// The last sample's `(time, phase seq, done, statements)`.
    last: Option<(Instant, u64, u64, u64)>,
    rate: Ewma,
    statements_rate: Ewma,
    /// When the statement count last grew: its rate is shown only while it
    /// does (the parse), not frozen through the steps after it.
    statements_moved: Option<Instant>,
    memory_limit: Option<u64>,
    spill: Option<u64>,
    spill_peak: u64,
    spill_walked: Option<Instant>,
    last_line: Option<Instant>,
    start_shown: bool,
    bar_shown: bool,
    /// A sample panicked: show nothing more.
    broken: bool,
    spinner: usize,
    /// What the build wrote to stderr past its last newline (Unix bar mode).
    #[cfg_attr(not(unix), allow(dead_code))]
    pending: Vec<u8>,
}

/// How often the bar is redrawn, and the plain/json modes sample.
const BAR_TICK: Duration = Duration::from_millis(200);
const LINE_TICK: Duration = Duration::from_millis(1000);
/// How often the spill directory is measured.
const SPILL_EVERY: Duration = Duration::from_secs(2);

impl Renderer {
    fn new(mode: Mode, command: &'static str, interval: Duration, out: Output) -> Self {
        Renderer {
            mode,
            command,
            interval,
            out,
            started: Instant::now(),
            started_epoch: now_epoch(),
            plan: None,
            steps: Steps::default(),
            last_seq: 0,
            last: None,
            rate: Ewma::default(),
            statements_rate: Ewma::default(),
            statements_moved: None,
            memory_limit: super::buildinfo::memory_limit_bytes(),
            spill: None,
            spill_peak: 0,
            spill_walked: None,
            last_line: None,
            start_shown: false,
            bar_shown: false,
            broken: false,
            spinner: 0,
            pending: Vec::new(),
        }
    }

    fn tick(&self) -> Duration {
        match self.mode {
            Mode::Bar => BAR_TICK,
            _ => LINE_TICK.min(self.interval),
        }
    }

    #[cfg(unix)]
    fn run(
        mut self,
        stop: Arc<AtomicBool>,
        wait: std::sync::mpsc::Receiver<()>,
        pipe: Option<std::fs::File>,
    ) {
        match pipe {
            Some(mut pipe) => loop {
                let stopping = stop.load(Ordering::Acquire);
                let timeout = if stopping {
                    Duration::ZERO
                } else {
                    self.tick()
                };
                let eof = if intercept::readable(&pipe, timeout) {
                    self.pass_through(&mut pipe)
                } else {
                    stopping
                };
                if eof {
                    self.flush_partial();
                    break;
                }
                self.safe_update();
            },
            None => self.run_plain(stop, wait),
        }
        self.end();
    }

    #[cfg(not(unix))]
    fn run(
        mut self,
        stop: Arc<AtomicBool>,
        wait: std::sync::mpsc::Receiver<()>,
        _pipe: Option<()>,
    ) {
        self.run_plain(stop, wait);
        self.end();
    }

    fn run_plain(&mut self, stop: Arc<AtomicBool>, wait: std::sync::mpsc::Receiver<()>) {
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            self.safe_update();
            let _ = wait.recv_timeout(self.tick());
        }
    }

    /// Read what the build wrote to stderr and print its complete lines
    /// above the bar. True at the pipe's end.
    #[cfg(unix)]
    fn pass_through(&mut self, pipe: &mut std::fs::File) -> bool {
        use std::io::Read;
        let mut buf = [0u8; 1 << 16];
        let n = match pipe.read(&mut buf) {
            Ok(0) => return true,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return false,
            Err(_) => return true,
        };
        self.pending.extend_from_slice(&buf[..n]);
        if let Some(last_nl) = self.pending.iter().rposition(|&b| b == b'\n') {
            let lines: Vec<u8> = self.pending.drain(..=last_nl).collect();
            self.clear_bar();
            self.out.write_str(&String::from_utf8_lossy(&lines));
            self.redraw();
        }
        false
    }

    /// Print a last line the build left without its newline.
    #[cfg(unix)]
    fn flush_partial(&mut self) {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.clear_bar();
            self.out.write_str(&String::from_utf8_lossy(&rest));
            self.out.write_str("\n");
        }
    }

    fn sample(&mut self) -> Snapshot {
        if self.plan.is_none() {
            let plan = SHARED.lock().ok().and_then(|s| s.plan.clone());
            if let Some(p) = plan {
                // The commands give their plan before their first step, so
                // nothing observed so far is lost by starting over.
                self.steps = Steps::from_plan(&p.phases);
                self.plan = Some(p);
                self.last_seq = 0;
            }
        }
        core::snapshot()
    }

    /// Take a sample, fold it in, and show what is due.
    fn update(&mut self) {
        let now = Instant::now();
        let snap = self.sample();
        let mut ended = Vec::new();
        let phase_changed = snap.seq != self.last_seq;
        if phase_changed {
            ended = self.steps.observe(&snap.begun);
            self.last_seq = snap.seq;
            self.rate = Ewma::default();
        }
        // Rates over this sample.
        if let Some((t, seq, done, st)) = self.last {
            let dt = (now - t).as_secs_f64();
            if seq == snap.seq && snap.done >= done {
                self.rate.sample((snap.done - done) as f64, dt);
            }
            if snap.statements > st {
                self.statements_rate
                    .sample((snap.statements - st) as f64, dt);
                self.statements_moved = Some(now);
            }
        }
        self.last = Some((now, snap.seq, snap.done, snap.statements));
        if snap.spill_dir.is_some() && self.spill_walked.is_none_or(|t| now - t >= SPILL_EVERY) {
            let bytes = snap.spill_dir.as_deref().map(dir_bytes).unwrap_or(0);
            self.spill = Some(bytes);
            self.spill_peak = self.spill_peak.max(bytes);
            self.spill_walked = Some(now);
        } else if snap.spill_dir.is_none() && self.spill.is_some() {
            self.spill = Some(0);
        }

        let view = self.view(&snap, now);
        match self.mode {
            Mode::Bar => {
                for i in ended {
                    self.step_done_line(i);
                }
                self.draw(&view);
            }
            Mode::Plain | Mode::Json => {
                if !self.start_shown && (self.plan.is_some() || snap.phase.is_some()) {
                    self.emit_start();
                    self.start_shown = true;
                }
                let due = self.last_line.is_some_and(|t| now - t >= self.interval);
                if phase_changed && snap.phase.is_some() {
                    self.emit("phase", &view, &ended);
                    self.last_line = Some(now);
                } else if due && snap.phase.is_some() {
                    self.emit("progress", &view, &[]);
                    self.last_line = Some(now);
                }
            }
            Mode::Off => {}
        }
    }

    fn view(&self, snap: &Snapshot, now: Instant) -> View {
        let (step, steps) = self.steps.position();
        let percent = snap
            .total
            .filter(|&t| t > 0)
            .map(|t| (snap.done.min(t) as f64 / t as f64 * 100.0).clamp(0.0, 100.0))
            .or_else(|| snap.total.map(|_| 100.0));
        let (rss, peak_rss) = rss();
        let elapsed = (now - self.started).as_secs_f64();
        let (eta, eta_rough) = match self.eta(snap, now) {
            Some((e, r)) => (Some(e), r),
            None => (None, true),
        };
        View {
            step,
            steps,
            phase: snap.phase,
            done: snap.done,
            total: snap.total,
            unit: snap.unit,
            percent,
            rate: self.rate.rate,
            mb_per_s: (snap.unit == Unit::Bytes)
                .then_some(self.rate.rate)
                .flatten()
                .map(|r| r / 1e6),
            statements_per_s: self.statements_rate.rate.filter(|r| *r > 0.0).filter(|_| {
                self.statements_moved
                    .is_some_and(|t| now - t < Duration::from_secs(3))
            }),
            statements: snap.statements,
            terms: snap.terms,
            chunks: snap.chunks,
            rss,
            peak_rss,
            memory_limit: self.memory_limit,
            spill: self.spill,
            spill_peak: (self.spill_peak > 0).then_some(self.spill_peak),
            elapsed,
            eta,
            eta_rough,
            finish_epoch: eta.map(|e| (self.started_epoch + elapsed + e).round() as i64),
        }
    }

    /// The remaining seconds and whether they are only an estimate.
    fn eta(&self, snap: &Snapshot, now: Instant) -> Option<(f64, bool)> {
        let plan = self.plan.as_ref()?;
        let cur = self.steps.cur?;
        let step = &self.steps.steps[cur];
        let in_step = step.started.map_or(0.0, |t| (now - t).as_secs_f64());
        let fraction = snap
            .total
            .filter(|&t| t > 0)
            .map(|t| snap.done.min(t) as f64 / t as f64);
        // The reference: the parse step's duration, measured or projected.
        // A projection is an estimate until 5% of the parse is behind it —
        // and, in the external builder, until a chunk has been sealed: the
        // seals stall the parse, and the rate before the first one has none.
        let ref_phase = reference_phase(plan.kind);
        let (reference, mut rough) = match self.steps.get(ref_phase) {
            Some(s) if s.state == State::Done => (s.secs?, false),
            Some(s) if s.state == State::Running => {
                let f = fraction.filter(|f| *f > 0.0)?;
                let unsealed = matches!(plan.kind, Kind::Bounded | Kind::Merge) && snap.chunks == 0;
                (in_step / f, f < 0.05 || unsealed)
            }
            _ => return None,
        };
        // This step: its own rate once it has one, else the model — and once
        // a step with nothing to count runs past the model, a quarter of its
        // time so far again, as an estimate.
        let here = match fraction {
            Some(f) if f > 0.0 && in_step >= 1.0 => in_step / f - in_step,
            _ => {
                let model = weight(plan, step.phase) * reference;
                if in_step > model && snap.total.is_none() {
                    rough = true;
                }
                (model - in_step).max(0.25 * in_step)
            }
        };
        let rest: f64 = self.steps.steps[cur + 1..]
            .iter()
            .filter(|s| s.state == State::Pending)
            .map(|s| weight(plan, s.phase) * reference)
            .sum();
        Some(((here + rest).max(0.0), rough))
    }

    fn step_label(&self, phase: Phase) -> &'static str {
        label(phase, self.plan.as_ref().map(|p| p.kind))
    }

    fn step_done_line(&mut self, i: usize) {
        let (n, label, secs) = {
            let counted = |s: &&Step| s.state != State::Skipped;
            let k = self.steps.steps[..=i].iter().filter(counted).count();
            let n = self.steps.steps.iter().filter(counted).count();
            let s = &self.steps.steps[i];
            (
                format!("{k}/{n}"),
                self.step_label(s.phase),
                s.secs.unwrap_or(0.0),
            )
        };
        self.clear_bar();
        self.out
            .write_str(&format!("  done [{n}] {label} in {}\n", fmt_duration(secs)));
    }

    // ---- the bar ----

    fn clear_bar(&mut self) {
        if self.bar_shown {
            self.out.write_str("\r\x1b[2K");
            self.bar_shown = false;
        }
    }

    #[cfg(unix)]
    fn redraw(&mut self) {
        if self.mode == Mode::Bar {
            self.safe_update();
        }
    }

    /// [`Self::update`], and if it ever panics, no more progress: the build
    /// goes on, and so does the pass-through of its own lines.
    fn safe_update(&mut self) {
        if self.broken {
            return;
        }
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.update())).is_ok();
        if !ok {
            self.broken = true;
            self.bar_shown = false;
        }
    }

    fn draw(&mut self, v: &View) {
        let Some(phase) = v.phase else {
            return;
        };
        if let Ok(mut s) = SHARED.lock() {
            if s.hold {
                if !s.held {
                    self.clear_bar();
                    s.held = true;
                    HELD.notify_all();
                }
                return;
            }
        }
        self.spinner = (self.spinner + 1) % SPINNER.len();
        let width = self.out.columns().unwrap_or(100).max(20) - 1;
        let label = self.step_label(phase);
        // Each segment with the priority it keeps its place by (lowest is
        // dropped first when the terminal is narrow).
        let mut segs: Vec<(u8, String)> = vec![(9, format!("[{}/{}] {label}", v.step, v.steps))];
        segs.push((
            8,
            match v.percent {
                Some(p) => format!("{} {p:>5.1}%", bar(p, 20)),
                None => format!(
                    "{} {}",
                    SPINNER[self.spinner],
                    count_with_unit(v.done, v.unit)
                ),
            },
        ));
        if let Some(speed) = speed(v) {
            segs.push((6, speed));
        }
        let totals = totals(v);
        if !totals.is_empty() {
            segs.push((3, totals));
        }
        if let Some(r) = v.rss {
            segs.push((4, format!("RAM {}", ram(r, v.memory_limit))));
        }
        if let Some(s) = v.spill.filter(|s| *s > 0) {
            segs.push((2, format!("spill {}", fmt_bytes(s))));
        }
        segs.push((7, fmt_clock_elapsed(v.elapsed)));
        if let Some(e) = v.eta {
            let tilde = if v.eta_rough { "~" } else { "" };
            segs.push((7, format!("ETA {tilde}{}", fmt_duration(e))));
            if let Some(f) = v.finish_epoch {
                segs.push((5, format!("≈ {}", fmt_finish(f))));
            }
        }
        let line = fit(segs, width);
        self.out.write_str(&format!("\r\x1b[2K{line}"));
        self.bar_shown = true;
    }

    // ---- plain and json ----

    fn emit_start(&mut self) {
        let phases: Vec<&'static str> = self
            .plan
            .as_ref()
            .map(|p| p.phases.iter().map(|p| p.as_str()).collect())
            .unwrap_or_default();
        match self.mode {
            Mode::Json => {
                let o = serde_json::json!({
                    "event": "start",
                    "command": self.command,
                    "t": 0.0,
                    "steps": phases.len(),
                    "phases": phases,
                    "memory_limit_bytes": self.memory_limit,
                });
                self.out.write_str(&format!("{o}\n"));
            }
            _ => {
                if !phases.is_empty() {
                    self.out.write_str(&format!(
                        "progress: {} in {} steps: {}\n",
                        self.command,
                        phases.len(),
                        phases.join(", ")
                    ));
                }
            }
        }
    }

    fn emit(&mut self, event: &str, v: &View, ended: &[usize]) {
        let ended: Vec<(&'static str, f64)> = ended
            .iter()
            .map(|&i| {
                let s = &self.steps.steps[i];
                (s.phase.as_str(), s.secs.unwrap_or(0.0))
            })
            .collect();
        match self.mode {
            Mode::Json => {
                let mut o = json_view(event, self.command, v);
                if event == "phase" {
                    o["ended"] = ended
                        .iter()
                        .map(|(p, secs)| serde_json::json!({"phase": p, "seconds": round3(*secs)}))
                        .collect();
                }
                self.out.write_str(&format!("{o}\n"));
            }
            _ => {
                let mut line = format!("progress: {} {}", self.command, plain_view(v));
                if !ended.is_empty() {
                    let done: Vec<String> = ended
                        .iter()
                        .map(|(p, secs)| format!("{p} {}", fmt_duration(*secs)))
                        .collect();
                    line.push_str(&format!(" (after {})", done.join(", ")));
                }
                line.push('\n');
                self.out.write_str(&line);
            }
        }
    }

    /// The last line: the summary, or where the run stopped.
    fn end(mut self) {
        if self.broken {
            return;
        }
        let now = Instant::now();
        let snap = self.sample();
        if snap.seq != self.last_seq {
            let ended = self.steps.observe(&snap.begun);
            if self.mode == Mode::Bar {
                for i in ended {
                    self.step_done_line(i);
                }
            }
        }
        let view = self.view(&snap, now);
        let failed_at = self.steps.cur.map(|c| self.steps.steps[c].phase);
        self.steps.close(now);
        if self.mode == Mode::Bar {
            if let Some(c) = self.steps.cur {
                self.step_done_line(c);
            }
        }
        let summary = SHARED.lock().ok().and_then(|s| s.summary.clone());
        let elapsed = view.elapsed;
        let peak = rss().1;
        match (self.mode, summary) {
            (Mode::Json, s) => {
                let o = match s {
                    Some(s) => serde_json::json!({
                        "event": "done",
                        "command": self.command,
                        "t": round3(elapsed),
                        "elapsed_s": round3(elapsed),
                        "statements": s.statements,
                        "peak_rss_bytes": peak,
                        "output": s.output,
                        "output_bytes": s.output_bytes,
                        "spill_peak_bytes": (self.spill_peak > 0).then_some(self.spill_peak),
                    }),
                    None => serde_json::json!({
                        "event": "failed",
                        "command": self.command,
                        "t": round3(elapsed),
                        "elapsed_s": round3(elapsed),
                        "step": view.step,
                        "steps": view.steps,
                        "phase": failed_at.map(Phase::as_str),
                        "peak_rss_bytes": peak,
                    }),
                };
                self.out.write_str(&format!("{o}\n"));
            }
            (_, Some(s)) => {
                self.clear_bar();
                let prefix = if self.mode == Mode::Plain {
                    "progress: "
                } else {
                    ""
                };
                let mut line = format!(
                    "{prefix}{} done in {}: {} statements",
                    self.command,
                    fmt_duration(elapsed),
                    group_digits(s.statements)
                );
                if let Some(p) = peak {
                    line.push_str(&format!(", peak RAM {}", fmt_bytes(p)));
                }
                if self.spill_peak > 0 {
                    line.push_str(&format!(", spill peak {}", fmt_bytes(self.spill_peak)));
                }
                line.push_str(&format!(
                    ", output {} ({})\n",
                    fmt_decimal_bytes(s.output_bytes),
                    s.output
                ));
                self.out.write_str(&line);
            }
            (_, None) => {
                self.clear_bar();
                let prefix = if self.mode == Mode::Plain {
                    "progress: "
                } else {
                    ""
                };
                let at = failed_at
                    .map(|p| format!(" in step {}/{} {}", view.step, view.steps, p.as_str()))
                    .unwrap_or_default();
                self.out.write_str(&format!(
                    "{prefix}{} stopped after {}{at}\n",
                    self.command,
                    fmt_duration(elapsed)
                ));
            }
        }
    }
}

/// The step whose measured duration prices the others.
fn reference_phase(kind: Kind) -> Phase {
    match kind {
        Kind::InRamStream => Phase::ParseDictionary,
        Kind::InRamText => Phase::Parse,
        Kind::Bounded | Kind::Merge => Phase::ParseChunk,
        Kind::Recard => Phase::CardDerive,
    }
}

/// A step's expected duration, in units of the reference step's.
///
/// `rete estimate` prices a whole build at 3–5× its parse-only pass; these
/// split that per step. Fitted to recorded builds (the `run.phases` of their
/// build records):
///
/// - **in RAM**: the OpenCitations M=1200 sample with provenance (18.9 M
///   statements, six layouts): the second parse pass 1.2× the first, the card
///   0.45–1.6× (0.9 taken), the six permutations 0.11×, the text index
///   0.1–0.75×;
/// - **memory-bounded**: OpenCitations Meta v9.2.1 (5.71 B statements,
///   `--memory-budget-mb 12000`): dictionary merge 0.05× the parse+chunk
///   step, remap 0.02×, text index 0.03×, the six permutations 0.17×, the
///   label pass 0.08×;
/// - **merge**: no merge has a record to fit; its walk of the shards is taken
///   to cost what a parse does, so the steps after it weigh as in a build;
/// - **recard**: the card derivation is the reference.
fn weight(plan: &Plan, phase: Phase) -> f64 {
    let perms = plan.perms.max(1) as f64 / 6.0;
    let bounded = |phase: Phase| match phase {
        Phase::ParseChunk => 1.0,
        Phase::FrequentObjects => 0.03,
        Phase::Dictionary => 0.05,
        Phase::Encode => 0.025,
        Phase::TextIndex => 0.03,
        Phase::IndexPermutations => 0.17 * perms,
        Phase::NamedGraphs => 0.08 * perms,
        Phase::Write => 0.01,
        Phase::LabelsSchema => {
            if plan.label_index {
                0.08
            } else {
                0.02
            }
        }
        Phase::CardDerive => 0.035,
        Phase::StarterQueries => 0.01,
        Phase::ContentHash => 0.005,
        Phase::Read => 0.02,
        _ => 0.02,
    };
    match plan.kind {
        Kind::InRamStream | Kind::InRamText => match phase {
            Phase::ParseDictionary | Phase::Parse => 1.0,
            Phase::ParseEncode => 1.2,
            Phase::Reason => 1.0,
            Phase::Dictionary => 0.3,
            Phase::Encode => 0.3,
            Phase::CardDerive => 0.9,
            Phase::Pyramid => 0.5,
            Phase::TextIndex => 0.4,
            Phase::LabelsSchema => 0.05,
            Phase::DictionarySection => 0.015,
            Phase::IndexPermutations => 0.11 * perms,
            Phase::Serialize => 0.03,
            Phase::StarterQueries => 0.05,
            Phase::Write => 0.01,
            _ => 0.02,
        },
        Kind::Bounded => bounded(phase),
        Kind::Merge => match phase {
            Phase::Read => 0.1,
            p => bounded(p),
        },
        Kind::Recard => match phase {
            Phase::Read => 0.0,
            Phase::CardDerive => 1.0,
            Phase::StarterQueries => 0.1,
            Phase::Write => 0.5,
            Phase::ContentHash => 0.2,
            _ => 0.05,
        },
    }
}

/// The name a step goes by on the bar (`plain` and `json` print the build
/// record's names).
fn label(phase: Phase, kind: Option<Kind>) -> &'static str {
    let external = matches!(kind, Some(Kind::Bounded | Kind::Merge));
    match phase {
        Phase::Read => "read",
        Phase::Parse => "parse",
        Phase::Reason => "reason",
        Phase::ParseDictionary => "parse, pass 1 (dictionary)",
        Phase::ParseEncode => "parse, pass 2 (encode)",
        Phase::ParseChunk if kind == Some(Kind::Merge) => "read shards + spill chunks",
        Phase::ParseChunk => "parse + spill chunks",
        Phase::FrequentObjects => "frequent objects",
        Phase::Dictionary if external => "merge dictionaries",
        Phase::Dictionary => "dictionary",
        Phase::Encode if external => "remap chunks",
        Phase::Encode => "encode",
        Phase::CardDerive => "card",
        Phase::Pyramid => "pyramid",
        Phase::TextIndex => "text index",
        Phase::LabelsSchema => "labels + schema",
        Phase::DictionarySection => "dictionary section",
        Phase::IndexPermutations => "index permutations",
        Phase::NamedGraphs => "named graphs",
        Phase::Serialize => "serialize",
        Phase::StarterQueries => "starter queries",
        Phase::Write => "write",
        Phase::ContentHash => "content hash",
    }
}

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

/// A `width`-cell bar at `percent`, in eighths.
fn bar(percent: f64, width: usize) -> String {
    const PARTS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = ((percent.clamp(0.0, 100.0) / 100.0) * (width * 8) as f64).round() as usize;
    let mut s = String::with_capacity(width + 2);
    s.push('▕');
    for i in 0..width {
        let cell = eighths.saturating_sub(i * 8).min(8);
        s.push(if cell == 8 { '█' } else { PARTS[cell] });
    }
    s.push('▏');
    s
}

/// Keep the segments that fit `width`, dropping the lowest priority first.
fn fit(mut segs: Vec<(u8, String)>, width: usize) -> String {
    let len = |segs: &[(u8, String)]| -> usize {
        segs.iter().map(|(_, s)| s.chars().count()).sum::<usize>()
            + 2 * segs.len().saturating_sub(1)
    };
    while len(&segs) > width && segs.len() > 1 {
        let (i, _) = segs
            .iter()
            .enumerate()
            .min_by_key(|(i, (p, _))| (*p, usize::MAX - i))
            .expect("non-empty");
        segs.remove(i);
    }
    let line = segs
        .into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join("  ");
    line.chars().take(width).collect()
}

fn speed(v: &View) -> Option<String> {
    match v.unit {
        Unit::Bytes => {
            let mut s = format!("{:.1} MB/s", v.mb_per_s.filter(|r| *r > 0.0)?);
            if let Some(st) = v.statements_per_s {
                s.push_str(&format!(", {}/s", fmt_count_unit(st as u64, "st")));
            }
            Some(s)
        }
        Unit::None => None,
        u => Some(format!(
            "{}/s",
            fmt_count_unit(v.rate.filter(|r| *r >= 1.0)? as u64, unit_short(u))
        )),
    }
}

fn unit_short(u: Unit) -> &'static str {
    match u {
        Unit::None => "",
        Unit::Bytes => "B",
        Unit::Statements => "st",
        Unit::Terms => "terms",
        Unit::Records => "rec",
        Unit::Queries => "queries",
    }
}

fn count_with_unit(n: u64, u: Unit) -> String {
    match u {
        Unit::Bytes => fmt_decimal_bytes(n),
        Unit::None => String::new(),
        u => fmt_count_unit(n, unit_short(u)),
    }
}

fn totals(v: &View) -> String {
    let mut parts = Vec::new();
    if v.statements > 0 {
        parts.push(fmt_count_unit(v.statements, "st"));
    }
    if v.terms > 0 {
        parts.push(fmt_count_unit(v.terms, "terms"));
    }
    if v.chunks > 0 {
        parts.push(format!(
            "{} chunk{}",
            v.chunks,
            if v.chunks == 1 { "" } else { "s" }
        ));
    }
    parts.join(", ")
}

fn ram(rss: u64, limit: Option<u64>) -> String {
    match limit {
        Some(l) => format!("{} / {}", fmt_bytes(rss), fmt_bytes(l)),
        None => fmt_bytes(rss),
    }
}

/// The `plain` line after `progress: <command> `.
fn plain_view(v: &View) -> String {
    let phase = v.phase.map(Phase::as_str).unwrap_or("-");
    let mut s = format!("{}/{} {phase}", v.step, v.steps);
    match v.percent {
        Some(p) => s.push_str(&format!(" {p:.1}%")),
        None => {
            let c = count_with_unit(v.done, v.unit);
            if !c.is_empty() {
                s.push_str(&format!(" {c}"));
            }
        }
    }
    if let Some(sp) = speed(v) {
        s.push_str(&format!(" {sp}"));
    }
    let totals = totals(v);
    if !totals.is_empty() {
        s.push_str(&format!(" | {totals}"));
    }
    if let Some(r) = v.rss {
        s.push_str(&format!(" | RAM {}", fmt_bytes(r)));
        if let Some(p) = v.peak_rss {
            s.push_str(&format!(", peak {}", fmt_bytes(p)));
        }
        if let Some(l) = v.memory_limit {
            s.push_str(&format!(" of {}", fmt_bytes(l)));
        }
    }
    if let Some(sp) = v.spill.filter(|sp| *sp > 0 || v.spill_peak.is_some()) {
        s.push_str(&format!(" | spill {}", fmt_bytes(sp)));
        if let Some(pk) = v.spill_peak.filter(|p| *p > sp) {
            s.push_str(&format!(", peak {}", fmt_bytes(pk)));
        }
    }
    s.push_str(&format!(" | {} elapsed", fmt_duration(v.elapsed)));
    if let Some(e) = v.eta {
        if v.eta_rough {
            s.push_str(&format!(", ETA ~{} (estimate)", fmt_duration(e)));
        } else {
            s.push_str(&format!(", ETA {}", fmt_duration(e)));
        }
        if let Some(f) = v.finish_epoch {
            s.push_str(&format!(" ≈ {}", fmt_finish(f)));
        }
    }
    s
}

/// One `json` object (see `docs/cli.md` for the fields).
fn json_view(event: &str, command: &str, v: &View) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "command": command,
        "t": round3(v.elapsed),
        "step": v.step,
        "steps": v.steps,
        "phase": v.phase.map(Phase::as_str),
        "done": v.done,
        "total": v.total,
        "unit": v.unit.as_str(),
        "percent": v.percent.map(|p| (p * 10.0).round() / 10.0),
        "rate": v.rate.map(round3),
        "mb_per_s": v.mb_per_s.map(round3),
        "statements_per_s": v.statements_per_s.map(round3),
        "statements": v.statements,
        "terms": v.terms,
        "chunks": v.chunks,
        "rss_bytes": v.rss,
        "peak_rss_bytes": v.peak_rss,
        "memory_limit_bytes": v.memory_limit,
        "spill_bytes": v.spill,
        "elapsed_s": round3(v.elapsed),
        "eta_s": v.eta.map(round3),
        "eta_estimate": v.eta.map(|_| v.eta_rough),
        "finish_at": v.finish_epoch.map(super::buildinfo::epoch_to_rfc3339),
    })
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

// ---- measurements ----

/// `(VmRSS, VmHWM)` from `/proc/self/status`; `None` where there is none.
fn rss() -> (Option<u64>, Option<u64>) {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let field = |name: &str| {
        s.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Bytes under `dir`, recursively.
fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---- formatting ----

/// `812 MiB`, `4.0 GiB`.
fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// `412.3 MB` — file sizes, as `ls -l --si` and the build's own lines give them.
fn fmt_decimal_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// `5.97 M st`, `812 k terms`, `42 rec`.
fn fmt_count_unit(n: u64, unit: &str) -> String {
    let v = n as f64;
    let num = if n >= 1_000_000_000 {
        format!("{:.2} G", v / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2} M", v / 1e6)
    } else if n >= 10_000 {
        format!("{:.0} k", v / 1e3)
    } else {
        format!("{n}")
    };
    if unit.is_empty() {
        num
    } else {
        format!("{num} {unit}")
    }
}

/// `5,971,936`.
fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `42s`, `3m 02s`, `1h 05m`, `2d 03h`.
fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d {:02}h", s / 86_400, (s % 86_400) / 3600)
    }
}

/// `0:42`, `3:02`, `1:05:09` — the bar's elapsed clock.
fn fmt_clock_elapsed(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }
}

/// `14:32 31 Oct UTC`: the finish time, in UTC (the CLI carries no time-zone
/// database, and the containers it mostly runs in are on UTC anyway).
fn fmt_finish(epoch: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, mo, d, h, m, _) = super::buildinfo::civil_from_epoch(epoch);
    let month = MONTHS[((mo - 1).clamp(0, 11)) as usize];
    format!("{h:02}:{m:02} {d} {month} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_bar_on_a_terminal_and_nothing_elsewhere() {
        assert_eq!(resolve_mode(None, None, true).unwrap(), Mode::Bar);
        assert_eq!(resolve_mode(None, None, false).unwrap(), Mode::Off);
        assert_eq!(resolve_mode(Some("auto"), None, false).unwrap(), Mode::Off);
        assert_eq!(
            resolve_mode(None, Some("plain"), false).unwrap(),
            Mode::Plain
        );
        assert_eq!(
            resolve_mode(Some("off"), Some("json"), true).unwrap(),
            Mode::Off,
            "the flag wins over the variable"
        );
        assert_eq!(resolve_mode(Some("bar"), None, false).unwrap(), Mode::Bar);
        assert!(resolve_mode(None, Some("loud"), false).is_err());
    }

    #[test]
    fn steps_skip_what_does_not_run_and_add_what_was_not_planned() {
        let t = Instant::now();
        let at = |s: u64| t + Duration::from_secs(s);
        let mut s = Steps::from_plan(&[
            Phase::ParseChunk,
            Phase::Dictionary,
            Phase::Encode,
            Phase::NamedGraphs,
            Phase::Write,
        ]);
        let mut log = vec![(Phase::ParseChunk, at(0))];
        s.observe(&log);
        assert_eq!(s.position(), (1, 5));
        // Dictionary came and went between two samples: done and timed,
        // not skipped.
        log.extend([(Phase::Dictionary, at(10)), (Phase::Encode, at(13))]);
        assert_eq!(s.observe(&log), vec![0, 1]);
        assert_eq!(s.steps[1].secs, Some(3.0));
        assert_eq!(s.position(), (3, 5));
        // No named graphs: the step drops out of the count.
        log.push((Phase::Write, at(20)));
        s.observe(&log);
        assert_eq!(s.position(), (4, 4));
        // An unplanned step is added after the current one.
        log.push((Phase::LabelsSchema, at(21)));
        s.observe(&log);
        assert_eq!(s.position(), (5, 5));
        assert!(s.observe(&log).is_empty(), "nothing new: nothing ends");
    }

    #[test]
    fn bar_and_fit() {
        assert_eq!(bar(0.0, 4).chars().count(), 6);
        assert_eq!(bar(100.0, 4), "▕████▏");
        assert_eq!(bar(50.0, 4), "▕██  ▏");
        let line = fit(
            vec![
                (9, "a".repeat(10)),
                (1, "b".repeat(10)),
                (5, "c".repeat(10)),
            ],
            25,
        );
        assert_eq!(line, format!("{}  {}", "a".repeat(10), "c".repeat(10)));
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_duration(42.4), "42s");
        assert_eq!(fmt_duration(182.0), "3m 02s");
        assert_eq!(fmt_duration(3900.0), "1h 05m");
        assert_eq!(fmt_bytes(812 << 20), "812 MiB");
        assert_eq!(fmt_bytes(4 << 30), "4.0 GiB");
        assert_eq!(fmt_count_unit(5_971_936, "st"), "5.97 M st");
        assert_eq!(group_digits(5_971_936), "5,971,936");
        // 2026-10-31T14:32:05Z
        assert_eq!(fmt_finish(1_793_457_125), "14:32 31 Oct UTC");
    }

    #[test]
    fn the_eta_prices_the_steps_to_come_from_the_parse() {
        let plan = Plan {
            kind: Kind::Bounded,
            phases: vec![
                Phase::ParseChunk,
                Phase::Dictionary,
                Phase::IndexPermutations,
            ],
            perms: 6,
            label_index: false,
        };
        let mut r = Renderer::new(Mode::Off, "build", Duration::from_secs(30), Output::Stderr);
        r.plan = Some(plan.clone());
        r.steps = Steps::from_plan(&plan.phases);
        let t0 = Instant::now();
        r.steps.observe(&[(Phase::ParseChunk, t0)]);
        let now = t0 + Duration::from_secs(10);
        let snap = Snapshot {
            phase: Some(Phase::ParseChunk),
            done: 10,
            total: Some(100),
            chunks: 1,
            ..Default::default()
        };
        let (eta, rough) = r.eta(&snap, now).unwrap();
        // 10 s for 10%: 90 s to go in the parse, then 0.05 + 0.17 of 100 s.
        assert!((eta - (90.0 + 22.0)).abs() < 1e-6, "{eta}");
        assert!(!rough, "10% of the parse is past the 5% mark");
        let early = Snapshot {
            done: 2,
            ..snap.clone()
        };
        assert!(r.eta(&early, now).unwrap().1, "2% in: an estimate");
        let unsealed = Snapshot { chunks: 0, ..snap };
        assert!(
            r.eta(&unsealed, now).unwrap().1,
            "no chunk sealed yet: an estimate"
        );
    }
}
