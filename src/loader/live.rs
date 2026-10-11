//! 更新中の JSONL は確定した行までの解析状態を保持する。
//! 既読部分のチェックサムを照合してから末尾だけ解析し、書き換え時は読み直す。

use std::io::{self, Read, Seek};

use super::*;

const BLOCK: usize = 64 * 1024;

#[derive(Default)]
struct Prefix {
    len: u64,
    hashes: Vec<u128>,
    tail: Vec<u8>,
}

fn checksum(bytes: &[u8]) -> u128 {
    twox_hash::XxHash3_128::oneshot(bytes)
}

/// 読み出しバッファ内で完結する区間は直接検証し、短い読み出しだけ連結する。
fn verify_chunk(
    reader: &mut impl BufRead,
    fallback: &mut Vec<u8>,
    len: usize,
    matches: impl FnOnce(&[u8]) -> bool,
) -> io::Result<bool> {
    if len == 0 {
        return Ok(matches(&[]));
    }
    let buffer = reader.fill_buf()?;
    if buffer.len() >= len {
        let matched = matches(&buffer[..len]);
        reader.consume(len);
        Ok(matched)
    } else {
        fallback.resize(len, 0);
        reader.read_exact(fallback)?;
        Ok(matches(fallback))
    }
}

impl Prefix {
    fn verify_length(&self, file: &fs::File) -> io::Result<bool> {
        if self.len == 0 {
            return Ok(true);
        }
        Ok(file.metadata()?.len() >= self.len)
    }

    fn verify_reader(&self, reader: &mut impl BufRead) -> io::Result<bool> {
        let mut fallback = Vec::new();
        for &hash in &self.hashes {
            if !verify_chunk(reader, &mut fallback, BLOCK, |bytes| {
                checksum(bytes) == hash
            })? {
                return Ok(false);
            }
        }
        verify_chunk(reader, &mut fallback, self.tail.len(), |bytes| {
            bytes == self.tail
        })
    }

