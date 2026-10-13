//! JSONL の行読み出しと JSON 解析。大きな行の確保と読み飛ばしをここで管理する。

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

/// ファイル全体をバイト列で読み、行ごとの String 確保を避ける。
fn read_all(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_default()
}

/// 行のUTF-8をまとめて検証し、JSON文字列ごとの再検証を省く。
/// 不正な入力では従来のデシリアライザを使い、エラーの扱いも維持する。
pub(super) fn json_row(line: &[u8]) -> serde_json::Result<Value> {
    match simdutf8::basic::from_utf8(line) {
        Ok(text) => serde_json::from_str(text),
        Err(_) => serde_json::from_slice(line),
    }
}

#[cfg(test)]
fn lines(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    memchr::memchr_iter(b'\n', buf)
        .chain(std::iter::once(buf.len()))
        .scan(0, |start, end| {
            let line = &buf[*start..end];
            *start = end + 1;
            Some(line)
        })
        .filter(|l| !l.is_empty())
}

/// 大きなログは一定量ずつ読み、不要な巨大行は先頭だけ見て読み捨てる。
/// 必要な行をまたぐ場合だけ、再利用するバッファに連結する
pub(super) struct BufferedLines<R> {
    pub(super) reader: R,
    pub(super) line: Vec<u8>,
    pub(super) skip: fn(&[u8]) -> bool,
    pub(super) position: u64,
    pub(super) last_complete: u64,
}

impl<R: BufRead> BufferedLines<R> {
    pub(super) fn next_record(&mut self) -> Option<(&[u8], bool)> {
        self.next_line()?;
        Some((&self.line, self.last_complete == self.position))
    }

    pub(super) fn next_line(&mut self) -> Option<&[u8]> {
        'line: loop {
            self.line.clear();
            let mut chunks = Vec::new();
            let mut decided = false;
            let mut skipping = false;
            loop {
                let buf = self.reader.fill_buf().ok()?;
                if buf.is_empty() {
                    finish_line(&mut self.line, chunks);
                    return (!skipping && !self.line.is_empty() && !(self.skip)(&self.line))
                        .then_some(self.line.as_slice());
                }
                let end = memchr::memchr(b'\n', buf).unwrap_or(buf.len());
                let complete = end < buf.len();
                if !skipping {
                    let mut copied = 0;
                    if !decided {
                        copied = end.min(160 - self.line.len());
                        self.line.extend_from_slice(&buf[..copied]);
                        if self.line.len() == 160 || complete {
                            skipping = (self.skip)(&self.line);
                            decided = true;
                        }
                    }
                    if !skipping {
                        append_line(&mut self.line, &mut chunks, &buf[copied..end]);
                    }
                }
                let consumed = end + usize::from(complete);
                self.reader.consume(consumed);
                self.position += consumed as u64;
                if complete {
                    self.last_complete = self.position;
                }
                if complete {
                    finish_line(&mut self.line, chunks);
                    if skipping || self.line.is_empty() {
                        continue 'line;
                    }
                    return Some(&self.line);
                }
            }
        }
    }
}

/// 巨大な行の再確保では古い領域と新しい領域が同時に残る。
/// 容量を超えて2 MiBより大きくなる行だけ分割し、長さが分かってから一度確保する。
fn append_line(line: &mut Vec<u8>, chunks: &mut Vec<Vec<u8>>, bytes: &[u8]) {
    if chunks.is_empty()
        && (line.len() + bytes.len() <= line.capacity()
            || line.len() + bytes.len() <= 2 * 1024 * 1024)
    {
        line.extend_from_slice(bytes);
    } else {
        if chunks.is_empty() {
            chunks.push(std::mem::take(line));
        }
        chunks.push(bytes.to_vec());
    }
}

fn finish_line(line: &mut Vec<u8>, chunks: Vec<Vec<u8>>) {
    if chunks.is_empty() {
        return;
    }
    let len = chunks.iter().map(Vec::len).sum();
    line.reserve_exact(len);
    for chunk in chunks {
        line.extend_from_slice(&chunk);
    }
}

pub(super) enum LogLines {
    Memory { bytes: Vec<u8>, offset: usize },
    Buffered(BufferedLines<BufReader<fs::File>>),
}

impl LogLines {
    pub(super) fn open(p: &Path, skip: fn(&[u8]) -> bool) -> Self {
        if fs::metadata(p).is_ok_and(|m| m.len() > 8 * 1024 * 1024)
            && let Ok(file) = fs::File::open(p)
        {
            Self::Buffered(BufferedLines {
                reader: BufReader::with_capacity(256 * 1024, file),
                line: Vec::new(),
                skip,
                position: 0,
                last_complete: 0,
            })
        } else {
            Self::Memory {
                bytes: read_all(p),
                offset: 0,
            }
        }
    }

