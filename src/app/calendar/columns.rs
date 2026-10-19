//! カレンダーの重なり列の配置・幅の計算。

/// 時間帯が重なる区間だけを横に並べる。各区間の (列, 幅の列数, その重なりの列数) を返す。
/// 右の列がその時間帯に空いていれば、その分だけ広げて名前を読みやすくする。
/// 見た目で接しないよう、間が 8px 未満のものも重なりとみなす
pub(super) fn pack_columns_small<const SORTED: bool>(
    spans: &[(f64, f64)],
    spp: f64,
    order: &[usize],
) -> Vec<(usize, usize, usize)> {
    let pad = 8.0 * spp;
    let mut out = vec![(0, 1, 1); spans.len()];
    let mut cols: Vec<f64> = Vec::new();
    let mut members = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        // 重なりの連鎖（クラスタ）ごとに列を割り当てる
        cols.clear();
        members.clear();
        let mut cluster_end = f64::MIN;
        while i < spans.len() {
            let k = if SORTED { i } else { order[i] };
            let (s, e) = spans[k];
            if !members.is_empty() && s >= cluster_end + pad {
                break;
            }
            let e = e.max(s + pad);
            let col = match cols.iter().position(|&ce| ce + pad <= s) {
                Some(c) => {
                    cols[c] = e;
                    c
                }
                None => {
                    cols.push(e);
                    cols.len() - 1
                }
            };
            out[k].0 = col;
            members.push(k);
            cluster_end = cluster_end.max(e);
            i += 1;
        }
        let n = cols.len();
        for &k in &members {
            out[k].2 = n;
        }
        for &k in &members {
            let (s, e) = (spans[k].0, spans[k].1.max(spans[k].0 + pad));
            let blocked = |c: usize| {
                members.iter().any(|&o| {
                    o != k
                        && out[o].0 == c
                        && spans[o].0 < e + pad
                        && s < spans[o].1.max(spans[o].0 + pad) + pad
                })
            };
            out[k].1 = (out[k].0 + 1..n).take_while(|&c| !blocked(c)).count() + 1;
        }
    }
    out
}

