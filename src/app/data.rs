//! 履歴更新の反映と、表示対象・プロジェクト・トラックの抽出。

use super::{
    Context, Delta, DetailTab, HashMap, HashSet, MIN_BLOCK, Prefs, Project, Rireki, Row, Source,
    SourceFilter, Track, TrackIndex, base_name, is_live, lane_flags, prefs, track_index,
    view_index,
};

impl Rireki {
    /// 画面の状態が前回の保存から変わっていれば保存する
    pub(super) fn persist(&mut self) {
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        let mut folded: Vec<String> = self.folded.iter().cloned().collect();
        folded.sort();
        let p = Prefs {
            mode: self.mode,
            statistics: self.statistics_open,
            calendar: self.calendar,
            sidebar: self.sidebar_open,
            sidebar_w: self.sidebar_w,
            label_w: self.label_w,
            detail_w: self.detail_w,
            gran: self.gran,
            in_view_only: self.in_view_only,
            directory_timeline: self.directory_timeline,
            cost_timeline: self.cost_timeline,
            source: self.source,
            focus: self.focus.clone(),
            collapsed,
            folded,
            detail_tab: self.detail_tab,
            detail_reversed: self.detail_reversed,
        };
        if p != self.saved {
            prefs::save(&p);
            self.saved = p;
        }
    }

