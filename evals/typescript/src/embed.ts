/**
 * Naive local hashing embedding — ported bit-for-bit (integer arithmetic and
 * sign) from the canonical Rust implementation in `hash-embed/src/lib.rs`
 * (originally `cli/src/embed.rs`).
 *
 * This is NOT a semantic embedding model. It's the classic feature-hashing
 * / "hashing trick": lowercase + whitespace-tokenize the text, hash each
 * token into one of `DIM` buckets via FNV-1a, accumulate +1/-1 into that
 * bucket (the sign comes from a different bit of the same hash), then
 * L2-normalize. It exists purely so the Python, TypeScript, and Rust eval
 * harnesses embed `evals/fixtures/golden_v1.json` identically and their
 * scores are comparable across languages.
 *
 * Regular JS `number` (float64) can't represent the full 64-bit FNV-1a
 * hash exactly past 2^53, so the hash computation uses `BigInt` throughout
 * — the token->bucket assignment and sign must match the Rust/Python
 * implementations exactly (integer/bitwise arithmetic); float rounding vs
 * Rust's f32 path (~1e-6) is fine since it doesn't affect cosine-similarity
 * ranking.
 */

export const DIM = 128;

const OFFSET_BASIS = 0xcbf29ce484222325n;
const PRIME = 0x100000001b3n;
const MASK = 0xffffffffffffffffn;

function fnv1a(bytes: Uint8Array): bigint {
  let h = OFFSET_BASIS;
  for (const b of bytes) {
    h ^= BigInt(b);
    h = (h * PRIME) & MASK;
  }
  return h;
}

export function hashEmbed(text: string): number[] {
  const v = new Array<number>(DIM).fill(0);
  const tokens = text.toLowerCase().split(/\s+/).filter((t) => t.length > 0);
  const encoder = new TextEncoder();
  for (const token of tokens) {
    const h = fnv1a(encoder.encode(token));
    const bucket = Number(h % BigInt(DIM));
    const sign = ((h >> 32n) & 1n) === 0n ? 1.0 : -1.0;
    v[bucket] += sign;
  }
  const norm = Math.sqrt(v.reduce((s, x) => s + x * x, 0));
  if (norm > 0) {
    for (let i = 0; i < v.length; i++) v[i] /= norm;
  }
  return v;
}