#[derive(Clone, Copy)]
pub(super) struct ColumnEnd(f64, usize);
impl PartialEq for ColumnEnd {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for ColumnEnd {}
impl PartialOrd for ColumnEnd {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ColumnEnd {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

pub(super) fn pack_columns_large<const SORTED: bool>(
    spans: &[(f64, f64)],
    spp: f64,
    order: &[usize],
) -> Vec<(usize, usize, usize)> {
    use std::{cmp::Reverse, collections::BinaryHeap};
    let pad = 8.0 * spp;
    let indexed = pad.is_finite() && pad >= 0.0;
    let mut out = vec![(0, 1, 1); spans.len()];
    let mut cols: Vec<f64> = Vec::new();
    let mut members = Vec::new();
    let mut ending: BinaryHeap<Reverse<ColumnEnd>> = BinaryHeap::new();
    let mut available: BinaryHeap<Reverse<usize>> = BinaryHeap::new();
    let mut scratch = ColumnWidthScratch::default();
    let mut i = 0;
    while i < spans.len() {
        cols.clear();
        members.clear();
        ending.clear();
        available.clear();
        let mut heap = false;
        let mut cluster_end = f64::MIN;
        while i < spans.len() {
            let k = if SORTED { i } else { order[i] };
            let (s, e) = spans[k];
            if !members.is_empty() && s >= cluster_end + pad {
                break;
            }
            let e = e.max(s + pad);
            if indexed && !heap && cols.len() >= 32 {
                ending.extend(cols.iter().enumerate().filter_map(|(c, &end)| {
                    let end = end + pad;
                    (!end.is_nan()).then_some(Reverse(ColumnEnd(end, c)))
                }));
                heap = true;
            }
            let free = if heap {
                if s.is_nan() {
                    None
                } else {
                    while ending.peek().is_some_and(|v| v.0.0 <= s) {
                        available.push(Reverse(ending.pop().unwrap().0.1));
                    }
                    available.pop().map(|v| v.0)
                }
            } else {
                cols.iter().position(|&ce| ce + pad <= s)
            };
            let col = match free {
                Some(c) => {
                    cols[c] = e;
                    c
                }
                None => {
                    cols.push(e);
                    cols.len() - 1
                }
            };
            if heap && !(e + pad).is_nan() {
                ending.push(Reverse(ColumnEnd(e + pad, col)));
            }
            out[k].0 = col;
            members.push(k);
            cluster_end = cluster_end.max(e);
            i += 1;
        }
        let n = cols.len();
        if n == 1 {
            continue;
        }
        for &k in &members {
            out[k].2 = n;
        }
        let start = spans[members[0]].0;
        if indexed
            && pad > 0.0
            && start + pad + pad > start
            && spans[*members.last().unwrap()].0 == start
        {
            continue;
        }
        let widths =
            indexed && members.len() >= 64 && members.iter().all(|&k| !spans[k].0.is_nan());
        if widths {
            scratch.offsets.clear();
            scratch.offsets.resize(n + 1, 0usize);
            for &k in &members {
                scratch.offsets[out[k].0 + 1] += 1;
            }
            for c in 1..=n {
                scratch.offsets[c] += scratch.offsets[c - 1];
            }
            scratch.next.clear();
            scratch.next.extend_from_slice(&scratch.offsets[..n]);
            scratch.intervals.clear();
            scratch.intervals.resize(members.len(), (0.0, 0.0));
            for &k in &members {
                let c = out[k].0;
                let slot = scratch.next[c];
                let (s, end) = spans[k];
                let end = end.max(s + pad) + pad;
                let end = if slot == scratch.offsets[c] {
                    end
                } else {
                    scratch.intervals[slot - 1].1.max(end)
                };
                scratch.intervals[slot] = (s, end);
                scratch.next[c] += 1;
            }
        }
        if widths
            && n < 32
            && members.len() >= 4096
            && members.windows(2).all(|pair| {
                let upper = |k: usize| spans[k].1.max(spans[k].0 + pad) + pad;
                upper(pair[0]).total_cmp(&upper(pair[1])).is_le()
            })
        {
            cursor_column_widths(spans, pad, &members, &mut out, &mut scratch);
            continue;
        }
        if widths && n >= 32 && members.len() >= 128 {
            expand_indexed_columns(spans, pad, &members, &mut out, &mut scratch);
            continue;
        }
        for &k in &members {
            let (s, e) = (spans[k].0, spans[k].1.max(spans[k].0 + pad));
            let blocked = |c: usize| {
                if widths {
                    let column = &scratch.intervals[scratch.offsets[c]..scratch.offsets[c + 1]];
                    let end = column.partition_point(|&(start, _)| start < e + pad);
                    end > 0 && s < column[end - 1].1
                } else {
                    members.iter().any(|&o| {
                        o != k
                            && out[o].0 == c
                            && spans[o].0 < e + pad
                            && s < spans[o].1.max(spans[o].0 + pad) + pad
                    })
                }
            };
            out[k].1 = (out[k].0 + 1..n).take_while(|&c| !blocked(c)).count() + 1;
        }
    }
    out
}

pub(super) fn pack_columns(spans: &[(f64, f64)], spp: f64) -> Vec<(usize, usize, usize)> {
    if spans.len() <= 1 {
        return vec![(0, 1, 1); spans.len()];
    }
    if spans
        .windows(2)
        .all(|pair| pair[0].0.total_cmp(&pair[1].0).is_le())
    {
        let pad = 8.0 * spp;
        if pad.is_finite()
            && pad >= 0.0
            && spans.windows(2).all(|pair| {
                pair[0].0.is_finite()
                    && pair[1].0.is_finite()
                    && pair[0].1.max(pair[0].0 + pad) + pad <= pair[1].0
            })
        {
            return vec![(0, 1, 1); spans.len()];
        }
        if spans.len() < 64 {
            pack_columns_small::<true>(spans, spp, &[])
        } else {
            pack_columns_large::<true>(spans, spp, &[])
        }
    } else {
        let mut order: Vec<usize> = (0..spans.len()).collect();
        order.sort_unstable_by(|&a, &b| spans[a].0.total_cmp(&spans[b].0).then(a.cmp(&b)));
        if spans.len() < 64 {
            pack_columns_small::<false>(spans, spp, &order)
        } else {
            pack_columns_large::<false>(spans, spp, &order)
        }
    }
}

#[derive(Default)]
pub(super) struct ColumnWidthScratch {
    pub(super) offsets: Vec<usize>,
    pub(super) next: Vec<usize>,
    pub(super) intervals: Vec<(f64, f64)>,
    pub(super) tree: Vec<f64>,
}

pub(super) fn first_blocking_column(
    tree: &[f64],
    node: usize,
    left: usize,
    right: usize,
    from: usize,
    start: f64,
) -> Option<usize> {
    if right <= from || tree[node] <= start {
        return None;
    }
    if right - left == 1 {
        return Some(left);
    }
    let middle = left + (right - left) / 2;
    first_blocking_column(tree, node * 2, left, middle, from, start)
        .or_else(|| first_blocking_column(tree, node * 2 + 1, middle, right, from, start))
}

pub(super) fn sweep_column_widths(
    spans: &[(f64, f64)],
    pad: f64,
    members: &[usize],
    first: usize,
    out: &mut [(usize, usize, usize)],
    scratch: &mut ColumnWidthScratch,
) {
    let n = out[members[0]].2;
    let base = n.next_power_of_two();
    // The prefix index is no longer used after switching to the sweep.
    scratch.intervals = Vec::new();
    scratch.next.clear();
    let upper = |k: usize| spans[k].1.max(spans[k].0 + pad) + pad;
    let remaining = &members[first..];
    let queries = if remaining
        .windows(2)
        .all(|pair| upper(pair[0]).total_cmp(&upper(pair[1])).is_le())
    {
        remaining
    } else {
        scratch.next.extend_from_slice(remaining);
        scratch
            .next
            .sort_unstable_by(|&a, &b| upper(a).total_cmp(&upper(b)).then(a.cmp(&b)));
        &scratch.next
    };
    let from = queries.iter().map(|&k| out[k].0 + 1).min().unwrap_or(n);
    if from == n {
        return;
    }
    scratch.tree.clear();
    scratch.tree.resize(base * 2, f64::NEG_INFINITY);
    let mut active = 0;
    for &k in queries {
        let end = upper(k);
        while active < members.len() && spans[members[active]].0 < end {
            let other = members[active];
            let mut node = base + out[other].0;
            let value = upper(other);
            if out[other].0 >= from && value > scratch.tree[node] {
                scratch.tree[node] = value;
                node /= 2;
                while node > 0 {
                    let value = scratch.tree[node * 2].max(scratch.tree[node * 2 + 1]);
                    if value <= scratch.tree[node] {
                        break;
                    }
                    scratch.tree[node] = value;
                    node /= 2;
                }
            }
            active += 1;
        }
        let col = out[k].0;
        let blocked =
            first_blocking_column(&scratch.tree, 1, 0, base, col + 1, spans[k].0).unwrap_or(n);
        out[k].1 = blocked - col;
    }
}

pub(super) fn expand_indexed_columns(
    spans: &[(f64, f64)],
    pad: f64,
    members: &[usize],
    out: &mut [(usize, usize, usize)],
    scratch: &mut ColumnWidthScratch,
) {
    let n = scratch.offsets.len() - 1;
    let mut probes = 0;
    let mut budget = members.len() * 8;
    for (position, &k) in members.iter().enumerate() {
        if probes >= budget {
            let remaining = &members[position..];
            let ordered = || {
                remaining.windows(2).all(|pair| {
                    let upper = |k: usize| spans[k].1.max(spans[k].0 + pad) + pad;
                    upper(pair[0]).total_cmp(&upper(pair[1])).is_le()
                })
            };
            if remaining.len() >= 64 && (n >= 128 || ordered()) {
                sweep_column_widths(spans, pad, members, position, out, scratch);
                return;
            }
            // Sorting is not worthwhile with few columns or few queries left.
            budget = usize::MAX;
        }
        let (start, end) = (spans[k].0, spans[k].1.max(spans[k].0 + pad) + pad);
        let own = out[k].0;
        let mut col = own + 1;
        while col < n {
            let column = &scratch.intervals[scratch.offsets[col]..scratch.offsets[col + 1]];
            let end = column.partition_point(|&(s, _)| s < end);
            if end > 0 && start < column[end - 1].1 {
                break;
            }
            col += 1;
        }
        if budget != usize::MAX {
            probes += col - own - usize::from(col == n);
        }
        out[k].1 = col - own;
    }
}

pub(super) fn cursor_column_widths(
    spans: &[(f64, f64)],
    pad: f64,
    members: &[usize],
    out: &mut [(usize, usize, usize)],
    scratch: &mut ColumnWidthScratch,
) {
    let n = scratch.offsets.len() - 1;
    scratch.next.clear();
    scratch.next.extend_from_slice(&scratch.offsets[..n]);
    for &k in members {
        let start = spans[k].0;
        let end = spans[k].1.max(start + pad) + pad;
        let own = out[k].0;
        let mut col = own + 1;
        while col < n {
            let first = scratch.offsets[col];
            let last = scratch.offsets[col + 1];
            let mut next = scratch.next[col];
            while next < last && scratch.intervals[next].0 < end {
                next += 1;
            }
            scratch.next[col] = next;
            if next > first && start < scratch.intervals[next - 1].1 {
                break;
            }
            col += 1;
        }
        out[k].1 = col - own;
    }
}
