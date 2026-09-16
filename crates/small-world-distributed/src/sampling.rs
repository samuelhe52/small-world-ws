use rand::{SeedableRng, seq::index::sample};
use rand_chacha::ChaCha8Rng;

pub(crate) fn sample_sources(n: usize, count: usize, seed: u64) -> Vec<usize> {
    let actual = count.min(n);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    sample(&mut rng, n, actual).into_vec()
}
