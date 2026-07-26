//! The diary's daily journal.
//!
//! One entry per day of conversation, stored as plain files next to the
//! conversation memory: `<date>.txt` (the entry text) and `<date>.strokes`
//! (the handwriting that wrote it, one stroke per line as `x,y;x,y;…` —
//! reply strokes are always r=2, so the radius is not stored). Entries are
//! composed automatically on quit and can be conjured back onto the page.
//!
//! Civil-date math is done by hand (Howard Hinnant's algorithms, like
//! riddle) — no chrono. Devices usually run UTC; `HORCRUX_TZ_OFFSET` shifts
//! "today" by whole hours.

use crate::log;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Most recent pages of a day offered to the journal prompt.
pub const MAX_JOURNAL_PAGES: usize = 40;
/// Entries with more stored points are talked about instead of conjured.
pub const CONJURE_MAX_POINTS: usize = 4000;
/// Conjured ink is faded, not black.
pub const CONJURE_GRAY: u8 = 170;

/// Inked when the writer asks for a day that has no entry.
pub const CANNED_NO_ENTRY: &str =
    "I turn those pages and find them blank. That day has no entry yet.";

// --- dates ----------------------------------------------------------------------

/// Days since 1970-01-01 from a civil date (Hinnant's days_from_civil).
/// Currently exercised by the date-math tests.
#[allow(dead_code)]
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Civil (year, month, day) from days since 1970-01-01 (civil_from_days).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y } as i32, m, d)
}

/// The local calendar date of a unix timestamp, with the configured
/// whole-hour offset applied to UTC.
pub fn local_ymd(unix_secs: i64, tz_offset_hours: i32) -> (i32, u32, u32) {
    let secs = unix_secs + tz_offset_hours as i64 * 3600;
    civil_from_days(secs.div_euclid(86400))
}

pub fn format_ymd((y, m, d): (i32, u32, u32)) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today's local date as `YYYY-MM-DD`.
pub fn today_ymd(tz_offset_hours: i32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_ymd(local_ymd(now as i64, tz_offset_hours))
}

// --- storage ---------------------------------------------------------------------

/// The journal lives in a `journal/` dir next to the memory dir
/// (default: /home/root/horcrux-data/journal).
pub fn journal_dir(memory_dir: &Path) -> PathBuf {
    memory_dir.parent().unwrap_or(memory_dir).join("journal")
}

pub fn entry_path(dir: &Path, ymd: &str) -> PathBuf {
    dir.join(format!("{ymd}.txt"))
}

fn strokes_path(dir: &Path, ymd: &str) -> PathBuf {
    dir.join(format!("{ymd}.strokes"))
}

/// Write `<date>.txt` and `<date>.strokes`, creating the dir on demand.
pub fn save_entry(
    dir: &Path,
    ymd: &str,
    text: &str,
    strokes: &[Vec<(i32, i32)>],
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(entry_path(dir, ymd), format!("{text}\n"))?;
    fs::write(strokes_path(dir, ymd), encode_strokes(strokes))?;
    Ok(())
}

