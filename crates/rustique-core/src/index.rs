//! A CPython-style compact hash index for insertion-ordered maps and sets.
//!
//! Entries live in insertion order in *parallel storage owned by the caller* (a key column,
//! a value column, …). [`HashIndex`] only maps a hash to the position of an entry in that
//! storage; key equality is supplied by the caller as a closure over positions, so the same
//! index serves native keys (`i64`, strings) and keys whose equality needs outside help
//! (Python objects).
//!
//! The layout is CPython's compact dict (`Objects/dictobject.c`): a power-of-two table of
//! *indices* into the dense entry storage, whose width adapts to the table size (`u8` up to
//! 256 slots, then `u16`, `u32`, `u64`), probed with CPython's recurrence
//! (`i = i*5 + perturb + 1`, `perturb >>= 5`), filled to at most `USABLE_FRACTION` (2/3)
//! and regrown to `GROWTH_RATE` (3 × live entries) when full. Every entry's hash is stored
//! and compared before the caller's equality runs. Hashes are used as given: callers pass
//! Python hashes for Python keys (so probe sequences match CPython's) and well-mixed hashes
//! for everything else.
//!
//! Removing an entry leaves a *tombstone*: positions of the other entries stay stable, which
//! keeps live iterators and parallel storage valid. When tombstones pile up
//! ([`HashIndex::wants_compaction`]), [`HashIndex::compact`] squeezes them out and returns a
//! [`Remap`] that the caller applies to each parallel vector.
//!
//! [`KeyIndex`] bundles the index with an owned key vector, and [`OrderedMap`] adds values;
//! both work without closures for keys that implement [`Hash`] + [`Eq`].

use std::borrow::Borrow;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};

/// Below this many tombstones compaction is never worth it.
const MIN_TOMBSTONES_TO_COMPACT: usize = 16;

/// `PyDict_MINSIZE`: the smallest table.
const MIN_SIZE: usize = 8;

/// `PERTURB_SHIFT`.
const PERTURB_SHIFT: u32 = 5;

/// `USABLE_FRACTION(n)`: entries a table of `n` slots holds before it grows.
#[inline]
const fn usable_fraction(size: usize) -> usize {
    (size << 1) / 3
}

/// `calculate_log2_keysize`, as a size: the smallest power of two `>= max(minsize, 8)`.
#[inline]
fn size_for(minsize: usize) -> usize {
    minsize.max(MIN_SIZE).checked_next_power_of_two().unwrap_or(1 << (usize::BITS - 1))
}

/// `estimate_log2_keysize`, as a size: a table that holds `n` entries without growing.
#[inline]
fn size_to_hold(n: usize) -> usize {
    size_for(n.saturating_mul(3).saturating_add(1) / 2)
}

/// One index slot: an entry position, or `EMPTY` (never used) / `DUMMY` (used, then freed).
trait SlotWidth: Copy + Eq {
    const EMPTY: Self;
    const DUMMY: Self;
    /// Positions `< LIMIT` are representable.
    const LIMIT: usize;
    fn from_pos(pos: usize) -> Self;
    fn pos(self) -> usize;
}

macro_rules! slot_width {
    ($($t:ty),*) => {$(
        impl SlotWidth for $t {
            const EMPTY: Self = <$t>::MAX;
            const DUMMY: Self = <$t>::MAX - 1;
            const LIMIT: usize = if (<$t>::MAX as u128) - 1 > usize::MAX as u128 {
                usize::MAX
            } else {
                (<$t>::MAX - 1) as usize
            };
            #[inline(always)]
            fn from_pos(pos: usize) -> Self {
                debug_assert!(pos < Self::LIMIT);
                pos as $t
            }
            #[inline(always)]
            fn pos(self) -> usize {
                self as usize
            }
        }
    )*};
}
slot_width!(u8, u16, u32, u64);

/// The index table, in the narrowest width that represents every position it may hold.
#[derive(Clone, Default)]
enum Slots {
    /// No table yet (an empty index allocates nothing).
    #[default]
    None,
    U8(Box<[u8]>),
    U16(Box<[u16]>),
    U32(Box<[u32]>),
    U64(Box<[u64]>),
}

/// Run `$body` with `$s` bound to the slot slice of whatever width `$slots` has.
macro_rules! with_slots {
    ($slots:expr, $s:ident => $body:expr, None => $none:expr) => {
        match $slots {
            Slots::None => $none,
            Slots::U8($s) => $body,
            Slots::U16($s) => $body,
            Slots::U32($s) => $body,
            Slots::U64($s) => $body,
        }
    };
}

impl Slots {
    /// An all-`EMPTY` table of `size` slots (a power of two) able to hold positions `< bound`.
    fn new(size: usize, bound: usize) -> Slots {
        fn filled<W: SlotWidth>(size: usize) -> Box<[W]> {
            vec![W::EMPTY; size].into_boxed_slice()
        }
        if bound < u8::LIMIT {
            Slots::U8(filled(size))
        } else if bound < u16::LIMIT {
            Slots::U16(filled(size))
        } else if bound < u32::LIMIT {
            Slots::U32(filled(size))
        } else {
            Slots::U64(filled(size))
        }
    }

    #[inline]
    fn size(&self) -> usize {
        with_slots!(self, s => s.len(), None => 0)
    }

    /// Positions this table can represent.
    #[inline]
    fn limit(&self) -> usize {
        match self {
            Slots::None => 0,
            Slots::U8(_) => u8::LIMIT,
            Slots::U16(_) => u16::LIMIT,
            Slots::U32(_) => u32::LIMIT,
            Slots::U64(_) => u64::LIMIT,
        }
    }

    fn fill_empty(&mut self) {
        fn fill<W: SlotWidth>(s: &mut [W]) {
            s.fill(W::EMPTY);
        }
        with_slots!(self, s => fill(s), None => ());
    }
}

/// CPython's probe sequence over a table of `mask + 1` slots.
#[derive(Clone, Copy)]
struct Probe {
    i: usize,
    perturb: usize,
    mask: usize,
}

impl Probe {
    #[inline(always)]
    fn new(hash: u64, mask: usize) -> Probe {
        Probe { i: hash as usize & mask, perturb: hash as usize, mask }
    }

    #[inline(always)]
    fn advance(&mut self) {
        self.perturb >>= PERTURB_SHIFT;
        self.i = (self.i.wrapping_mul(5).wrapping_add(self.perturb).wrapping_add(1)) & self.mask;
    }
}

/// Position of the live entry with `hash` for which `eq` holds (`lookdict` / `do_lookup`).
#[inline(always)]
fn probe_find<W: SlotWidth>(slots: &[W], hashes: &[u64], hash: u64, mut eq: impl FnMut(usize) -> bool) -> Option<usize> {
    let mut p = Probe::new(hash, slots.len() - 1);
    loop {
        let s = slots[p.i];
        if s == W::EMPTY {
            return None;
        }
        if s != W::DUMMY {
            let pos = s.pos();
            if hashes[pos] == hash && eq(pos) {
                return Some(pos);
            }
        }
        p.advance();
    }
}

