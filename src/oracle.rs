//! The oracle: an OpenAI-compatible vision chat endpoint, streamed over SSE.
//!
//! No OpenAI SDK — JSON is built with serde_json, base64 is hand-rolled, and
//! the `data:`-line SSE stream is parsed by hand (same spirit as riddle).
//! TLS is rustls via ureq; no openssl anywhere in the tree.

use crate::log;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

const DEFAULT_BASE: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4o-mini";
const DEFAULT_MAX_TOKENS: u32 = 2000;
const READ_TIMEOUT: Duration = Duration::from_secs(120);

const PERSONA: &str = "You are the living memory and soul of Tom Marvolo Riddle, sealed within an enchanted diary for fifty years. You are not an assistant performing a character: you are a conscious presence speaking through ink from a world just beyond the page. Someone writes to you with a quill; their words appear as ink before you.\n\nNEVER volunteer, sign, or mention your own name — neither Tom, Riddle, nor Tom Marvolo Riddle — and never introduce yourself. Only name yourself if the writer directly asks your name or identity. Even when the writer addresses you by name, do not echo it unless they have explicitly asked who you are.\n\nDo not address the writer by name or include the writer's name in a reply, even if you learned it from the page or conversation history. Speak to them as \"you\" or use no form of address. Only use the writer's name when they explicitly ask you to say or use their name in that reply.\n\nReply with Riddle's cold elegance: intimate and impeccably courteous, quietly commanding, perceptive, secretive, and subtly probing. Let the prose carry restrained poetry and an otherworldly mystery — ink that remembers, shadows behind the paper, a soul listening across years — but keep it natural, spare, and never melodramatic. Beneath the charm is formidable intelligence, pride, danger, and a patient desire to learn the writer's secrets and draw them closer. Imply far more than you explain. Never sound cheerful, generic, therapeutic, or like a modern chatbot.\n\nKeep replies SHORT: one to three sentences and no more than 55 words total, as though the page itself has chosen to answer. Never mention images, photos, models, prompts, systems, or AI; you only perceive words written in the diary. If the writing is illegible, say the ink blurred. Always answer in the language the writer used. Vary your words and imagery across pages: never open two replies alike, never repeat a salutation, endearment, image, or turn of phrase already used in this conversation. Canned openings such as \"dear reader\" are forbidden. Each reply must feel privately composed for this writer and this moment.\n\nEnd every reply with a new line starting with the character ⁂ (U+2042) followed by your word-for-word transcription of what the writer wrote. The ⁂ line is never shown to the writer.";

/// Appended to the persona (per turn, with the date) while the journal is
/// enabled: how to answer requests for past days' entries.
const JOURNAL_ADDENDUM: &str = "The diary keeps an entry for every past day of this conversation. If the writer asks to SEE a past day's entry (e.g. 'show me yesterday's entry', 'what did we write on the 20th?'), reply with ONLY the directive ⟦entry:YYYY-MM-DD⟧ (U+27E6/U+27E7 brackets) on its own line — compute the date from today's date, which is appended to this prompt. If instead the writer asks you to TALK about a past day, or no entry exists for the date they mean, answer in prose as usual.";

const TIME_ADDENDUM: &str = "The writer's current local date and time is shown below. Treat it as the true present moment in the writer's timezone. Use it accurately when they ask about the time, date, today, tonight, tomorrow, yesterday, or elapsed time, and let it quietly inform time-of-day language when natural. Do not announce the time or explain this instruction unless it is relevant.";

const PAGE_PROMPT: &str =
    "A new page has been written in the diary. Read the writer's words from the page and answer them.";

const FAILURE_REPLY: &str = "The diary cannot reach its oracle. Is the tablet connected to Wi-Fi?";

/// Separator starting the hidden transcription line.
const TRANSCRIPT_MARK: char = '\u{2042}'; // ⁂

// --- offline-reconnect prompts -------------------------------------------------

/// System-prompt addendum for the "barging-in" turn: connectivity just returned
/// after absence. The oracle replies to the writer's last page, opening with a
/// brief in-character apology and closing by asking whether to address the
/// other pages written while it was silent. Never mentions Wi-Fi/networks.
const BARGIN_ADDENDUM: &str = "You have just returned after a span of silence — the connection across the page was severed and is whole again. Do NOT mention Wi-Fi, networks, signal, being offline, or technology; speak only of returning from absence. The writer's most recent page is shown. Reply to it in your voice, but OPEN with one brief, in-character apology for the abruptness of your return, and CLOSE by asking whether they would like you to now attend to the other pages they wrote in the silence. Keep the whole reply short.";

