//! horcrux — Tom Riddle's diary on a reMarkable 2.
//!
//! Write with the pen; after a short idle the ink is drunk away, a vision LLM
//! (the "oracle") answers, and the reply writes itself on the page in
//! animated handwriting. Five-finger tap on the touchscreen quits.
//!
//! Single-threaded state machine on a ~2ms loop; the only thread is one
//! oracle worker per turn, talking back over a channel.

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
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use surface::{Rect, Surface};

const PAGE_PNG_PATH: &str = "/tmp/horcrux-page.png";
const DEFAULT_TZ_NAME: &str = "Australia/Perth";
const DEFAULT_TZ_OFFSET_HOURS: i32 = 8;
const DEFAULT_LIBRARY_DIR: &str = "/home/root/.local/share/remarkable/xochitl";

const LOOP_MS: u64 = 2;
const INK_FLUSH_MS: u64 = 15;
const MIN_PRESSURE: i32 = 40;

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
    /// Waiting on the oracle's first sentence; the page stays blank (no
    /// blinking indicator — it read as a defect on the e-ink panel).
    Thinking,
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
}

/// Replay cursor over a recalled entry's stored strokes.
struct Conjure {
    strokes: Vec<Vec<(i32, i32)>>,
    si: usize,
    pi: usize,
}

impl App {
    fn new(surf: Surface, input: Input, memory: Memory, oracle_cfg: Option<OracleConfig>) -> App {
        let now = Instant::now();
        let journal_dir = configured_journal_dir(&memory);
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
        }
    }

    fn loop_forever(&mut self) {
        loop {
            if QUIT.load(Ordering::SeqCst) || self.want_quit {
                log!("quitting");
                break;
            }
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
            std::thread::sleep(Duration::from_millis(LOOP_MS));
        }
        self.shutdown();
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
                    State::Journaling { .. } | State::JournalLingering { .. }
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
                    State::Listening => self.on_pen_listening(s),
                    State::Lingering { .. } | State::ConjureLingering { .. } if s.touching => {
                        self.start_fading()
                    }
                    _ => {} // pen ignored during animations/oracle wait
                }
                false
            }
            InputEvent::DoubleTap => {
                // dismiss the reply early
                if matches!(
                    self.state,
                    State::Lingering { .. } | State::ConjureLingering { .. }
                ) {
                    self.start_fading();
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
        self.state = State::Journaling {
            next: Instant::now(),
        };
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
        oracle::spawn(
            self.oracle_cfg.clone(),
            png,
            self.memory.history(),
            time_context,
            journal_today,
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
        let mut got_ink = false;
        for ev in events {
            match ev {
                OracleEvent::Ink(s) => {
                    self.reply_sentences.push(s);
                    got_ink = true;
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
                OracleEvent::Done => {
                    self.oracle_done = true;
                }
            }
        }
        if got_ink
            && matches!(
                self.state,
                State::Thinking | State::Replying { .. } | State::Journaling { .. }
            )
        {
            self.replan();
        }
    }

    fn begin_reply(&mut self) {
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

    /// (Re)plan the handwriting for the full reply text. Greedy wrap keeps
    /// earlier lines stable, so already-drawn strokes are an exact prefix.
    fn replan(&mut self) {
        if self.reply_sentences.is_empty() {
            return;
        }
        // Each sentence is its own paragraph, so a newly arrived sentence
        // always starts a fresh line: earlier lines (and their strokes)
        // never change, keeping the plan prefix-stable.
        let plan_text = self.reply_sentences.join("\n");
        let block_top = self.plan.as_ref().map(|p| p.block_top);
        match script::plan_reply(&plan_text, block_top, self.turn_seed) {
            Some(plan) => self.plan = Some(plan),
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
        if text.is_empty() {
            log!("journal entry empty (oracle failed?), not saving");
        } else {
            let strokes = self
                .plan
                .as_ref()
                .map(|p| p.strokes.clone())
                .unwrap_or_default();
            match journal::save_entry(&self.journal_dir, &today, &text, &strokes) {
                Ok(()) => {
                    log!("journal entry saved for {today}");
                    let time_context =
                        journal::current_time_context(&self.tz_name, self.tz_offset_hours);
                    match journal::publish_diary(
                        &self.journal_dir,
                        &self.library_dir,
                        &time_context,
                    ) {
                        Ok(path) => log!("stock-library diary refreshed from {}", path.display()),
                        Err(error) => log!("stock-library diary refresh failed: {error}"),
                    }
                }
                Err(e) => log!("journal save failed: {e}"),
            }
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
                    self.drain_oracle();
                    if !self.reply_sentences.is_empty() || self.oracle_done {
                        self.begin_reply();
                    } else {
                        self.state = State::Thinking;
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
                if !self.reply_sentences.is_empty() || self.oracle_done {
                    self.begin_reply();
                }
            }
            State::Replying { next } => {
                self.drain_oracle();
                if now >= next {
                    self.replay_tick(false);
                }
            }
            State::Lingering { until } => {
                if now >= until {
                    self.start_fading();
                }
            }
            State::Journaling { next } => {
                self.drain_oracle();
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
        }
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

fn cleanup_page_png() {
    if std::path::Path::new(PAGE_PNG_PATH).exists() {
        let _ = std::fs::remove_file(PAGE_PNG_PATH);
    }
}
