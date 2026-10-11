//! レーンの表示と活動を同じ状態バイトへ記録する。
use crate::model::Session;

pub(super) const SHOW: u8 = 1;
pub(super) const ACTIVE: u8 = 2;

/// statesはレーン数と同じ長さのゼロ初期化した作業領域。
pub(super) fn mark(s: &Session, (v0, v1): (f64, f64), in_range: bool, has_children: bool, states: &mut [u8]) {
    if has_children && !states.is_empty() { states[0] = SHOW; }
    for &(li, _) in &s.tree {
        if !in_range { continue; }
        let lane = &s.lanes[li];
        let i = lane.events.partition_point(|e| e.start < v0);
        if !(lane.events.get(i).is_some_and(|e| e.start <= v1) || (i > 0 && lane.cum_end[i - 1] >= v0)) {
            continue;
        }
        states[li] |= ACTIVE;
        let mut cur = Some(li);
        while let Some(c) = cur {
            let old = states[c];
            states[c] |= SHOW;
            if old & SHOW != 0 && c != li { break; }
            cur = s.parents.get(c).copied().flatten();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Source};

    // 変更前の二つのbool配列を、境界・親子関係の参照実装として残す。
    fn reference(s: &Session, (v0, v1): (f64, f64), in_range: bool, children: bool) -> Vec<u8> {
        let visible = |li: usize| {
            let l = &s.lanes[li];
            let i = l.events.partition_point(|e| e.start < v0);
            l.events.get(i).is_some_and(|e| e.start <= v1) || (i > 0 && l.cum_end[i - 1] >= v0)
        };
        let mut show = vec![false; s.lanes.len()];
        if children && !show.is_empty() { show[0] = true; }
        let mut active = vec![false; s.lanes.len()];
        for &(li, _) in &s.tree {
            if in_range && visible(li) {
                active[li] = true;
                let mut cur = Some(li);
                while let Some(c) = cur {
                    if std::mem::replace(&mut show[c], true) && c != li { break; }
                    cur = s.parents.get(c).copied().flatten();
                }
            }
        }
        show.into_iter().zip(active).map(|(show, active)| u8::from(show) | (u8::from(active) << 1)).collect()
    }

    fn session(lanes: usize) -> Session {
        let mut s = Session {
            id: "flags".into(), source: Source::Claude, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(),
            lanes: (0..lanes).map(|li| {
                let mut lane = Lane::new(format!("lane-{li}"));
                if li > 0 { lane.spawned_by = Some(format!("call-{li}")); }
                lane.events = [li as f64, li as f64 + 10.0].into_iter().enumerate().map(|(i, start)| Event {
                    start, end: if i == 0 {start + 20.0} else {start}, kind: Kind::Tool,
                    name: "exec_command".into(), summary: Default::default(), input: String::new(), output: String::new(),
                    call_id: Some(format!("call-{}", li + 1).into()), text_ref: Default::default(),
                    usage: Default::default(), model: None, status: Default::default(),
                }).collect();
                lane
            }).collect(), start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        };
        s.finalize();s
    }

    #[test]
    fn flags_match_visible_ancestors_children_and_floating_boundaries() {
        for n in [0, 1, 2, 31, 32, 33, 65, 257] {
            let s = session(n);
            for range in [(-10.0,-1.0), (0.0,0.0), (10.0,10.0), (20.0,20.0), (31.0,35.0), (100.0,110.0),
                (f64::NEG_INFINITY,f64::INFINITY), (f64::NAN,8.0), (8.0,f64::NAN), (8.0,2.0)] {
                for in_range in [false,true] { for children in [false,true] {
                    let mut states = vec![0; n];
                    mark(&s, range, in_range, children, &mut states);
                    assert_eq!(states, reference(&s, range, in_range, children), "{n} {range:?} {in_range} {children}");
                }}
            }
        }
    }

    #[test]
    fn flags_preserve_missing_reordered_and_unlisted_ancestors() {
        let mut s = session(4);
        s.tree = vec![(3,3), (1,1), (3,3)];
        s.parents.truncate(3);
        for children in [false,true] {
            let mut states = [0;4];
            mark(&s, (5.0,5.0), true, children, &mut states);
            assert_eq!(states.as_slice(), reference(&s, (5.0,5.0), true, children));
        }
        s.tree.clear();
        let mut states = [0;4];
        mark(&s, (f64::NEG_INFINITY,f64::INFINITY), true, true, &mut states);
        assert_eq!(states, [SHOW,0,0,0]);
        let s = session(4);
        let mut states = [0;4];
        mark(&s, (23.0,23.0), true, false, &mut states);
        assert_eq!(states[0], SHOW);
        assert_ne!(states[3] & ACTIVE, 0);
    }
}

/// mainだけの通常の木なら、作業配列を使わず表示と活動を求める。
pub(super) fn single(s: &Session, (v0, v1): (f64, f64), in_range: bool, has_children: bool) -> Option<u8> {
    if s.lanes.len() != 1 || s.tree.as_slice() != [(0, 0)] || s.parents.first().copied().flatten().is_some() {
        return None;
    }
    let lane = &s.lanes[0];
    let active = in_range && {
        let i = lane.events.partition_point(|e| e.start < v0);
        lane.events.get(i).is_some_and(|e| e.start <= v1) || (i > 0 && lane.cum_end[i - 1] >= v0)
    };
    Some(if active { SHOW | ACTIVE } else if has_children { SHOW } else { 0 })
}

#[cfg(test)]
mod single_tests {
    use super::*;
    use crate::model::{Event,Kind,Lane,Source};
    fn session()->Session {
        let mut lane=Lane::new("main");
        lane.events=[(0.0,20.0),(10.0,10.0),(30.0,31.0)].into_iter().map(|(start,end)|Event {
            start,end,kind:Kind::Tool,name:"exec_command".into(),summary:Default::default(),input:String::new(),output:String::new(),
            call_id:None,text_ref:Default::default(),usage:Default::default(),model:None,status:Default::default(),
        }).collect();
        let mut s=Session {id:"single".into(),source:Source::Codex,cwd:String::new(),headless:false,title:String::new(),branch:String::new(),version:String::new(),
            lanes:vec![lane],start:0.0,end:0.0,parents:Vec::new(),tree:Vec::new()};
        s.finalize();s
    }
    #[test]
    fn single_state_matches_original_marks_at_boundaries_and_for_visible_children() {
        let mut s=session();
        for empty in [false,true] {
            if empty {s.lanes[0].events.clear();s.finalize();}
            for range in [(-10.0,-1.0),(0.0,0.0),(10.0,10.0),(20.0,20.0),(21.0,29.0),(30.0,30.0),(31.0,31.0),(40.0,40.0),
                (f64::NEG_INFINITY,f64::INFINITY),(f64::NAN,8.0),(8.0,f64::NAN),(8.0,2.0)] {
                for in_range in [false,true] {for children in [false,true] {
                    let mut reference=[0];mark(&s,range,in_range,children,&mut reference);
                    assert_eq!(single(&s,range,in_range,children),Some(reference[0]),"{range:?} {empty} {in_range} {children}");
                }}
            }
        }
    }
    #[test]
    fn single_state_keeps_noncanonical_trees_and_parent_layouts_on_original_path() {
        let mut s=session();s.parents.clear();assert!(single(&s,(0.0,1.0),true,false).is_some());
        s.parents=vec![Some(0)];assert!(single(&s,(0.0,1.0),true,false).is_none());
        s.parents=vec![None];s.tree=vec![(0,2)];assert!(single(&s,(0.0,1.0),true,false).is_none());
        s.tree=vec![(0,0),(0,0)];assert!(single(&s,(0.0,1.0),true,false).is_none());
        s.tree.clear();assert!(single(&s,(0.0,1.0),true,false).is_none());
        s.lanes.push(Lane::new("child"));s.finalize();assert!(single(&s,(0.0,1.0),true,false).is_none());
        s.lanes.clear();s.finalize();assert!(single(&s,(0.0,1.0),true,false).is_none());
    }
}
