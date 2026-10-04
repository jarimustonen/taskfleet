//! Tail-follow primitive over a run's `events.jsonl`.
//!
//! Polling-based (no inotify in MVP). Each tick re-reads from the
//! recorded byte offset; returns all parsed events whose `seq` is
//! greater than the supplied `since_seq` cursor.
//!
//! Designed to be cheap to call repeatedly: keeps a byte offset so we
//! never re-parse already-seen lines. The first call seeks to find the
//! offset corresponding to `since_seq`; subsequent calls start at the
//! cached `pos`.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use taskfleet_core::Event;

use crate::error::CliError;

/// Maximum bytes of a corrupt line surfaced (escaped) in a
/// `supervisor.event_log_skipped_line` event / log line.
const CORRUPT_EXCERPT_BYTES: usize = 100;

/// A newline-terminated line that failed to parse, parked by [`EventTail::poll`]
/// for the caller to report-and-skip via [`EventTail::take_new_corrupt`].
#[derive(Debug, Clone)]
pub struct CorruptLine {
    /// Byte offset of the start of the bad line within the file. Doubles as
    /// the dedup key so the same position is reported at most once.
    pub byte_offset: u64,
    /// Length of the bad line in bytes, including its trailing `\n`. Used to
    /// advance the cursor past it.
    byte_len: u64,
    /// Bounded, escaped excerpt of the line for diagnostics.
    pub line_excerpt: String,
}

pub struct EventTail {
    path: PathBuf,
    /// Byte offset of the start of the next unread line.
    pos: u64,
    /// Highest `seq` returned so far. Newly-parsed events with
    /// `seq <= last_seq` are skipped — defends against a torn read or
    /// a replayed `--from-seq` cursor.
    last_seq: u64,
    /// Set when [`poll`](Self::poll) stopped at a newline-terminated line that
    /// failed to parse. The valid prefix was committed and `pos` parked at the
    /// bad line's start; the caller reports-and-skips via
    /// [`take_new_corrupt`](Self::take_new_corrupt). Until then every `poll`
    /// returns no new events rather than re-erroring on the same bytes — this
    /// is what breaks the supervisor's old warn-spam loop.
    corrupt: Option<CorruptLine>,
    /// `(byte_offset, excerpt-hash)` of corrupt lines already reported, so a
    /// corrupt line is surfaced at most once. Keying on the content hash as
    /// well as the offset means a *different* corruption that happens to land
    /// at a previously-reported offset (after a truncate+rewrite) is still
    /// reported. In-memory only; a fresh process re-reports, which is fine.
    reported_corrupt: HashSet<(u64, u64)>,
}

impl EventTail {
    /// Build a tail starting just past `since_seq`. The first `poll()`
    /// call will skip over already-seen lines.
    pub fn new(path: impl Into<PathBuf>, since_seq: u64) -> Self {
        Self {
            path: path.into(),
            pos: 0,
            last_seq: since_seq,
            corrupt: None,
            reported_corrupt: HashSet::new(),
        }
    }

