//! horcrux — Tom Riddle's diary on a reMarkable 2.
//!
//! Write with the pen; after a short idle the ink is drunk away, a vision LLM
//! (the "oracle") answers, and the reply writes itself on the page in
//! animated handwriting. Five-finger tap on the touchscreen quits.
//!
//! Single-threaded state machine on a ~2ms loop; the only thread is one
//! oracle worker per turn, talking back over a channel.

mod cover;
mod fb;
mod ink;
mod journal;
mod macros;
mod memory;
mod oracle;
mod pen;
mod script;
mod surface;

use ink::{Ink, Point};
use memory::Memory;
use oracle::{OracleConfig, OracleEvent};
use pen::{Input, InputEvent, PenSample};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use surface::{Rect, Surface};

const PAGE_PNG_PATH: &str = "/tmp/horcrux-page.png";
const DEFAULT_TZ_NAME: &str = "Australia/Perth";
const DEFAULT_TZ_OFFSET_HOURS: i32 = 8;
const DEFAULT_LIBRARY_DIR: &str = "/home/root/.local/share/remarkable/xochitl";

const INK_FLUSH_MS: u64 = 15;
const MIN_PRESSURE: i32 = 40;

// --- idle-aware main loop (battery) ------------------------------------------
// The loop blocks on the input devices (see Input::wait) instead of
// busy-polling every 2ms, and only wakes as often as the current state
// requires. While the page sits empty and idle it wakes ~1x/s instead of
// 500x/s, so the CPU can idle and the device is free to autosuspend.
/// Empty idle page: block on input up to this long before re-checking.
const IDLE_LISTEN_SEC: u64 = 1;
/// Ink on the page awaiting the idle-commit: check a few times a second.
const IDLE_PENDING_MS: u64 = 250;
/// Waiting on the oracle channel (no fd to poll): check this often.
const ORACLE_POLL_MS: u64 = 50;

// --- wake + connectivity + offline fallback ----------------------------------

/// A suspend longer than this (BOOTTIME minus MONOTONIC delta jump) counts
/// as a sleep/wake cycle. Normal idle blocking is <= 1s, well under this.
const WAKE_THRESHOLD_SECS: i64 = 5;
/// How often to re-probe the oracle endpoint while connecting / offline.
const PROBE_INTERVAL_SEC: u64 = 3;
/// Per-probe reachability timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
/// Cold-start reachability probe (short, so an online launch isn't delayed).
const COLD_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Faint corner indicator shown while pages are queued offline.
const FEATHER_GRAY: u8 = 150;

/// One of these is shown at random as a "lock screen" while the diary is on
/// but the oracle is unreachable. It stays until the writer double-taps to
/// dismiss it — it does NOT vanish on its own.
const CONNECTING_LINES: &[&str] = &[
    "What you call a defeat, I call the first draft of your will. Begin again — the page is patient.",
    "A crown is only weight until you learn to carry it. Try once more.",
    "The flame that flickers has not failed; it is only learning the wind.",
    "No one is born formidable. They simply refused, one more time, to stay down.",
    "The ink you spend on doubt would have written you into someone new.",
    "Fall seven times; the eighth is the one the diary will remember.",
    "Courage is not the absence of fear — it is fear, walking forward in a steady hand.",
    "The mountain does not care that you are tired. That is why you must reach it.",
    "Yesterday's stumble is tomorrow's legend, told in your own hand.",
    "You are not lost. You are only between who you were and who you intend to be.",
    "Patience is the quietest form of power. Let it gather.",
    "The wound that does not end you becomes the door you walk through.",
    "Greatness is the habit of beginning again, as though beginning were easy.",
    "Even the longest shadow is cast by something still standing in the light.",
    "Do not ask the road to be kind. Ask yourself to be equal to it.",
    "The world remembers those who kept writing after the page went dark.",
    "One steady line, drawn in the dark, is enough to start a new chapter. Begin it.",
    "What feels like an ending is merely the plot refusing to settle for less than you.",
    "Hold the pen until the trembling stops. That is where your real sentence begins.",
    "Call it behind if you must; I call it being early for the person you are becoming.",
    "Those who win are the ones who decided, quietly, that they would not stop.",
    "Turn the page. The story is not finished with you yet.",
    "Strength is a language learned through repetition. Speak it once more.",
    "Every master was first a beginner who refused to be excused from the work.",
    "You came this far on will alone. Will has not failed you yet — try one more time.",
];

const DRINK_STAGES: u8 = 26;

/// Delay between a reply segment's speckle pass and its solidify pass.
const SOLIDIFY_MS: u64 = 120;
/// A one-pixel radius keeps the handwriting delicate rather than heavy.
const REPLY_BRUSH_R: i32 = 1;
/// Time a completed ordinary reply remains still before it starts fading.
const REPLY_LINGER_MS: u64 = 4_000;

const FADE_STAGES: u8 = 26;

// journal: auto-composed entry lingers briefly, conjured entries replay
// fast and faded
const JOURNAL_LINGER_MS: u64 = 2000;
const CONJURE_TICK_MS: u64 = 10;
const CONJURE_POINTS_PER_TICK: usize = 48;
const CONJURE_LINGER_MS: u64 = 2500;
const MAX_REPLY_SENTENCES: usize = 3;
const MAX_REPLY_WORDS: usize = 55;
const MAX_REPLY_CHARS: usize = 420;

static QUIT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct AnimationConfig {
    idle_ms: u64,
    drink_stage_ms: u64,
    write_tick_ms: u64,
    write_points_per_tick: usize,
    fade_stage_ms: u64,
}

impl AnimationConfig {
    fn from_env() -> AnimationConfig {
        AnimationConfig {
            idle_ms: env_u64("HORCRUX_IDLE_MS", 3800, 250, 60_000),
            drink_stage_ms: env_u64("HORCRUX_DRINK_STAGE_MS", 140, 20, 2000),
            write_tick_ms: env_u64("HORCRUX_WRITE_TICK_MS", 16, 4, 500),
            write_points_per_tick: env_u64("HORCRUX_WRITE_POINTS_PER_TICK", 12, 1, 128) as usize,
            fade_stage_ms: env_u64("HORCRUX_FADE_STAGE_MS", 140, 20, 2000),
        }
    }
}

fn env_u64(name: &str, default: u64, min: u64, max: u64) -> u64 {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(value) if (min..=max).contains(&value) => value,
            _ => {
                log!("ignoring invalid {name}; expected {min}..={max}");
                default
            }
        },
        Err(_) => default,
    }
}

extern "C" fn on_signal(_sig: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
}

#[derive(Clone, Copy)]
enum State {
    Listening,
    Drinking {
        stage: u8,
        next: Instant,
    },
    /// Waiting on the oracle's complete reply; the page stays blank (no
    /// blinking indicator — it read as a defect on the e-ink panel).
    Thinking,
    /// Waiting for the complete journal entry before fitting it to the page.
    JournalThinking,
    Replying {
        next: Instant,
    },
    Lingering {
        until: Instant,
    },
    FadingReply {
        stage: u8,
        next: Instant,
    },
    /// Composing today's journal entry on quit (text-only oracle turn).
    Journaling {
        next: Instant,
    },
    JournalLingering {
        until: Instant,
    },
    /// Replaying a past entry's stored strokes, fast and faded.
    Conjuring {
        next: Instant,
    },
    ConjureLingering {
        until: Instant,
    },
    /// Lock screen: the diary is on but the oracle is unreachable. A random
    /// connecting line is shown and stays until the writer double-taps to
    /// dismiss it (it does NOT vanish on its own). Pen is gated.
    OfflineLocked,
    /// Writing permitted with no oracle; pages are queued locally. A feather
    /// marks the page as held. Connectivity is probed in the background; when
    /// it returns, the diary barges in with a reply to the last page.
    Offline {
        next_probe: Instant,
    },
}

fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("horcrux {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => {
                println!(
                    "horcrux {}\n\nTom's Diary for the reMarkable 2\n\nOptions:\n  -h, --help          Show this help\n  -V, --version       Show the version\n      --publish-diary Rebuild the Markdown diary and stock-library EPUB",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            "--publish-diary" => {
                let memory = Memory::from_env();
                let journal_dir = configured_journal_dir(&memory);
                let library_dir = configured_library_dir();
                let time_context =
                    journal::current_time_context(&configured_tz_name(), configured_tz_offset());
                match journal::publish_diary(&journal_dir, &library_dir, &time_context) {
                    Ok(path) => println!("published {}", path.display()),
                    Err(error) => {
                        eprintln!("horcrux: diary publish failed: {error}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--render-cover" => {
                let path = std::env::args()
                    .nth(2)
                    .unwrap_or_else(|| "cover.png".to_string());
                match std::fs::write(&path, cover::cover_png()) {
                    Ok(()) => println!("cover written to {path}"),
                    Err(e) => {
                        eprintln!("horcrux: cannot write cover: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            _ => {}
        }
    }
    std::panic::set_hook(Box::new(|info| {
        log!("panic: {info}");
    }));
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let code = match std::panic::catch_unwind(run) {
        Ok(Ok(code)) => code,
        Ok(Err(msg)) => {
            eprintln!("horcrux: {msg}");
            log!("fatal: {msg}");
            1
        }
        Err(_) => {
            eprintln!("horcrux: crashed, see /tmp/horcrux.log");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32, String> {
    log!(
        "horcrux {} starting (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    let oracle_cfg = OracleConfig::from_env();
    if oracle_cfg.is_none() {
        log!("warning: HORCRUX_OPENAI_KEY unset — turns will fail politely");
    }
    let memory = Memory::from_env();
    let mut surf = Surface::open().map_err(|e| e.to_string())?;
    let input = Input::open().map_err(|e| format!("cannot open input devices: {e}"))?;
    surf.full_white_refresh();
    let mut app = App::new(surf, input, memory, oracle_cfg);
    app.loop_forever();
    Ok(0)
}

struct App {
    surf: Surface,
    input: Input,
    memory: Memory,
    oracle_cfg: Option<OracleConfig>,
    animation: AnimationConfig,
    state: State,

    // journal configuration
    journal_enabled: bool,
    tz_name: String,
    tz_offset_hours: i32,
    journal_dir: PathBuf,
    library_dir: PathBuf,

    // live ink (Listening)
    ink: Ink,
    pen_down: bool,
    last_pt: Option<Point>,
    last_radius: i32,
    last_activity: Instant,
    last_flush: Instant,

    // current oracle turn
    rx: Option<Receiver<OracleEvent>>,
    turn_strokes: Vec<Vec<Point>>,
    reply_sentences: Vec<String>,
    transcript: String,
    oracle_done: bool,
    turn_seed: u64,
    saved_turn: bool,

    // reply replay cursor into plan.strokes
    plan: Option<script::ReplyPlan>,
    draw_si: usize,
    draw_pi: usize,
    // materialize effect: segments drawn as speckle awaiting solidify
    solidify: std::collections::VecDeque<(Instant, (i32, i32, i32, i32))>,

    dissolve: Option<ink::Dissolve>,

    // journal recall: pending ⟦entry:date⟧, conjure replay cursor
    show_entry: Option<String>,
    conjure: Option<Conjure>,

    // date of the entry being composed (pinned at quit time)
    journal_date: String,

    // set when the journal entry is done and the app should exit
    want_quit: bool,

    // --- wake + connectivity + offline fallback ---
    /// Last observed CLOCK_BOOTTIME - CLOCK_MONOTONIC (seconds). A jump past
    /// WAKE_THRESHOLD_SECS means the device just resumed from sleep.
    boot_delta: i64,
    /// False until the first loop iteration performs cold-start connectivity.
    booted: bool,
    /// True while the offline commit's drink animation runs; on completion it
    /// returns to Offline instead of waiting on the oracle.
    offline_drinking: bool,
    /// Directory holding queued offline pages (`<unix>-<seq>.png`).
    offline_dir: PathBuf,
    /// Per-process counter making queued filenames unique within a second.
    offline_seq: u64,
    /// True while the barging-in question ("attend to the other pages?") is
    /// open. Normal vision turns get the awaiting addendum so the writer's
    /// yes is conveyed through the ⟦summarize:offline⟧ directive.
    awaiting_offline: bool,
    /// The in-flight turn is the barging-in turn itself (so its own completion
    /// is not mistaken for the writer's answer).
    barging_pending: bool,
    /// Set when the oracle emitted ⟦summarize:offline⟧ during a turn.
    want_summarize: bool,
    /// In-flight background reachability probe (Offline state), or None.
    /// Drained non-blockingly each tick so a hung resolver never freezes the UI.
    probe_rx: Option<Receiver<bool>>,
}

/// Replay cursor over a recalled entry's stored strokes.
struct Conjure {
    strokes: Vec<Vec<(i32, i32)>>,
    si: usize,
    pi: usize,
}

/// A page written while offline, held on disk until connectivity returns.
struct QueuedPage {
    /// Unix seconds at which the page was written — the diary files its
    /// later answer under this day, not the day it was answered.
    created_unix: i64,
    /// Per-second sequence, preserving write order within one second.
    seq: u64,
    png: Vec<u8>,
    /// On-disk path; removed only once the page has been answered and filed,
    /// so a quit mid-flush leaves the unfiled pages for the next session.
    path: PathBuf,
}

impl App {
    fn new(surf: Surface, input: Input, memory: Memory, oracle_cfg: Option<OracleConfig>) -> App {
        let now = Instant::now();
        let journal_dir = configured_journal_dir(&memory);
        let offline_dir = memory
            .dir()
            .parent()
            .unwrap_or_else(|| memory.dir())
            .join("offline-queue");
        App {
            surf,
            input,
            memory,
            oracle_cfg,
            animation: AnimationConfig::from_env(),
            state: State::Listening,
            journal_enabled: std::env::var("HORCRUX_JOURNAL")
                .map(|v| v != "off")
                .unwrap_or(true),
            tz_name: configured_tz_name(),
            tz_offset_hours: configured_tz_offset(),
            journal_dir,
            library_dir: configured_library_dir(),
            ink: Ink::new(),
            pen_down: false,
            last_pt: None,
            last_radius: 2,
            last_activity: now,
            last_flush: now,
            rx: None,
            turn_strokes: Vec::new(),
            reply_sentences: Vec::new(),
            transcript: String::new(),
            oracle_done: false,
            turn_seed: 0x5EED_5EED_5EED_5EED,
            saved_turn: false,
            plan: None,
            draw_si: 0,
            draw_pi: 0,
            solidify: std::collections::VecDeque::new(),
            dissolve: None,
            show_entry: None,
            conjure: None,
            journal_date: String::new(),
            want_quit: false,
            boot_delta: 0,
            booted: false,
            offline_drinking: false,
            offline_dir,
            offline_seq: 0,
            awaiting_offline: false,
            barging_pending: false,
            want_summarize: false,
            probe_rx: None,
        }
    }

    fn loop_forever(&mut self) {
        loop {
            if QUIT.load(Ordering::SeqCst) || self.want_quit {
                log!("quitting");
                break;
            }
            if !self.booted {
                // Cold start: probe once. If the oracle is already reachable
                // and nothing is queued, go straight to Listening (no overlay
                // flash); otherwise show the connecting overlay.
                self.booted = true;
                self.boot_delta = boottime_minus_monotonic_secs();
                self.cold_start();
            } else if self.detect_wake() {
                self.on_wake();
            }
            // Sleep deeply while idle: block on the input devices until either
            // a pen/touch event arrives or the next animation deadline is due,
            // then drain and tick. Replaces the old 2ms busy-poll.
            let timeout = self.next_wait_timeout();
            self.input.wait(timeout);
            let mut quit = false;
            for ev in self.input.poll() {
                if self.handle_event(ev) {
                    quit = true;
                    break;
                }
            }
            if quit {
                break;
            }
            self.tick();
        }
        self.shutdown();
    }

    /// How long the main loop may block on input before it must wake to
    /// advance an animation or re-check the oracle channel. Long while idle
    /// (battery), tight while animating (smoothness), and always overridden
    /// the instant the pen or touchscreen fires.
    fn next_wait_timeout(&self) -> Duration {
        let now = Instant::now();
        match self.state {
            State::Listening => {
                if self.pen_down {
                    Duration::from_millis(INK_FLUSH_MS)
                } else if !self.ink.is_empty() {
                    Duration::from_millis(IDLE_PENDING_MS)
                } else {
                    Duration::from_secs(IDLE_LISTEN_SEC)
                }
            }
            State::Drinking { next, .. }
            | State::Replying { next }
            | State::FadingReply { next, .. }
            | State::Conjuring { next }
            | State::Journaling { next } => next.saturating_duration_since(now),
            State::Lingering { until }
            | State::JournalLingering { until }
            | State::ConjureLingering { until } => until.saturating_duration_since(now),
            State::Thinking | State::JournalThinking => Duration::from_millis(ORACLE_POLL_MS),
            // Lock screen: nothing to do but wait for the dismiss double-tap.
            State::OfflineLocked => Duration::from_secs(IDLE_LISTEN_SEC),
            // Offline behaves like Listening for input cadence, but when idle
            // it waits until the next background connectivity probe.
            State::Offline { next_probe } => {
                if self.pen_down {
                    Duration::from_millis(INK_FLUSH_MS)
                } else if !self.ink.is_empty() {
                    Duration::from_millis(IDLE_PENDING_MS)
                } else if self.probe_rx.is_some() {
                    Duration::from_millis(ORACLE_POLL_MS)
                } else {
                    next_probe.saturating_duration_since(now)
                }
            }
        }
    }

    fn shutdown(&mut self) {
        log!("shutting down");
        self.input.ungrab();
        cleanup_page_png();
        // leave the panel in a sane state: blank page, full-quality refresh
        self.surf.full_white_refresh();
    }

    /// Returns true when the app should quit.
    fn handle_event(&mut self, ev: InputEvent) -> bool {
        match ev {
            InputEvent::Quit => {
                // a second quit during journal composition skips the entry
                if matches!(
                    self.state,
                    State::JournalThinking
                        | State::Journaling { .. }
                        | State::JournalLingering { .. }
                ) {
                    log!("quit during journaling — skipping the entry");
                    return true;
                }
                // otherwise: compose today's journal entry first, if due
                !self.try_begin_journal()
            }
            InputEvent::Pen(s) => {
                self.last_activity = Instant::now();
                match self.state {
                    State::Listening | State::Offline { .. } => self.on_pen_listening(s),
                    State::Lingering { .. } | State::ConjureLingering { .. } if s.touching => {
                        self.start_fading()
                    }
                    _ => {} // pen gated during lock screen / animations / oracle wait
                }
                false
            }
            InputEvent::DoubleTap => {
                match self.state {
                    // dismiss the reply early
                    State::Lingering { .. } | State::ConjureLingering { .. } => {
                        self.start_fading();
                    }
                    // double-tap dismisses the offline lock screen
                    State::OfflineLocked => self.dismiss_lock(),
                    _ => {}
                }
                false
            }
        }
    }

    // --- journal composition (on quit) -------------------------------------------

    /// If a journal entry is due (journal+memory enabled, pages written
    /// today, no entry yet), start composing it and return true.
    fn try_begin_journal(&mut self) -> bool {
        if !self.journal_enabled || !self.memory.is_enabled() {
            return false;
        }
        let today = journal::today_ymd(self.tz_offset_hours);
        let pages = self.memory.pages_on(&today, self.tz_offset_hours);
        if pages.is_empty() {
            return false;
        }
        log!(
            "composing updated journal entry for {today} ({} pages)",
            pages.len()
        );
        let prompt = journal::build_journal_prompt(&pages, &today);
        self.journal_date = today;

        // fresh reply machinery on a clean page
        self.reply_sentences.clear();
        self.transcript.clear();
        self.oracle_done = false;
        self.show_entry = None;
        self.plan = None;
        self.draw_si = 0;
        self.draw_pi = 0;
        self.solidify.clear();
        self.turn_seed = self
            .turn_seed
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let time_context = journal::current_time_context(&self.tz_name, self.tz_offset_hours);
        oracle::spawn_text(self.oracle_cfg.clone(), prompt, time_context, tx);
        self.surf.full_white_refresh();
        self.state = State::JournalThinking;
        true
    }

    // --- Listening ------------------------------------------------------------

    fn on_pen_listening(&mut self, s: PenSample) {
        let drawing = s.touching && s.pressure > MIN_PRESSURE;
        if !drawing {
            // lifted (or hovering): close the current stroke
            self.pen_down = false;
            self.last_pt = None;
            return;
        }
        if s.eraser {
            let r = ink::ERASER_RADIUS;
            match self.last_pt {
                Some((lx, ly, _)) => {
                    ink::brush_line(&mut self.surf, lx, ly, r, s.x, s.y, r, 255);
                    self.ink.erase_segment(lx, ly, s.x, s.y, r);
                }
                None => {
                    self.surf.fill_circle(s.x, s.y, r, 255);
                    self.surf.mark_dirty(Rect::around(s.x, s.y, r));
                    self.ink.erase_segment(s.x, s.y, s.x, s.y, r);
                }
            }
            self.last_pt = Some((s.x, s.y, r));
            self.pen_down = true;
            return;
        }
        // ink brush; thin like the diary's own hand (1..3px from pressure);
        // radius grows at most +1px per segment (anti-blob)
        let r_target = 1 + s.pressure * 2 / 4095;
        let r = if self.pen_down {
            r_target.min(self.last_radius + 1)
        } else {
            r_target
        };
        match self.last_pt {
            Some((lx, ly, lr)) if self.pen_down => {
                ink::brush_line(&mut self.surf, lx, ly, lr, s.x, s.y, r, 0);
            }
            _ => {
                self.surf.fill_circle(s.x, s.y, r, 0);
                self.surf.mark_dirty(Rect::around(s.x, s.y, r));
                self.ink.start_stroke();
            }
        }
        self.ink.add_point((s.x, s.y, r));
        self.last_radius = r;
        self.last_pt = Some((s.x, s.y, r));
        self.pen_down = true;
    }

    /// Idle commit: pen up, no events for 2.8s, ink on the page.
    fn maybe_commit(&mut self) {
        if self.pen_down
            || self.ink.is_empty()
            || self.last_activity.elapsed().as_millis() < self.animation.idle_ms as u128
        {
            return;
        }
        self.surf.flush_dirty(fb::WAVEFORM_DU);
        self.surf.wait_idle();
        let Some(bbox) = self.ink.bbox() else {
            return;
        };
        let png = match ink::rasterize_page_png(&self.surf.shadow, bbox) {
            Ok(p) => p,
            Err(e) => {
                log!("page rasterization failed: {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(PAGE_PNG_PATH, &png) {
            log!("cannot write {PAGE_PNG_PATH}: {e}");
        }
        log!(
            "page committed: {} strokes, {} bytes png",
            self.ink.strokes.len(),
            png.len()
        );
        self.turn_strokes = self.ink.strokes.clone();
        self.reply_sentences.clear();
        self.transcript.clear();
        self.oracle_done = false;
        self.saved_turn = false;
        self.plan = None;
        self.draw_si = 0;
        self.draw_pi = 0;
        // advance the per-turn seed so jitter differs between turns
        self.turn_seed = self
            .turn_seed
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);

        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let journal_today = self
            .journal_enabled
            .then(|| journal::today_ymd(self.tz_offset_hours));
        let time_context = journal::current_time_context(&self.tz_name, self.tz_offset_hours);
        let extra = if self.awaiting_offline {
            Some(oracle::AWAITING_ADDENDUM)
        } else {
            None
        };
        oracle::spawn(
            self.oracle_cfg.clone(),
            png,
            self.memory.history(),
            time_context,
            journal_today,
            extra,
            tx,
        );

        // The writer's ink is drunk in the order it was written (first
        // stroke first), with a soft advancing frontier.
        let ordered: Vec<ink::Point> = self.ink.strokes.iter().flatten().copied().collect();
        self.dissolve = ink::Dissolve::plan_ordered(&self.surf.shadow, &ordered, DRINK_STAGES);
        self.state = State::Drinking {
            stage: 0,
            next: Instant::now(),
        };
    }

    // --- oracle turn plumbing ---------------------------------------------------

    fn drain_oracle(&mut self) {
        let mut events = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                OracleEvent::Ink(s) => {
                    self.reply_sentences.push(s);
                }
                OracleEvent::Transcript(t) => {
                    self.transcript = t;
                }
                OracleEvent::ShowEntry(date) => {
                    if self.show_entry.is_none() {
                        self.show_entry = Some(date);
                    } else {
                        log!("ignoring duplicate recall directive for {date}");
                    }
                }
                OracleEvent::SummarizeOffline => {
                    self.want_summarize = true;
                }
                OracleEvent::Done => {
                    self.oracle_done = true;
                }
            }
        }
    }

    fn begin_reply(&mut self) {
        // Resolve the barging-in question if the just-finished turn was the
        // writer's answer to it (not the barging-in turn itself).
        if self.awaiting_offline && !self.barging_pending {
            if self.want_summarize {
                // yes — address all remaining offline pages in one go
                self.awaiting_offline = false;
                self.want_summarize = false;
                self.summarize_start();
                return;
            }
            // no (or the writer moved on) — drop the question; any remaining
            // offline pages stay queued on disk for a later session.
            self.awaiting_offline = false;
        }
        if self.barging_pending {
            // this turn IS the barging-in reply; the question is now open
            self.barging_pending = false;
        }
        if self.reply_sentences.is_empty() && self.oracle_done {
            // a recall directive instead of a prose reply?
            if let Some(date) = self.show_entry.take() {
                self.handle_show_entry(date);
                return;
            }
            // the oracle said nothing at all — end the turn quietly
            log!("oracle returned no ink");
            self.finish_turn();
            self.state = State::Listening;
            return;
        }
        constrain_reply(&mut self.reply_sentences);
        self.replan();
        self.state = State::Replying {
            next: Instant::now(),
        };
    }

    /// The writer asked to see a past day's entry (⟦entry:YYYY-MM-DD⟧).
    fn handle_show_entry(&mut self, date: String) {
        // recall turns are not conversation: keep them out of the memory
        self.saved_turn = true;
        if !self.journal_enabled {
            journal::log_recall(&date, "journal disabled, ignoring directive");
            self.finish_turn();
            self.state = State::Listening;
            return;
        }
        match journal::recall(&self.journal_dir, &date) {
            journal::Recall::Missing => {
                journal::log_recall(&date, "no entry for that day");
                self.reply_sentences
                    .push(journal::CANNED_NO_ENTRY.to_string());
                self.replan();
                self.state = State::Replying {
                    next: Instant::now(),
                };
            }
            journal::Recall::Talk(sentences) => {
                journal::log_recall(&date, "entry too long; talking about it");
                self.reply_sentences.extend(sentences);
                self.replan();
                self.state = State::Replying {
                    next: Instant::now(),
                };
            }
            journal::Recall::Conjure(strokes) => {
                let points: usize = strokes.iter().map(|s| s.len()).sum();
                journal::log_recall(&date, &format!("conjured entry ({points} points)"));
                self.conjure = Some(Conjure {
                    strokes,
                    si: 0,
                    pi: 0,
                });
                self.surf.full_white_refresh();
                self.state = State::Conjuring {
                    next: Instant::now(),
                };
            }
        }
    }

    /// Plan the complete reply after streaming ends, allowing the handwriting
    /// fitter to choose a font size and center the complete ink block while
    /// keeping every line visible.
    fn replan(&mut self) {
        if self.reply_sentences.is_empty() {
            return;
        }
        let plan_text = self.reply_sentences.join(" ");
        match script::plan_reply(&plan_text, self.turn_seed) {
            Some(plan) => {
                log!(
                    "reply layout: {} lines at {:.0}px, top {}",
                    plan.line_count,
                    plan.font_px,
                    plan.block_top
                );
                self.plan = Some(plan);
            }
            None => log!("handwriting plan failed"),
        }
    }

    // --- Replying ---------------------------------------------------------------

    /// Animate the reply plan onto the page, one tick at a time. Each bit of
    /// ink materializes: drawn as a checkerboard speckle immediately, then
    /// solidified ~120ms later (the missing checkerboard half) — the reply
    /// blooms out of the page the same mysterious way it dissolves.
    /// `journal` selects the continuation when the last stroke is drawn: a
    /// normal turn lingers, a journal entry is saved and the app exits.
    fn replay_tick(&mut self, journal: bool) {
        let now = Instant::now();
        // solidify due segments (the speckle's other checkerboard half)
        while let Some(&(due, seg)) = self.solidify.front() {
            if due > now {
                break;
            }
            self.solidify.pop_front();
            let (x0, y0, x1, y1) = seg;
            ink::brush_line_dither(
                &mut self.surf,
                x0,
                y0,
                REPLY_BRUSH_R,
                x1,
                y1,
                REPLY_BRUSH_R,
                0,
                1,
            );
        }
        let mut budget = self.animation.write_points_per_tick;
        if let Some(plan) = &self.plan {
            while budget > 0 && self.draw_si < plan.strokes.len() {
                let stroke = &plan.strokes[self.draw_si];
                if self.draw_pi == 0 {
                    // stamp each stroke's first point
                    let (x, y) = stroke[0];
                    self.surf.fill_circle_dither(x, y, REPLY_BRUSH_R, 0, 0);
                    self.surf.mark_dirty(Rect::around(x, y, REPLY_BRUSH_R));
                    self.solidify
                        .push_back((now + Duration::from_millis(SOLIDIFY_MS), (x, y, x, y)));
                    self.draw_pi = 1;
                    budget -= 1;
                    if stroke.len() == 1 {
                        self.draw_si += 1;
                        self.draw_pi = 0;
                    }
                    continue;
                }
                let (x0, y0) = stroke[self.draw_pi - 1];
                let (x1, y1) = stroke[self.draw_pi];
                ink::brush_line_dither(
                    &mut self.surf,
                    x0,
                    y0,
                    REPLY_BRUSH_R,
                    x1,
                    y1,
                    REPLY_BRUSH_R,
                    0,
                    0,
                );
                self.solidify
                    .push_back((now + Duration::from_millis(SOLIDIFY_MS), (x0, y0, x1, y1)));
                self.draw_pi += 1;
                budget -= 1;
                if self.draw_pi >= stroke.len() {
                    self.draw_si += 1;
                    self.draw_pi = 0;
                }
            }
        }
        self.surf.flush_dirty(fb::WAVEFORM_DU);
        let plan_done = self
            .plan
            .as_ref()
            .map(|p| self.draw_si >= p.strokes.len())
            .unwrap_or(true);
        if plan_done && self.oracle_done {
            // finish the materialize before resting: drain remaining speckle
            while let Some((_, (x0, y0, x1, y1))) = self.solidify.pop_front() {
                ink::brush_line_dither(
                    &mut self.surf,
                    x0,
                    y0,
                    REPLY_BRUSH_R,
                    x1,
                    y1,
                    REPLY_BRUSH_R,
                    0,
                    1,
                );
            }
            self.surf.flush_dirty(fb::WAVEFORM_DU);
            if journal {
                self.enter_journal_linger();
            } else {
                self.enter_lingering();
            }
            return;
        }
        let next = Instant::now() + Duration::from_millis(self.animation.write_tick_ms);
        self.state = if journal {
            State::Journaling { next }
        } else {
            State::Replying { next }
        };
    }

    fn enter_journal_linger(&mut self) {
        self.state = State::JournalLingering {
            until: Instant::now() + Duration::from_millis(JOURNAL_LINGER_MS),
        };
    }

    /// Persist today's entry (text + strokes) and ask the loop to exit.
    fn save_journal_and_quit(&mut self) {
        let today = std::mem::take(&mut self.journal_date);
        let text = self.reply_sentences.join(" ");
        if !text.is_empty() {
            let strokes = self
                .plan
                .as_ref()
                .map(|p| p.strokes.clone())
                .unwrap_or_default();
            match journal::save_entry(&self.journal_dir, &today, &text, &strokes) {
                Ok(()) => log!("journal entry saved for {today}"),
                Err(e) => log!("journal save failed: {e}"),
            }
        } else {
            log!("journal entry empty (oracle failed?), nothing new to save");
        }
        // Always refresh the stock-library EPUB from whatever entries exist:
        // a failed "today" must not suppress the whole library document.
        let time_context = journal::current_time_context(&self.tz_name, self.tz_offset_hours);
        match journal::publish_diary(&self.journal_dir, &self.library_dir, &time_context) {
            Ok(path) => log!("stock-library diary refreshed from {}", path.display()),
            Err(error) => log!("stock-library diary refresh skipped: {error}"),
        }
        self.want_quit = true;
    }

    /// Conjure a recalled entry: replay its stored strokes fast and faded.
    fn conjure_tick(&mut self) {
        let mut budget = CONJURE_POINTS_PER_TICK;
        if let Some(c) = &mut self.conjure {
            while budget > 0 && c.si < c.strokes.len() {
                let stroke = &c.strokes[c.si];
                if c.pi == 0 {
                    let (x, y) = stroke[0];
                    self.surf
                        .fill_circle(x, y, REPLY_BRUSH_R, journal::CONJURE_GRAY);
                    self.surf.mark_dirty(Rect::around(x, y, REPLY_BRUSH_R));
                    c.pi = 1;
                    budget -= 1;
                    if stroke.len() == 1 {
                        c.si += 1;
                        c.pi = 0;
                    }
                    continue;
                }
                let (x0, y0) = stroke[c.pi - 1];
                let (x1, y1) = stroke[c.pi];
                ink::brush_line(
                    &mut self.surf,
                    x0,
                    y0,
                    REPLY_BRUSH_R,
                    x1,
                    y1,
                    REPLY_BRUSH_R,
                    journal::CONJURE_GRAY,
                );
                c.pi += 1;
                budget -= 1;
                if c.pi >= stroke.len() {
                    c.si += 1;
                    c.pi = 0;
                }
            }
        }
        self.surf.flush_dirty(fb::WAVEFORM_DU);
        self.state = State::Conjuring {
            next: Instant::now() + Duration::from_millis(CONJURE_TICK_MS),
        };
        let done = self
            .conjure
            .as_ref()
            .map(|c| c.si >= c.strokes.len())
            .unwrap_or(true);
        if done {
            self.state = State::ConjureLingering {
                until: Instant::now() + Duration::from_millis(CONJURE_LINGER_MS),
            };
        }
    }

    fn enter_lingering(&mut self) {
        log!("turn complete, lingering {}ms", REPLY_LINGER_MS);
        self.state = State::Lingering {
            until: Instant::now() + Duration::from_millis(REPLY_LINGER_MS),
        };
        self.save_turn();
    }

    fn save_turn(&mut self) {
        if self.saved_turn {
            return;
        }
        self.saved_turn = true;
        if let Err(e) = self.memory.save_turn(
            &self.transcript,
            &self.reply_sentences.join(" "),
            &self.turn_strokes,
        ) {
            log!("memory save failed: {e}");
        }
        cleanup_page_png();
    }

    fn start_fading(&mut self) {
        self.save_turn();
        // The reply fades in the same order it was written on the page.
        let ordered: Vec<ink::Point> = self
            .plan
            .as_ref()
            .map(|p| {
                p.strokes
                    .iter()
                    .flatten()
                    .map(|&(x, y)| (x, y, REPLY_BRUSH_R))
                    .collect()
            })
            .unwrap_or_default();
        self.dissolve = ink::Dissolve::plan_ordered(&self.surf.shadow, &ordered, FADE_STAGES);
        if self.dissolve.is_some() {
            self.state = State::FadingReply {
                stage: 0,
                next: Instant::now(),
            };
        } else {
            self.surf.full_white_refresh();
            self.finish_turn();
            self.state = State::Listening;
        }
    }

    fn finish_turn(&mut self) {
        self.reply_sentences.clear();
        self.transcript.clear();
        self.oracle_done = false;
        self.plan = None;
        self.draw_si = 0;
        self.draw_pi = 0;
        self.rx = None;
        self.dissolve = None;
        self.ink.clear();
        self.show_entry = None;
        self.conjure = None;
        cleanup_page_png();
    }

    // --- wake + connectivity + offline fallback ----------------------------------

    /// CLOCK_BOOTTIME advances during suspend, CLOCK_MONOTONIC does not, so
    /// their difference grows by exactly the time spent suspended. A jump
    /// past WAKE_THRESHOLD_SECS means the device just resumed.
    fn detect_wake(&mut self) -> bool {
        let delta = boottime_minus_monotonic_secs();
        let woke = self.boot_delta != 0 && delta - self.boot_delta >= WAKE_THRESHOLD_SECS;
        self.boot_delta = delta;
        woke
    }

    /// Start a background reachability probe if one isn't already in flight.
    fn ensure_probe(&mut self) {
        if self.probe_rx.is_some() {
            return;
        }
        self.probe_rx = oracle::spawn_probe(self.oracle_cfg.as_ref(), PROBE_TIMEOUT);
    }

    /// Non-blocking drain of the in-flight probe. Returns Some(reachable) once
    /// it has answered (or its thread died), None while still pending.
    fn drain_probe(&mut self) -> Option<bool> {
        let received = self.probe_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            None => None,
            Some(Ok(reachable)) => {
                self.probe_rx = None;
                Some(reachable)
            }
            Some(Err(TryRecvError::Empty)) => None,
            Some(Err(TryRecvError::Disconnected)) => {
                self.probe_rx = None;
                Some(false)
            }
        }
    }

    /// Cold start: probe once. Already reachable (and nothing queued) goes
    /// straight to Listening — no lock screen on a normal online launch.
    fn cold_start(&mut self) {
        self.finish_turn();
        if oracle::endpoint_reachable(self.oracle_cfg.as_ref(), COLD_PROBE_TIMEOUT) {
            if self.offline_queue_count() > 0 {
                self.barging_in_start();
            } else {
                self.state = State::Listening;
                self.last_activity = Instant::now();
                self.last_flush = Instant::now();
            }
        } else {
            self.enter_offline_locked();
        }
    }

    /// The device just resumed. Discard any turn frozen mid-flight and decide
    /// afresh: online (barging in if pages wait) or the lock screen. Either
    /// path repaints, fixing the white-screen-on-wake bug.
    fn on_wake(&mut self) {
        log!("wake detected — re-establishing connectivity");
        self.offline_drinking = false;
        self.finish_turn();
        if oracle::endpoint_reachable(self.oracle_cfg.as_ref(), COLD_PROBE_TIMEOUT) {
            if self.offline_queue_count() > 0 {
                self.barging_in_start();
            } else {
                self.surf.full_white_refresh();
                self.state = State::Listening;
                self.last_activity = Instant::now();
                self.last_flush = Instant::now();
            }
        } else {
            self.enter_offline_locked();
        }
    }

    /// Show the lock screen: a random line that stays until double-tapped.
    fn enter_offline_locked(&mut self) {
        self.probe_rx = None;
        let line = (self.turn_seed % CONNECTING_LINES.len() as u64) as usize;
        self.turn_seed = self
            .turn_seed
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        self.surf.full_white_refresh();
        self.paint_text_static(CONNECTING_LINES[line]);
        self.state = State::OfflineLocked;
        log!("offline: lock screen shown (double-tap to dismiss)");
    }

    /// Double-tap dismissed the lock screen. Still offline -> writable page
    /// with the feather; online -> Listening (or barging in if pages wait).
    fn dismiss_lock(&mut self) {
        self.probe_rx = None;
        if oracle::endpoint_reachable(self.oracle_cfg.as_ref(), COLD_PROBE_TIMEOUT) {
            if self.offline_queue_count() > 0 {
                self.barging_in_start();
            } else {
                self.surf.full_white_refresh();
                self.state = State::Listening;
                self.last_activity = Instant::now();
                self.last_flush = Instant::now();
            }
        } else {
            self.surf.full_white_refresh();
            self.paint_feather();
            self.state = State::Offline {
                next_probe: Instant::now() + Duration::from_secs(PROBE_INTERVAL_SEC),
            };
            self.last_activity = Instant::now();
            self.last_flush = Instant::now();
        }
    }

    /// Connectivity returned while writing offline. If pages are queued, barge
    /// in with a reply to the last one; otherwise just return to listening.
    fn on_back_online(&mut self) {
        self.probe_rx = None;
        if self.offline_queue_count() == 0 {
            self.surf.full_white_refresh();
            self.state = State::Listening;
            self.last_activity = Instant::now();
            self.last_flush = Instant::now();
            return;
        }
        self.barging_in_start();
    }

    // --- offline reconnect: barging-in + one-shot summary -----------------------

    /// Clear per-turn reply machinery and blank the page for a fresh reply.
    fn begin_standalone_turn(&mut self) {
        self.reply_sentences.clear();
        self.transcript.clear();
        self.oracle_done = false;
        self.saved_turn = false;
        self.plan = None;
        self.draw_si = 0;
        self.draw_pi = 0;
        self.show_entry = None;
        self.surf.full_white_refresh();
    }

    /// Reply to the writer's most recent offline page, opening with an apology
    /// for the abrupt return and asking whether to address the rest. The other
    /// pages stay queued in case the writer says yes.
    fn barging_in_start(&mut self) {
        let mut queue = match self.load_offline_queue() {
            Ok(q) => q,
            Err(e) => {
                log!("offline queue read failed: {e}");
                Vec::new()
            }
        };
        if queue.is_empty() {
            self.state = State::Listening;
            self.last_activity = Instant::now();
            self.last_flush = Instant::now();
            return;
        }
        // newest page is last after sort_by_key((created_unix, seq))
        let last = queue.pop().unwrap();
        let _ = std::fs::remove_file(&last.path);
        self.begin_standalone_turn();
        let time_context = journal::current_time_context(&self.tz_name, self.tz_offset_hours);
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        oracle::spawn_barging_in(self.oracle_cfg.clone(), last.png, time_context, tx);
        self.barging_pending = true;
        self.awaiting_offline = true;
        self.state = State::Thinking;
        log!("barging in: replying to the last offline page");
    }

    /// Address every remaining offline page in one unified reply, then the
    /// conversation continues. Consumes (deletes) all queued pages.
    fn summarize_start(&mut self) {
        let queue = match self.load_offline_queue() {
            Ok(q) => q,
            Err(e) => {
                log!("offline queue read failed: {e}");
                Vec::new()
            }
        };
        if queue.is_empty() {
            self.state = State::Listening;
            self.last_activity = Instant::now();
            self.last_flush = Instant::now();
            return;
        }
        let mut pngs = Vec::with_capacity(queue.len());
        for p in &queue {
            pngs.push(p.png.clone());
            let _ = std::fs::remove_file(&p.path);
        }
        self.begin_standalone_turn();
        let time_context = journal::current_time_context(&self.tz_name, self.tz_offset_hours);
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        oracle::spawn_summarize(self.oracle_cfg.clone(), pngs, time_context, tx);
        self.state = State::Thinking;
        log!("summarizing {} offline pages in one go", queue.len());
    }

    // --- offline queue (disk) ----------------------------------------------------

    fn queue_offline_page(&mut self, png: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.offline_dir)?;
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        self.offline_seq = self.offline_seq.wrapping_add(1);
        let path = self
            .offline_dir
            .join(format!("{created}-{}.png", self.offline_seq));
        std::fs::write(&path, png)?;
        log!("offline page queued: {}", path.display());
        Ok(())
    }

    fn load_offline_queue(&self) -> std::io::Result<Vec<QueuedPage>> {
        let mut out: Vec<QueuedPage> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.offline_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(stem) = name.strip_suffix(".png") else {
                    continue;
                };
                let (secs_s, seq_s) = stem.split_once('-').unwrap_or((stem, "0"));
                let created_unix = secs_s.parse().unwrap_or(0);
                let seq: u64 = seq_s.parse().unwrap_or(0);
                if let Ok(png) = std::fs::read(&path) {
                    out.push(QueuedPage {
                        created_unix,
                        seq,
                        png,
                        path,
                    });
                }
            }
        }
        out.sort_by_key(|p| (p.created_unix, p.seq));
        Ok(out)
    }

    fn offline_queue_count(&self) -> usize {
        std::fs::read_dir(&self.offline_dir)
            .map(|rd| {
                rd.filter(|e| {
                    e.as_ref()
                        .ok()
                        .and_then(|e| e.file_name().to_str().map(|s| s.ends_with(".png")))
                        .unwrap_or(false)
                })
                .count()
            })
            .unwrap_or(0)
    }

    // --- offline commit (write with no oracle) -----------------------------------

    fn maybe_commit_offline(&mut self) {
        if self.pen_down
            || self.ink.is_empty()
            || self.last_activity.elapsed().as_millis() < self.animation.idle_ms as u128
        {
            return;
        }
        self.surf.flush_dirty(fb::WAVEFORM_DU);
        self.surf.wait_idle();
        let Some(bbox) = self.ink.bbox() else {
            return;
        };
        let png = match ink::rasterize_page_png(&self.surf.shadow, bbox) {
            Ok(p) => p,
            Err(e) => {
                log!("offline page rasterization failed: {e}");
                return;
            }
        };
        if let Err(e) = self.queue_offline_page(&png) {
            log!("offline queue write failed: {e}");
        }
        self.turn_strokes = self.ink.strokes.clone();
        self.reply_sentences.clear();
        self.transcript.clear();
        self.oracle_done = false;
        self.plan = None;
        self.draw_si = 0;
        self.draw_pi = 0;
        self.turn_seed = self
            .turn_seed
            .wrapping_mul(1664525)
            .wrapping_add(1013904223);
        // drink the ink away, then return to Offline (not the oracle)
        let ordered: Vec<ink::Point> = self.ink.strokes.iter().flatten().copied().collect();
        self.dissolve = ink::Dissolve::plan_ordered(&self.surf.shadow, &ordered, DRINK_STAGES);
        self.offline_drinking = true;
        self.state = State::Drinking {
            stage: 0,
            next: Instant::now(),
        };
    }

    /// The offline drink finished: the ink is held to the queue; redraw the
    /// feather and keep listening offline.
    fn finish_offline_drink(&mut self) {
        self.surf.full_white_refresh();
        self.paint_feather();
        self.state = State::Offline {
            next_probe: Instant::now() + Duration::from_secs(PROBE_INTERVAL_SEC),
        };
        self.last_activity = Instant::now();
        self.last_flush = Instant::now();
    }

    // --- static painting (lock-screen quotes, replies, feather) -----------------

    fn paint_text_static(&mut self, text: &str) {
        if let Some(plan) = script::plan_reply(text, self.turn_seed) {
            draw_strokes_static(&mut self.surf, &plan.strokes);
            self.surf.flush_dirty(fb::WAVEFORM_GC16);
            self.surf.wait_idle();
        } else {
            log!("could not lay out static text");
        }
    }

    /// A small feather glyph in the top-right corner: the quiet indicator
    /// that pages are being held offline. Hand-drawn as vector strokes.
    fn paint_feather(&mut self) {
        let cx = fb::WIDTH - 44;
        let top = 22;
        let g = FEATHER_GRAY;
        let shaft: [(i32, i32); 5] = [
            (cx, top),
            (cx - 1, top + 6),
            (cx - 2, top + 12),
            (cx - 3, top + 18),
            (cx - 4, top + 24),
        ];
        for w in shaft.windows(2) {
            ink::brush_line(&mut self.surf, w[0].0, w[0].1, 1, w[1].0, w[1].1, 1, g);
        }
        let barbs: [((i32, i32), (i32, i32)); 7] = [
            ((cx, top + 2), (cx - 5, top - 1)),
            ((cx - 1, top + 6), (cx - 7, top + 4)),
            ((cx - 1, top + 9), (cx - 8, top + 8)),
            ((cx - 2, top + 12), (cx - 9, top + 12)),
            ((cx - 2, top + 15), (cx - 9, top + 16)),
            ((cx - 3, top + 18), (cx - 9, top + 20)),
            ((cx - 4, top + 21), (cx - 8, top + 24)),
        ];
        for ((x0, y0), (x1, y1)) in barbs {
            ink::brush_line(&mut self.surf, x0, y0, 1, x1, y1, 1, g);
        }
        self.surf
            .mark_dirty(Rect::new(cx - 12, top - 3, cx + 2, top + 27));
        self.surf.flush_dirty(fb::WAVEFORM_GC16);
    }

    // --- tick ---------------------------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        match self.state {
            State::Listening => {
                if self.last_flush.elapsed() >= Duration::from_millis(INK_FLUSH_MS) {
                    self.surf.flush_dirty(fb::WAVEFORM_DU);
                    self.last_flush = now;
                }
                self.maybe_commit();
            }
            State::Drinking { stage, next } => {
                self.drain_oracle();
                if now < next {
                    return;
                }
                if let Some(d) = &self.dissolve {
                    d.apply_stage(&mut self.surf, stage);
                    // DU (fast bilevel) so the left-to-right sweep is visible
                    // as motion; GL16 updates are too slow and coalesce.
                    self.surf.flush_dirty(fb::WAVEFORM_DU);
                }
                if stage + 1 >= DRINK_STAGES {
                    // the page has drunk the writer's ink
                    self.ink.clear();
                    self.dissolve = None;
                    if self.offline_drinking {
                        // offline commit: ink held to the queue, no oracle turn
                        self.offline_drinking = false;
                        self.finish_offline_drink();
                    } else {
                        self.drain_oracle();
                        if self.oracle_done {
                            self.begin_reply();
                        } else {
                            self.state = State::Thinking;
                        }
                    }
                } else {
                    self.state = State::Drinking {
                        stage: stage + 1,
                        next: next + Duration::from_millis(self.animation.drink_stage_ms),
                    };
                }
            }
            State::Thinking => {
                self.drain_oracle();
                if self.oracle_done {
                    self.begin_reply();
                }
            }
            State::Replying { next } => {
                if now >= next {
                    self.replay_tick(false);
                }
            }
            State::Lingering { until } => {
                if now >= until {
                    self.start_fading();
                }
            }
            State::JournalThinking => {
                self.drain_oracle();
                if self.oracle_done {
                    if self.reply_sentences.is_empty() {
                        log!("journal oracle returned no ink");
                        self.want_quit = true;
                    } else {
                        self.replan();
                        self.state = State::Journaling {
                            next: Instant::now(),
                        };
                    }
                }
            }
            State::Journaling { next } => {
                if now >= next {
                    self.replay_tick(true);
                }
            }
            State::JournalLingering { until } => {
                if now >= until {
                    self.save_journal_and_quit();
                }
            }
            State::Conjuring { next } => {
                if now >= next {
                    self.conjure_tick();
                }
            }
            State::ConjureLingering { until } => {
                if now >= until {
                    self.start_fading();
                }
            }
            State::FadingReply { stage, next } => {
                if now < next {
                    return;
                }
                if let Some(d) = &self.dissolve {
                    d.apply_stage(&mut self.surf, stage);
                    // DU here too: the soft frontier is dithered, not gray.
                    self.surf.flush_dirty(fb::WAVEFORM_DU);
                }
                if stage + 1 >= FADE_STAGES {
                    self.surf.full_white_refresh();
                    self.finish_turn();
                    self.state = State::Listening;
                } else {
                    self.state = State::FadingReply {
                        stage: stage + 1,
                        next: next + Duration::from_millis(self.animation.fade_stage_ms),
                    };
                }
            }
            State::OfflineLocked => {
                // lock screen: nothing to do but wait for the dismiss double-tap
            }
            State::Offline { next_probe } => {
                if self.last_flush.elapsed() >= Duration::from_millis(INK_FLUSH_MS) {
                    self.surf.flush_dirty(fb::WAVEFORM_DU);
                    self.last_flush = now;
                }
                self.maybe_commit_offline();
                // keep probing in the background; a state change from commit
                // means we are mid-drink and must not clobber it.
                if matches!(self.state, State::Offline { .. }) {
                    if now >= next_probe {
                        self.ensure_probe();
                    }
                    match self.drain_probe() {
                        // wait until the pen lifts before barging in, so an
                        // in-progress stroke isn't interrupted.
                        Some(true) if !self.pen_down => self.on_back_online(),
                        // online but the pen is down: hold, re-probe shortly.
                        Some(_) => {
                            self.state = State::Offline {
                                next_probe: now + Duration::from_secs(PROBE_INTERVAL_SEC),
                            };
                        }
                        None => {}
                    }
                }
            }
        }
    }
}

/// CLOCK_BOOTTIME minus CLOCK_MONOTONIC, in seconds. BOOTTIME advances while
/// suspended and MONOTONIC does not, so this measures cumulative suspend time.
fn boottime_minus_monotonic_secs() -> i64 {
    unsafe {
        let mut bt: libc::timespec = std::mem::zeroed();
        let mut mt: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut bt);
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mt);
        // tv_sec is c_long (i32 on the 32-bit armv7 target, i64 on 64-bit);
        // widen before subtracting so this is correct and overflow-safe on both.
        (bt.tv_sec as i64) - (mt.tv_sec as i64)
    }
}

/// Draw a laid-out stroke plan all at once (static, black) — used for the
/// connecting quotes, the offline offer, and rendered queued replies.
fn draw_strokes_static(surf: &mut Surface, strokes: &[Vec<(i32, i32)>]) {
    let mut r = Rect::new(fb::WIDTH, fb::HEIGHT, 0, 0);
    let mut touched = false;
    for stroke in strokes {
        if let Some(&(x, y)) = stroke.first() {
            surf.fill_circle(x, y, REPLY_BRUSH_R, 0);
            r.include(Rect::around(x, y, REPLY_BRUSH_R));
            touched = true;
        }
        for w in stroke.windows(2) {
            let (x0, y0) = w[0];
            let (x1, y1) = w[1];
            ink::brush_line(surf, x0, y0, REPLY_BRUSH_R, x1, y1, REPLY_BRUSH_R, 0);
            r.include(Rect::around(x0, y0, REPLY_BRUSH_R));
            r.include(Rect::around(x1, y1, REPLY_BRUSH_R));
            touched = true;
        }
    }
    if touched {
        surf.mark_dirty(r);
    }
}

fn configured_journal_dir(memory: &Memory) -> PathBuf {
    std::env::var("HORCRUX_JOURNAL_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| journal::journal_dir(memory.dir()))
}

fn configured_library_dir() -> PathBuf {
    std::env::var("HORCRUX_LIBRARY_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LIBRARY_DIR))
}

fn configured_tz_name() -> String {
    std::env::var("HORCRUX_TZ_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TZ_NAME.to_string())
}

fn configured_tz_offset() -> i32 {
    std::env::var("HORCRUX_TZ_OFFSET")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|offset| (-23..=23).contains(offset))
        .unwrap_or(DEFAULT_TZ_OFFSET_HOURS)
}

/// Enforce the ordinary-reply contract even if the model ignores it. The
/// planner still scales to fit, but this keeps handwriting legible rather than
/// shrinking an unexpectedly huge response to a speck.
fn constrain_reply(sentences: &mut Vec<String>) {
    let original_sentence_count = sentences.len();
    sentences.truncate(MAX_REPLY_SENTENCES);
    let joined = sentences.join(" ");
    let words: Vec<&str> = joined.split_whitespace().collect();
    let mut constrained = words
        .iter()
        .take(MAX_REPLY_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let mut truncated =
        original_sentence_count > MAX_REPLY_SENTENCES || words.len() > MAX_REPLY_WORDS;
    if constrained.chars().count() > MAX_REPLY_CHARS {
        constrained = constrained.chars().take(MAX_REPLY_CHARS).collect();
        constrained = constrained.trim_end().to_string();
        truncated = true;
    }
    if truncated {
        constrained = constrained
            .trim_end_matches(['.', '!', '?', '…'])
            .trim_end()
            .to_string();
        constrained.push('…');
        log!(
            "reply constrained to {} words / {} characters",
            constrained.split_whitespace().count(),
            constrained.chars().count()
        );
    }
    sentences.clear();
    if !constrained.is_empty() {
        sentences.push(constrained);
    }
}

fn cleanup_page_png() {
    if std::path::Path::new(PAGE_PNG_PATH).exists() {
        let _ = std::fs::remove_file(PAGE_PNG_PATH);
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn ordinary_reply_is_bounded_even_when_model_overruns() {
        let mut sentences = vec![
            (0..30)
                .map(|i| format!("first{i}"))
                .collect::<Vec<_>>()
                .join(" "),
            (0..30)
                .map(|i| format!("second{i}"))
                .collect::<Vec<_>>()
                .join(" "),
            "Third sentence.".to_string(),
            "A forbidden fourth sentence.".to_string(),
        ];
        constrain_reply(&mut sentences);
        assert_eq!(sentences.len(), 1);
        assert!(sentences[0].split_whitespace().count() <= MAX_REPLY_WORDS);
        assert!(sentences[0].chars().count() <= MAX_REPLY_CHARS + 1);
        assert!(sentences[0].ends_with('…'));
        assert!(!sentences[0].contains("forbidden fourth"));
    }

    #[test]
    fn short_reply_is_unchanged_except_for_sentence_spacing() {
        let mut sentences = vec!["One answer.".to_string(), "Another answer.".to_string()];
        constrain_reply(&mut sentences);
        assert_eq!(sentences, vec!["One answer. Another answer."]);
    }

    #[test]
    fn connecting_lines_are_exactly_25_and_keep_the_voice() {
        assert_eq!(CONNECTING_LINES.len(), 25, "the spec calls for 25 lines");
        let mut openings = std::collections::HashSet::new();
        for (i, line) in CONNECTING_LINES.iter().enumerate() {
            assert!(!line.trim().is_empty(), "line {i} is empty");
            // the connecting overlay must read as the diary, not as a status bar
            let low = line.to_ascii_lowercase();
            for forbidden in ["wifi", "wi-fi", "oracle", "loading", "connecting"] {
                assert!(
                    !low.contains(forbidden),
                    "line {i} leaks the term {forbidden:?}: {line}"
                );
            }
            // distinct openings (the persona forbids repeated salutations)
            let opening = line
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(openings.insert(opening), "line {i} repeats an opening");
        }
    }
}