/// Like [`probe_find`], also returning the first free (`EMPTY` or `DUMMY`) slot of the
/// sequence when the key is absent, so an insertion needs no second probe.
#[inline(always)]
fn probe_find_or_free<W: SlotWidth>(
    slots: &[W],
    hashes: &[u64],
    hash: u64,
    mut eq: impl FnMut(usize) -> bool,
) -> Result<usize, usize> {
    let mut p = Probe::new(hash, slots.len() - 1);
    let mut free = None;
    loop {
        let s = slots[p.i];
        if s == W::EMPTY {
            return Err(free.unwrap_or(p.i));
        }
        if s == W::DUMMY {
            free.get_or_insert(p.i);
        } else {
            let pos = s.pos();
            if hashes[pos] == hash && eq(pos) {
                return Ok(pos);
            }
        }
        p.advance();
    }
}

/// The first free slot of `hash`'s probe sequence (`find_empty_slot`).
#[inline(always)]
fn probe_free<W: SlotWidth>(slots: &[W], hash: u64) -> usize {
    let mut p = Probe::new(hash, slots.len() - 1);
    loop {
        let s = slots[p.i];
        if s == W::EMPTY || s == W::DUMMY {
            return p.i;
        }
        p.advance();
    }
}

/// The slot holding position `pos`, whose hash is `hash` (`lookdict_index`).
#[inline(always)]
fn probe_slot_of<W: SlotWidth>(slots: &[W], hash: u64, pos: usize) -> Option<usize> {
    let mut p = Probe::new(hash, slots.len() - 1);
    loop {
        let s = slots[p.i];
        if s == W::EMPTY {
            return None;
        }
        if s != W::DUMMY && s.pos() == pos {
            return Some(p.i);
        }
        p.advance();
    }
}

/// Maps hashes to entry positions in caller-owned, insertion-ordered storage.
///
/// Positions `0..entries()` are either live or tombstones. New entries are always appended
/// at position `entries()`.
#[derive(Clone, Default)]
pub struct HashIndex {
    slots: Slots,
    /// Hash of the entry at each position (stale for tombstones).
    hashes: Vec<u64>,
    /// Tombstone flags, materialised only once an entry is removed (empty: all live).
    dead: Vec<bool>,
    len: usize,
    /// Insertions left before the table must grow (`dk_usable`).
    usable: usize,
}

impl fmt::Debug for HashIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashIndex")
            .field("len", &self.len)
            .field("entries", &self.entries())
            .field("slots", &self.slots.size())
            .finish()
    }
}

impl HashIndex {
    pub const fn new() -> Self {
        HashIndex { slots: Slots::None, hashes: Vec::new(), dead: Vec::new(), len: 0, usable: 0 }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut index = HashIndex::new();
        if capacity > 0 {
            index.hashes.reserve(capacity);
            index.rebuild(size_to_hold(capacity));
        }
        index
    }

    /// Build an index over `hashes`, all live, at positions `0..n`. The caller guarantees
    /// that the corresponding keys are pairwise distinct.
    pub fn from_hashes(hashes: impl IntoIterator<Item = u64>) -> Self {
        let hashes: Vec<u64> = hashes.into_iter().collect();
        let len = hashes.len();
        let mut index = HashIndex { slots: Slots::None, hashes, dead: Vec::new(), len, usable: 0 };
        if len > 0 {
            index.rebuild(size_to_hold(len));
        }
        index
    }

    /// Number of live entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of positions in use, live or tombstone (the length the caller's parallel
    /// storage must have).
    #[inline]
    pub fn entries(&self) -> usize {
        self.hashes.len()
    }

    #[inline]
    pub fn tombstones(&self) -> usize {
        self.entries() - self.len
    }

    /// Number of index slots (0 until the first insertion).
    #[inline]
    pub fn slots(&self) -> usize {
        self.slots.size()
    }

    #[inline]
    pub fn is_live(&self, pos: usize) -> bool {
        pos < self.entries() && (self.dead.is_empty() || !self.dead[pos])
    }

    /// Hash of the live entry at `pos`.
    #[inline]
    pub fn hash_at(&self, pos: usize) -> Option<u64> {
        self.is_live(pos).then(|| self.hashes[pos])
    }

    /// Hashes of all positions, tombstones included (stale there).
    #[inline]
    pub fn raw_hashes(&self) -> &[u64] {
        &self.hashes
    }

    /// Position of the live entry with `hash` for which `eq(position)` holds. `eq` only
    /// runs for live entries whose stored hash equals `hash`.
    #[inline(always)]
    pub fn find(&self, hash: u64, eq: impl FnMut(usize) -> bool) -> Option<usize> {
        let hashes = &self.hashes;
        with_slots!(&self.slots, s => probe_find(s, hashes, hash, eq), None => None)
    }