    fn push(&mut self, mut bytes: &[u8]) {
        self.len += bytes.len() as u64;
        while !bytes.is_empty() {
            if self.tail.is_empty() && bytes.len() >= BLOCK {
                let (blocks, remaining) = bytes.as_chunks::<BLOCK>();
                self.hashes
                    .extend(blocks.iter().map(|block| checksum(block)));
                bytes = remaining;
                continue;
            }
            let n = bytes.len().min(BLOCK - self.tail.len());
            self.tail.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            if self.tail.len() == BLOCK {
                self.hashes.push(checksum(&self.tail));
                self.tail.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventText;
    use std::io::Cursor;

    struct LogFixture(PathBuf);

    impl LogFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("rireki-live-reader-{}-{n}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> PathBuf { self.0.join("session.jsonl") }
    }

    impl Drop for LogFixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn message(text: &str) -> String {
        serde_json::json!({"type":"response_item", "timestamp":"2026-10-09T00:00:00Z",
            "payload":{"type":"message", "role":"user", "content":[{"text":text}]}}).to_string()
    }

    fn check_cached_codex(file: &mut CachedFile, path: &Path) -> LiveStats {
        let mut stats = LiveStats::default();
        let ParsedFile::Codex(Some(Loaded::Session(actual))) = file.read(path, &mut stats) else {
            panic!("session expected");
        };
        let Some(Loaded::Session(expected)) = load_codex(path) else { panic!("session expected"); };
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.title, expected.title);
        let events = |lane: &Lane| lane.events.iter().map(|e| {
            let text = match e.text() {
                EventText::Inline(i, o) => (i.to_owned(), o.to_owned()),
                EventText::Pending(i, o) => (i, o),
                EventText::Row(_) => panic!("parser must not use database rows"),
            };
            (e.start.to_bits(), e.end.to_bits(), e.kind, e.status, e.name.clone(), e.summary.clone(), text)
        }).collect::<Vec<_>>();
        assert_eq!(events(&actual.lanes[0]), events(&expected.lanes[0]));
        stats
    }

    #[test]
    fn verified_read_ahead_is_parsed_once_and_edits_reset_the_reader() {
        let fixture = LogFixture::new();
        let path = fixture.path();
        let original = format!("{}\n", message(&"original ".repeat(9000)));
        fs::write(&path, &original).unwrap();
        let mut cache = CachedFile::new(JobKind::Codex);
        check_cached_codex(&mut cache, &path);
        let stats = check_cached_codex(&mut cache, &path);
        assert_eq!(stats.verified_bytes, original.len() as u64);
        assert_eq!(stats.parsed_lines, 0);
        let extended = format!("{original}{}\n{}\n", message("append one"), message("append two"));
        fs::write(&path, &extended).unwrap();
        let stats = check_cached_codex(&mut cache, &path);
        assert_eq!(stats.verified_bytes, original.len() as u64);
        assert_eq!(stats.parsed_lines, 2);
        assert_eq!(stats.resets, 0);
        fs::write(&path, extended.replacen("original", "modified", 1)).unwrap();
        assert_eq!(check_cached_codex(&mut cache, &path).resets, 1);
        fs::write(&path, format!("{}\n", message("truncated"))).unwrap();
        assert_eq!(check_cached_codex(&mut cache, &path).resets, 1);
    }

    #[test]
    fn unterminated_line_rewinds_inside_and_beyond_the_read_buffer() {
        for len in [17, BLOCK + 17, 5 * BLOCK + 17] {
            let fixture = LogFixture::new();
            let path = fixture.path();
            let complete = format!("{}\n", message("complete"));
            let tail = message(&format!("tail {}", "x".repeat(len)));
            let original = format!("{complete}{tail}");
            fs::write(&path, &original).unwrap();
            let mut cache = CachedFile::new(JobKind::Codex);
            check_cached_codex(&mut cache, &path);
            let stats = check_cached_codex(&mut cache, &path);
            assert_eq!(stats.verified_bytes, original.len() as u64);
            assert_eq!(stats.parsed_lines, 1);
            assert_eq!(stats.resets, 0);
            fs::write(&path, format!("{original}\n{}\n", message("after tail"))).unwrap();
            let stats = check_cached_codex(&mut cache, &path);
            assert_eq!(stats.parsed_lines, 2);
            assert_eq!(stats.resets, 0);
            assert_eq!(check_cached_codex(&mut cache, &path).parsed_lines, 0);
        }
    }

    fn bytes(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i.wrapping_mul(31) ^ (i >> 4)) as u8)
            .collect()
    }

    #[test]
    fn prefix_chunks_keep_hashes_tail_and_exact_consumed_position() {
        for len in [0, 17, BLOCK, BLOCK + 17, 2 * BLOCK, 2 * BLOCK + 17] {
            let original = bytes(len);
            for chunk in [1, 17, BLOCK - 1, BLOCK, BLOCK + 1, 4 * BLOCK] {
                let mut prefix = Prefix::default();
                for part in original.chunks(chunk) {
                    prefix.push(part);
                }
                assert_eq!(prefix.len, len as u64);
                assert_eq!(
                    prefix.hashes,
                    (0..len / BLOCK)
                        .map(|i| checksum(&original[i * BLOCK..(i + 1) * BLOCK]))
                        .collect::<Vec<_>>()
                );
                assert_eq!(prefix.tail, original[len / BLOCK * BLOCK..]);
                let mut extended = original.clone();
                extended.extend_from_slice(b"unread append");
                for capacity in [1, 31, BLOCK - 1, BLOCK, BLOCK + 1, 4 * BLOCK] {
                    let mut reader = BufReader::with_capacity(capacity, Cursor::new(&extended));
                    assert!(prefix.verify_reader(&mut reader).unwrap());
                    assert_eq!(reader.stream_position().unwrap(), len as u64);
                    let mut remaining = Vec::new();
                    reader.read_to_end(&mut remaining).unwrap();
                    assert_eq!(remaining, b"unread append");
                }
            }
        }
    }

