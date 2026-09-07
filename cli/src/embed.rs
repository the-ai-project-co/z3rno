//! A naive local **lexical** embedding, used by `store`/`recall` (see
//! `main.rs`) whenever the caller doesn't pass `--embedding` explicitly.
//!
//! This is **not** a semantic embedding model. It's the classic
//! feature-hashing / "hashing trick": lowercase + whitespace-tokenize the
//! text, hash each token into one of [`DIM`] buckets, accumulate +1/-1 into
//! that bucket (the sign comes from a different bit of the same hash, so
//! it isn't just "count of collisions"), then L2-normalize. Two pieces of
//! text that share more words end up with a higher dot product — a crude
//! bag-of-words similarity, nothing more. It has no notion of synonyms,
//! word order, or meaning.
//!
//! It exists so `z3rno store`/`z3rno recall` work end-to-end with zero
//! external services and zero API keys — the whole point of a CLI that's
//! useful the moment you install it. For anything beyond kicking the
//! tires, supply real embeddings from an LLM provider via `--embedding`,
//! or go through the SDKs/server API, which take embeddings as plain
//! `Vec<f32>` and don't care how they were produced.
//!
//! `store` and `recall` both call [`hash_embed`] — the exact same function
//! — so a recall query lands in the same vector space as the memories it's
//! meant to find.

/// Fixed output width. 128 is small enough to stay fast and cheap to store,
/// large enough that unrelated short phrases rarely collide into the same
/// few buckets.
pub const DIM: usize = 128;

/// Embeds `text` into a `DIM`-dimensional, L2-normalized vector. See the
/// module doc for what this is and isn't.
pub fn hash_embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    for token in text.to_lowercase().split_whitespace() {
        let h = fnv1a(token.as_bytes());
        let bucket = (h % DIM as u64) as usize;
        // `bucket` is derived from the low bits of `h` (DIM is a power of
        // two, so `h % DIM` is exactly those bits); the sign bit is pulled
        // from higher up the hash so it isn't just restating the bucket.
        let sign = if (h >> 32) & 1 == 0 { 1.0 } else { -1.0 };
        v[bucket] += sign;
    }
    l2_normalize(&mut v);
    v
}

/// FNV-1a: tiny, dependency-free, and deterministic across processes,
/// platforms, and Rust versions — unlike `std::hash::Hash` +
/// `DefaultHasher`, which only promises stability within a single build.
/// Determinism here is load-bearing: `store` and `recall` must hash the
/// same token to the same bucket every time, including across separate CLI
/// invocations.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        assert_eq!(hash_embed("hello world"), hash_embed("hello world"));
    }

    #[test]
    fn has_the_documented_dimension() {
        assert_eq!(hash_embed("anything").len(), DIM);
    }

    #[test]
    fn is_l2_normalized() {
        let v = hash_embed("the quick brown fox jumps over the lazy dog");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn empty_text_is_the_zero_vector_not_a_panic() {
        let v = hash_embed("");
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn is_order_invariant_bag_of_words() {
        // Same multiset of tokens, different order -> identical vector.
        assert_eq!(hash_embed("brown fox quick"), hash_embed("quick fox brown"));
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(hash_embed("Hello World"), hash_embed("hello world"));
    }

    #[test]
    fn shares_more_signal_between_related_text_than_unrelated_text() {
        let a = hash_embed("the cat sat on the mat");
        let b = hash_embed("the cat sat on the rug");
        let c = hash_embed("quarterly revenue exceeded forecasts");

        let dot = |x: &[f32], y: &[f32]| -> f32 { x.iter().zip(y).map(|(p, q)| p * q).sum() };

        assert!(dot(&a, &b) > dot(&a, &c));
    }
}