    /// Read all events appended since the last poll. Returns an empty
    /// vec when nothing new is available. Tolerates the file not yet
    /// existing (a child run's `events.jsonl` may lag the
    /// `child.spawned` event by a few ms).
    pub fn poll(&mut self) -> Result<Vec<Event>, CliError> {
        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(CliError::system(
                    "io_error",
                    format!("open {}: {}", self.path.display(), e),
                ));
            }
        };
        let len = f
            .metadata()
            .map_err(|e| {
                CliError::system(
                    "io_error",
                    format!("metadata {}: {}", self.path.display(), e),
                )
            })?
            .len();
        if len < self.pos {
            // File truncated under us (unlikely in production, but
            // tests/dev may rm+re-create). Restart from the beginning;
            // duplicate-seq guard below will skip already-seen events. Drop any
            // parked corrupt line too — its byte offset is meaningless against
            // the rewritten file.
            self.pos = 0;
            self.corrupt = None;
        }
        // Parked at a known corrupt line: surface nothing new until the caller
        // reports-and-skips it. This is the loop-breaker — without it, every
        // tick re-reads the same offset, re-errors, and the tail never
        // progresses (the F17 warn-spam / CPU-burn bug). Checked AFTER the
        // truncation reset so a rewritten file un-parks correctly.
        if self.corrupt.is_some() {
            return Ok(Vec::new());
        }
        if len == self.pos {
            return Ok(Vec::new());
        }
        f.seek(SeekFrom::Start(self.pos)).map_err(|e| {
            CliError::system("io_error", format!("seek {}: {}", self.path.display(), e))
        })?;
        let mut reader = BufReader::new(f);
        let mut out = Vec::new();
        let mut consumed: u64 = 0;
        // Reused across iterations so the tick doesn't allocate per line.
        let mut buf: Vec<u8> = Vec::new();
        loop {
            // Read raw bytes (not `read_line`) so a torn tail cutting a
            // multi-byte UTF-8 sequence is tolerated as a partial write, and
            // so byte offsets are exact for the corrupt-line cursor.
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf).map_err(|e| {
                CliError::system("io_error", format!("read {}: {}", self.path.display(), e))
            })?;
            if n == 0 {
                break;
            }
            if buf.last() != Some(&b'\n') {
                // Partial line (mid-append). Don't consume its bytes —
                // the next poll will retry from this offset.
                break;
            }
            let trimmed = trim_line_end(&buf);
            if trimmed.is_empty() {
                consumed += n as u64;
                continue;
            }
            let Ok(ev) = serde_json::from_slice::<Event>(trimmed) else {
                // Newline-terminated, unparseable: interior corruption.
                // Commit the valid prefix, park `pos` at this line's start,
                // and return what we have. The caller reports-and-skips.
                self.pos += consumed;
                self.corrupt = Some(CorruptLine {
                    byte_offset: self.pos,
                    byte_len: n as u64,
                    line_excerpt: excerpt(trimmed),
                });
                return Ok(out);
            };
            consumed += n as u64;
            if ev.seq <= self.last_seq {
                continue;
            }
            self.last_seq = ev.seq;
            out.push(ev);
        }
        self.pos += consumed;
        Ok(out)
    }

    /// If [`poll`](Self::poll) parked at a corrupt line not yet reported,
    /// advance the cursor past it and return its details (exactly once per
    /// byte offset). Returns `None` when there is no parked line, or the
    /// parked line's offset was already reported (still advancing past it so
    /// the tail makes progress). The caller emits the one-shot
    /// `supervisor.event_log_skipped_line` event for a `Some`.
    pub fn take_new_corrupt(&mut self) -> Option<CorruptLine> {
        let c = self.corrupt.take()?;
        // Always advance so the tail progresses and never re-stalls here.
        self.pos += c.byte_len;
        if self
            .reported_corrupt
            .insert((c.byte_offset, excerpt_hash(&c.line_excerpt)))
        {
            Some(c)
        } else {
            None
        }
    }

    /// Restart the read cursor at the start of the file after the log was
    /// rewritten in place (e.g. a corrupt line was quarantined out, shifting
    /// every byte offset). `last_seq` is preserved so already-returned events
    /// are skipped by the duplicate-seq guard in [`poll`](Self::poll) rather
    /// than reprocessed; the parked corrupt line — now excised from disk — is
    /// cleared so the next poll re-reads the healed prefix cleanly.
    pub fn restart(&mut self) {
        self.pos = 0;
        self.corrupt = None;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
}

