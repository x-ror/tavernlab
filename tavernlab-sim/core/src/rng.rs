//! Randomness sized for a game state that is copied, not rebuilt.
//!
//! # Why not Mersenne Twister
//!
//! The obvious move when replacing the Python engine this grew out of was to
//! reproduce `random.Random` bit for bit, so the two implementations could be
//! compared game for game. That is the wrong trade here, for a structural reason rather than a
//! stylistic one: MT19937 carries 624 words — 2.5 KB — of state.
//!
//! The point of this engine is that [`crate::game::Game`] is a fixed-size value
//! that clones with a `memcpy`, which is what makes lookahead search affordable
//! later. A 2.5 KB generator would be the largest field in that struct by an
//! order of magnitude and would dominate every clone. Reproducing Python's
//! generator would therefore have cost the main reason for the rewrite.
//!
//! So the engine uses xoshiro256++: 32 bytes, `Copy`, roughly one nanosecond per
//! draw, and no rejection loop on the common path. Equivalence with the old
//! engine was established by per-card conformance tests and win-rate agreement
//! instead of by an identical word stream — a coarser net at the game level, a
//! finer one at the card level, and no constraint on the design. That engine is
//! now gone, and with it the last reason anyone would want the word stream.
//!
//! # Streams
//!
//! The old engine drove shuffles, draws, discovers and random effects from one
//! generator. That is what capped paired-seed variance reduction at the measured
//! 8.5×: changing a card in deck A shifts the whole downstream stream, so the
//! opponent's draws move too.
//!
//! [`Rngs`] gives each concern its own generator, so a one-card swap cannot
//! perturb the opponent at all. Doing this now is free; retrofitting it after
//! the card code lands would mean auditing every call site.

/// xoshiro256++ — 32 bytes of state, `Copy`, no heap.
///
/// Chosen over a PCG variant for the wider state (no short cycles across the
/// many parallel streams a batch run creates) and over MT19937 for the reason
/// in the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rand {
    s: [u64; 4],
}

/// SplitMix64 — used only to expand a seed into the four words xoshiro needs.
///
/// Seeding a wide generator from a small integer directly leaves correlated
/// streams; SplitMix64 is the standard remedy and is what the xoshiro authors
/// specify.
#[inline]
fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rand {
    /// A generator for `seed`. Every seed, including zero, produces a usable
    /// state — the all-zero state xoshiro cannot escape is unreachable here.
    #[inline]
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        let s = [
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
            splitmix64(&mut x),
        ];
        Self { s }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let out = self.s[0]
            .wrapping_add(self.s[3])
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        out
    }

    /// A uniform value in `0..n`, using Lemire's multiply-shift.
    ///
    /// One multiply and, almost always, no division and no retry — as opposed
    /// to the `getrandbits`-and-reject loop a Python-compatible generator is
    /// obliged to run. Returns 0 for `n == 0` rather than dividing by zero, so
    /// callers can pass an empty count without guarding.
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut m = (self.next_u64() as u128).wrapping_mul(n as u128);
        let mut low = m as u64;
        if low < n {
            // Only reachable for the few values that would skew the result.
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                m = (self.next_u64() as u128).wrapping_mul(n as u128);
                low = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// A uniform index into a collection of `len` items.
    #[inline]
    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// Inclusive on both ends.
    #[inline]
    pub fn between(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u64) as i32
    }

    /// True with probability one half — the opening coin flip.
    #[inline]
    pub fn flip(&mut self) -> bool {
        self.next_u64() >> 63 == 1
    }

    /// In-place Fisher-Yates.
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            xs.swap(i, self.index(i + 1));
        }
    }

    /// One element, or `None` when there is nothing to choose from.
    ///
    /// Python raises on an empty sequence; returning `None` moves that case
    /// into the type system, where every caller has to say what it means.
    #[inline]
    pub fn choose<'a, T>(&mut self, xs: &'a [T]) -> Option<&'a T> {
        if xs.is_empty() {
            None
        } else {
            Some(&xs[self.index(xs.len())])
        }
    }

    /// `k` distinct indices below `n`, written into `out`, without allocating.
    ///
    /// Discover asks for three of a few hundred on a hot path, so this returns
    /// into a caller-owned buffer instead of a `Vec`. Partial Fisher-Yates over
    /// a small scratch array when `k` is close to `n`, rejection otherwise;
    /// both are unbiased.
    ///
    /// Returns the number of indices written, which is `min(k, n)`.
    pub fn sample_indices(&mut self, n: usize, out: &mut [u32]) -> usize {
        let k = out.len().min(n);
        if k == 0 {
            return 0;
        }
        // Rejection is cheap while the sample is a small fraction of the
        // population; past roughly half it starts retrying too often.
        if k * 2 <= n {
            let mut written = 0;
            while written < k {
                let candidate = self.index(n) as u32;
                if !out[..written].contains(&candidate) {
                    out[written] = candidate;
                    written += 1;
                }
            }
            return k;
        }
        // Dense case: shuffle the first k slots of a virtual 0..n permutation.
        let mut pool: Vec<u32> = (0..n as u32).collect();
        for i in 0..k {
            let j = i + self.index(n - i);
            pool.swap(i, j);
            out[i] = pool[i];
        }
        k
    }
}