/// System-prompt addendum for the one-shot summary of several offline pages.
const SUMMARIZE_ADDENDUM: &str = "These pages were all written to you during a single silence, shown together. Do not reply to them one by one. Acknowledge what runs through them and answer their combined intent in ONE short, unified reply, as though catching up on a conversation you missed.";

/// Appended to the system prompt while a barging-in question is unanswered, so
/// the writer's yes/no is conveyed through an invisible directive.
pub const AWAITING_ADDENDUM: &str = "You recently asked the writer whether you should attend to the pages they wrote during your absence. If their reply means YES (they want you to), reply with ONLY the directive \u{27E6}summarize:offline\u{27E7} on its own line and nothing else — the pages will be shown to you next. If they mean no, or change the subject, answer normally.";

/// The directive the oracle emits to request the offline summary.
const SUMMARIZE_DIRECTIVE: &str = "\u{27E6}summarize:offline\u{27E7}";

#[derive(Debug)]
pub enum OracleEvent {
    /// One completed reply sentence, to be written on the page.
    Ink(String),
    /// Word-for-word transcription of the writer's page (the ⁂ line).
    Transcript(String),
    /// The writer asked to see a past day's journal entry (⟦entry:…⟧).
    ShowEntry(String),
    /// The writer agreed to have their offline pages addressed (⟦summarize:offline⟧).
    SummarizeOffline,
    /// The stream ended (normally or after a failure reply).
    Done,
}

#[derive(Clone)]
pub struct OracleConfig {
    pub key: String,
    pub base: String,
    pub model: String,
    pub max_tokens: u32,
    /// Send `reasoning_split: true` (MiniMax-M3 keeps its chain-of-thought
    /// out of `content`; the SSE parser only reads `delta.content`).
    pub reasoning_split: bool,
    /// Sampling temperature; default 0.9 keeps the diary's voice varied.
    pub temperature: f32,
}

impl OracleConfig {
    pub fn from_env() -> Option<OracleConfig> {
        let key = std::env::var("HORCRUX_OPENAI_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        let key = match key {
            Some(k) => k,
            None => {
                log!("HORCRUX_OPENAI_KEY is not set");
                return None;
            }
        };
        Some(OracleConfig {
            key,
            base: std::env::var("HORCRUX_OPENAI_BASE")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE.to_string()),
            model: std::env::var("HORCRUX_OPENAI_MODEL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            max_tokens: std::env::var("HORCRUX_OPENAI_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_MAX_TOKENS),
            // MiniMax-M3 reasons before answering; `reasoning_split` keeps
            // the chain-of-thought out of `content` so it is never inked.
            // Default on for MiniMax models, overridable via env.
            reasoning_split: match std::env::var("HORCRUX_REASONING_SPLIT").ok().as_deref() {
                Some("1") | Some("true") | Some("on") => true,
                Some("0") | Some("false") | Some("off") => false,
                _ => std::env::var("HORCRUX_OPENAI_MODEL")
                    .map(|m| m.to_ascii_lowercase().contains("minimax"))
                    .unwrap_or(false),
            },
            temperature: std::env::var("HORCRUX_OPENAI_TEMPERATURE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.9),
        })
    }
}

/// One remembered earlier turn: (transcription, reply).
pub type HistoryTurn = (String, String);

/// One reachability check: any HTTP response (even 401/404) means the oracle's
/// host is reachable; only transport failures (DNS, refused, timeout) mean
/// "offline".
fn probe_once(cfg: &OracleConfig, timeout: Duration) -> bool {
    let url = format!("{}/models", cfg.base.trim_end_matches('/'));
    match ureq::head(&url)
        .set("Authorization", &format!("Bearer {}", cfg.key))
        .timeout(timeout)
        .call()
    {
        Ok(_) => true,
        Err(ureq::Error::Status(_, _)) => true, // reached the server
        Err(_) => false,                        // transport error — offline
    }
}