    pub(super) fn next_line(&mut self) -> Option<&[u8]> {
        match self {
            Self::Memory { bytes, offset } => loop {
                if *offset >= bytes.len() {
                    return None;
                }
                let end =
                    memchr::memchr(b'\n', &bytes[*offset..]).map_or(bytes.len(), |i| i + *offset);
                let start = *offset;
                *offset = end + 1;
                if start < end {
                    return Some(&bytes[start..end]);
                }
            },
            Self::Buffered(reader) => reader.next_line(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::skip_codex_line;
    use super::*;

    #[test]
    fn json_rows_keep_values_key_order_and_errors_at_utf8_boundaries() {
        fn same(input: &[u8]) {
            match (serde_json::from_slice::<Value>(input), json_row(input)) {
                (Ok(old), Ok(new)) => {
                    assert_eq!(old, new);
                    assert_eq!(
                        serde_json::to_vec(&old).unwrap(),
                        serde_json::to_vec(&new).unwrap()
                    );
                }
                (Err(old), Err(new)) => {
                    assert_eq!(old.classify(), new.classify());
                    assert_eq!((old.line(), old.column()), (new.line(), new.column()));
                    assert_eq!(old.to_string(), new.to_string());
                }
                _ => panic!("JSON row result differs"),
            }
        }
        for prefix in [
            0, 1, 59, 60, 61, 62, 63, 64, 65, 66, 123, 124, 125, 126, 127, 128, 129, 130,
        ] {
            let row = format!(
                r#"{{"prefix":"{}","text":"日本語😀é\n\uD83D\uDE00","duplicate":1,"duplicate":2,"number":-0.0}}"#,
                " ".repeat(prefix)
            );
            for end in 0..=row.len() {
                same(&row.as_bytes()[..end]);
            }
            let at = row.find("日本語").unwrap();
            for invalid in [
                &b"\xff"[..],
                &b"\xc0\xaf"[..],
                &b"\xed\xa0\x80"[..],
                &b"\xf4\x90\x80\x80"[..],
            ] {
                let mut bytes = row.as_bytes().to_vec();
                bytes.splice(at..at + 3, invalid.iter().copied());
                same(&bytes);
            }
        }
        for row in [
            r#""\uD800""#,
            r#"{"\uDC00":0}"#,
            "1e400",
            "[true,]",
            "{} trailing",
            "\"control\x01\"",
        ] {
            same(row.as_bytes());
        }
        same(format!("{}0{}", "[".repeat(130), "]".repeat(130)).as_bytes());
    }

    #[test]
    fn buffered_lines_preserve_bytes_and_skip_across_boundaries() {
        let mut input = b"\nabc\r\n".to_vec();
        input.extend_from_slice(
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"text\":\"",
        );
        input.extend(std::iter::repeat_n(b'x', 2_000_000));
        input.extend_from_slice(
            b"\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"m\"}}\n",
        );
        input.extend_from_slice(b"\xff\n\n");
        input.extend_from_slice("日本語の最後の行".as_bytes());
        let expected: Vec<_> = lines(&input)
            .filter(|l| !skip_codex_line(l))
            .map(<[u8]>::to_vec)
            .collect();
        for cap in [1, 7, 159, 160, 161, 256 * 1024] {
            let mut reader = BufferedLines {
                reader: BufReader::with_capacity(cap, input.as_slice()),
                line: Vec::new(),
                skip: skip_codex_line,
                position: 0,
                last_complete: 0,
            };
            let mut actual = Vec::new();
            while let Some(line) = reader.next_line() {
                actual.push(line.to_vec());
            }
            assert_eq!(actual, expected, "buffer capacity={cap}");
            assert!(
                reader.line.capacity() < 1024,
                "不要な巨大行を保持していないこと"
            );
        }
    }

    #[test]
    fn buffered_lines_keep_large_rows_without_extra_capacity() {
        let first = "日本語".repeat(300_000);
        let last = "最後の行".repeat(400_000);
        let input = format!(
            "{first}\n\n{{\"type\":\"world_state\",\"ignored\":\"{}\"}}\n{last}",
            "x".repeat(3 * 1024 * 1024)
        );
        for cap in [64 * 1024, 256 * 1024, 4 * 1024 * 1024] {
            let mut reader = BufferedLines {
                reader: BufReader::with_capacity(cap, input.as_bytes()),
                line: Vec::new(),
                skip: skip_codex_line,
                position: 0,
                last_complete: 0,
            };
            assert_eq!(reader.next_line(), Some(first.as_bytes()));
            assert_eq!(reader.next_line(), Some(last.as_bytes()));
            assert_eq!(reader.next_line(), None);
            assert_eq!(reader.line.capacity(), last.len());
        }
    }

    #[test]
    fn buffered_lines_keep_long_required_rows_and_final_skipped_row() {
        let input = format!(
            "{}\n\n{}",
            "日本語".repeat(1000),
            "{\"type\":\"world_state\",\"payload\":{}}"
        );
        let mut reader = BufferedLines {
            reader: BufReader::with_capacity(17, input.as_bytes()),
            line: Vec::new(),
            skip: skip_codex_line,
            position: 0,
            last_complete: 0,
        };
        assert_eq!(reader.next_line(), Some("日本語".repeat(1000).as_bytes()));
        assert_eq!(reader.next_line(), None);
    }
}
