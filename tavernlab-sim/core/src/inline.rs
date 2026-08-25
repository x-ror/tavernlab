//! A fixed-capacity vector that lives inside its owner.
//!
//! Every collection in a game state has a hard cap the rules impose: seven
//! board slots, ten cards in hand, five secrets. Storing those in `Vec` would
//! put three pointers and a heap allocation behind each one, which is what
//! makes cloning a position expensive — and cheap cloning is the reason this
//! engine exists.
//!
//! [`Inline`] stores its elements in place, so a whole game is one flat value
//! that copies with a `memcpy` and fits in cache. There is no `unsafe` here:
//! unused slots hold `T::default()` rather than uninitialised memory, and
//! `new()` is deliberately not `const` so that stays true. An earlier version
//! zeroed the buffer instead, which quietly required every stored type's
//! `Default` to be the all-zero bit pattern — `Target` does not satisfy that,
//! because Rust chooses enum layouts freely.

use core::ops::{Index, IndexMut};

/// A vector of at most `N` elements, stored inline.
///
/// The length is a `u16` rather than a `u8`: board and hand caps fit in a byte,
/// but the legal-action buffer does not, and alignment means the wider counter
/// costs nothing for every case that would have fitted.
#[derive(Clone, Copy)]
pub struct Inline<T: Copy + Default, const N: usize> {
    len: u16,
    items: [T; N],
}

impl<T: Copy + Default, const N: usize> Default for Inline<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default, const N: usize> Inline<T, N> {
    const CAPACITY_FITS: () = assert!(N <= u16::MAX as usize, "Inline capacity must fit in a u16");

    /// Deliberately not `const`: `Default::default` cannot be called in a
    /// `const fn`, and the alternative — zeroing the buffer — would silently
    /// require every stored type's `Default` to be the all-zero bit pattern.
    /// `Target` does not satisfy that, because Rust chooses enum layouts
    /// freely. Nothing needs an `Inline` in const context.
    pub fn new() -> Self {
        #[allow(clippy::let_unit_value)]
        let () = Self::CAPACITY_FITS;
        Self {
            len: 0,
            items: [T::default(); N],
        }
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len as usize >= N
    }

    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.items[..self.len as usize]
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.len as usize]
    }

    /// Append, returning false when full.
    ///
    /// Returning a bool rather than panicking is deliberate: "the board is
    /// full" and "the hand is full" are ordinary game states with defined
    /// rules, not programming errors.
    #[inline]
    pub fn push(&mut self, v: T) -> bool {
        if self.is_full() {
            return false;
        }
        self.items[self.len as usize] = v;
        self.len += 1;
        true
    }

    /// Insert at `at`, shifting the tail right. Returns false when full.
    /// `at` beyond the current length appends.
    pub fn insert(&mut self, at: usize, v: T) -> bool {
        if self.is_full() {
            return false;
        }
        let at = at.min(self.len as usize);
        let mut i = self.len as usize;
        while i > at {
            self.items[i] = self.items[i - 1];
            i -= 1;
        }
        self.items[at] = v;
        self.len += 1;
        true
    }

    /// Remove and return the element at `at`, shifting the tail left.
    ///
    /// Order matters on a Hearthstone board — adjacency effects and attack
    /// ordering both read it — so this is not `swap_remove`.
    pub fn remove(&mut self, at: usize) -> Option<T> {
        if at >= self.len as usize {
            return None;
        }
        let out = self.items[at];
        for i in at..self.len as usize - 1 {
            self.items[i] = self.items[i + 1];
        }
        self.len -= 1;
        self.items[self.len as usize] = T::default();
        Some(out)
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        let out = self.items[self.len as usize];
        self.items[self.len as usize] = T::default();
        Some(out)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.items = [T::default(); N];
        self.len = 0;
    }

    #[inline]
    pub fn get(&self, i: usize) -> Option<&T> {
        self.as_slice().get(i)
    }

    #[inline]
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.as_mut_slice().get_mut(i)
    }

    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.as_slice().first()
    }

    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.as_slice().last()
    }

    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.as_mut_slice().last_mut()
    }

    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    /// Keep only the elements matching `pred`, preserving order.
    pub fn retain(&mut self, mut pred: impl FnMut(&T) -> bool) {
        let mut w = 0;
        for r in 0..self.len as usize {
            if pred(&self.items[r]) {
                self.items[w] = self.items[r];
                w += 1;
            }
        }
        for slot in self.items.iter_mut().take(self.len as usize).skip(w) {
            *slot = T::default();
        }
        self.len = w as u16;
    }

    #[inline]
    pub fn swap(&mut self, a: usize, b: usize) {
        self.as_mut_slice().swap(a, b);
    }

    /// Fill from an iterator, stopping at capacity. Returns how many were taken.
    pub fn extend_from(&mut self, it: impl IntoIterator<Item = T>) -> usize {
        let mut n = 0;
        for v in it {
            if !self.push(v) {
                break;
            }
            n += 1;
        }
        n
    }
}

impl<T: Copy + Default + PartialEq, const N: usize> Inline<T, N> {
    #[inline]
    pub fn contains(&self, v: &T) -> bool {
        self.as_slice().contains(v)
    }

