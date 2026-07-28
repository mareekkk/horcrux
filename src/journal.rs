//! The diary's daily journal.
//!
//! One entry per day of conversation, stored as plain files next to the
//! conversation memory: `<date>.txt` (the entry text) and `<date>.strokes`
//! (the handwriting that wrote it, one stroke per line as `x,y;x,y;…` —
//! reply strokes use the application's fixed brush radius, so it is not
//! stored). Entries are
//! composed automatically on quit and can be conjured back onto the page.
//!
//! Civil-date math is done by hand (Howard Hinnant's algorithms, like
//! riddle) — no chrono. Devices usually run UTC; `HORCRUX_TZ_OFFSET` shifts
//! the writer's local date and clock by whole hours.

use crate::log;
use serde_json::json;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Entries with more stored points are talked about instead of conjured.
pub const CONJURE_MAX_POINTS: usize = 4000;
/// Conjured ink is faded, not black.
pub const CONJURE_GRAY: u8 = 170;
/// Stable stock-library identity: updates replace one document, not create
/// another copy after every session.
const LIBRARY_DOCUMENT_ID: &str = "8f3b0d2a-4c79-4a78-9c1e-7e5b2a6d9041";
const DIARY_TITLE: &str = "Horcrux Diary";
const MARKDOWN_FILE: &str = "Horcrux Diary.md";

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

/// Local date and 24-hour clock as `YYYY-MM-DD HH:MM`.
pub fn local_datetime(unix_secs: i64, tz_offset_hours: i32) -> String {
    let secs = unix_secs + tz_offset_hours as i64 * 3600;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let seconds_today = secs.rem_euclid(86400);
    let hour = seconds_today / 3600;
    let minute = seconds_today % 3600 / 60;
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}")
}

pub fn format_utc_offset(tz_offset_hours: i32) -> String {
    format!("UTC{tz_offset_hours:+03}:00")
}

/// Current local date, time, timezone name, and offset for the oracle prompt.
pub fn current_time_context(tz_name: &str, tz_offset_hours: i32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{} ({tz_name}, {})",
        local_datetime(now as i64, tz_offset_hours),
        format_utc_offset(tz_offset_hours)
    )
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

/// Rebuild the canonical Markdown diary and its EPUB stock-library companion.
///
/// reMarkable does not open Markdown directly, so the `.md` remains the source
/// of truth in the journal directory while an EPUB with the same content is
/// atomically refreshed in xochitl's document library.
pub fn publish_diary(dir: &Path, library_dir: &Path, time_context: &str) -> io::Result<PathBuf> {
    let entries = diary_entries(dir)?;
    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no journal entries to publish",
        ));
    }

    let markdown = diary_markdown(&entries, time_context);
    let markdown_path = dir.join(MARKDOWN_FILE);
    write_atomic(&markdown_path, markdown.as_bytes())?;

    fs::create_dir_all(library_dir)?;
    let epub = build_epub(&entries, time_context);
    let epub_path = library_dir.join(format!("{LIBRARY_DOCUMENT_ID}.epub"));

    // These are xochitl-generated views of the EPUB. Clearing only the
    // app-owned document's derivatives makes xochitl repaginate new content
    // when the normal interface starts again.
    for suffix in ["pdf", "epubindex", "pagedata"] {
        remove_file_if_present(&library_dir.join(format!("{LIBRARY_DOCUMENT_ID}.{suffix}")))?;
    }
    let thumbnails = library_dir.join(format!("{LIBRARY_DOCUMENT_ID}.thumbnails"));
    if thumbnails.exists() {
        fs::remove_dir_all(thumbnails)?;
    }
    write_atomic(&epub_path, &epub)?;

    let content = json!({
        "coverPageNumber": -1,
        "customZoomCenterX": 0,
        "customZoomCenterY": 936,
        "customZoomOrientation": "portrait",
        "customZoomPageHeight": 1872,
        "customZoomPageWidth": 1404,
        "customZoomScale": 1,
        "documentMetadata": {
            "authors": ["Horcrux"],
            "publicationDate": "",
            "publisher": "",
            "title": DIARY_TITLE
        },
        "dummyDocument": false,
        "extraMetadata": {},
        "fileType": "epub",
        "fontName": "",
        "formatVersion": 1,
        "keyboardMetadata": {},
        "lineHeight": -1,
        "margins": 125,
        "orientation": "portrait",
        "originalPageCount": 0,
        "pageCount": 0,
        "pageTags": [],
        "pages": [],
        "redirectionPageMap": [],
        "sizeInBytes": epub.len().to_string(),
        "tags": [],
        "textAlignment": "left",
        "textScale": 1,
        "zoomMode": "bestFit"
    });
    write_atomic(
        &library_dir.join(format!("{LIBRARY_DOCUMENT_ID}.content")),
        content.to_string().as_bytes(),
    )?;

    let metadata_path = library_dir.join(format!("{LIBRARY_DOCUMENT_ID}.metadata"));
    let mut metadata = fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| {
            json!({
                "deleted": false,
                "lastModified": "0",
                "lastOpened": "0",
                "lastOpenedPage": 0,
                "metadatamodified": true,
                "modified": true,
                "parent": "",
                "pinned": false,
                "synced": false,
                "type": "DocumentType",
                "version": 0,
                "visibleName": DIARY_TITLE
            })
        });
    let version = metadata
        .get("version")
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
        .saturating_add(1);
    let modified_ms = unix_now().saturating_mul(1000).to_string();
    metadata["deleted"] = json!(false);
    metadata["lastModified"] = json!(modified_ms);
    metadata["metadatamodified"] = json!(true);
    metadata["modified"] = json!(true);
    metadata["synced"] = json!(false);
    metadata["type"] = json!("DocumentType");
    metadata["version"] = json!(version);
    metadata["visibleName"] = json!(DIARY_TITLE);
    // Write metadata last: it is the stock UI/cloud sync change signal.
    write_atomic(&metadata_path, metadata.to_string().as_bytes())?;

    Ok(markdown_path)
}