    #[test]
    fn prefix_rejects_edits_in_each_block_and_tail_and_truncated_reads() {
        let original = bytes(3 * BLOCK + 17);
        let mut prefix = Prefix::default();
        prefix.push(&original);
        for at in [
            0,
            BLOCK - 1,
            BLOCK,
            2 * BLOCK - 1,
            2 * BLOCK,
            3 * BLOCK - 1,
            3 * BLOCK,
            original.len() - 1,
        ] {
            let mut edited = original.clone();
            edited[at] ^= 1;
            for capacity in [31, BLOCK + 1, 4 * BLOCK] {
                let mut reader = BufReader::with_capacity(capacity, Cursor::new(&edited));
                assert!(!prefix.verify_reader(&mut reader).unwrap());
            }
        }
        for end in [0, BLOCK - 1, BLOCK, 3 * BLOCK, original.len() - 1] {
            for capacity in [31, BLOCK + 1, 4 * BLOCK] {
                let mut reader = BufReader::with_capacity(capacity, Cursor::new(&original[..end]));
                assert_eq!(
                    prefix.verify_reader(&mut reader).unwrap_err().kind(),
                    io::ErrorKind::UnexpectedEof
                );
            }
        }
    }
}

/// BufferedLines が実際に消費したバイトだけ記録する。読み出しバッファの先読みは含めない。
struct DigestReader {
    reader: BufReader<fs::File>,
    prefix: Prefix,
    position: u64,
}

impl Read for DigestReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let buf = self.fill_buf()?;
        let n = buf.len().min(output.len());
        output[..n].copy_from_slice(&buf[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for DigestReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        let checked = self
            .prefix
            .len
            .saturating_sub(self.position)
            .min(amount as u64) as usize;
        let buf = self.reader.buffer();
        self.prefix.push(&buf[checked..amount]);
        self.position += amount as u64;
        self.reader.consume(amount);
    }
}

#[derive(Clone)]
enum Parser {
    Claude(ClaudeParser),
    Codex(CodexParser),
}

enum ParsedFile {
    Claude(ClaudeParse),
    Codex(Option<Loaded>),
}

impl Parser {
    fn finish(self) -> ParsedFile {
        match self {
            Self::Claude(p) => ParsedFile::Claude(p.finish()),
            Self::Codex(p) => ParsedFile::Codex(p.finish()),
        }
    }

    /// 公開版では本文と表示する状態だけを複製する。
    /// 改行前の行を読むための分岐では、引き続き解析状態全体をcloneする。
    fn snapshot(&self) -> ParsedFile {
        match self {
            Self::Claude(p) => ParsedFile::Claude(
                ClaudeParser {
                    lane: p.lane.clone(),
                    cwd: p.cwd.clone(),
                    title: p.title.clone(),
                    session_id: p.session_id.clone(),
                    headless: p.headless,
                    branch: p.branch.clone(),
                    version: p.version.clone(),
                    usages: p.usages.clone(),
                    ..ClaudeParser::new("")
                }
                .finish(),
            ),
            Self::Codex(p) if p.hidden => ParsedFile::Codex(None),
            Self::Codex(p) => ParsedFile::Codex(
                CodexParser {
                    lane: p.lane.clone(),
                    id: p.id.clone(),
                    cwd: p.cwd.clone(),
                    title: p.title.clone(),
                    parent: p.parent.clone(),
                    headless: p.headless,
                    branch: p.branch.clone(),
                    version: p.version.clone(),
                    unassigned: p.unassigned,
                    model: p.model.clone(),
                    ..CodexParser::new()
                }
                .finish(),
            ),
        }
    }

    fn hidden(&self) -> bool {
        matches!(self, Self::Codex(parser) if parser.hidden)
    }

    fn new(kind: JobKind) -> Self {
        match kind {
            JobKind::Codex => {
                let mut parser = CodexParser::new();
                parser.pending.1 = Some(TextChanges::default());
                Self::Codex(parser)
            }
            _ => {
                let mut parser = ClaudeParser::new("main");
                parser.pending.1 = Some(TextChanges::default());
                Self::Claude(parser)
            }
        }
    }