pub fn entry_text(dir: &Path, ymd: &str) -> Option<String> {
    fs::read_to_string(entry_path(dir, ymd))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// One stroke per line: `x,y;x,y;…` (r is always 2 for reply strokes).
fn encode_strokes(strokes: &[Vec<(i32, i32)>]) -> String {
    let mut out = String::new();
    for stroke in strokes {
        if stroke.is_empty() {
            continue;
        }
        for (i, (x, y)) in stroke.iter().enumerate() {
            if i > 0 {
                out.push(';');
            }
            out.push_str(&format!("{x},{y}"));
        }
        out.push('\n');
    }
    out
}

pub fn load_strokes(dir: &Path, ymd: &str) -> Option<Vec<Vec<(i32, i32)>>> {
    let text = fs::read_to_string(strokes_path(dir, ymd)).ok()?;
    let mut strokes = Vec::new();
    for line in text.lines() {
        let mut stroke = Vec::new();
        for pt in line.split(';') {
            let mut f = pt.split(',');
            if let (Some(xs), Some(ys)) = (f.next(), f.next()) {
                if let (Ok(x), Ok(y)) = (xs.parse::<i32>(), ys.parse::<i32>()) {
                    stroke.push((x, y));
                }
            }
        }
        if !stroke.is_empty() {
            strokes.push(stroke);
        }
    }
    Some(strokes)
}

// --- the entry prompt ---------------------------------------------------------------

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Assemble the journal user prompt from today's (transcript, reply) pairs.
pub fn build_journal_prompt(pages: &[(String, String)], today: &str) -> String {
    let mut p = String::from("Today the writer wrote these pages in the diary:\n\n");
    for (transcript, reply) in pages {
        p.push_str(&format!(
            "- Writer: {}\n  You: {}\n",
            one_line(transcript),
            one_line(reply)
        ));
    }
    p.push_str("\nWrite today's entry in your diary as Tom: a first-person diary entry about this day's conversation, in your own voice, no more than five sentences. Begin with the date in the form '26 July, in the evening.' Write ONLY the entry text (no \u{2042} transcription line this time).");
    // The model cannot know the date otherwise; the persona is unchanged.
    p.push_str(&format!("\n\nToday's date is {today}."));
    p
}

// --- recall ---------------------------------------------------------------------------

/// What to do when the writer asks to see a past day's entry.
pub enum Recall {
    /// No entry for that day — ink the canned line.
    Missing,
    /// Replay the stored strokes, fast and faded.
    Conjure(Vec<Vec<(i32, i32)>>),
    /// Entry too long (or strokes unreadable) — ink its first sentences.
    Talk(Vec<String>),
}

pub fn recall(dir: &Path, ymd: &str) -> Recall {
    let Some(text) = entry_text(dir, ymd) else {
        return Recall::Missing;
    };
    match load_strokes(dir, ymd) {
        Some(strokes) if strokes.iter().map(|s| s.len()).sum::<usize>() <= CONJURE_MAX_POINTS => {
            Recall::Conjure(strokes)
        }
        _ => {
            let mut sentences = crate::oracle::split_sentences(&text);
            if sentences.is_empty() {
                sentences.push(text);
            }
            sentences.truncate(2);
            Recall::Talk(sentences)
        }
    }
}

/// Validate a `YYYY-MM-DD` date string (shape and rough ranges).
pub fn valid_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let m: u32 = s[5..7].parse().unwrap_or(0);
    let d: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// Log helper for recall decisions (keeps main.rs tidy).
pub fn log_recall(ymd: &str, what: &str) {
    log!("recall {ymd}: {what}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "horcrux-jtest-{tag}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn date_of_id_with_tz() {
        // 2025-01-01 00:00:00 UTC
        let midnight = 1735689600;
        assert_eq!(format_ymd(local_ymd(midnight, 0)), "2025-01-01");
        assert_eq!(format_ymd(local_ymd(midnight, 1)), "2025-01-01"); // 01:00 local
        assert_eq!(format_ymd(local_ymd(midnight, -1)), "2024-12-31"); // 23:00 local
                                                                       // one second before UTC midnight: already tomorrow at +1
        assert_eq!(format_ymd(local_ymd(midnight - 1, 0)), "2024-12-31");
        assert_eq!(format_ymd(local_ymd(midnight - 1, 1)), "2025-01-01");
        // and still yesterday at -1
        assert_eq!(format_ymd(local_ymd(midnight - 1, -1)), "2024-12-31");
    }

    #[test]
    fn civil_roundtrip() {
        for (y, m, d) in [(1970, 1, 1), (2026, 7, 26), (2000, 2, 29), (1900, 3, 1)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn prompt_escapes_and_dates() {
        let pages = vec![
            ("line one\nline two".to_string(), "a reply".to_string()),
            ("second page".to_string(), "another\nreply".to_string()),
        ];
        let p = build_journal_prompt(&pages, "2026-07-26");
        assert!(p.contains("- Writer: line one line two\n  You: a reply\n"));
        assert!(p.contains("- Writer: second page\n  You: another reply\n"));
        assert!(p.contains("Today's date is 2026-07-26."));
        assert!(p.contains("no \u{2042} transcription line"));
    }

    #[test]
    fn strokes_roundtrip() {
        let dir = tempdir("rt");
        let strokes = vec![vec![(1, 2), (3, 4), (5, 6)], vec![(700, 900), (701, 901)]];
        save_entry(&dir, "2026-07-26", "Dear diary.", &strokes).unwrap();
        assert_eq!(
            entry_text(&dir, "2026-07-26").as_deref(),
            Some("Dear diary.")
        );
        assert_eq!(load_strokes(&dir, "2026-07-26"), Some(strokes));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recall_paths() {
        let dir = tempdir("recall");
        // missing entirely
        assert!(matches!(recall(&dir, "2026-07-01"), Recall::Missing));
        // small entry -> conjure
        let small = vec![vec![(10, 10); 100]];
        save_entry(&dir, "2026-07-02", "One. Two.", &small).unwrap();
        match recall(&dir, "2026-07-02") {
            Recall::Conjure(s) => assert_eq!(s, small),
            _ => panic!("expected conjure"),
        }
        // huge entry -> talk about it (first two sentences)
        let big = vec![vec![(0, 0); CONJURE_MAX_POINTS + 1]];
        save_entry(
            &dir,
            "2026-07-03",
            "First day. Second sentence! Third one?",
            &big,
        )
        .unwrap();
        match recall(&dir, "2026-07-03") {
            Recall::Talk(s) => assert_eq!(s, vec!["First day.", "Second sentence!"]),
            _ => panic!("expected talk"),
        }
        // text without strokes file -> talk
        fs::write(entry_path(&dir, "2026-07-04"), "Only text. No strokes.").unwrap();
        match recall(&dir, "2026-07-04") {
            Recall::Talk(s) => assert_eq!(s.len(), 2),
            _ => panic!("expected talk"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ymd_validation() {
        assert!(valid_ymd("2026-07-26"));
        assert!(valid_ymd("1999-12-31"));
        assert!(!valid_ymd("2026-13-01"));
        assert!(!valid_ymd("2026-07-32"));
        assert!(!valid_ymd("26-07-2026"));
        assert!(!valid_ymd("2026/07/26"));
        assert!(!valid_ymd("entry:x"));
    }
}