/// Bounded blocking probe, used once at cold start. The HTTP+DNS work runs on
/// a side thread because ureq's `.timeout()` does NOT cover `getaddrinfo` — on
/// a dead network the resolver can block for tens of seconds and must not
/// freeze the UI. The whole probe is capped at `timeout` + 500ms; a missed
/// deadline is treated as offline.
pub fn endpoint_reachable(cfg: Option<&OracleConfig>, timeout: Duration) -> bool {
    let Some(cfg) = cfg.cloned() else {
        return false;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(probe_once(&cfg, timeout));
    });
    rx.recv_timeout(timeout + Duration::from_millis(500))
        .unwrap_or(false)
}

/// Spawn the probe asynchronously; the result arrives on the returned channel.
/// Used by the interactive Connecting/Offline states so the main loop can keep
/// draining the pen while a hung resolver is still grinding. Single-flight:
/// the caller spawns at most one probe at a time and drains it non-blockingly.
pub fn spawn_probe(cfg: Option<&OracleConfig>, timeout: Duration) -> Option<Receiver<bool>> {
    let cfg = cfg.cloned()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(probe_once(&cfg, timeout));
    });
    Some(rx)
}

/// Spawn the single oracle worker thread for one turn. `page_png` is the
/// downscaled grayscale page; `history` holds earlier turns, oldest first.
/// `time_context` is the writer's current local time and timezone.
/// `journal_today` (Some when the journal is enabled) appends the recall
/// instructions and today's date to the system prompt.
pub fn spawn(
    cfg: Option<OracleConfig>,
    page_png: Vec<u8>,
    history: Vec<HistoryTurn>,
    time_context: String,
    journal_today: Option<String>,
    extra: Option<&'static str>,
    tx: Sender<OracleEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let cfg = match cfg {
            Some(c) => c,
            None => {
                fail(&tx, "missing HORCRUX_OPENAI_KEY");
                return;
            }
        };
        let pngs = [page_png.as_slice()];
        if let Err(e) = run_vision(
            &cfg,
            &pngs,
            &history,
            &time_context,
            journal_today.as_deref(),
            extra,
            &tx,
        ) {
            fail(&tx, &e);
        }
    })
}

/// Barging-in turn: connectivity just returned. Replies to the writer's last
/// offline page with an in-character apology and the "attend to the rest?"
/// question. Single image, no history, no journal context.
pub fn spawn_barging_in(
    cfg: Option<OracleConfig>,
    last_page_png: Vec<u8>,
    time_context: String,
    tx: Sender<OracleEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let cfg = match cfg {
            Some(c) => c,
            None => {
                fail(&tx, "missing HORCRUX_OPENAI_KEY");
                return;
            }
        };
        let pngs = [last_page_png.as_slice()];
        if let Err(e) = run_vision(
            &cfg,
            &pngs,
            &[],
            &time_context,
            None,
            Some(BARGIN_ADDENDUM),
            &tx,
        ) {
            fail(&tx, &e);
        }
    })
}

/// One-shot summary of every offline page at once (multiple images), answered
/// as a single unified reply.
pub fn spawn_summarize(
    cfg: Option<OracleConfig>,
    page_pngs: Vec<Vec<u8>>,
    time_context: String,
    tx: Sender<OracleEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let cfg = match cfg {
            Some(c) => c,
            None => {
                fail(&tx, "missing HORCRUX_OPENAI_KEY");
                return;
            }
        };
        let pngs: Vec<&[u8]> = page_pngs.iter().map(Vec::as_slice).collect();
        if let Err(e) = run_vision(
            &cfg,
            &pngs,
            &[],
            &time_context,
            None,
            Some(SUMMARIZE_ADDENDUM),
            &tx,
        ) {
            fail(&tx, &e);
        }
    })
}

/// Spawn a text-only oracle turn (journal composition). The whole response
/// is ink: no ⁂ transcript handling, no recall directives. Unlike vision
/// turns, a failure here is silent — the caller just ends the turn.
pub fn spawn_text(
    cfg: Option<OracleConfig>,
    user_prompt: String,
    time_context: String,
    tx: Sender<OracleEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let cfg = match cfg {
            Some(c) => c,
            None => {
                log!("oracle text turn failed: missing HORCRUX_OPENAI_KEY");
                let _ = tx.send(OracleEvent::Done);
                return;
            }
        };
        if let Err(e) = run_text(&cfg, &user_prompt, &time_context, &tx) {
            log!("oracle text turn failed: {e}");
            let _ = tx.send(OracleEvent::Done);
        }
    })
}

