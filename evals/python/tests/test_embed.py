"""Conformance test: `hash_embed` must match the canonical Rust
implementation (cli/src/embed.rs) bucket-for-bucket and sign-for-sign, at
least at 1e-4 float tolerance. Reference vectors were computed by directly
executing the Rust implementation.
"""

import pytest

from z3rno_evals.embed import DIM, hash_embed


def _nonzero(v: list[float]) -> dict[int, float]:
    return {i: x for i, x in enumerate(v) if x != 0.0}


def test_hello_world_matches_rust_reference():
    v = hash_embed("hello world")
    assert len(v) == DIM
    expected = {11: 0.7071068, 115: 0.7071068}
    got = _nonzero(v)
    assert got.keys() == expected.keys()
    for bucket, value in expected.items():
        assert got[bucket] == pytest.approx(value, abs=1e-4)


def test_pangram_matches_rust_reference():
    v = hash_embed("the quick brown fox jumps over the lazy dog")
    expected = {
        14: 0.3015113,
        28: 0.3015113,
        47: -0.3015113,
        79: 0.3015113,
        90: 0.3015113,
        105: 0.3015113,
        111: 0.3015113,
        124: -0.6030227,
    }
    got = _nonzero(v)
    assert got.keys() == expected.keys()
    for bucket, value in expected.items():
        assert got[bucket] == pytest.approx(value, abs=1e-4)


def test_empty_string_is_zero_vector():
    v = hash_embed("")
    assert len(v) == DIM
    assert all(x == 0.0 for x in v)


def test_is_deterministic():
    assert hash_embed("hello world") == hash_embed("hello world")


def test_is_case_insensitive():
    assert hash_embed("Hello World") == hash_embed("hello world")


def test_is_order_invariant_bag_of_words():
    assert hash_embed("brown fox quick") == hash_embed("quick fox brown")
