//! 表示順と起動元から作る、親の件数に応じたトラック索引。
use std::collections::HashMap;
use std::hash::BuildHasherDefault;

#[derive(Default)]
pub(super) struct SessionHash(u64);
impl std::hash::Hasher for SessionHash {
    fn finish(&self) -> u64 {
        let mut hash = self.0;
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^ (hash >> 31)
    }
    fn write_usize(&mut self, index: usize) { self.0 = index as u64; }
    fn write(&mut self, _: &[u8]) { unreachable!("session indices write a single usize"); }
}

pub(super) enum Children {
    Sorted(Vec<(usize, Vec<usize>)>),
    Hashed(HashMap<usize, Vec<usize>, BuildHasherDefault<SessionHash>>),
}
impl Default for Children {
    fn default() -> Self { Self::Sorted(Vec::new()) }
}
impl Children {
    fn is_empty(&self) -> bool {
        match self {Self::Sorted(entries)=>entries.is_empty(),Self::Hashed(map)=>map.is_empty()}
    }
    #[cfg(test)]
    fn capacity(&self) -> usize {
        match self {Self::Sorted(entries)=>entries.capacity(),Self::Hashed(map)=>map.capacity()}
    }
}
#[derive(Default)]
pub(super) struct TrackIndex {
    roots: Vec<usize>,
    pub(super) children: Children,
}
impl TrackIndex {
    pub(super) fn new(order: &[usize], parents: &HashMap<usize, (usize, usize, f64)>) -> Self {
        if parents.is_empty() { return Self::default(); }
        let mut roots = Vec::with_capacity(order.len());
        let mut map: HashMap<usize, Vec<usize>, BuildHasherDefault<SessionHash>> = Default::default();
        for &session in order {
            if let Some(&(parent, _, _)) = parents.get(&session) { map.entry(parent).or_default().push(session); }
            else { roots.push(session); }
        }
        let children = if map.len() <= 8 {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_unstable_by_key(|&(parent, _)| parent);
            Children::Sorted(entries)
        } else { Children::Hashed(map) };
        if children.is_empty() { roots = Vec::new(); }
        Self { roots, children }
    }
    pub(super) fn roots<'a>(&'a self, order: &'a [usize]) -> &'a [usize] {
        if self.children.is_empty() { order } else { &self.roots }
    }
}
impl Children {
    pub(super) fn children(&self, session: usize) -> &[usize] {
        match self {
            Self::Hashed(map) => map.get(&session).map(Vec::as_slice).unwrap_or(&[]),
            Self::Sorted(entries) => {
                let Some(&(first, _)) = entries.first() else { return &[]; };
                if session < first || session > entries.last().unwrap().0 { return &[]; }
                entries.binary_search_by_key(&session, |&(parent, _)| parent).map_or(&[], |i| entries[i].1.as_slice())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(order:&[usize], parents:&HashMap<usize,(usize,usize,f64)>, queries:&[usize]) {
        let mut roots=Vec::new();let mut expected:HashMap<usize,Vec<usize>>=Default::default();
        for &si in order {
            if let Some(&(parent,_,_))=parents.get(&si) {expected.entry(parent).or_default().push(si);}
            else {roots.push(si);}
        }
        let index=TrackIndex::new(order,parents);assert_eq!(index.roots(order),roots);
        for &query in queries {assert_eq!(index.children.children(query),expected.get(&query).map(Vec::as_slice).unwrap_or(&[]));}
        if expected.is_empty() {
            assert_eq!(index.roots(order).as_ptr(),order.as_ptr());assert_eq!(index.roots.capacity(),0);assert_eq!(index.children.capacity(),0);
        }
    }
    #[test]
    fn index_keeps_roots_child_order_missing_parents_and_cycles() {
        let order=[8,5,3,9,2,7,0];
        let parents=HashMap::from([(8,(5,1,-0.0)),(3,(5,2,3.0)),(9,(50,0,4.0)),(0,(2,0,5.0)),(2,(7,0,6.0)),(7,(2,0,7.0)),(60,(5,0,8.0))]);
        check(&order,&parents,&[0,2,3,5,7,8,9,50,60,usize::MAX]);
        let index=TrackIndex::new(&order,&parents);assert_eq!(index.roots(&order),[5]);assert_eq!(index.children.children(5),[8,3]);
        assert_eq!(index.children.children(2),[7,0]);assert_eq!(index.children.children(50),[9]);
        let order=[2,7,9];check(&order,&parents,&[2,7,50]);assert!(TrackIndex::new(&order,&parents).roots(&order).is_empty());
    }
    #[test]
    fn rebuild_replaces_parent_order_scope_and_empty_storage() {
        let mut parents=HashMap::from([(1,(0,0,1.0)),(2,(0,0,2.0))]);
        check(&[2,1,0,3],&parents,&[0,1,2,3]);check(&[3,0,1],&parents,&[0,1,2,3]);
        parents.insert(1,(3,0,4.0));check(&[3,0,1],&parents,&[0,1,2,3]);
        let mut index=TrackIndex::new(&[3,0,1],&parents);assert_eq!(index.children.children(3),[1]);
        parents.clear();index=TrackIndex::new(&[3,0,1],&parents);assert!(index.children.children(3).is_empty());assert_eq!(index.children.capacity(),0);
        check(&[3,0,1],&parents,&[0,1,2,3]);parents.insert(99,(0,0,5.0));check(&[3,0,1],&parents,&[0,99]);check(&[],&parents,&[0,99]);
    }
    #[test]
    fn small_and_large_storage_keep_identical_semantics_at_the_boundary() {
        for count in [0,1,7,8,9,16,4096] {
            let order:Vec<_>=(0..count*2).rev().collect();
            let parents:HashMap<_,_>=(0..count).map(|i|(i*2+1,(i*2,0,0.0))).collect();
            let index=TrackIndex::new(&order,&parents);
            assert_eq!(matches!(index.children,Children::Sorted(_)),count<=8);
            check(&order,&parents,&(0..count*2+3).collect::<Vec<_>>());
        }
    }
    #[test]
    fn dense_sparse_and_high_number_queries_keep_complete_keys() {
        for step in [1,17,257] {
            let mut order:Vec<_>=(0..4096).map(|i|i*step).collect();let high=usize::MAX-4096;
            order.extend([high,high+1,usize::MAX]);order.reverse();
            let parents:HashMap<_,_>=order.iter().enumerate().filter(|(i,_)|i%3!=0).map(|(i,&si)|(si,((i/3)*step,0,i as f64))).collect();
            let mut queries:Vec<_>=(0..4096).map(|i|i*step).collect();queries.extend([high,high+1,usize::MAX,usize::MAX/2]);check(&order,&parents,&queries);
            check(&[high,0,1],&HashMap::from([(high,(usize::MAX,0,0.0))]),&[0,1,high,usize::MAX]);
        }
    }
}