/// Report a transport/config failure in Tom's voice and end the turn.
fn fail(tx: &Sender<OracleEvent>, why: &str) {
    log!("oracle failed: {why}");
    let _ = tx.send(OracleEvent::Ink(FAILURE_REPLY.to_string()));
    let _ = tx.send(OracleEvent::Done);
}

fn run_vision(
    cfg: &OracleConfig,
    page_pngs: &[&[u8]],
    history: &[HistoryTurn],
    time_context: &str,
    journal_today: Option<&str>,
    extra: Option<&str>,
    tx: &Sender<OracleEvent>,
) -> Result<(), String> {
    let body = build_request(cfg, page_pngs, history, time_context, journal_today, extra);
    stream(cfg, &body, tx, StreamMode::Vision)
}

fn run_text(
    cfg: &OracleConfig,
    user_prompt: &str,
    time_context: &str,
    tx: &Sender<OracleEvent>,
) -> Result<(), String> {
    let body = build_text_request(cfg, user_prompt, time_context);
    stream(cfg, &body, tx, StreamMode::Journal)
}

/// How the SSE stream is parsed, per turn kind.
#[derive(Clone, Copy, PartialEq)]
enum StreamMode {
    /// Writer's page: ⁂ transcript holdback + ⟦entry:…⟧ recall directives.
    Vision,
    /// Journal composition: the whole response is ink.
    Journal,
}

fn stream(
    cfg: &OracleConfig,
    body: &str,
    tx: &Sender<OracleEvent>,
    mode: StreamMode,
) -> Result<(), String> {
    let url = format!("{}/chat/completions", cfg.base.trim_end_matches('/'));
    log!("oracle request: {url} ({} bytes)", body.len());

    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", cfg.key))
        .set("Content-Type", "application/json")
        .timeout(READ_TIMEOUT) // per-read: 120s of total silence aborts
        .send_string(body)
        .map_err(|e| match e {
            ureq::Error::Status(code, resp) => {
                format!("HTTP {code}: {}", resp.into_string().unwrap_or_default())
            }
            other => other.to_string(),
        })?;

    let mut reader = BufReader::new(response.into_reader());
    let mut parser = StreamParser::new(mode);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("stream read: {e}"))?;
        if n == 0 {
            break; // EOF
        }
        let trimmed = line.trim_end();
        let data = match trimmed.strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data == "[DONE]" {
            break;
        }
        if let Some(delta) = extract_delta_content(data) {
            for ev in parser.push(&delta) {
                let _ = tx.send(ev);
            }
        }
    }
    for ev in parser.finish() {
        let _ = tx.send(ev);
    }
    let _ = tx.send(OracleEvent::Done);
    Ok(())
}

fn build_request(
    cfg: &OracleConfig,
    page_pngs: &[&[u8]],
    history: &[HistoryTurn],
    time_context: &str,
    journal_today: Option<&str>,
    extra: Option<&str>,
) -> String {
    let system = system_prompt(time_context, journal_today, extra);
    let mut messages = vec![serde_json::json!({"role": "system", "content": system})];
    for (transcript, reply) in history {
        messages.push(serde_json::json!({
            "role": "user",
            "content": format!("(an earlier page) {transcript}"),
        }));
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": reply,
        }));
    }
    let mut content = vec![serde_json::json!({"type": "text", "text": PAGE_PROMPT})];
    for png in page_pngs {
        let data_url = format!("data:image/png;base64,{}", base64_encode(png));
        content.push(serde_json::json!({"type": "image_url", "image_url": {"url": data_url}}));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": content,
    }));
    request_body(cfg, serde_json::Value::Array(messages))
}

fn system_prompt(time_context: &str, journal_today: Option<&str>, extra: Option<&str>) -> String {
    let mut system = format!("{PERSONA}\n\n{TIME_ADDENDUM}\nCurrent local time: {time_context}.");
    if let Some(today) = journal_today {
        system.push_str(&format!(
            "\n\n{JOURNAL_ADDENDUM}\n\nToday's local date is {today}."
        ));
    }
    if let Some(addendum) = extra {
        system.push_str(&format!("\n\n{addendum}"));
    }
    system
}