/// Stable hash of a corrupt-line excerpt, used (with the byte offset) as the
/// dedup key so distinct corruptions at the same offset are not conflated.
fn excerpt_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Strip a single trailing line terminator (`\n`, optionally preceded by
/// `\r`) from a raw line, leaving interior/leading bytes untouched.
fn trim_line_end(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    if end > 0 && buf[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && buf[end - 1] == b'\r' {
            end -= 1;
        }
    }
    &buf[..end]
}

/// Render a bounded, escaped prefix of a corrupt line for an event payload /
/// log message. Bytes are lossily decoded and control characters escaped so
/// the excerpt can't inject newlines or ANSI sequences into output.
fn excerpt(line: &[u8]) -> String {
    let shown = &line[..line.len().min(CORRUPT_EXCERPT_BYTES)];
    let mut out: String = String::from_utf8_lossy(shown).escape_debug().to_string();
    if line.len() > CORRUPT_EXCERPT_BYTES {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_line(path: &Path, seq: u64, kind: &str) {
        let line = format!(
            "{{\"ts\":\"2026-06-12T00:00:00Z\",\"seq\":{seq},\"kind\":\"{kind}\",\"run_id\":\"01jxsnap000000000000000000\",\"data\":{{}}}}\n"
        );
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(line.as_bytes()).unwrap();
    }

    #[test]
    fn missing_file_yields_empty() {
        let dir = TempDir::new().unwrap();
        let mut t = EventTail::new(dir.path().join("missing.jsonl"), 0);
        assert_eq!(t.poll().unwrap().len(), 0);
    }

    #[test]
    fn reads_new_events_only() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("events.jsonl");
        write_line(&p, 1, "node.created");
        write_line(&p, 2, "node.report");

        let mut t = EventTail::new(&p, 0);
        let first = t.poll().unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].seq, 1);
        assert_eq!(first[1].seq, 2);

        // Second poll: nothing new.
        assert_eq!(t.poll().unwrap().len(), 0);

        // Append more.
        write_line(&p, 3, "child.spawned");
        let next = t.poll().unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].seq, 3);
    }

    #[test]
    fn skips_already_consumed_seqs() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("events.jsonl");
        write_line(&p, 1, "a");
        write_line(&p, 2, "b");
        write_line(&p, 3, "c");
        let mut t = EventTail::new(&p, 2);
        let evs = t.poll().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].seq, 3);
    }

    #[test]
    fn parks_at_corrupt_line_then_skips_without_looping() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("events.jsonl");
        write_line(&p, 1, "a");
        // A newline-terminated garbage line in the middle.
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(b"{garbage not json\n").unwrap();
        }
        write_line(&p, 2, "b");

        let mut t = EventTail::new(&p, 0);
        // First poll: valid prefix returned, parked at the corrupt line.
        let evs = t.poll().unwrap();
        assert_eq!(evs.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1]);
        // While parked, repeated polls yield nothing AND never error — this
        // is the broken-loop fix (old behavior: a hard error every tick).
        assert_eq!(t.poll().unwrap().len(), 0);
        assert_eq!(t.poll().unwrap().len(), 0);
        // The caller reports-and-skips the corrupt line exactly once.
        let c = t.take_new_corrupt().expect("a corrupt line is parked");
        assert!(c.byte_offset > 0);
        assert!(
            c.line_excerpt.contains("garbage"),
            "excerpt: {}",
            c.line_excerpt
        );
        // After skipping, the tail progresses to the line after it.
        let evs = t.poll().unwrap();
        assert_eq!(evs.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2]);
        // Nothing more is parked.
        assert!(t.take_new_corrupt().is_none());
        assert_eq!(t.poll().unwrap().len(), 0);
    }

    #[test]
    fn ignores_partial_trailing_line() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("events.jsonl");
        write_line(&p, 1, "a");
        // Append a partial line (no newline).
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(b"{\"ts\":\"2026").unwrap();
        }
        let mut t = EventTail::new(&p, 0);
        let evs = t.poll().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].seq, 1);
        // Finish the line and re-poll.
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(
                b"-06-12T00:00:00Z\",\"seq\":2,\"kind\":\"b\",\"run_id\":\"01jxsnap000000000000000000\",\"data\":{}}\n",
            )
            .unwrap();
        }
        let evs = t.poll().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].seq, 2);
    }
}
