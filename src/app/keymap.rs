//! 操作（アクション）とキー割り当て、macOS のメニューバー
//!
//! メニューには各操作に最初に登録したキーが表示されるので、修飾キー付きのものを先に登録する。
//! 1 文字のショートカット（Google カレンダー風の D / W / T など）は、検索欄に入力している間は効かせない。

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, actions};

use crate::input;

actions!(
    rireki,
    [
        Quit,
        Hide,
        HideOthers,
        CloseWindow,
        FocusSearch,
        ToggleSidebar,
        ToggleInViewOnly,
        ShowTimeline,
        ShowDay,
        ShowWeek,
        ShowStatistics,
        GoToday,
        GoPrev,
        GoNext,
        ZoomIn,
        ZoomOut,
        Back,
        Forward,
        NextEvent,
        PrevEvent,
        DetailEvents,
        DetailFiles,
        DetailSession,
        DetailAnalysis,
        ToggleDetailFullscreen,
        CancelAnalysisSearch,
        Dismiss,
        ShowHelp,
        SearchUp,
        SearchDown,
        SearchConfirm,
        SearchCancel,
    ]
);

/// キー割り当て・メニューバー・アプリ全体の操作を登録する
pub fn init(cx: &mut App) {
    input::bind_keys(cx);
    const APP: Option<&str> = Some("Rireki");
    // 1 文字のショートカットは文字入力中には効かせない
    const VIEW: Option<&str> = Some("Rireki && !TextInput");
    const SEARCH: Option<&str> = Some("TextInput && !EventAnalysis");
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-f", FocusSearch, APP),
        KeyBinding::new("cmd-k", FocusSearch, APP),
        KeyBinding::new("ctrl-cmd-s", ToggleSidebar, APP),
        KeyBinding::new("cmd-b", ToggleSidebar, APP),
        KeyBinding::new("shift-cmd-v", ToggleInViewOnly, APP),
        KeyBinding::new("v", ToggleInViewOnly, VIEW),
        KeyBinding::new("cmd-1", ShowTimeline, APP),
        KeyBinding::new("cmd-2", ShowDay, APP),
        KeyBinding::new("cmd-3", ShowWeek, APP),
        KeyBinding::new("cmd-4", ShowStatistics, APP),
        KeyBinding::new("d", ShowDay, VIEW),
        KeyBinding::new("w", ShowWeek, VIEW),
        KeyBinding::new("cmd-t", GoToday, APP),
        KeyBinding::new("t", GoToday, VIEW),
        KeyBinding::new("cmd-left", GoPrev, APP),
        KeyBinding::new("cmd-right", GoNext, APP),
        KeyBinding::new("left", GoPrev, VIEW),
        KeyBinding::new("right", GoNext, VIEW),
        KeyBinding::new("cmd-=", ZoomIn, APP),
        KeyBinding::new("cmd-+", ZoomIn, APP),
        KeyBinding::new("=", ZoomIn, VIEW),
        KeyBinding::new("+", ZoomIn, VIEW),
        KeyBinding::new("cmd--", ZoomOut, APP),
        KeyBinding::new("-", ZoomOut, VIEW),
        KeyBinding::new("cmd-[", Back, APP),
        KeyBinding::new("cmd-]", Forward, APP),
        KeyBinding::new("j", NextEvent, VIEW),
        KeyBinding::new("k", PrevEvent, VIEW),
        KeyBinding::new("1", DetailSession, VIEW),
        KeyBinding::new("2", DetailEvents, VIEW),
        KeyBinding::new("3", DetailFiles, VIEW),
        KeyBinding::new("4", DetailAnalysis, VIEW),
        KeyBinding::new("shift-cmd-enter", ToggleDetailFullscreen, APP),
        KeyBinding::new("escape", CancelAnalysisSearch, Some("TextInput && EventAnalysis")),
        KeyBinding::new("escape", Dismiss, APP),
        KeyBinding::new("cmd-/", ShowHelp, APP),
        KeyBinding::new("?", ShowHelp, VIEW),
        KeyBinding::new("up", SearchUp, SEARCH),
        KeyBinding::new("down", SearchDown, SEARCH),
        KeyBinding::new("enter", SearchConfirm, SEARCH),
        KeyBinding::new("up", SearchUp, VIEW),
        KeyBinding::new("down", SearchDown, VIEW),
        KeyBinding::new("enter", SearchConfirm, VIEW),
        KeyBinding::new("escape", SearchCancel, SEARCH),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    // 1 ウィンドウのアプリなので、閉じたら終了する
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.set_menus(vec![
        Menu {
            name: "rireki".into(),
            items: vec![
                MenuItem::action("rireki を隠す", Hide),
                MenuItem::action("ほかを隠す", HideOthers),
                MenuItem::separator(),
                MenuItem::action("rireki を終了", Quit),
            ],
        },
        Menu {
            name: "編集".into(),
            items: vec![
                MenuItem::os_action("カット", input::Cut, OsAction::Cut),
                MenuItem::os_action("コピー", input::Copy, OsAction::Copy),
                MenuItem::os_action("ペースト", input::Paste, OsAction::Paste),
                MenuItem::os_action("すべてを選択", input::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("検索", FocusSearch),
            ],
        },
        Menu {
            name: "表示".into(),
            items: vec![
                MenuItem::action("タイムライン", ShowTimeline),
                MenuItem::action("日", ShowDay),
                MenuItem::action("週", ShowWeek),
                MenuItem::action("統計", ShowStatistics),
                MenuItem::separator(),
                MenuItem::action("拡大", ZoomIn),
                MenuItem::action("縮小", ZoomOut),
                MenuItem::separator(),
                MenuItem::action("サイドバーの表示を切り替え", ToggleSidebar),
                MenuItem::action("詳細パネルの全画面表示を切り替え", ToggleDetailFullscreen),
                MenuItem::action("イベント分析", DetailAnalysis),
                MenuItem::action("表示範囲内のセッションだけを表示", ToggleInViewOnly),
            ],
        },
        Menu {
            name: "移動".into(),
            items: vec![
                MenuItem::action("今日", GoToday),
                MenuItem::action("前の期間", GoPrev),
                MenuItem::action("次の期間", GoNext),
                MenuItem::separator(),
                MenuItem::action("戻る", Back),
                MenuItem::action("進む", Forward),
                MenuItem::action("次のイベント", NextEvent),
                MenuItem::action("前のイベント", PrevEvent),
                MenuItem::separator(),
                MenuItem::action("詳細を閉じる", Dismiss),
            ],
        },
        Menu { name: "ウインドウ".into(), items: vec![MenuItem::action("閉じる", CloseWindow)] },
        Menu {
            name: "ヘルプ".into(), items: vec![MenuItem::action("キーボードショートカット", ShowHelp)]
        },
    ]);
}