    #[inline]
    pub fn position(&self, v: &T) -> Option<usize> {
        self.as_slice().iter().position(|x| x == v)
    }

    /// Remove the first element equal to `v`, preserving order.
    pub fn remove_value(&mut self, v: &T) -> bool {
        match self.position(v) {
            Some(i) => {
                self.remove(i);
                true
            }
            None => false,
        }
    }
}

impl<T: Copy + Default + core::fmt::Debug, const N: usize> core::fmt::Debug for Inline<T, N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl<T: Copy + Default + PartialEq, const N: usize> PartialEq for Inline<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Copy + Default, const N: usize> Index<usize> for Inline<T, N> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        &self.as_slice()[i]
    }
}

impl<T: Copy + Default, const N: usize> IndexMut<usize> for Inline<T, N> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.as_mut_slice()[i]
    }
}

impl<'a, T: Copy + Default, const N: usize> IntoIterator for &'a Inline<T, N> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Copy + Default, const N: usize> FromIterator<T> for Inline<T, N> {
    /// Elements past capacity are dropped; callers that care check `len`.
    fn from_iter<I: IntoIterator<Item = T>>(it: I) -> Self {
        let mut out = Self::new();
        out.extend_from(it);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type V = Inline<u16, 7>;

    #[test]
    fn vacated_slots_hold_default_not_stale_data() {
        // The property that replaced the old zeroing invariant: whatever a
        // slot held, removing the element must leave `T::default()` there, so
        // no removed value can be read back through a later `push`.
        let mut v: V = [1u16, 2, 3].into_iter().collect();
        v.pop();
        v.push(9);
        assert_eq!(v.as_slice(), &[1, 2, 9]);
    }

    #[test]
    fn push_until_full_then_refuse() {
        let mut v = V::new();
        for i in 0..7 {
            assert!(v.push(i), "push {i} should fit");
        }
        assert!(v.is_full());
        assert!(!v.push(99), "the eighth board slot does not exist");
        assert_eq!(v.len(), 7);
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn remove_preserves_order() {
        // Board order drives adjacency effects, so this must not swap-remove.
        let mut v: V = [10u16, 20, 30, 40].into_iter().collect();
        assert_eq!(v.remove(1), Some(20));
        assert_eq!(v.as_slice(), &[10, 30, 40]);
        assert_eq!(v.remove(9), None);
    }

    #[test]
    fn insert_shifts_right() {
        let mut v: V = [1u16, 2, 3].into_iter().collect();
        assert!(v.insert(1, 99));
        assert_eq!(v.as_slice(), &[1, 99, 2, 3]);
        // Past the end appends rather than leaving a hole.
        assert!(v.insert(50, 7));
        assert_eq!(v.as_slice(), &[1, 99, 2, 3, 7]);
    }

    #[test]
    fn insert_into_a_full_vector_fails_without_corrupting() {
        let mut v: V = (0u16..7).collect();
        assert!(!v.insert(0, 100));
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn pop_and_clear_reset_vacated_slots() {
        let mut v: V = [5u16, 6].into_iter().collect();
        assert_eq!(v.pop(), Some(6));
        assert_eq!(v.len(), 1);
        v.clear();
        assert!(v.is_empty());
        assert_eq!(v.pop(), None);
    }

    #[test]
    fn retain_keeps_order() {
        let mut v: V = (0u16..7).collect();
        v.retain(|x| x % 2 == 0);
        assert_eq!(v.as_slice(), &[0, 2, 4, 6]);
    }

    #[test]
    fn retain_everything_and_nothing() {
        let mut v: V = (0u16..5).collect();
        v.retain(|_| true);
        assert_eq!(v.len(), 5);
        v.retain(|_| false);
        assert!(v.is_empty());
    }

    #[test]
    fn remove_value_takes_only_the_first() {
        let mut v: V = [1u16, 2, 1, 3].into_iter().collect();
        assert!(v.remove_value(&1));
        assert_eq!(v.as_slice(), &[2, 1, 3]);
        assert!(!v.remove_value(&9));
    }

    #[test]
    fn from_iter_stops_at_capacity() {
        // A deck that tries to shuffle in more than fits should truncate, not
        // panic — the rules already cap these zones.
        let v: V = (0u16..100).collect();
        assert_eq!(v.len(), 7);
        assert_eq!(v[6], 6);
    }

    #[test]
    fn copying_is_a_value_copy_not_a_share() {
        let a: V = [1u16, 2, 3].into_iter().collect();
        let mut b = a;
        b.push(4);
        assert_eq!(a.len(), 3, "the original moved when the copy was mutated");
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn equality_ignores_vacated_slots() {
        let mut a: V = [1u16, 2, 3].into_iter().collect();
        a.pop();
        let b: V = [1u16, 2].into_iter().collect();
        assert_eq!(a, b);
    }

    #[test]
    fn size_is_the_payload_plus_the_counter() {
        // The whole point: no pointer, no capacity field, no allocation.
        assert_eq!(size_of::<Inline<u16, 7>>(), 16); // 14 bytes + len
        assert_eq!(size_of::<Inline<u8, 10>>(), 12); // 10 bytes + len, aligned
    }
}