    fn push(&mut self, line: &[u8]) {
        match self {
            Self::Claude(parser) => parser.push(line),
            Self::Codex(parser) => parser.push(line),
        }
    }

    fn lane(&self) -> &Lane {
        match self {
            Self::Claude(p) => &p.lane,
            Self::Codex(p) => &p.lane,
        }
    }

    fn unfrozen_texts(&self) -> usize {
        let (lane, pending) = match self {
            Self::Claude(p) => (&p.lane, &p.pending),
            Self::Codex(p) => (&p.lane, &p.pending),
        };
        let changes = pending.1.as_ref().unwrap();
        lane.events.len() - changes.first + changes.edited.len()
    }

    fn freeze_texts(&mut self) {
        let (lane, pending) = match self {
            Self::Claude(p) => (&mut p.lane, &mut p.pending),
            Self::Codex(p) => (&mut p.lane, &mut p.pending),
        };
        let changes = pending.1.as_mut().unwrap();
        let mut indices = std::mem::take(&mut changes.edited);
        indices.extend(changes.first..lane.events.len());
        changes.first = lane.events.len();
        // 既に圧縮した本文は再処理しない。結果が届いた呼び出しと新しいイベントだけを保存する。
        // 小さい単位で共有し、1イベントの更新によって巨大な旧本文群を保持し続けない。
        for chunk in indices.chunks(256) {
            let mut changed = Vec::new();
            let mut texts = Vec::new();
            for &i in chunk {
                let event = &mut lane.events[i];
                if event.input.is_empty() && event.output.is_empty() {
                    continue;
                }
                let pack = |s: &str| {
                    if s.is_empty() {
                        Vec::new()
                    } else {
                        lz4_flex::compress_prepend_size(s.as_bytes())
                    }
                };
                texts.push((pack(&event.input), pack(&event.output)));
                event.input = String::new();
                event.output = String::new();
                changed.push(i);
            }
            if !changed.is_empty() {
                let source = crate::text::Texts::pending(Arc::new(texts));
                for (index, i) in changed.into_iter().enumerate() {
                    lane.events[i].text_ref = source.reference(index);
                }
            }
        }
    }

