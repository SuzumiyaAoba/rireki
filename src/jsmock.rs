//! Codex コードモード (`exec`) の JS を QuickJS で実行し、`tools.xxx(...)` の呼び出しを記録する
//!
//! `tools` はすべての呼び出しを記録してダミーの結果を返すモックにする。変数・テンプレートリテラル・
//! ループで組み立てた引数も評価後の値で取れるが、前の呼び出しの結果に依存する引数や分岐は
//! ダミーの結果に基づくため、実際とは異なりうる（推定値）。

use std::time::{Duration, Instant};

use rquickjs::{Context, Runtime};
use serde_json::Value;

/// モックの挙動を変えたら上げる（キャッシュが無効になる）
pub const ENGINE_VERSION: i64 = 3;

/// 1 回の実行に許す時間。無限ループや重い計算はここで打ち切る
const TIMEOUT: Duration = Duration::from_millis(300);

const PRELUDE: &str = r#"
var __calls = [];
// 配列を回している最中の要素から取った名前。`[["build", "make"], ...].map(([name, cmd]) => tools.x({cmd}))`
// のように、要素の 0 番目（または name などのキー）がその呼び出しの名前になっていることが多い
var __item = null;
var __label = (x) => {
  let l = null;
  if (Array.isArray(x)) l = x[0];
  else if (x && typeof x === "object") l = ["name", "label", "title", "id"].map((k) => x[k]).find((v) => typeof v === "string");
  return typeof l === "string" && l.length <= 60 ? l : null;
};
for (const m of ["map", "forEach", "flatMap", "filter", "some", "every", "find"]) {
  const orig = Array.prototype[m];
  Array.prototype[m] = function (f, self) {
    return orig.call(this, function (x, i, a) {
      const prev = __item;
      __item = __label(x);
      try { return f.call(self, x, i, a); } finally { __item = prev; }
    });
  };
}
var __iter = Array.prototype[Symbol.iterator];
Array.prototype[Symbol.iterator] = function () {
  // 分割代入 `[name, cmd]` も反復を使うので、名前が取れない要素では __item を変えない
  const it = __iter.call(this), saved = __item;
  let mine = false;
  return {
    next() {
      const r = it.next();
      const l = r.done ? null : __label(r.value);
      if (l !== null) { __item = l; mine = true; }
      else if (r.done && mine) { __item = saved; mine = false; }
      return r;
    },
    [Symbol.iterator]() { return this; },
  };
};
var __result = () => ({ exit_code: 0, output: "", stdout: "", stderr: "", wall_time_seconds: 0, session_id: null });
var tools = new Proxy({}, {
  get: (_, name) => (...args) => {
    if (__calls.length >= 200) throw new Error("too many calls");
    // 呼び出し元（ユーザーのコード）の行:列。名前（代入先の変数など）を探すのに使う
    const at = /:(\d+):(\d+)\)?\s*$/.exec(new Error().stack.split("\n")[1] || "");
    __calls.push([String(name), args.length === 1 ? args[0] : args, at ? [+at[1], +at[2]] : null, __item]);
    return Promise.resolve(__result());
  },
});
var __noop = () => {};
var text = __noop, store = __noop, image = __noop, notify = __noop, yield_control = __noop;
var load = () => undefined;
var console = { log: __noop, error: __noop, warn: __noop, info: __noop };
var setTimeout = (f) => { Promise.resolve().then(f); return 0; };
"#;

/// モック実行で記録したツール呼び出し
pub struct Call {
    pub name: String,
    pub args: Value,
    /// 呼び出し箇所の JS 内のバイト位置
    pub at: Option<usize>,
    /// 配列を回して呼んだときの、要素の 0 番目（または name などのキー）
    pub label: Option<String>,
}

/// JS を実行し、記録した呼び出しを返す。1 件も取れなければ None
pub fn run(js: &str) -> Option<Vec<Call>> {
    let rt = Runtime::new().ok()?;
    rt.set_memory_limit(64 << 20);
    rt.set_max_stack_size(1 << 20);
    let deadline = Instant::now() + TIMEOUT;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|ctx| {
        ctx.eval::<(), _>(PRELUDE).ok()?;
        // トップレベル await を使うので async 関数で包む。例外は記録済みの呼び出しを残して無視する
        let src = format!("(async () => {{\n{js}\n}})().catch(() => {{}});");
        let _ = ctx.eval::<(), _>(src);
        Some(())
    })?;
    while Instant::now() <= deadline {
        match rt.execute_pending_job() {
            Ok(true) => {}
            _ => break,
        }
    }
    // 打ち切り後でも記録を読めるよう、割り込みを解除する
    rt.set_interrupt_handler(None);
    let json: String = ctx.with(|ctx| ctx.eval("JSON.stringify(__calls)").ok())?;
    type RecordedCall = (String, Value, Option<(usize, usize)>, Option<String>);
    let raw: Vec<RecordedCall> = serde_json::from_str(&json).ok()?;
    let calls: Vec<Call> = raw
        .into_iter()
        .map(|(name, args, pos, label)| Call { name, args, at: pos.and_then(|(l, c)| offset(js, l, c)), label })
        .collect();
    (!calls.is_empty()).then_some(calls)
}

/// スタックの行:列（1 始まり。ラッパーの 1 行目を含む）を JS 内のバイト位置にする
fn offset(js: &str, line: usize, col: usize) -> Option<usize> {
    let start = if line <= 2 {
        0
    } else {
        js.match_indices('\n').nth(line - 3).map(|(i, _)| i + 1)?
    };
    let l = &js[start..];
    let l = &l[..l.find('\n').unwrap_or(l.len())];
    Some(start + l.char_indices().nth(col.saturating_sub(1)).map_or(l.len(), |(i, _)| i))
}

/// キャッシュのキー（FNV-1a 64bit。Rust のバージョンに依存しない）
pub fn hash(js: &str) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in js.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_and_loops() {
        let js = r#"
            const files = ["a.rs", "b.rs"];
            for (const f of files) text(await tools.exec_command({ cmd: `cat ${f}` }));
            const r = await tools.exec_command({ cmd: "ls" });
            JSON.parse(r.output);
            await tools.exec_command({ cmd: "never" });
        "#;
        let calls = run(js).unwrap();
        let cmds: Vec<_> = calls.iter().map(|c| c.args["cmd"].as_str().unwrap()).collect();
        assert_eq!(cmds, ["cat a.rs", "cat b.rs", "ls"]);
    }

    #[test]
    fn positional_and_timeout() {
        let calls = run(r#"tools.apply_patch("*** Begin Patch"); while (true) {}"#).unwrap();
        assert_eq!(calls[0].name, "apply_patch");
        assert_eq!(calls[0].args, "*** Begin Patch");
    }
}