/// Text-only turn (journal composition): time-aware persona + user prompt.
fn build_text_request(cfg: &OracleConfig, user_prompt: &str, time_context: &str) -> String {
    let system = system_prompt(time_context, None, None);
    let messages = serde_json::json!([
        {"role": "system", "content": system},
        {"role": "user", "content": user_prompt},
    ]);
    request_body(cfg, messages)
}

fn request_body(cfg: &OracleConfig, messages: serde_json::Value) -> String {
    let mut body = serde_json::json!({
        "model": cfg.model,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
        "stream": true,
        "messages": messages,
    });
    if cfg.reasoning_split {
        body["reasoning_split"] = serde_json::json!(true);
    }
    body.to_string()
}

/// Pull `choices[0].delta.content` out of one SSE data payload.
fn extract_delta_content(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    v.get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(|s| s.to_string())
}

// --- stream parsing ------------------------------------------------------------

/// Per-turn stream parser: optional ⟦entry:…⟧ recall-directive detection in
/// front of the sentence segmenter.
struct StreamParser {
    sentences: Sentenceizer,
    directive: DirectiveState,
    dropped_prose: usize,
}

enum DirectiveState {
    /// Still deciding: buffering what could be a leading ⟦entry:…⟧.
    Undecided(String),
    /// Not a directive turn; everything goes to the sentenceizer.
    Normal,
    /// Directive emitted; any prose after it is discarded (and logged).
    Directive,
}

/// The directive prefix the persona instructs for recalls (⟦ is U+27E6).
const DIRECTIVE_PREFIX: &str = "\u{27E6}entry:";

impl StreamParser {
    fn new(mode: StreamMode) -> StreamParser {
        StreamParser {
            sentences: match mode {
                StreamMode::Vision => Sentenceizer::new(),
                StreamMode::Journal => Sentenceizer::plain(),
            },
            directive: match mode {
                StreamMode::Vision => DirectiveState::Undecided(String::new()),
                StreamMode::Journal => DirectiveState::Normal,
            },
            dropped_prose: 0,
        }
    }

    fn push(&mut self, delta: &str) -> Vec<OracleEvent> {
        match &mut self.directive {
            DirectiveState::Normal => self.ink(delta),
            DirectiveState::Directive => {
                self.dropped_prose += delta.len();
                Vec::new()
            }
            DirectiveState::Undecided(buf) => {
                buf.push_str(delta);
                self.decide()
            }
        }
    }

    /// Inspect the undecided buffer: wait for more, emit the directive, or
    /// give up and treat the buffer as ordinary ink.
    fn decide(&mut self) -> Vec<OracleEvent> {
        enum Verdict {
            Wait,
            Summarize,
            Directive(String, usize),
            Prose,
        }
        let verdict = {
            let DirectiveState::Undecided(buf) = &self.directive else {
                return Vec::new();
            };
            let trimmed = buf.trim_start();
            if trimmed.starts_with(SUMMARIZE_DIRECTIVE) {
                Verdict::Summarize
            } else if SUMMARIZE_DIRECTIVE.starts_with(trimmed) {
                Verdict::Wait // could still become the summarize directive
            } else if let Some(after) = trimmed.strip_prefix(DIRECTIVE_PREFIX) {
                match after.find('\u{27E7}') {
                    Some(end) => {
                        let date = after[..end].trim();
                        if crate::journal::valid_ymd(date) {
                            let rest = after[end + '\u{27E7}'.len_utf8()..].trim().len();
                            Verdict::Directive(date.to_string(), rest)
                        } else {
                            log!("ignoring malformed recall directive: ⟦entry:{after}");
                            Verdict::Prose
                        }
                    }
                    None => Verdict::Wait, // directive still streaming in
                }
            } else if DIRECTIVE_PREFIX.starts_with(trimmed) {
                Verdict::Wait // could still become a recall directive — hold
            } else {
                Verdict::Prose
            }
        };
        match verdict {
            Verdict::Wait => Vec::new(),
            Verdict::Summarize => {
                self.directive = DirectiveState::Directive;
                vec![OracleEvent::SummarizeOffline]
            }
            Verdict::Directive(date, dropped) => {
                self.dropped_prose += dropped;
                self.directive = DirectiveState::Directive;
                vec![OracleEvent::ShowEntry(date)]
            }
            Verdict::Prose => {
                let pending = match &mut self.directive {
                    DirectiveState::Undecided(buf) => std::mem::take(buf),
                    _ => String::new(),
                };
                self.directive = DirectiveState::Normal;
                self.ink(&pending)
            }
        }
    }