    fn retained_bytes(&self) -> usize {
        let lane = self.lane();
        let events = lane.events.capacity() * std::mem::size_of::<Event>()
            + lane
                .events
                .iter()
                .map(|e| {
                    e.summary.len() + 2 * std::mem::size_of::<usize>()
                        + e.input.capacity()
                        + e.output.capacity()
                        + e.call_id.as_ref().map_or(0, |id| id.len() + 2 * std::mem::size_of::<usize>())
                })
                .sum::<usize>();
        let strings = |map: &HashMap<String, usize>| {
            map.capacity() * (std::mem::size_of::<(String, usize)>() + 1)
                + map.keys().map(String::capacity).sum::<usize>()
        };
        let intern = |i: &Interner| {
            i.0.capacity() * (std::mem::size_of::<(String, Arc<str>)>() + 1)
                + i.0
                    .iter()
                    .map(|(k, v)| k.capacity() + v.len() + 2 * std::mem::size_of::<usize>())
                    .sum::<usize>()
        };
        events
            + std::mem::size_of::<Self>()
            + lane.name.capacity()
            + lane.model.capacity()
            + lane.effort.capacity()
            + lane.origin.capacity()
            + lane.spawned_by.as_ref().map_or(0, String::capacity)
            + match self {
                Self::Claude(p) => {
                    strings(&p.pending.0)
                        + p.pending
                            .1
                            .as_ref()
                            .map_or(0, |c| c.edited.capacity() * std::mem::size_of::<usize>())
                        + intern(&p.names)
                        + intern(&p.models)
                        + strings(&p.usage_ix)
                        + p.usages.capacity()
                            * std::mem::size_of::<(usize, Usage, Option<Arc<str>>)>()
                        + [&p.cwd, &p.title, &p.session_id, &p.branch, &p.version]
                            .iter()
                            .map(|s| s.capacity())
                            .sum::<usize>()
                }
                Self::Codex(p) => {
                    strings(&p.pending.0)
                        + p.pending
                            .1
                            .as_ref()
                            .map_or(0, |c| c.edited.capacity() * std::mem::size_of::<usize>())
                        + intern(&p.names)
                        + p.parent.as_ref().map_or(0, String::capacity)
                        + [&p.id, &p.cwd, &p.title, &p.branch, &p.version, &p.goal]
                            .iter()
                            .map(|s| s.capacity())
                            .sum::<usize>()
                }
            }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct LiveStats {
    pub verified_bytes: u64,
    pub parsed_lines: usize,
    pub resets: usize,
}

struct CachedFile {
    kind: JobKind,
    parser: Parser,
    prefix: Prefix,
    committed: u64,
}

impl CachedFile {
    fn new(kind: JobKind) -> Self {
        Self {
            kind,
            parser: Parser::new(kind),
            prefix: Prefix::default(),
            committed: 0,
        }
    }

    fn read(&mut self, path: &Path, stats: &mut LiveStats) -> ParsedFile {
        let Ok(file) = fs::File::open(path) else {
            *self = Self::new(self.kind);
            return self.parser.snapshot();
        };
        // 短くなったファイルの旧状態は、読み出しバッファを確保する前に解放する。
        if !self.prefix.verify_length(&file).unwrap_or(false) {
            stats.resets += 1;
            *self = Self::new(self.kind);
        }
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let verified = self.prefix.verify_reader(&mut reader).unwrap_or(false);
        if verified {
            stats.verified_bytes += self.prefix.len;
        } else {
            stats.resets += 1;
            *self = Self::new(self.kind);
        }
        // 内部サブエージェントは最初の該当メタデータで解析を止める。
        // 再利用するのもそこまでの状態なので、表示しない巨大な末尾は読まない。
        if self.parser.hidden() {
            return self.parser.snapshot();
        }
        // 検証の先読みも同じバッファで解析する。未確定行だけ戻し、書き換え時は先頭へ戻る。
        let rewind = self.prefix.len - self.committed;
        let positioned = if verified && rewind <= i64::MAX as u64 {
            reader.seek_relative(-(rewind as i64))
        } else {
            reader.seek(io::SeekFrom::Start(self.committed)).map(|_| ())
        };
        if positioned.is_err() {
            *self = Self::new(self.kind);
            return self.parser.snapshot();
        }
        let reader = DigestReader {
            reader,
            prefix: std::mem::take(&mut self.prefix),
            position: self.committed,
        };
        let mut lines = BufferedLines {
            reader,
            line: Vec::new(),
            skip: if self.kind == JobKind::Codex {
                skip_codex_line
            } else {
                |_| false
            },
            position: self.committed,
            last_complete: self.committed,
        };
        let mut tail = None;
        while let Some((line, complete)) = lines.next_record() {
            stats.parsed_lines += 1;
            if complete {
                self.parser.push(line);
                if self.parser.hidden() {
                    break;
                }
                // 初回も古い呼び出しへの大量の結果も、小分けに本文を共有する。
                // イベント列全体を毎回走査せず、新規と更新した本文だけを圧縮する。
                if self.parser.unfrozen_texts() >= 256 {
                    self.parser.freeze_texts();
                }
            } else {
                // 改行前の最終行は公開版だけで読む。次の追記で同じ行を再解析できる。
                self.parser.freeze_texts();
                let mut snapshot = self.parser.clone();
                snapshot.push(line);
                snapshot.freeze_texts();
                tail = Some(snapshot);
            }
        }
        self.committed = lines.last_complete;
        self.prefix = lines.reader.prefix;
        self.parser.freeze_texts();
        match tail {
            Some(parser) => parser.finish(),
            None => self.parser.snapshot(),
        }
    }
}

/// 1つの更新単位。Claudeではメインと各サブエージェントの解析状態を別々に保持する。
#[derive(Default)]
pub struct LiveLog {
    files: HashMap<PathBuf, CachedFile>,
    stats: LiveStats,
}

impl LiveLog {
    fn read(&mut self, path: &Path, kind: JobKind) -> ParsedFile {
        let file = self
            .files
            .entry(path.to_owned())
            .or_insert_with(|| CachedFile::new(kind));
        if file.kind != kind {
            *file = CachedFile::new(kind);
        }
        file.read(path, &mut self.stats)
    }

    pub fn last_update(&self) -> LiveStats {
        self.stats
    }

    /// 容量上限のための保守的な見積もり。共有本文は保持元ごとに一度数える。
    pub fn retained_bytes(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        let texts = self
            .files
            .values()
            .flat_map(|f| &f.parser.lane().events)
            .filter(|e| e.text_ref.key().is_some_and(|key| seen.insert(key)))
            .map(|e| e.text_ref.retained_bytes())
            .sum::<usize>();
        self.files
            .iter()
            .map(|(path, f)| {
                path.capacity()
                    + f.parser.retained_bytes()
                    + f.prefix.hashes.capacity() * std::mem::size_of::<u128>()
                    + f.prefix.tail.capacity()
            })
            .sum::<usize>()
            + texts
            + self.files.capacity() * (std::mem::size_of::<(PathBuf, CachedFile)>() + 1)
    }

    pub fn parse(&mut self, job: &Job) -> Vec<Loaded> {
        self.stats = LiveStats::default();
        let p = &job.path;
        match job.kind {
            JobKind::Codex => {
                let ParsedFile::Codex(parsed) = self.read(p, job.kind) else {
                    unreachable!()
                };
                parsed.into_iter().collect()
            }
            JobKind::ClaudeLegacyAgent => {
                let Some(name) = p.file_stem().and_then(|n| n.to_str()) else {
                    return Vec::new();
                };
                let ParsedFile::Claude(mut parsed) = self.read(p, job.kind) else {
                    unreachable!()
                };
                parsed.lane.name = name.to_owned();
                if parsed.session_id.is_empty() {
                    Vec::new()
                } else {
                    vec![Loaded::Child(parsed.session_id, parsed.lane)]
                }
            }
            JobKind::Claude => {
                let ParsedFile::Claude(parsed) = self.read(p, job.kind) else {
                    unreachable!()
                };
                let mut lanes = vec![parsed.lane];
                let mut subs: Vec<_> = read_dir(&p.with_extension("").join("subagents"))
                    .into_iter()
                    .filter(|f| f.extension().is_some_and(|e| e == "jsonl"))
                    .collect();
                subs.sort();
                self.files
                    .retain(|path, _| path == p || subs.binary_search(path).is_ok());
                for sub in subs {
                    let meta: Value = fs::read_to_string(sub.with_extension("meta.json"))
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(Value::Null);
                    let ParsedFile::Claude(parsed) = self.read(&sub, JobKind::Claude) else {
                        unreachable!()
                    };
                    let mut lane = parsed.lane;
                    let ty = meta
                        .get("agentType")
                        .and_then(Value::as_str)
                        .unwrap_or("agent");
                    let desc = meta
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    lane.name = format!("{ty}: {desc}");
                    lane.spawned_by = meta
                        .get("toolUseId")
                        .and_then(Value::as_str)
                        .map(String::from);
                    if !lane.events.is_empty() {
                        lanes.push(lane);
                    }
                }
                vec![Loaded::Session(Session {
                    id: if parsed.session_id.is_empty() {
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default()
                            .to_owned()
                    } else {
                        parsed.session_id
                    },
                    source: Source::Claude,
                    cwd: parsed.cwd,
                    headless: parsed.headless,
                    title: parsed.title,
                    branch: parsed.branch,
                    version: parsed.version,
                    lanes,
                    start: 0.0,
                    end: 0.0,
                    parents: Vec::new(),
                    tree: Vec::new(),
                })]
            }
        }
    }
}
