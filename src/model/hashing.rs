//! Store の内部セッション添字専用のハッシュ。外部文字列には使わない。

#[derive(Default)]
pub struct SessionIndexHasher(u64);
impl std::hash::Hasher for SessionIndexHasher {
    #[inline]
    fn finish(&self) -> u64 {
        mix_index(self.0)
    }
    #[inline]
    fn write_usize(&mut self, index: usize) {
        self.0 = index as u64;
    }
    #[inline]
    fn write(&mut self, _: &[u8]) {
        unreachable!("session indices write a single usize");
    }
}

#[inline]
pub(crate) fn mix_index(value: u64) -> u64 {
    let mut hash = value;
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}