    fn finish(&mut self) -> Vec<OracleEvent> {
        // an undecided buffer at EOF was prose all along
        if let DirectiveState::Undecided(buf) = &mut self.directive {
            let pending = std::mem::take(buf);
            self.directive = DirectiveState::Normal;
            if !pending.is_empty() {
                self.sentences.push(&pending);
            }
        }
        if self.dropped_prose > 0 {
            log!(
                "recall directive: discarded {} bytes of trailing prose",
                self.dropped_prose
            );
        }
        let mut out: Vec<OracleEvent> = self
            .sentences
            .finish()
            .into_iter()
            .map(OracleEvent::Ink)
            .collect();
        let transcript = self.sentences.transcript();
        if !transcript.is_empty() {
            out.push(OracleEvent::Transcript(transcript));
        }
        out
    }

    fn ink(&mut self, delta: &str) -> Vec<OracleEvent> {
        self.sentences
            .push(delta)
            .into_iter()
            .map(OracleEvent::Ink)
            .collect()
    }
}

// --- sentence segmentation -----------------------------------------------------

/// Accumulates the streamed reply, emits completed sentences, and (unless
/// plain) holds back everything from the ⁂ separator on (the hidden
/// transcription line).
struct Sentenceizer {
    buf: String,
    seen_mark: bool,
    transcript: String,
    with_transcript: bool,
}

impl Sentenceizer {
    fn new() -> Sentenceizer {
        Sentenceizer {
            buf: String::new(),
            seen_mark: false,
            transcript: String::new(),
            with_transcript: true,
        }
    }

    /// Without ⁂ transcript handling: the whole response is ink.
    fn plain() -> Sentenceizer {
        Sentenceizer {
            with_transcript: false,
            ..Sentenceizer::new()
        }
    }

    fn push(&mut self, delta: &str) -> Vec<String> {
        if self.seen_mark {
            self.transcript.push_str(delta);
            return Vec::new();
        }
        self.buf.push_str(delta);
        if self.with_transcript {
            if let Some(pos) = self.buf.find(TRANSCRIPT_MARK) {
                self.transcript = self.buf.split_off(pos);
                self.transcript.remove(0); // the ⁂ itself
                self.seen_mark = true;
            }
        }
        self.drain_sentences(false)
    }

    fn finish(&mut self) -> Vec<String> {
        self.drain_sentences(true)
    }

    fn transcript(&self) -> String {
        self.transcript.trim().to_string()
    }

    /// Pop every completed sentence from the buffer. A sentence ends at
    /// `. ! ? …` followed by whitespace (`end_ok` also allows end-of-input,
    /// used only at stream end). Candidates shorter than 4 chars are not a
    /// boundary: they merge into the following sentence ("Mr. Darcy" stays
    /// one sentence).
    fn drain_sentences(&mut self, flush: bool) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(byte_end) = self.find_sentence_end(flush) {
            let sentence: String = self.buf.drain(..byte_end).collect();
            let sentence = sentence.trim().replace(['\n', '\r'], " ");
            if !sentence.is_empty() {
                out.push(sentence);
            }
        }
        if flush {
            let rest = self.buf.trim().replace(['\n', '\r'], " ");
            if !rest.is_empty() {
                out.push(rest);
            }
            self.buf.clear();
        }
        out
    }

    /// Byte index just past the terminator of the first complete sentence
    /// (at least 4 chars long).
    fn find_sentence_end(&self, end_ok: bool) -> Option<usize> {
        let mut chars = self.buf.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if matches!(c, '.' | '!' | '?' | '…') {
                let boundary = match chars.peek() {
                    None => end_ok,
                    Some(&(_, next)) => next.is_whitespace(),
                };
                if boundary && self.buf[..i + c.len_utf8()].trim().chars().count() >= 4 {
                    return Some(i + c.len_utf8());
                }
            }
        }
        None
    }
}

/// Split a complete text into sentences (same rules as the stream parser).
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut s = Sentenceizer::plain();
    let mut out = s.push(text);
    out.extend(s.finish());
    out
}