    /// Positions of all live entries whose hash equals `hash`, in probe order, each once.
    /// Use this when the equality test cannot run while the index is borrowed (e.g. it calls
    /// back into a runtime that may mutate the container).
    pub fn candidates(&self, hash: u64) -> Candidates<'_> {
        let size = self.slots.size();
        Candidates {
            index: self,
            probe: Probe::new(hash, size.wrapping_sub(1)),
            hash,
            done: size == 0,
            seen: Vec::new(),
        }
    }

    /// Make sure one more entry fits without exceeding the load limit, growing the table
    /// to `GROWTH_RATE` if needed (positions are preserved).
    #[inline]
    fn ensure_usable(&mut self) {
        if self.usable == 0 || self.entries() + 1 >= self.slots.limit() {
            self.grow();
        }
    }

    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        // `GROWTH_RATE(d) = d->ma_used * 3`, at least room for one more entry.
        let size = size_for(self.len.saturating_mul(3)).max(size_to_hold(self.len + 1));
        self.rebuild(size);
    }

    /// Rebuild the index table with `size` slots from the live entries (positions kept).
    fn rebuild(&mut self, size: usize) {
        let size = size.max(size_to_hold(self.len));
        let mut usable = usable_fraction(size) - self.len;
        // Every position that may be appended before the next rebuild must be representable.
        let bound = self.entries() + usable + 1;
        let mut slots = Slots::new(size, bound);
        if bound >= slots.limit() {
            usable = slots.limit().saturating_sub(self.entries() + 1).min(usable);
        }
        fn fill<W: SlotWidth>(s: &mut [W], hashes: &[u64], dead: &[bool]) {
            for (pos, &hash) in hashes.iter().enumerate() {
                if dead.is_empty() || !dead[pos] {
                    let i = probe_free(s, hash);
                    s[i] = W::from_pos(pos);
                }
            }
        }
        with_slots!(&mut slots, s => fill(s, &self.hashes, &self.dead), None => ());
        self.slots = slots;
        self.usable = usable;
    }

    /// Append a new live entry with `hash` and return its position (always the previous
    /// `entries()`). The caller guarantees the key is not already present and appends the
    /// key (and value) to its storage.
    #[inline]
    pub fn push(&mut self, hash: u64) -> usize {
        self.ensure_usable();
        let pos = self.entries();
        fn put<W: SlotWidth>(s: &mut [W], hash: u64, pos: usize) {
            let i = probe_free(s, hash);
            s[i] = W::from_pos(pos);
        }
        with_slots!(&mut self.slots, s => put(s, hash, pos), None => unreachable!("table allocated above"));
        self.record_push(hash);
        pos
    }

    /// Find the live entry with `hash` for which `eq(position)` holds, or append a new one,
    /// with a single probe. Returns the position and whether it was appended (in which case
    /// the caller appends the key to its storage).
    #[inline]
    pub fn find_or_push(&mut self, hash: u64, eq: impl FnMut(usize) -> bool) -> (usize, bool) {
        let pos = self.entries();
        let found = {
            let hashes = &self.hashes;
            with_slots!(&self.slots, s => probe_find_or_free(s, hashes, hash, eq), None => Err(usize::MAX))
        };
        match found {
            Ok(existing) => (existing, false),
            Err(free) => {
                if free == usize::MAX || self.usable == 0 || pos + 1 >= self.slots.limit() {
                    return (self.push(hash), true);
                }
                fn put<W: SlotWidth>(s: &mut [W], i: usize, pos: usize) {
                    s[i] = W::from_pos(pos);
                }
                with_slots!(&mut self.slots, s => put(s, free, pos), None => ());
                self.record_push(hash);
                (pos, true)
            }
        }
    }

    #[inline]
    fn record_push(&mut self, hash: u64) {
        self.hashes.push(hash);
        if !self.dead.is_empty() {
            self.dead.push(false);
        }
        self.len += 1;
        self.usable -= 1;
    }

    fn mark_dead(&mut self, pos: usize) {
        if self.dead.is_empty() {
            self.dead = vec![false; self.entries()];
        }
        self.dead[pos] = true;
        self.len -= 1;
    }

    /// Free the slot holding live position `pos` (it becomes `DUMMY`).
    fn free_slot(&mut self, pos: usize) {
        fn free<W: SlotWidth>(s: &mut [W], hash: u64, pos: usize) {
            match probe_slot_of(s, hash, pos) {
                Some(i) => s[i] = W::DUMMY,
                None => unreachable!("live entry {pos} missing from the index"),
            }
        }
        let hash = self.hashes[pos];
        with_slots!(&mut self.slots, s => free(s, hash, pos), None => unreachable!("live entry without a table"));
    }

    /// Remove the live entry with `hash` for which `eq(position)` holds, leaving a tombstone.
    pub fn remove(&mut self, hash: u64, eq: impl FnMut(usize) -> bool) -> Option<usize> {
        let pos = self.find(hash, eq)?;
        self.free_slot(pos);
        self.mark_dead(pos);
        Some(pos)
    }

    /// Remove the live entry at `pos`, leaving a tombstone. Returns `false` if `pos` is not live.
    pub fn remove_at(&mut self, pos: usize) -> bool {
        if !self.is_live(pos) {
            return false;
        }
        self.free_slot(pos);
        self.mark_dead(pos);
        true
    }

    /// Drop every position `>= len` (live or not). The caller truncates its storage to `len`.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.entries() {
            return;
        }
        if len == 0 {
            self.clear();
            return;
        }
        for pos in len..self.entries() {
            if self.is_live(pos) {
                self.free_slot(pos);
                self.len -= 1;
            }
        }
        self.hashes.truncate(len);
        if self.len == len {
            self.dead = Vec::new();
        } else {
            self.dead.truncate(len);
        }
    }

    /// Remove all entries (keeps the allocation).
    pub fn clear(&mut self) {
        self.slots.fill_empty();
        self.hashes.clear();
        self.dead = Vec::new();
        self.len = 0;
        self.usable = usable_fraction(self.slots.size()).min(self.slots.limit().saturating_sub(1));
    }

    pub fn reserve(&mut self, additional: usize) {
        self.hashes.reserve(additional);
        if !self.dead.is_empty() {
            self.dead.reserve(additional);
        }
        let bound = self.entries().saturating_add(additional);
        if additional > self.usable || bound >= self.slots.limit() {
            // Presize for the merged size (like `dict_merge`), growing at least
            // geometrically so that repeated small reservations stay amortised O(1).
            let size = size_to_hold(self.len.saturating_add(additional)).max(size_for(self.len.saturating_mul(3)));
            self.rebuild(size);
        }
    }

    /// Live positions in insertion order.
    pub fn iter(&self) -> LivePositions<'_> {
        LivePositions { dead: &self.dead, front: 0, back: self.entries(), remaining: self.len }
    }

    /// First live position `>= from`.
    #[inline]
    pub fn next_live(&self, from: usize) -> Option<usize> {
        if from >= self.entries() {
            return None;
        }
        if self.dead.is_empty() {
            return Some(from);
        }
        self.dead[from..].iter().position(|&d| !d).map(|i| from + i)
    }

    /// Last live position before `before`.
    #[inline]
    pub fn prev_live(&self, before: usize) -> Option<usize> {
        let end = before.min(self.entries());
        if self.dead.is_empty() {
            return end.checked_sub(1);
        }
        self.dead[..end].iter().rposition(|&d| !d)
    }

    /// Last live position.
    #[inline]
    pub fn last_live(&self) -> Option<usize> {
        self.prev_live(self.entries())
    }

    /// Whether tombstones outnumber live entries enough that [`compact`](Self::compact)
    /// pays for itself (amortised O(1) per removal).
    #[inline]
    pub fn wants_compaction(&self) -> bool {
        let dead = self.tombstones();
        dead >= MIN_TOMBSTONES_TO_COMPACT && dead > self.len
    }

    /// Squeeze out tombstones: live entries keep their relative order and move to positions
    /// `0..len()`, and the table is rebuilt for `GROWTH_RATE` (like `dictresize`). Apply the
    /// returned [`Remap`] to every parallel vector.
    pub fn compact(&mut self) -> Remap {
        let dead = std::mem::take(&mut self.dead);
        if dead.is_empty() {
            return Remap { dead, old_len: self.entries() };
        }
        let old_len = dead.len();
        let mut i = 0;
        self.hashes.retain(|_| {
            let keep = !dead[i];
            i += 1;
            keep
        });
        self.hashes.shrink_to(self.len.max(8) * 2);
        self.rebuild(size_for(self.len.saturating_mul(3)));
        Remap { dead, old_len }
    }

    /// Replace the hash of every live entry with `rehash(position)` (positions and
    /// tombstones are preserved). The caller guarantees the keys stay pairwise distinct.
    pub fn rehash(&mut self, mut rehash: impl FnMut(usize) -> u64) {
        for pos in 0..self.entries() {
            if self.is_live(pos) {
                self.hashes[pos] = rehash(pos);
            }
        }
        self.rebuild(self.slots.size().max(size_to_hold(self.len)));
    }
}

/// Live positions with a given hash, in probe order (see [`HashIndex::candidates`]).
pub struct Candidates<'a> {
    index: &'a HashIndex,
    probe: Probe,
    hash: u64,
    done: bool,
    /// Positions already yielded: CPython's probe sequence may revisit a slot.
    seen: Vec<usize>,
}

impl Iterator for Candidates<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        fn step<W: SlotWidth>(s: &[W], i: usize) -> Option<Option<usize>> {
            let slot = s[i];
            if slot == W::EMPTY {
                None
            } else if slot == W::DUMMY {
                Some(None)
            } else {
                Some(Some(slot.pos()))
            }
        }
        while !self.done {
            let at = with_slots!(&self.index.slots, s => step(s, self.probe.i), None => None);
            self.probe.advance();
            match at {
                None => self.done = true,
                Some(Some(pos)) if self.index.hashes[pos] == self.hash && !self.seen.contains(&pos) => {
                    self.seen.push(pos);
                    return Some(pos);
                }
                Some(_) => {}
            }
        }
        None
    }
}

/// Iterator over live positions (see [`HashIndex::iter`]).
#[derive(Clone, Debug)]
pub struct LivePositions<'a> {
    /// Tombstone flags (empty: all live).
    dead: &'a [bool],
    front: usize,
    back: usize,
    remaining: usize,
}

