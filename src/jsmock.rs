//! Codex コードモード (`exec`) の JS を QuickJS で実行し、`tools.xxx(...)` の呼び出しを記録する
//!
//! `tools` はすべての呼び出しを記録してダミーの結果を返すモックにする。変数・テンプレートリテラル・
//! ループで組み立てた引数も評価後の値で取れるが、前の呼び出しの結果に依存する引数や分岐は
//! ダミーの結果に基づくため、実際とは異なりうる（推定値）。

use std::time::{Duration, Instant};

use rquickjs::{Context, Runtime};
use serde_json::Value;

/// モックの挙動を変えたら上げる（キャッシュが無効になる）
pub const ENGINE_VERSION: i64 = 1;

/// 1 回の実行に許す時間。無限ループや重い計算はここで打ち切る
const TIMEOUT: Duration = Duration::from_millis(300);

const PRELUDE: &str = r#"
var __calls = [];
var __result = () => ({ exit_code: 0, output: "", stdout: "", stderr: "", wall_time_seconds: 0, session_id: null });
var tools = new Proxy({}, {
  get: (_, name) => (...args) => {
    if (__calls.length >= 200) throw new Error("too many calls");
    __calls.push([String(name), args.length === 1 ? args[0] : args]);
    return Promise.resolve(__result());
  },
});
var __noop = () => {};
var text = __noop, store = __noop, image = __noop, notify = __noop, yield_control = __noop;
var load = () => undefined;
var console = { log: __noop, error: __noop, warn: __noop, info: __noop };
var setTimeout = (f) => { Promise.resolve().then(f); return 0; };
"#;

/// JS を実行し、記録した (ツール名, 引数) を返す。1 件も取れなければ None
pub fn run(js: &str) -> Option<Vec<(String, Value)>> {
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
    let calls: Vec<(String, Value)> = serde_json::from_str(&json).ok()?;
    (!calls.is_empty()).then_some(calls)
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
        let cmds: Vec<_> = calls.iter().map(|(_, a)| a["cmd"].as_str().unwrap()).collect();
        assert_eq!(cmds, ["cat a.rs", "cat b.rs", "ls"]);
    }

    #[test]
    fn positional_and_timeout() {
        let calls = run(r#"tools.apply_patch("*** Begin Patch"); while (true) {}"#).unwrap();
        assert_eq!(calls[0].0, "apply_patch");
        assert_eq!(calls[0].1, "*** Begin Patch");
    }
}