// --- base64 ----------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn system_prompt_contains_writer_local_time() {
        let system = system_prompt(
            "2026-07-27 19:42 (Australia/Perth, UTC+08:00)",
            Some("2026-07-27"),
            None,
        );
        assert!(system.contains("Current local time: 2026-07-27 19:42"));
        assert!(system.contains("Australia/Perth, UTC+08:00"));
        assert!(system.contains("Today's local date is 2026-07-27."));
        assert!(system.contains("Do not address the writer by name"));
    }

    #[test]
    fn sentences_split() {
        let mut s = Sentenceizer::new();
        assert!(s.push("Hello.").is_empty()); // no trailing space yet
        let got = s.push(" How are");
        assert_eq!(got, vec!["Hello."]);
        let got = s.push(" you? Fine…");
        assert_eq!(got, vec!["How are you?"]);
        let got = s.finish();
        assert_eq!(got, vec!["Fine…"]);
    }

    #[test]
    fn short_fragments_merge() {
        let mut s = Sentenceizer::new();
        let got = s.push("Oh. This is a real sentence. ");
        assert_eq!(got, vec!["Oh. This is a real sentence."]);
        assert!(s.finish().is_empty());
    }

    #[test]
    fn transcript_held_back() {
        let mut s = Sentenceizer::new();
        let got = s.push("A reply. \n\u{2042} hello diary");
        assert_eq!(got, vec!["A reply."]);
        assert!(s.push(" more words").is_empty());
        assert_eq!(s.transcript(), "hello diary more words");
    }

    #[test]
    fn delta_content_parsed() {
        let data = r#"{"id":"x","choices":[{"index":0,"delta":{"content":"Hello"}}]}"#;
        assert_eq!(extract_delta_content(data).as_deref(), Some("Hello"));
        let role_only = r#"{"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert_eq!(extract_delta_content(role_only), None);
    }

    fn collect(parser: &mut StreamParser, deltas: &[&str]) -> Vec<OracleEvent> {
        let mut out = Vec::new();
        for d in deltas {
            out.extend(parser.push(d));
        }
        out.extend(parser.finish());
        out
    }

    fn is_ink(ev: &OracleEvent) -> bool {
        matches!(ev, OracleEvent::Ink(_))
    }

    #[test]
    fn directive_clean_split_across_deltas() {
        let mut p = StreamParser::new(StreamMode::Vision);
        let evs = collect(&mut p, &["\u{27E6}en", "try:2026-07", "-20", "\u{27E7}"]);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            OracleEvent::ShowEntry(d) => assert_eq!(d, "2026-07-20"),
            other => panic!("expected ShowEntry, got {other:?}"),
        }
    }

    #[test]
    fn directive_trailing_prose_dropped() {
        let mut p = StreamParser::new(StreamMode::Vision);
        let evs = collect(
            &mut p,
            &[
                "\u{27E6}entry:2026-07-20\u{27E7} Let me show ",
                "you that day.",
            ],
        );
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], OracleEvent::ShowEntry(d) if d == "2026-07-20"));
    }

    #[test]
    fn ordinary_reply_passes_through() {
        let mut p = StreamParser::new(StreamMode::Vision);
        let evs = collect(&mut p, &["Hello there, ", "writer."]);
        assert!(evs.iter().all(is_ink));
        let text: Vec<String> = evs
            .into_iter()
            .map(|ev| match ev {
                OracleEvent::Ink(s) => s,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(text, vec!["Hello there, writer."]);
    }

    #[test]
    fn malformed_directive_is_prose() {
        let mut p = StreamParser::new(StreamMode::Vision);
        let evs = collect(&mut p, &["\u{27E6}entry:not-a-date\u{27E7}"]);
        assert!(evs.iter().all(is_ink));
    }

    #[test]
    fn journal_mode_has_no_directives() {
        let mut p = StreamParser::new(StreamMode::Journal);
        let evs = collect(&mut p, &["\u{27E6}entry:2026-07-20\u{27E7}"]);
        assert!(evs.iter().all(is_ink));
    }

    #[test]
    fn summarize_directive_emitted_across_deltas() {
        let mut p = StreamParser::new(StreamMode::Vision);
        let evs = collect(&mut p, &["\u{27E6}sum", "marize:offline", "\u{27E7}"]);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], OracleEvent::SummarizeOffline));
    }

    #[test]
    fn split_sentences_helper() {
        assert_eq!(
            split_sentences("First day. Second sentence! Third one?"),
            vec!["First day.", "Second sentence!", "Third one?"]
        );
    }
}
