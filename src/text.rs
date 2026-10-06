//! 保存待ちの圧縮本文を共有し、コミット後は行IDだけを保持する。

use std::sync::{Arc, Mutex};

pub(crate) type Compressed = Vec<(Vec<u8>, Vec<u8>)>;

enum State {
    Pending(Arc<Compressed>),
    Rows(Vec<i64>),
}

pub(crate) struct Texts(Mutex<State>);

impl Texts {
    pub(crate) fn pending(texts: Arc<Compressed>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(State::Pending(texts))))
    }

    pub(crate) fn committed(&self, rows: Vec<i64>) {
        *self.0.lock().unwrap() = State::Rows(rows);
    }

    pub(crate) fn rows(&self) -> Option<Vec<i64>> {
        match &*self.0.lock().unwrap() {
            State::Rows(rows) => Some(rows.clone()),
            State::Pending(_) => None,
        }
    }

    pub(crate) fn preserve(&self, texts: Compressed) {
        *self.0.lock().unwrap() = State::Pending(Arc::new(texts));
    }

    pub(crate) fn reference(self: &Arc<Self>, index: usize) -> TextRef {
        TextRef { source: Some(self.clone()), index: index as i64 }
    }
}

/// イベント本文の参照。保存待ちのイベントはファイルごとの状態と圧縮本文を共有する。
#[derive(Clone, Default)]
pub struct TextRef {
    source: Option<Arc<Texts>>,
    index: i64,
}

// 既存の行IDと同じ領域に収め、全イベントの常駐サイズを増やさない。
const _: () = assert!(std::mem::size_of::<TextRef>() <= std::mem::size_of::<Option<i64>>());

impl std::fmt::Debug for TextRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextRef").field("index", &self.index).finish_non_exhaustive()
    }
}

/// 詳細を読むときに取得する本文の状態。保存中に状態が変わっても取得結果は有効。
pub enum EventText<'a> {
    Inline(&'a str, &'a str),
    Pending(String, String),
    Row(i64),
}

impl TextRef {
    /// 差分解析で共有した本文を、解凍せずに安定側へ渡す。
    pub(crate) fn compressed(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let source = self.source.as_ref()?;
        match &*source.0.lock().unwrap() {
            State::Pending(texts) => Some(texts[self.index as usize].clone()),
            State::Rows(_) => None,
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        let Some(source) = &self.source else { return 0; };
        let overhead = std::mem::size_of::<Texts>() + 4 * std::mem::size_of::<usize>();
        overhead + match &*source.0.lock().unwrap() {
            State::Pending(texts) => std::mem::size_of::<Compressed>()
                + texts.capacity() * std::mem::size_of::<(Vec<u8>, Vec<u8>)>()
                + texts.iter().map(|(i, o)| i.capacity() + o.capacity()).sum::<usize>(),
            State::Rows(rows) => rows.capacity() * std::mem::size_of::<i64>(),
        }
    }

    /// 既存のSQLite行は共有状態を確保せず、IDを直接保持する。
    pub fn row(id: i64) -> Self {
        if id == 0 {
            // SQLiteには明示的にID 0も格納できる。通常の自動採番は1以上。
            Self { source: Some(Arc::new(Texts(Mutex::new(State::Rows(vec![0]))))), index: 0 }
        } else {
            Self { source: None, index: id }
        }
    }

    pub(crate) fn row_id(&self) -> Option<i64> {
        match &self.source {
            Some(source) => match &*source.0.lock().unwrap() {
                State::Rows(rows) => Some(rows[self.index as usize]),
                State::Pending(_) => None,
            },
            None => (self.index != 0).then_some(self.index),
        }
    }

    pub(crate) fn text(&self) -> EventText<'_> {
        let Some(source) = &self.source else {
            return match self.row_id() {
                Some(id) => EventText::Row(id),
                None => EventText::Inline("", ""),
            };
        };
        let texts = match &*source.0.lock().unwrap() {
            State::Rows(rows) => return EventText::Row(rows[self.index as usize]),
            State::Pending(texts) => texts.clone(),
        };
        let (input, output) = &texts[self.index as usize];
        EventText::Pending(crate::db::unpack(input), crate::db::unpack(output))
    }

    pub(crate) fn key(&self) -> Option<usize> { self.source.as_ref().map(|s| Arc::as_ptr(s) as usize) }

    pub(crate) fn pending_bytes(&self) -> usize {
        let Some(source) = &self.source else { return 0; };
        match &*source.0.lock().unwrap() {
            State::Pending(texts) => texts.iter().map(|(i, o)| i.len() + o.len()).sum(),
            State::Rows(_) => 0,
        }
    }
}