fn diary_entries(dir: &Path) -> io::Result<Vec<(String, String)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
            continue;
        }
        let Some(date) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_ymd(date) {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let text = text.trim();
        if !text.is_empty() {
            entries.push((date.to_string(), text.to_string()));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

fn diary_markdown(entries: &[(String, String)], time_context: &str) -> String {
    let mut markdown = format!("# {DIARY_TITLE}\n\n_Updated {time_context}_\n");
    for (date, text) in entries {
        markdown.push_str(&format!("\n## {}\n\n{text}\n", display_date(date)));
    }
    markdown
}

fn display_date(ymd: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let year = &ymd[0..4];
    let month = ymd[5..7].parse::<usize>().unwrap_or(0);
    let day = ymd[8..10].parse::<u32>().unwrap_or(0);
    let month = MONTHS.get(month.saturating_sub(1)).unwrap_or(&"");
    format!("{day} {month} {year}")
}

fn build_epub(entries: &[(String, String)], time_context: &str) -> Vec<u8> {
    let mut sections = String::new();
    for (date, text) in entries {
        sections.push_str(&format!(
            "<section><h2>{}</h2>",
            escape_xml(&display_date(date))
        ));
        for paragraph in text.split("\n\n") {
            sections.push_str("<p>");
            sections.push_str(&escape_xml(paragraph).replace('\n', "<br/>"));
            sections.push_str("</p>");
        }
        sections.push_str("</section>");
    }
    let diary = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" lang="en">
<head>
  <meta charset="UTF-8"/>
  <title>{DIARY_TITLE}</title>
  <style>
    body {{ font-family: serif; line-height: 1.55; margin: 7%; color: #111; }}
    h1 {{ font-size: 2em; margin-bottom: .2em; }}
    .updated {{ color: #555; font-style: italic; margin-bottom: 3em; }}
    section {{ break-before: page; page-break-before: always; }}
    h2 {{ font-size: 1.35em; margin-bottom: 1.2em; }}
    p {{ margin: 0 0 1em; text-align: left; }}
  </style>
</head>
<body>
  <h1>{DIARY_TITLE}</h1>
  <p class="updated">Updated {}</p>
  {sections}
</body>
</html>"#,
        escape_xml(time_context)
    );
    let modified = format!(
        "{}:00Z",
        local_datetime(unix_now() as i64, 0).replace(' ', "T")
    );
    let package = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="book-id">urn:uuid:{LIBRARY_DOCUMENT_ID}</dc:identifier>
    <dc:title>{DIARY_TITLE}</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Horcrux</dc:creator>
    <meta property="dcterms:modified">{modified}</meta>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cover-image" href="cover.png" media-type="image/png" properties="cover-image"/>
    <item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
    <item id="diary" href="diary.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="cover"/>
    <itemref idref="diary"/>
  </spine>
</package>"#
    );
    let cover_page = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" epub:type="cover">
<head><title>Cover</title><style>html,body{margin:0;padding:0}img{width:100%;height:100%;object-fit:contain}</style></head>
<body><img src="cover.png" alt="Horcrux"/></body>
</html>"#;
    let nav = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><h1>Contents</h1><ol><li><a href="diary.xhtml">{DIARY_TITLE}</a></li></ol></nav></body>
</html>"#
    );
    let container = br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let cover_png = crate::cover::cover_png();
    zip_store(&[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", container),
        ("OEBPS/content.opf", package.as_bytes()),
        ("OEBPS/nav.xhtml", nav.as_bytes()),
        ("OEBPS/cover.xhtml", cover_page.as_bytes()),
        ("OEBPS/cover.png", cover_png.as_slice()),
        ("OEBPS/diary.xhtml", diary.as_bytes()),
    ])
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn zip_store(entries: &[(&str, &[u8])]) -> Vec<u8> {
    struct Central {
        name: Vec<u8>,
        crc: u32,
        size: u32,
        offset: u32,
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let name = name.as_bytes();
        let size = u32::try_from(data.len()).expect("EPUB entry too large");
        let offset = u32::try_from(out.len()).expect("EPUB too large");
        let crc = crc32(data);
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0); // stored, not compressed
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, crc);
        push_u32(&mut out, size);
        push_u32(&mut out, size);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        central.push(Central {
            name: name.to_vec(),
            crc,
            size,
            offset,
        });
    }
    let central_offset = out.len() as u32;
    for entry in &central {
        push_u32(&mut out, 0x0201_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, entry.crc);
        push_u32(&mut out, entry.size);
        push_u32(&mut out, entry.size);
        push_u16(&mut out, entry.name.len() as u16);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, entry.offset);
        out.extend_from_slice(&entry.name);
    }
    let central_size = out.len() as u32 - central_offset;
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, central.len() as u16);
    push_u16(&mut out, central.len() as u16);
    push_u32(&mut out, central_size);
    push_u32(&mut out, central_offset);
    push_u16(&mut out, 0);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, data)?;
    fs::rename(temporary, path)
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn entry_text(dir: &Path, ymd: &str) -> Option<String> {
    fs::read_to_string(entry_path(dir, ymd))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// One stroke per line: `x,y;x,y;…` (the reply brush radius is fixed).
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
    p.push_str("\nRewrite today's entry as a first-person diary summary of the entire conversation so far today, in your own voice, no more than five sentences. Incorporate the latest pages so the entry replaces any earlier same-day summary. Begin with the date in the form '26 July, in the evening.' Write ONLY the entry text (no \u{2042} transcription line this time).");
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
        assert_eq!(local_datetime(midnight, 8), "2025-01-01 08:00");
        assert_eq!(local_datetime(midnight, -1), "2024-12-31 23:00");
        assert_eq!(format_utc_offset(8), "UTC+08:00");
        assert_eq!(format_utc_offset(-5), "UTC-05:00");
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
    fn markdown_and_library_epub_are_refreshed() {
        let root = tempdir("publish");
        let dir = root.join("journal");
        let library = root.join("library");
        save_entry(
            &dir,
            "2026-07-26",
            "The first shadow crossed the page.",
            &[],
        )
        .unwrap();
        save_entry(
            &dir,
            "2026-07-27",
            "A newer secret appeared & remained <unspoken>.",
            &[],
        )
        .unwrap();

        let markdown_path = publish_diary(
            &dir,
            &library,
            "2026-07-27 20:15 (Australia/Perth, UTC+08:00)",
        )
        .unwrap();
        let markdown = fs::read_to_string(markdown_path).unwrap();
        assert!(markdown.starts_with("# Horcrux Diary\n"));
        assert!(markdown.contains("## 26 July 2026"));
        assert!(markdown.contains("## 27 July 2026"));
        assert!(markdown.contains("A newer secret appeared & remained <unspoken>."));

        let epub_path = library.join(format!("{LIBRARY_DOCUMENT_ID}.epub"));
        let epub = fs::read(&epub_path).unwrap();
        assert_eq!(&epub[0..4], b"PK\x03\x04");
        assert!(epub
            .windows(b"application/epub+zip".len())
            .any(|window| window == b"application/epub+zip"));
        assert!(epub
            .windows(b"&amp; remained &lt;unspoken&gt;".len())
            .any(|window| window == b"&amp; remained &lt;unspoken&gt;"));

        let metadata_path = library.join(format!("{LIBRARY_DOCUMENT_ID}.metadata"));
        let first: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&metadata_path).unwrap()).unwrap();
        assert_eq!(first["visibleName"], "Horcrux Diary");
        assert_eq!(first["version"], 1);

        fs::write(library.join(format!("{LIBRARY_DOCUMENT_ID}.pdf")), b"stale").unwrap();
        fs::create_dir_all(library.join(format!("{LIBRARY_DOCUMENT_ID}.thumbnails"))).unwrap();
        publish_diary(
            &dir,
            &library,
            "2026-07-27 20:16 (Australia/Perth, UTC+08:00)",
        )
        .unwrap();
        let second: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(metadata_path).unwrap()).unwrap();
        assert_eq!(second["version"], 2);
        assert!(!library.join(format!("{LIBRARY_DOCUMENT_ID}.pdf")).exists());
        assert!(!library
            .join(format!("{LIBRARY_DOCUMENT_ID}.thumbnails"))
            .exists());
        let _ = fs::remove_dir_all(root);
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