    /// 読み込み結果を Store に反映する
    pub(super) fn apply(&mut self, delta: Delta, cx: &mut Context<Self>) -> bool {
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
        // 複数イベントの選択がレーンの末尾まで届いていれば、増えたイベントも選択に含めて追従させる
        let follow = self.selected.filter(|s| {
            self.replay.is_none()
                && s.first < s.last
                && self.valid(*s)
                && s.last + 1 == self.store.sessions[s.session].lanes[s.lane].events.len()
        });
        // 表示順に応じて最新のイベント側にいれば、増えたイベントまでスクロールして追う
        let at_latest = self.detail_at_latest() || self.detail_following || self.detail_follow;
        let Delta { snapshot, touched } = delta;
        if touched.is_empty() {
            return false;
        }
        let old = std::mem::replace(&mut self.store, snapshot);
        if let Some(server) = &self.mcp_server {
            server.update(self.store.clone());
        }
        self.layout_cache.get_mut().invalidate(&touched);
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results(cx);
        }
        // 選択中のセッションが更新されたら、範囲を確かめて詳細を作り直す
        if let Some(mut sel) = self.selected.filter(|s| touched.contains(&s.session)) {
            let following = follow == Some(sel);
            if follow == Some(sel) {
                if let Some(l) = self
                    .store
                    .sessions
                    .get(sel.session)
                    .and_then(|s| s.lanes.get(sel.lane))
                {
                    sel.last = sel.last.max(l.events.len().saturating_sub(1));
                }
                // 選択が伸びただけなので、絞り込みや開閉・スクロール位置は保つ
                self.selected = Some(sel);
            }
            if self.valid(sel) {
                // 再読み込みが終わるまで、本文と集計を同じ版のまま表示する。
                // リプレイ中は記録時刻と本文の版を固定し、終了後に更新を取り込む。
                if self.replay.is_none() {
                    self.refresh_detail(sel, cx);
                    self.detail_follow =
                        following && at_latest && self.detail_tab == DetailTab::Events;
                }
            } else {
                self.cancel_detail_load();
                self.reset_analysis();
                self.detail_fullscreen = false;
                self.selected = None;
                self.clear_detail_cache(cx);
                self.detail_changed(cx);
            }
        }
        self.sidebar_changed(cx);
        // 触れた番号を背景へ渡し、旧版の本文と検索索引の解放も UI スレッドで行わない。
        let touched_count = touched.len();
        let search = self.search.clone();
        cx.background_executor()
            .spawn(async move {
                search.lock().unwrap().invalidate_versions(
                    touched
                        .into_iter()
                        .filter_map(|si| old.sessions.get(si).map(|s| (si, s))),
                );
                drop(old);
            })
            .detach();
        if let Some(started) = started {
            println!(
                "{}",
                serde_json::json!({"ui_profile":"apply", "ms":started.elapsed().as_secs_f64()*1000.0,
                "touched":touched_count, "sessions":self.store.sessions.len()})
            );
        }
        true
    }

    pub(super) fn rebuild_order(&mut self) {
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
        let scope: Vec<usize> = (0..self.store.sessions.len())
            .filter(|&i| self.store.visible(i))
            .filter(|&i| match (self.source, self.store.sessions[i].source) {
                (SourceFilter::All, _) => true,
                (SourceFilter::Claude, s) => s == Source::Claude,
                (SourceFilter::Codex, s) => s == Source::Codex,
            })
            .collect();
        let mut order: Vec<usize> = scope
            .iter()
            .copied()
            .filter(|&i| {
                self.focus
                    .as_ref()
                    .is_none_or(|f| *f == self.store.sessions[i].cwd)
            })
            .collect();
        // 新しいセッションほど上に並べる
        order.sort_by(|&a, &b| {
            self.store.sessions[b]
                .start
                .total_cmp(&self.store.sessions[a].start)
        });
        self.projects = self.projects_of(&scope);
        self.scope = scope;
        self.order = order;
        self.order_extent.set(None);
        self.exec_parent = self.find_exec_parents();
        self.track_index = TrackIndex::new(&self.order, &self.exec_parent);
        self.live_sources = self.find_live_sources();
        self.data_gen += 1;
        if let Some(started) = started {
            println!(
                "{}",
                serde_json::json!({"ui_profile":"rebuild_order", "ms":started.elapsed().as_secs_f64()*1000.0,
                "visible":self.order.len(), "links":self.exec_parent.len()})
            );
        }
    }

    /// セッションをプロジェクトごとにまとめる（最近作業した順）
    pub(super) fn projects_of(&self, scope: &[usize]) -> Vec<Project> {
        let mut by: HashMap<&str, (usize, f64)> = HashMap::new();
        for &si in scope {
            let s = &self.store.sessions[si];
            let e = by.entry(s.cwd.as_str()).or_insert((0, f64::NEG_INFINITY));
            e.0 += 1;
            e.1 = e.1.max(s.end);
        }
        let mut names: HashMap<&str, usize> = HashMap::new();
        for cwd in by.keys() {
            *names.entry(base_name(cwd)).or_default() += 1;
        }
        let mut out: Vec<Project> = by
            .iter()
            .map(|(&cwd, &(sessions, last))| {
                let base = base_name(cwd);
                let name = if names[base] > 1 {
                    // 同じ名前のディレクトリが複数あるときは親ディレクトリで見分ける
                    let parent = rireki::paths::parent_name(cwd);
                    format!("{base} — {parent}")
                } else {
                    base.to_string()
                };
                Project {
                    cwd: cwd.to_string(),
                    name,
                    sessions,
                    last,
                }
            })
            .collect();
        out.sort_by(|a, b| b.last.total_cmp(&a.last));
        out
    }

    pub(super) fn find_live_sources(&self) -> (bool, bool) {
        let mut out = (false, false);
        for (i, s) in self.store.sessions.iter().enumerate() {
            if self.store.visible(i) && is_live(s.end) {
                match s.source {
                    Source::Claude => out.0 = true,
                    Source::Codex => out.1 = true,
                }
            }
        }
        out
    }

    /// プロジェクトの表示名
    pub(super) fn project_name(&self, cwd: &str) -> String {
        self.projects
            .iter()
            .find(|p| p.cwd == cwd)
            .map_or_else(|| base_name(cwd).to_string(), |p| p.name.clone())
    }

    pub(super) fn find_exec_parents(&self) -> HashMap<usize, (usize, usize, f64)> {
        view_index::exec_parents(&self.store.sessions, &self.order)
    }

    /// 期間内にイベントを持つレーンを、セッション → レーンの順で並べる。
    /// 別セッションとして起動された子は起動元セッションの直下に並べる
    pub(super) fn tracks_in(&self, v0: f64, v1: f64) -> Vec<Track> {
        self.tracks_for(v0, v1, !self.in_view_only)
    }

    /// `all` なら期間外のセッションも並べる（タイムラインでグレーアウトして出す）
    pub(super) fn tracks_for(&self, v0: f64, v1: f64, all: bool) -> Vec<Track> {
        let mut out = Vec::new();
        for &si in self.track_index.roots(&self.order) {
            if !all {
                let session = &self.store.sessions[si];
                if (session.end < v0 || session.start > v1)
                    && self.track_index.children.children(si).is_empty()
                {
                    continue;
                }
            }
            self.session_tracks(
                si,
                0,
                None,
                &self.track_index.children,
                (v0, v1),
                all,
                &mut out,
            );
        }
        // 最初に現れた順を保ったままディレクトリごとにまとめる
        let mut first_seen: HashMap<&str, usize> = Default::default();
        for t in &out {
            let n = first_seen.len();
            first_seen
                .entry(self.store.sessions[t.session].cwd.as_str())
                .or_insert(n);
        }
        // 比較のたびにディレクトリ文字列をハッシュせず、各レーンのキーを一度だけ求める。
        out.sort_by_cached_key(|t| first_seen[self.store.sessions[t.session].cwd.as_str()]);
        out
    }

    /// セッション si のレーン（と子セッション）を out に積む。何か積んだら true
    #[allow(clippy::too_many_arguments)]
    pub(super) fn session_tracks(
        &self,
        si: usize,
        base: usize,
        link: Option<(usize, usize, f64)>,
        children: &track_index::Children,
        (v0, v1): (f64, f64),
        all: bool,
        out: &mut Vec<Track>,
    ) -> bool {
        let s = &self.store.sessions[si];
        let at = out.len();
        let mut kids = Vec::new();
        for &ci in children.children(si) {
            self.session_tracks(
                ci,
                base + 1,
                self.exec_parent.get(&ci).copied(),
                children,
                (v0, v1),
                all,
                &mut kids,
            );
        }
        let in_range = !(s.end < v0 || s.start > v1);
        if !in_range && kids.is_empty() && !all {
            return false;
        }
        if !in_range {
            for &(li, depth) in &s.tree {
                // 従来の状態配列と同じ添字の検証を保つ。
                let _ = &s.lanes[li];
                if all || (li == 0 && !kids.is_empty()) {
                    out.push(Track {
                        session: si,
                        lane: li,
                        depth: base + depth,
                        link: if li == 0 { link } else { None },
                        active: false,
                    });
                }
            }
            out.extend(kids);
            return out.len() > at;
        }
        if let Some(state) = lane_flags::single(s, (v0, v1), in_range, !kids.is_empty()) {
            if state & lane_flags::SHOW != 0 || all {
                out.push(Track {
                    session: si,
                    lane: 0,
                    depth: base,
                    link,
                    active: state & lane_flags::ACTIVE != 0,
                });
            }
            out.extend(kids);
            return out.len() > at;
        }
        let mut inline = [0u8; 32];
        let mut heap;
        let states: &mut [u8] = if s.lanes.len() <= inline.len() {
            &mut inline[..s.lanes.len()]
        } else {
            heap = vec![0; s.lanes.len()];
            &mut heap
        };
        lane_flags::mark(s, (v0, v1), in_range, !kids.is_empty(), states);
        for &(li, depth) in &s.tree {
            if states[li] & lane_flags::SHOW != 0 || all {
                out.push(Track {
                    session: si,
                    lane: li,
                    depth: base + depth,
                    link: if li == 0 { link } else { None },
                    active: states[li] & lane_flags::ACTIVE != 0,
                });
            }
        }
        out.extend(kids);
        out.len() > at
    }

    /// ディレクトリ見出しを挟んだ行。折りたたんだグループはレーンを出さない
    pub(super) fn build_rows_in(&self, v0: f64, v1: f64) -> Vec<Row> {
        if self.directory_timeline {
            let groups = crate::layout::directory::groups(
                &mut self.layout_cache.borrow_mut(),
                &self.store.sessions,
                &self.order,
                &self.exec_parent,
                crate::layout::directory::View {
                    range: (v0, v1),
                    spp: self.spp,
                    gran: self.gran,
                    min_width: MIN_BLOCK,
                    in_view_only: self.in_view_only,
                },
            );
            let mut rows = Vec::new();
            for group in groups {
                let active = group.rows.iter().any(|r| !r.is_empty());
                let mut packed = group.rows.into_iter();
                rows.push(Row::Header {
                    cwd: group.cwd,
                    sessions: group.sessions,
                    collapsed: false,
                    active,
                    blocks: packed.next().unwrap_or_default(),
                });
                rows.extend(packed.map(Row::Packed));
            }
            return rows;
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut cur: Option<String> = None;
        let mut header = 0;
        let mut seen = HashSet::new();
        for t in self.tracks_in(v0, v1) {
            let cwd = &self.store.sessions[t.session].cwd;
            if cur.as_ref() != Some(cwd) {
                cur = Some(cwd.clone());
                header = rows.len();
                seen.clear();
                rows.push(Row::Header {
                    cwd: cwd.clone(),
                    sessions: Vec::new(),
                    collapsed: self.collapsed.contains(cwd),
                    active: false,
                    blocks: Vec::new(),
                });
            }
            let Some(Row::Header {
                sessions,
                collapsed,
                active,
                ..
            }) = rows.get_mut(header)
            else {
                unreachable!()
            };
            *active |= t.active;
            if seen.insert(t.session) {
                sessions.push(t.session);
            }
            if !*collapsed {
                rows.push(Row::Lane(t));
            }
        }
        rows
    }
}
