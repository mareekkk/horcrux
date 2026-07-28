//! Persistent diary memory.
//!
//! One TSV line per turn in `index.tsv` (id = unix seconds, transcript,
//! reply; tabs/newlines escaped) plus a `<id>.strokes` file with the
//! writer's ink, one stroke per line as `x,y,r;x,y,r;…`, decimated to one
//! point per 3px. Kept to the 400 most recent pages.

use crate::ink::Point;
use crate::oracle::HistoryTurn;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

const INDEX_FILE: &str = "index.tsv";
const MAX_PAGES: usize = 400;
/// Squared minimum distance between kept points (3px).
const DECIMATE_MIN_DIST2: i32 = 9;

pub struct Memory {
    dir: PathBuf,
    enabled: bool,
    turns: usize,
}

impl Memory {
    pub fn from_env() -> Memory {
        let enabled = std::env::var("HORCRUX_MEMORY")
            .map(|v| v != "off")
            .unwrap_or(true);
        let dir = std::env::var("HORCRUX_MEMORY_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/home/root/horcrux-data/memories".to_string());
        let turns = std::env::var("HORCRUX_MEMORY_TURNS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        Memory {
            dir: PathBuf::from(dir),
            enabled,
            turns,
        }
    }

    /// The last `turns` remembered (transcript, reply) pairs, oldest first.
    pub fn history(&self) -> Vec<HistoryTurn> {
        if !self.enabled {
            return Vec::new();
        }
        let Ok(text) = fs::read_to_string(self.dir.join(INDEX_FILE)) else {
            return Vec::new();
        };
        let mut all: Vec<HistoryTurn> = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                let (_id, t, r) = (f.next()?, f.next()?, f.next()?);
                Some((unescape(t), unescape(r)))
            })
            .collect();
        let excess = all.len().saturating_sub(self.turns);
        all.drain(..excess);
        all
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Every retained page written on local date `ymd`, chronological.
    /// The global memory store already caps retention at `MAX_PAGES`.
    pub fn pages_on(&self, ymd: &str, tz_offset_hours: i32) -> Vec<HistoryTurn> {
        if !self.enabled {
            return Vec::new();
        }
        let Ok(text) = fs::read_to_string(self.dir.join(INDEX_FILE)) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                let (id, t, r) = (f.next()?, f.next()?, f.next()?);
                let id: i64 = id.parse().ok()?;
                if crate::journal::format_ymd(crate::journal::local_ymd(id, tz_offset_hours)) == ymd
                {
                    Some((unescape(t), unescape(r)))
                } else {
                    None
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn with_dir(dir: PathBuf) -> Memory {
        Memory {
            dir,
            enabled: true,
            turns: 6,
        }
    }

    pub fn save_turn(
        &self,
        transcript: &str,
        reply: &str,
        strokes: &[Vec<Point>],
    ) -> io::Result<()> {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        self.save_turn_at(transcript, reply, strokes, id)
    }

    /// Like `save_turn` but with an explicit unix-second id, so an offline
    /// page that is answered later is filed under the moment it was *written*
    /// rather than the moment it was answered. The idle-commit gap (>= 3.8s)
    /// keeps these ids distinct from live turns.
    pub fn save_turn_at(
        &self,
        transcript: &str,
        reply: &str,
        strokes: &[Vec<Point>],
        id: i64,
    ) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        let mut index = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(INDEX_FILE))?;
        index.write_all(format!("{id}\t{}\t{}\n", escape(transcript), escape(reply)).as_bytes())?;
        fs::write(
            self.dir.join(format!("{id}.strokes")),
            encode_strokes(strokes),
        )?;
        self.prune();
        Ok(())
    }

    /// Keep only the newest MAX_PAGES entries, deleting orphan stroke files.
    fn prune(&self) {
        let Ok(text) = fs::read_to_string(self.dir.join(INDEX_FILE)) else {
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() <= MAX_PAGES {
            return;
        }
        let (drop, keep) = lines.split_at(lines.len() - MAX_PAGES);
        for line in drop {
            if let Some(id) = line.split('\t').next() {
                let _ = fs::remove_file(self.dir.join(format!("{id}.strokes")));
            }
        }
        let _ = fs::write(self.dir.join(INDEX_FILE), keep.join("\n") + "\n");
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One stroke per line: `x,y,r;x,y,r;…`, points decimated to >= 3px,
/// stroke endpoints always kept.
fn encode_strokes(strokes: &[Vec<Point>]) -> String {
    let mut out = String::new();
    for stroke in strokes {
        if stroke.is_empty() {
            continue;
        }
        let mut kept: Vec<Point> = Vec::new();
        let mut last: Option<Point> = None;
        for &p in stroke {
            let far_enough = match last {
                None => true,
                Some(l) => (p.0 - l.0).pow(2) + (p.1 - l.1).pow(2) >= DECIMATE_MIN_DIST2,
            };
            if far_enough {
                kept.push(p);
                last = Some(p);
            }
        }
        if let Some(&end) = stroke.last() {
            if kept.last() != Some(&end) {
                kept.push(end);
            }
        }
        for (i, (x, y, r)) in kept.iter().enumerate() {
            if i > 0 {
                out.push(';');
            }
            out.push_str(&format!("{x},{y},{r}"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_roundtrip() {
        let orig = "hello\tworld\nfoo\\bar";
        assert_eq!(unescape(&escape(orig)), orig);
    }

    #[test]
    fn decimation_keeps_endpoints() {
        let stroke: Vec<Point> = (0..10).map(|i| (i, 0, 3)).collect();
        let encoded = encode_strokes(&[stroke]);
        let line = encoded.trim_end();
        assert!(line.starts_with("0,0,3;"));
        assert!(line.ends_with("9,0,3"));
        // points 1..8 mostly decimated (only >= 3px steps kept)
        assert!(line.split(';').count() <= 6);
    }

    #[test]
    fn pages_on_includes_the_whole_day_and_filters_other_days() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("horcrux-mtest-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // 45 pages today (hourly from local midnight) + 2 yesterday
        let today = crate::journal::today_ymd(0);
        let mut index = String::new();
        let mut add = |id: i64, tag: &str| {
            index.push_str(&format!("{id}\twrote {tag}\treplied {tag}\n"));
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let today_start = now - now.rem_euclid(86400); // UTC midnight (tz=0)
        for i in 0..45 {
            add(today_start + 3600 + i * 60, &format!("today-{i}"));
        }
        add(today_start - 3600, "yesterday-a");
        add(today_start - 7200, "yesterday-b");
        fs::write(dir.join(INDEX_FILE), &index).unwrap();

        let mem = Memory::with_dir(dir.clone());
        let pages = mem.pages_on(&today, 0);
        assert_eq!(pages.len(), 45);
        assert_eq!(pages[0].0, "wrote today-0");
        assert_eq!(pages[44].0, "wrote today-44");
        // yesterday's pages are not in today's gather
        assert!(pages.iter().all(|(t, _)| !t.contains("yesterday")));
        let _ = fs::remove_dir_all(&dir);
    }
}