impl LivePositions<'_> {
    #[inline]
    fn is_live(&self, pos: usize) -> bool {
        self.dead.is_empty() || !self.dead[pos]
    }
}

impl Iterator for LivePositions<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        while self.front < self.back {
            let pos = self.front;
            self.front += 1;
            if self.is_live(pos) {
                self.remaining -= 1;
                return Some(pos);
            }
        }
        None
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for LivePositions<'_> {
    fn next_back(&mut self) -> Option<usize> {
        while self.back > self.front {
            self.back -= 1;
            if self.is_live(self.back) {
                self.remaining -= 1;
                return Some(self.back);
            }
        }
        None
    }
}

impl ExactSizeIterator for LivePositions<'_> {}

/// How positions moved during [`HashIndex::compact`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remap {
    /// Tombstone flag of every old position (empty: nothing moved).
    dead: Vec<bool>,
    old_len: usize,
}

impl Remap {
    /// Number of positions before compaction.
    pub fn old_len(&self) -> usize {
        self.old_len
    }

    /// Number of positions after compaction.
    pub fn new_len(&self) -> usize {
        self.old_len - self.dead.iter().filter(|&&d| d).count()
    }

    /// True when nothing moved.
    pub fn is_identity(&self) -> bool {
        !self.dead.contains(&true)
    }

    fn keeps(&self, old: usize) -> bool {
        self.dead.get(old).is_none_or(|&d| !d)
    }

    /// `(old, new)` position pairs of the surviving entries.
    pub fn moves(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.old_len).filter(|&old| self.keeps(old)).enumerate().map(|(new, old)| (old, new))
    }

    /// Compact a parallel vector the same way, dropping the elements at tombstones.
    /// Elements beyond `old_len()` (which a well-behaved caller never has) are kept.
    pub fn apply<T>(&self, items: &mut Vec<T>) {
        debug_assert_eq!(items.len(), self.old_len, "parallel storage out of sync");
        if self.dead.is_empty() {
            return;
        }
        let mut i = 0;
        items.retain(|_| {
            let keep = self.keeps(i);
            i += 1;
            keep
        });
    }

    /// Like [`apply`](Self::apply), but hands the removed elements back to the caller
    /// (e.g. to drop them later, outside a lock).
    pub fn apply_collect<T>(&self, items: &mut Vec<T>) -> Vec<T> {
        debug_assert_eq!(items.len(), self.old_len, "parallel storage out of sync");
        if self.dead.is_empty() {
            return Vec::new();
        }
        let old = std::mem::take(items);
        let kept = self.new_len();
        let mut removed = Vec::with_capacity(old.len().saturating_sub(kept));
        items.reserve(kept);
        for (i, item) in old.into_iter().enumerate() {
            if self.keeps(i) {
                items.push(item);
            } else {
                removed.push(item);
            }
        }
        removed
    }
}


/// The default hasher for native keys (see [`PyHasher`]).
pub type DefaultHasher = PyHashState;

/// `hash(v)` for an `int` `v`, with the common case (`|v| < 2**61 - 1`, where the hash is
/// `v` itself except that `-1` becomes `-2`) kept to a few instructions.
#[inline(always)]
pub fn py_hash_i64(v: i64) -> u64 {
    const P: i64 = crate::pyhash::MODULUS as i64;
    if usize::BITS >= 64 && v > -P && v < P {
        (if v == -1 { -2 } else { v }) as u64
    } else {
        crate::pyhash::hash_i64(v) as u64
    }
}

/// Builds [`PyHasher`]s: Python's own `hash()` for `i64` keys, seeded foldhash otherwise.
#[derive(Clone, Default, Debug)]
pub struct PyHashState(foldhash::fast::RandomState);

impl BuildHasher for PyHashState {
    type Hasher = PyHasher;

    #[inline]
    fn build_hasher(&self) -> PyHasher {
        PyHasher { inner: self.0.build_hasher(), first: None, writes: 0 }
    }
}

/// A hasher whose result for a key that hashes as a single `i64` is CPython's `hash()` of
/// that int (so native int keys share hashes, and probe sequences, with Python's `int`s);
/// any other input is hashed with seeded foldhash.
pub struct PyHasher {
    inner: foldhash::fast::FoldHasher<'static>,
    /// The first write, if it was an `i64` (not yet fed to `inner`).
    first: Option<i64>,
    writes: u32,
}

impl PyHasher {
    #[inline]
    fn flush(&mut self) {
        if let Some(v) = self.first.take() {
            self.inner.write_i64(v);
        }
    }
}

impl Hasher for PyHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.flush();
        self.inner.write(bytes);
        self.writes += 1;
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.flush();
        self.inner.write_u8(i);
        self.writes += 1;
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.flush();
        self.inner.write_u64(i);
        self.writes += 1;
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.flush();
        self.inner.write_usize(i);
        self.writes += 1;
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        if self.writes == 0 {
            self.first = Some(i);
        } else {
            self.flush();
            self.inner.write_i64(i);
        }
        self.writes += 1;
    }

    #[inline]
    fn finish(&self) -> u64 {
        // `first` is only kept while it is the sole write (any later write flushes it).
        match self.first {
            Some(v) => py_hash_i64(v),
            None => self.inner.finish(),
        }
    }
}


/// An insertion-ordered key set: an owned key vector plus its [`HashIndex`].
///
/// Methods ending in `_hashed` take an explicit hash and equality closure, so any key type
/// works (e.g. foreign objects hashed elsewhere). For `K: Hash + Eq` the convenience methods
/// ([`get`](Self::get), [`insert`](Self::insert), [`remove`](Self::remove)) hash with `S`.
#[derive(Clone)]
pub struct KeyIndex<K, S = DefaultHasher> {
    keys: Vec<K>,
    index: HashIndex,
    hasher: S,
}

impl<K: fmt::Debug, S> fmt::Debug for KeyIndex<K, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter().map(|(_, k)| k)).finish()
    }
}

impl<K, S: Default> Default for KeyIndex<K, S> {
    fn default() -> Self {
        Self::with_hasher(S::default())
    }
}

impl<K, S> KeyIndex<K, S> {
    pub fn with_hasher(hasher: S) -> Self {
        KeyIndex { keys: Vec::new(), index: HashIndex::new(), hasher }
    }

    pub fn with_capacity_and_hasher(capacity: usize, hasher: S) -> Self {
        KeyIndex { keys: Vec::with_capacity(capacity), index: HashIndex::with_capacity(capacity), hasher }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Positions in use, including tombstones.
    #[inline]
    pub fn entries(&self) -> usize {
        self.keys.len()
    }

    #[inline]
    pub fn index(&self) -> &HashIndex {
        &self.index
    }

    #[inline]
    pub fn hasher(&self) -> &S {
        &self.hasher
    }

    /// All key slots, including tombstone placeholders (check [`HashIndex::is_live`]).
    #[inline]
    pub fn raw_keys(&self) -> &[K] {
        &self.keys
    }

    /// The live key at `pos`.
    #[inline]
    pub fn get_at(&self, pos: usize) -> Option<&K> {
        if self.index.is_live(pos) { self.keys.get(pos) } else { None }
    }

    /// Live `(position, key)` pairs in insertion order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (usize, &K)> + '_ {
        self.index.iter().map(|pos| (pos, &self.keys[pos]))
    }