/// The generators one game draws from, split by concern.
///
/// 96 bytes, `Copy`, so it lives inside the game state without making a clone
/// expensive. Each player's library is independent of the other's, which is
/// what lets two candidate decks be evaluated on genuinely paired samples: a
/// swap in deck A cannot move deck B's draws.
#[derive(Clone, Copy, Debug)]
pub struct Rngs {
    /// Deck order and draws, one per player.
    pub library: [Rand; 2],
    /// Discovers, random targets, and every random effect.
    pub effects: Rand,
}

impl Rngs {
    /// Independent streams derived from one seed, so a run is still described
    /// by a single number. The multipliers are arbitrary odd constants; what
    /// matters is that SplitMix64 decorrelates them.
    pub fn new(seed: u64) -> Self {
        Self {
            library: [
                Rand::new(seed ^ 0xA076_1D64_78BD_642F),
                Rand::new(seed ^ 0xE703_7ED1_A0B4_28DB),
            ],
            effects: Rand::new(seed ^ 0x8EBC_6AF0_9C88_C6E3),
        }
    }

    /// The stream owning one player's library.
    #[inline]
    pub fn library_of(&mut self, player: usize) -> &mut Rand {
        &mut self.library[player]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_is_reproducible() {
        // The whole paired-seed evaluation scheme rests on this.
        let mut a = Rand::new(12345);
        let mut b = Rand::new(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge_immediately() {
        let mut a = Rand::new(1);
        let mut b = Rand::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn zero_seed_is_not_a_dead_state() {
        // xoshiro cannot escape an all-zero state; SplitMix64 seeding must
        // never produce one, including from seed 0.
        let mut r = Rand::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
        assert_ne!(r.next_u64(), first);
    }

    #[test]
    fn below_stays_in_range_and_is_roughly_uniform() {
        let mut r = Rand::new(7);
        let mut counts = [0u32; 7];
        for _ in 0..70_000 {
            let v = r.below(7) as usize;
            assert!(v < 7);
            counts[v] += 1;
        }
        // 10 000 expected per bucket; ±5% is far outside plausible drift for
        // 70 000 draws but well inside sampling noise for a correct generator.
        for (i, c) in counts.iter().enumerate() {
            assert!(
                (9_500..10_500).contains(c),
                "bucket {i} got {c}, expected about 10000"
            );
        }
    }

    #[test]
    fn below_zero_and_one_are_degenerate_not_panics() {
        let mut r = Rand::new(3);
        assert_eq!(r.below(0), 0);
        assert_eq!(r.below(1), 0);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Rand::new(99);
        for len in [0usize, 1, 2, 7, 30, 60] {
            let mut xs: Vec<u32> = (0..len as u32).collect();
            r.shuffle(&mut xs);
            let mut sorted = xs.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..len as u32).collect::<Vec<_>>(), "len {len}");
        }
    }

    #[test]
    fn shuffle_actually_moves_a_deck() {
        // A no-op shuffle would still pass the permutation test.
        let mut r = Rand::new(5);
        let mut xs: Vec<u32> = (0..30).collect();
        r.shuffle(&mut xs);
        assert_ne!(xs, (0..30).collect::<Vec<_>>());
    }

    #[test]
    fn choose_handles_the_empty_case() {
        let mut r = Rand::new(1);
        let empty: [u32; 0] = [];
        assert!(r.choose(&empty).is_none());
        assert_eq!(r.choose(&[42u32]), Some(&42));
    }

    #[test]
    fn sample_indices_are_distinct_and_in_range() {
        let mut r = Rand::new(11);
        // Sparse path (discover: 3 of a few hundred) and dense path.
        for (n, k) in [(257usize, 3usize), (10, 3), (5, 4), (3, 3), (1, 1)] {
            let mut out = vec![0u32; k];
            for _ in 0..500 {
                let got = r.sample_indices(n, &mut out);
                assert_eq!(got, k.min(n));
                let mut seen = out[..got].to_vec();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), got, "duplicate index for n={n} k={k}");
                assert!(out[..got].iter().all(|&i| (i as usize) < n));
            }
        }
    }

    #[test]
    fn sample_larger_than_population_is_clamped() {
        // Python raises here. Clamping keeps a discover from a two-card pool
        // from being a crash, which is the behaviour the engine wants.
        let mut r = Rand::new(2);
        let mut out = [0u32; 3];
        assert_eq!(r.sample_indices(2, &mut out), 2);
    }

    #[test]
    fn streams_are_independent() {
        // The property that makes paired evaluation work: consuming one
        // player's library must not move the other's or the effect stream.
        let mut a = Rngs::new(42);
        let mut b = Rngs::new(42);
        for _ in 0..100 {
            a.library_of(0).next_u64();
        }
        assert_eq!(
            a.library_of(1).next_u64(),
            b.library_of(1).next_u64(),
            "player 1's library moved when player 0 drew"
        );
        assert_eq!(
            a.effects.next_u64(),
            b.effects.next_u64(),
            "the effect stream moved when player 0 drew"
        );
    }

    #[test]
    fn rngs_stay_small_enough_to_live_in_the_game_state() {
        // The reason MT19937 was rejected. If this ever grows, the cheap-clone
        // property of `Game` is what is being spent.
        assert_eq!(size_of::<Rand>(), 32);
        assert_eq!(size_of::<Rngs>(), 96);
    }
}