    #[inline(always)]
    pub fn find_hashed(&self, hash: u64, mut eq: impl FnMut(&K) -> bool) -> Option<usize> {
        let keys = &self.keys;
        self.index.find(hash, |pos| eq(&keys[pos]))
    }

    /// Append `key` (which the caller guarantees is absent) and return its position.
    #[inline]
    pub fn push_hashed(&mut self, hash: u64, key: K) -> usize {
        let pos = self.index.push(hash);
        self.keys.push(key);
        pos
    }

    /// Swap `key` into the live entry at `pos`, returning the old key (`None`, and `key` is
    /// dropped, when `pos` is not live). The caller guarantees that `key` hashes like the
    /// key it replaces.
    pub fn replace_at(&mut self, pos: usize, key: K) -> Option<K> {
        if self.index.is_live(pos) { Some(std::mem::replace(&mut self.keys[pos], key)) } else { None }
    }

    /// Tombstone the live entry at `pos`, swapping `placeholder` into its slot; returns the
    /// removed key.
    pub fn remove_at(&mut self, pos: usize, placeholder: K) -> Option<K> {
        if self.index.remove_at(pos) { Some(std::mem::replace(&mut self.keys[pos], placeholder)) } else { None }
    }

    /// Drop positions `>= len`, returning the removed key slots (live or placeholder).
    pub fn truncate(&mut self, len: usize) -> Vec<K> {
        self.index.truncate(len);
        if len < self.keys.len() { self.keys.split_off(len) } else { Vec::new() }
    }

    /// Remove everything, returning the key slots.
    pub fn clear(&mut self) -> Vec<K> {
        self.index.clear();
        std::mem::take(&mut self.keys)
    }

    pub fn reserve(&mut self, additional: usize) {
        self.index.reserve(additional);
        self.keys.reserve(additional);
    }

    #[inline]
    pub fn wants_compaction(&self) -> bool {
        self.index.wants_compaction()
    }

    /// Squeeze out tombstones (keys included); apply the returned [`Remap`] to parallel
    /// storage. Placeholder keys are dropped here.
    pub fn compact(&mut self) -> Remap {
        let remap = self.index.compact();
        remap.apply(&mut self.keys);
        remap
    }

    /// Like [`compact`](Self::compact), but hands the dropped placeholders back.
    pub fn compact_collect(&mut self) -> (Remap, Vec<K>) {
        let remap = self.index.compact();
        let removed = remap.apply_collect(&mut self.keys);
        (remap, removed)
    }

    /// Consume into the raw key slots and the index.
    pub fn into_parts(self) -> (Vec<K>, HashIndex, S) {
        (self.keys, self.index, self.hasher)
    }

    /// Reassemble from parts. The caller guarantees `keys.len() == index.entries()` and that
    /// the index hashes were produced consistently with how lookups will hash.
    pub fn from_parts(keys: Vec<K>, index: HashIndex, hasher: S) -> Self {
        debug_assert_eq!(keys.len(), index.entries());
        KeyIndex { keys, index, hasher }
    }
}

impl<K: Hash + Eq, S: BuildHasher> KeyIndex<K, S> {
    #[inline(always)]
    pub fn hash_key<Q: Hash + ?Sized>(&self, key: &Q) -> u64 {
        self.hasher.hash_one(key)
    }

    /// Position of `key`.
    #[inline(always)]
    pub fn get<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.find_hashed(self.hash_key(key), |k| k.borrow() == key)
    }

    #[inline]
    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Insert `key` if absent. Returns its position and whether it was newly inserted.
    #[inline]
    pub fn insert(&mut self, key: K) -> (usize, bool) {
        let hash = self.hash_key(&key);
        let keys = &self.keys;
        let (pos, new) = self.index.find_or_push(hash, |pos| keys[pos] == key);
        if new {
            self.keys.push(key);
        }
        (pos, new)
    }

    /// Like [`insert`](Self::insert) for a borrowed form of the key; `make` builds the owned
    /// key only when it is inserted.
    #[inline]
    pub fn insert_with<Q>(&mut self, key: &Q, make: impl FnOnce(&Q) -> K) -> (usize, bool)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash_key(key);
        let keys = &self.keys;
        let (pos, new) = self.index.find_or_push(hash, |pos| keys[pos].borrow() == key);
        if new {
            self.keys.push(make(key));
        }
        (pos, new)
    }

    /// Build from keys that are known to be pairwise distinct.
    pub fn from_distinct(keys: Vec<K>, hasher: S) -> Self {
        let index = HashIndex::from_hashes(keys.iter().map(|k| hasher.hash_one(k)));
        KeyIndex { keys, index, hasher }
    }

    /// Recompute every hash with a new hasher (positions and tombstones are preserved).
    pub fn rehash_with<T: BuildHasher>(self, hasher: T) -> KeyIndex<K, T> {
        let KeyIndex { keys, mut index, .. } = self;
        index.rehash(|pos| hasher.hash_one(&keys[pos]));
        KeyIndex { keys, index, hasher }
    }
}

impl<K: Hash + Eq + Default, S: BuildHasher> KeyIndex<K, S> {
    /// Remove `key`, leaving a tombstone. Returns its old position and the key.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<(usize, K)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash_key(key);
        let keys = &self.keys;
        let pos = self.index.remove(hash, |pos| keys[pos].borrow() == key)?;
        Some((pos, std::mem::take(&mut self.keys[pos])))
    }
}

/// An insertion-ordered map with Python `dict` semantics: overwriting keeps a key's position,
/// removing preserves the order of the rest, re-inserting appends at the end.
#[derive(Clone)]
pub struct OrderedMap<K, V, S = DefaultHasher> {
    keys: KeyIndex<K, S>,
    values: Vec<Option<V>>,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for OrderedMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V, S: Default> Default for OrderedMap<K, V, S> {
    fn default() -> Self {
        OrderedMap { keys: KeyIndex::default(), values: Vec::new() }
    }
}

impl<K, V, S> OrderedMap<K, V, S> {
    #[inline]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn keys(&self) -> &KeyIndex<K, S> {
        &self.keys
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        self.keys.iter().filter_map(|(pos, k)| self.values[pos].as_ref().map(|v| (k, v)))
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.values.clear();
    }

    fn maybe_compact(&mut self) {
        if self.keys.wants_compaction() {
            let remap = self.keys.compact();
            remap.apply(&mut self.values);
        }
    }
}

impl<K: Hash + Eq + Default, V, S: BuildHasher> OrderedMap<K, V, S> {
    pub fn with_hasher(hasher: S) -> Self {
        OrderedMap { keys: KeyIndex::with_hasher(hasher), values: Vec::new() }
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.keys.get(key).and_then(|pos| self.values[pos].as_ref())
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.keys.get(key).and_then(|pos| self.values[pos].as_mut())
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.keys.contains(key)
    }

    /// Insert or overwrite; returns the previous value.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let (pos, new) = self.keys.insert(key);
        if new {
            self.values.push(Some(value));
            None
        } else {
            self.values[pos].replace(value)
        }
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let (pos, _) = self.keys.remove(key)?;
        let value = self.values[pos].take();
        self.maybe_compact();
        value
    }

    /// Remove and return the most recently inserted entry.
    pub fn pop(&mut self) -> Option<(K, V)> {
        let pos = self.keys.index().last_live()?;
        let mut keys = self.keys.truncate(pos);
        let mut values = self.values.split_off(pos);
        let key = keys.swap_remove(0);
        let value = values.swap_remove(0)?;
        Some((key, value))
    }
}

impl<K: Hash + Eq + Default, V, S: BuildHasher + Default> FromIterator<(K, V)> for OrderedMap<K, V, S> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = OrderedMap::with_hasher(S::default());
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compact_str::CompactString;
    use proptest::prelude::*;

    /// A model with Python dict semantics.
    #[derive(Default, Debug)]
    struct Model<K, V>(Vec<(K, V)>);

    impl<K: PartialEq + Clone, V: Clone> Model<K, V> {
        fn insert(&mut self, k: K, v: V) -> Option<V> {
            match self.0.iter_mut().find(|(key, _)| *key == k) {
                Some((_, old)) => Some(std::mem::replace(old, v)),
                None => {
                    self.0.push((k, v));
                    None
                }
            }
        }
        fn remove(&mut self, k: &K) -> Option<V> {
            let i = self.0.iter().position(|(key, _)| key == k)?;
            Some(self.0.remove(i).1)
        }
        fn get(&self, k: &K) -> Option<&V> {
            self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
        }
    }

    #[test]
    fn replace_at_swaps_live_keys_only() {
        let mut ki: KeyIndex<i64> = KeyIndex::default();
        ki.insert(1);
        ki.insert(2);
        assert_eq!(ki.replace_at(1, 2), Some(2));
        assert_eq!(ki.remove_at(0, 0), Some(1));
        assert_eq!(ki.replace_at(0, 1), None);
        assert_eq!(ki.replace_at(5, 1), None);
        assert_eq!(ki.iter().map(|(p, k)| (p, *k)).collect::<Vec<_>>(), vec![(1, 2)]);
    }

    #[test]
    fn table_sizes_follow_cpython() {
        // dk_size of a dict after n insertions without deletions (CPython 3.15).
        let mut idx = HashIndex::new();
        let mut sizes = Vec::new();
        for n in 0..100u64 {
            sizes.push(idx.slots());
            idx.push(n);
        }
        assert_eq!(sizes[0], 0);
        assert!(sizes[1..=5].iter().all(|&s| s == 8));
        assert!(sizes[6..=10].iter().all(|&s| s == 16));
        assert!(sizes[11..=21].iter().all(|&s| s == 32));
        assert!(sizes[22..=42].iter().all(|&s| s == 64));
        assert!(sizes[43..=85].iter().all(|&s| s == 128));
        assert_eq!(sizes[86], 256);
        assert_eq!(HashIndex::with_capacity(5).slots(), 8);
        assert_eq!(HashIndex::with_capacity(6).slots(), 16);
        assert_eq!(HashIndex::from_hashes(0..1000).slots(), 2048);
    }

    #[test]
    fn widths_grow_with_positions() {
        let n = 70_000u64;
        let mut idx = HashIndex::new();
        for k in 0..n {
            assert_eq!(idx.push(k.wrapping_mul(0x9E37_79B9_7F4A_7C15)), k as usize);
            if k % 997 == 0 {
                for probe in [0, k / 2, k] {
                    let h = probe.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    assert_eq!(idx.find(h, |p| p as u64 == probe), Some(probe as usize));
                }
            }
        }
        for k in (0..n).step_by(3) {
            assert!(idx.remove_at(k as usize));
        }
        for k in 0..n {
            let h = k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            assert_eq!(idx.find(h, |p| p as u64 == k).is_some(), k % 3 != 0);
        }
        let remap = idx.compact();
        assert_eq!(remap.new_len(), idx.len());
        assert_eq!(idx.slots(), size_for(idx.len() * 3));
    }

    #[test]
    fn identity_hashes_probe_like_cpython() {
        // Small ints hash to themselves: a dense run fills consecutive slots, and multiples
        // of the table size still resolve through `perturb`.
        let keys: Vec<i64> = (0..40).map(|k| k * 64).collect();
        let mut ki: KeyIndex<i64> = KeyIndex::default();
        for &k in &keys {
            assert!(ki.insert(k).1);
        }
        for (pos, &k) in keys.iter().enumerate() {
            assert_eq!(ki.get(&k), Some(pos));
            assert_eq!(ki.index().hash_at(pos), Some(crate::pyhash::hash_i64(k) as u64));
        }
        assert_eq!(ki.get(&1), None);
        for v in [0i64, 1, -1, -2, (1 << 61) - 2, (1 << 61) - 1, 1 << 61, -(1 << 61) + 2, -(1 << 61) + 1, -(1 << 61), i64::MAX, i64::MIN] {
            assert_eq!(py_hash_i64(v), crate::pyhash::hash_i64(v) as u64, "{v}");
        }
        assert_eq!(ki.hash_key(&-1i64), crate::pyhash::hash_i64(-1) as u64);
        assert_eq!(ki.hash_key(&i64::MIN), crate::pyhash::hash_i64(i64::MIN) as u64);
        // Other keys still hash with the seeded hasher (and differ from the int's hash).
        let s: KeyIndex<CompactString> = KeyIndex::default();
        assert_ne!(s.hash_key("1"), 1);
    }

    #[test]
    fn repeated_small_reservations_stay_cheap() {
        let mut idx = HashIndex::new();
        let mut rebuilds = 0;
        let mut last = 0;
        for k in 0..10_000u64 {
            idx.reserve(2);
            if idx.slots() != last {
                rebuilds += 1;
                last = idx.slots();
            }
            idx.push(k);
        }
        assert!(rebuilds < 20, "{rebuilds} rebuilds");
    }

    #[test]
    fn dummy_slots_are_reused_and_skipped() {
        let mut keys: Vec<i64> = Vec::new();
        let mut idx = HashIndex::new();
        for round in 0..1000i64 {
            let (pos, new) = idx.find_or_push(7, |p| keys[p] == round);
            assert!(new);
            keys.push(round);
            assert!(idx.remove_at(pos));
            if idx.wants_compaction() {
                idx.compact().apply(&mut keys);
            }
        }
        assert!(idx.slots() <= 16);
        assert_eq!(idx.find(7, |_| true), None);
    }

    #[test]
    fn empty_index() {
        let idx = HashIndex::new();
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
        assert_eq!(idx.find(42, |_| true), None);
        assert_eq!(idx.next_live(0), None);
        assert_eq!(idx.last_live(), None);
        assert_eq!(idx.iter().count(), 0);
    }

    #[test]
    fn push_find_remove_tombstones() {
        let keys = [10i64, 20, 30, 40];
        let mut idx = HashIndex::new();
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(idx.push(*k as u64 % 3), i);
        }
        let find = |idx: &HashIndex, k: i64| idx.find(k as u64 % 3, |p| keys[p] == k);
        assert_eq!(find(&idx, 30), Some(2));
        assert_eq!(idx.remove(30 % 3, |p| keys[p] == 30), Some(2));
        assert_eq!(find(&idx, 30), None);
        assert_eq!(idx.len(), 3);
        assert_eq!(idx.entries(), 4);
        assert_eq!(idx.tombstones(), 1);
        assert!(!idx.is_live(2));
        assert_eq!(idx.iter().collect::<Vec<_>>(), vec![0, 1, 3]);
        assert_eq!(idx.iter().rev().collect::<Vec<_>>(), vec![3, 1, 0]);
        assert_eq!(idx.next_live(2), Some(3));
        assert_eq!(idx.last_live(), Some(3));
        assert!(!idx.remove_at(2));
        assert!(idx.remove_at(3));
        assert_eq!(idx.last_live(), Some(1));
        assert_eq!(find(&idx, 40), None);
        assert_eq!(find(&idx, 10), Some(0));
    }

    #[test]
    fn find_or_push_and_lazy_tombstones() {
        let mut keys: Vec<i64> = Vec::new();
        let mut idx = HashIndex::new();
        for k in [5i64, 7, 5, 9, 7] {
            let (pos, new) = idx.find_or_push(k as u64 % 2, |p| keys[p] == k);
            if new {
                assert_eq!(pos, keys.len());
                keys.push(k);
            } else {
                assert_eq!(keys[pos], k);
            }
        }
        assert_eq!(keys, [5, 7, 9]);
        assert_eq!(idx.tombstones(), 0);
        assert_eq!(idx.prev_live(3), Some(2));
        assert_eq!(idx.prev_live(0), None);
        assert!(idx.remove_at(2));
        assert_eq!(idx.prev_live(3), Some(1));
        assert_eq!(idx.last_live(), Some(1));
        let (pos, new) = idx.find_or_push(11 % 2, |p| keys.get(p) == Some(&11));
        assert!(new && pos == 3);
        keys.push(11);
        assert!(idx.is_live(3) && !idx.is_live(2));
        assert_eq!(idx.iter().collect::<Vec<_>>(), [0, 1, 3]);
        idx.truncate(2);
        assert_eq!(idx.tombstones(), 0);
        assert_eq!(idx.iter().collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn key_index_insert_with_builds_lazily() {
        let mut ki: KeyIndex<CompactString> = KeyIndex::default();
        let mut built = 0;
        for s in ["x", "y", "x"] {
            ki.insert_with(s, |s| {
                built += 1;
                CompactString::from(s)
            });
        }
        assert_eq!(built, 2);
        assert_eq!(ki.get("y"), Some(1));
    }

    #[test]
    fn candidates_are_distinct_even_when_the_probe_sequence_revisits_a_slot() {
        for hash in 0..4096u64 {
            let mut idx = HashIndex::new();
            idx.push(hash.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let h = idx.hash_at(0).unwrap();
            assert_eq!(idx.candidates(h).collect::<Vec<_>>(), [0]);
        }
    }

    #[test]
    fn candidates_lists_all_equal_hashes() {
        let mut idx = HashIndex::new();
        for _ in 0..5 {
            idx.push(7);
        }
        idx.push(8);
        idx.remove_at(1);
        let mut c: Vec<_> = idx.candidates(7).collect();
        c.sort_unstable();
        assert_eq!(c, vec![0, 2, 3, 4]);
        assert_eq!(idx.candidates(9).count(), 0);
    }

    #[test]
    fn compaction_remaps_positions() {
        let mut keys: Vec<i64> = (0..100).collect();
        let mut idx = HashIndex::from_hashes(keys.iter().map(|&k| k as u64));
        for k in (0..100).filter(|k| k % 3 != 0) {
            assert!(idx.remove(k as u64, |p| keys[p] == k).is_some());
        }
        assert!(idx.wants_compaction());
        let remap = idx.compact();
        assert_eq!(remap.old_len(), 100);
        assert_eq!(remap.new_len(), 34);
        let moves: Vec<_> = remap.moves().collect();
        assert_eq!(moves[1], (3, 1));
        remap.apply(&mut keys);
        assert_eq!(keys, (0..100).filter(|k| k % 3 == 0).collect::<Vec<_>>());
        assert_eq!(idx.entries(), 34);
        assert_eq!(idx.tombstones(), 0);
        for (pos, &k) in keys.iter().enumerate() {
            assert_eq!(idx.find(k as u64, |p| keys[p] == k), Some(pos));
            assert_eq!(idx.hash_at(pos), Some(k as u64));
        }
        assert_eq!(idx.find(1, |p| keys[p] == 1), None);
    }

    #[test]
    fn remap_collect_returns_removed() {
        let mut idx = HashIndex::from_hashes([1, 2, 3, 4]);
        idx.remove_at(0);
        idx.remove_at(2);
        let remap = idx.compact();
        let mut v = vec!["a", "b", "c", "d"];
        let removed = remap.apply_collect(&mut v);
        assert_eq!(v, ["b", "d"]);
        assert_eq!(removed, ["a", "c"]);
        assert!(!remap.is_identity());
        let remap = idx.compact();
        assert!(remap.is_identity());
    }

    #[test]
    fn truncate_and_rehash() {
        let keys: Vec<i64> = (0..10).collect();
        let mut idx = HashIndex::from_hashes(keys.iter().map(|&k| k as u64));
        idx.remove_at(8);
        idx.truncate(7);
        assert_eq!(idx.len(), 7);
        assert_eq!(idx.entries(), 7);
        assert_eq!(idx.find(9, |p| keys[p] == 9), None);
        assert_eq!(idx.find(6, |p| keys[p] == 6), Some(6));
        idx.remove_at(3);
        idx.rehash(|p| (keys[p] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        assert_eq!(idx.find(5u64.wrapping_mul(0x9E37_79B9_7F4A_7C15), |p| keys[p] == 5), Some(5));
        assert_eq!(idx.find(5, |p| keys[p] == 5), None);
        assert!(!idx.is_live(3));
        assert_eq!(idx.push(99), 7);
        idx.truncate(0);
        assert!(idx.is_empty());
        assert_eq!(idx.entries(), 0);
    }

    #[test]
    fn key_index_str_borrow_lookup() {
        let mut ki: KeyIndex<CompactString> = KeyIndex::default();
        assert_eq!(ki.insert("a".into()), (0, true));
        assert_eq!(ki.insert("b".into()), (1, true));
        assert_eq!(ki.insert("a".into()), (0, false));
        assert_eq!(ki.get("b"), Some(1));
        assert!(!ki.contains("zz"));
        assert_eq!(ki.remove("a"), Some((0, CompactString::from("a"))));
        assert_eq!(ki.get("a"), None);
        assert_eq!(ki.insert("a".into()), (2, true));
        assert_eq!(ki.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        let ki = ki.rehash_with(foldhash::fast::FixedState::with_seed(7));
        assert_eq!(ki.get("a"), Some(2));
        assert_eq!(ki.get_at(0), None);
    }

    #[test]
    fn ordered_map_basics() {
        let mut m: OrderedMap<i64, &str> = OrderedMap::default();
        assert_eq!(m.insert(1, "a"), None);
        assert_eq!(m.insert(2, "b"), None);
        assert_eq!(m.insert(1, "c"), Some("a"));
        assert_eq!(m.iter().collect::<Vec<_>>(), [(&1, &"c"), (&2, &"b")]);
        assert_eq!(m.remove(&1), Some("c"));
        assert_eq!(m.insert(1, "d"), None);
        assert_eq!(m.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(m.pop(), Some((1, "d")));
        assert_eq!(m.pop(), Some((2, "b")));
        assert_eq!(m.pop(), None);
        let m: OrderedMap<i64, i64> = (0..5).map(|i| (i % 3, i)).collect();
        assert_eq!(m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(), [(0, 3), (1, 4), (2, 2)]);
    }

    #[test]
    fn heavy_churn_keeps_order_and_memory_bounded() {
        let mut m: OrderedMap<i64, i64> = OrderedMap::default();
        let mut model: Model<i64, i64> = Model::default();
        for round in 0..20_000i64 {
            let k = (round * 7919) % 257;
            if round % 3 == 0 {
                assert_eq!(m.remove(&k), model.remove(&k));
            } else {
                assert_eq!(m.insert(k, round), model.insert(k, round));
            }
        }
        let got: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(got, model.0);
        assert!(m.keys().entries() <= 2 * m.len() + MIN_TOMBSTONES_TO_COMPACT + 1);
    }

    #[test]
    fn colliding_hashes_with_closures() {
        // Every key hashes to one of two values; equality must still resolve.
        let mut keys: Vec<u32> = Vec::new();
        let mut idx = HashIndex::new();
        let h = |k: u32| u64::from(k % 2);
        for k in 0..200u32 {
            if idx.find(h(k), |p| keys[p] == k).is_none() {
                idx.push(h(k));
                keys.push(k);
            }
        }
        for k in (0..200u32).step_by(5) {
            let pos = idx.remove(h(k), |p| keys[p] == k).unwrap();
            assert_eq!(keys[pos], k);
        }
        for k in 0..200u32 {
            assert_eq!(idx.find(h(k), |p| keys[p] == k).is_some(), k % 5 != 0);
        }
    }

    #[derive(Clone, Debug)]
    enum Op {
        Insert(i16, u32),
        Remove(i16),
        Pop,
        Get(i16),
        Compact,
        Clear,
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            6 => (any::<i16>().prop_map(|k| k % 64), any::<u32>()).prop_map(|(k, v)| Op::Insert(k, v)),
            4 => any::<i16>().prop_map(|k| Op::Remove(k % 64)),
            1 => Just(Op::Pop),
            3 => any::<i16>().prop_map(|k| Op::Get(k % 64)),
            1 => Just(Op::Compact),
            1 => Just(Op::Clear),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn ordered_map_matches_model(ops in proptest::collection::vec(op_strategy(), 0..400)) {
            let mut m: OrderedMap<i64, u32> = OrderedMap::default();
            let mut model: Model<i64, u32> = Model::default();
            let mut std_map = std::collections::HashMap::new();
            for op in ops {
                match op {
                    Op::Insert(k, v) => {
                        let k = i64::from(k);
                        prop_assert_eq!(m.insert(k, v), model.insert(k, v));
                        std_map.insert(k, v);
                    }
                    Op::Remove(k) => {
                        let k = i64::from(k);
                        prop_assert_eq!(m.remove(&k), model.remove(&k));
                        std_map.remove(&k);
                    }
                    Op::Pop => {
                        let got = m.pop();
                        let want = model.0.pop();
                        if let Some((k, _)) = &want {
                            std_map.remove(k);
                        }
                        prop_assert_eq!(got, want);
                    }
                    Op::Get(k) => {
                        let k = i64::from(k);
                        prop_assert_eq!(m.get(&k), model.get(&k));
                        prop_assert_eq!(m.get(&k), std_map.get(&k));
                    }
                    Op::Compact => m.maybe_compact(),
                    Op::Clear => {
                        m.clear();
                        model.0.clear();
                        std_map.clear();
                    }
                }
                prop_assert_eq!(m.len(), model.0.len());
                prop_assert_eq!(m.len(), std_map.len());
            }
            let got: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect();
            prop_assert_eq!(got, model.0);
        }

        #[test]
        fn raw_index_with_forced_collisions(ops in proptest::collection::vec(op_strategy(), 0..300)) {
            // Parallel storage owned by the "caller"; hashes collide heavily (k mod 4).
            let mut keys: Vec<Option<i64>> = Vec::new();
            let mut idx = HashIndex::new();
            let mut model: Model<i64, ()> = Model::default();
            let h = |k: i64| k.rem_euclid(4) as u64;
            for op in ops {
                match op {
                    Op::Insert(k, v) => {
                        let k = i64::from(k);
                        if v % 2 == 0 {
                            if idx.find(h(k), |p| keys[p] == Some(k)).is_none() {
                                prop_assert_eq!(idx.push(h(k)), keys.len());
                                keys.push(Some(k));
                            }
                        } else {
                            let (pos, new) = idx.find_or_push(h(k), |p| keys[p] == Some(k));
                            if new {
                                prop_assert_eq!(pos, keys.len());
                                keys.push(Some(k));
                            } else {
                                prop_assert_eq!(keys[pos], Some(k));
                            }
                        }
                        model.insert(k, ());
                    }
                    Op::Remove(k) => {
                        let k = i64::from(k);
                        let removed = idx.remove(h(k), |p| keys[p] == Some(k));
                        if let Some(p) = removed {
                            keys[p] = None;
                        }
                        prop_assert_eq!(removed.is_some(), model.remove(&k).is_some());
                    }
                    Op::Pop => {
                        if let Some(p) = idx.last_live() {
                            idx.truncate(p);
                            let k = keys[p].unwrap();
                            keys.truncate(p);
                            prop_assert_eq!(model.0.pop().map(|(k, _)| k), Some(k));
                        } else {
                            prop_assert!(model.0.is_empty());
                        }
                    }
                    Op::Get(k) => {
                        let k = i64::from(k);
                        let found = idx.find(h(k), |p| keys[p] == Some(k));
                        prop_assert_eq!(found.is_some(), model.get(&k).is_some());
                        if let Some(p) = found {
                            prop_assert_eq!(idx.hash_at(p), Some(h(k)));
                        }
                    }
                    Op::Compact => {
                        if idx.wants_compaction() || idx.tombstones() > 0 {
                            let remap = idx.compact();
                            remap.apply(&mut keys);
                            prop_assert!(keys.iter().all(Option::is_some));
                        }
                    }
                    Op::Clear => {
                        idx.clear();
                        keys.clear();
                        model.0.clear();
                    }
                }
                prop_assert_eq!(idx.entries(), keys.len());
                prop_assert_eq!(idx.len(), model.0.len());
            }
            let order: Vec<i64> = idx.iter().map(|p| keys[p].unwrap()).collect();
            let want: Vec<i64> = model.0.iter().map(|(k, _)| *k).collect();
            prop_assert_eq!(&order, &want);
            let back: Vec<i64> = idx.iter().rev().map(|p| keys[p].unwrap()).collect();
            prop_assert_eq!(back, want.into_iter().rev().collect::<Vec<_>>());
        }

        #[test]
        fn str_keys_match_model(ops in proptest::collection::vec((0u8..3, "[a-e]{0,3}"), 0..300)) {
            let mut ki: KeyIndex<CompactString> = KeyIndex::default();
            let mut model: Vec<String> = Vec::new();
            for (kind, s) in ops {
                match kind {
                    0 | 1 => {
                        let (pos, new) = ki.insert(CompactString::from(s.as_str()));
                        prop_assert_eq!(new, !model.contains(&s));
                        if new { model.push(s.clone()); }
                        prop_assert_eq!(ki.get_at(pos).map(|k| k.as_str()), Some(s.as_str()));
                    }
                    _ => {
                        let removed = ki.remove(s.as_str());
                        let want = model.iter().position(|m| *m == s).map(|i| model.remove(i));
                        prop_assert_eq!(removed.map(|(_, k)| k.to_string()), want);
                        if ki.wants_compaction() {
                            ki.compact();
                        }
                    }
                }
            }
            let got: Vec<String> = ki.iter().map(|(_, k)| k.to_string()).collect();
            prop_assert_eq!(got, model);
        }
    }
}
