import math

import pytest

from z3rno_evals.metrics import faithfulness, latency_percentiles, mrr, recall_at_k


class TestRecallAtK:
    def test_normal_case(self):
        # 2 of 3 expected ids present in the top 3 -> 2/3.
        retrieved = ["a", "b", "c", "d"]
        expected = ["a", "c", "z"]
        assert recall_at_k(retrieved, expected, 3) == pytest.approx(2 / 3)

    def test_zero_hit_case(self):
        assert recall_at_k(["a", "b"], ["x", "y"], 2) == 0.0

    def test_nan_when_expected_empty(self):
        assert math.isnan(recall_at_k(["a", "b"], [], 2))

    def test_k_limits_the_window(self):
        # "b" is expected but only "a" is within top_k=1.
        assert recall_at_k(["a", "b"], ["a", "b"], 1) == pytest.approx(0.5)


class TestMRR:
    def test_normal_case(self):
        # first expected id ("b") found at rank 2 -> 1/2.
        assert mrr(["a", "b", "c"], ["b", "c"]) == pytest.approx(0.5)

    def test_first_rank_hit(self):
        assert mrr(["b", "a"], ["b"]) == pytest.approx(1.0)

    def test_zero_when_none_found(self):
        assert mrr(["a", "b"], ["z"]) == 0.0

    def test_nan_when_expected_empty(self):
        assert math.isnan(mrr(["a", "b"], []))


class TestLatencyPercentiles:
    def test_normal_case(self):
        # nearest-rank over 10 sorted values 1..10:
        # p50 -> idx ceil(0.5*10)-1=4 -> 5; p95 -> idx ceil(9.5)-1=9 -> 10;
        # p99 -> idx ceil(9.9)-1=9 -> 10.
        latencies = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
        result = latency_percentiles([float(x) for x in latencies])
        assert result.p50 == 5.0
        assert result.p95 == 10.0
        assert result.p99 == 10.0
        assert result.mean == pytest.approx(5.5)
        assert result.count == 10

    def test_empty_is_all_zero(self):
        result = latency_percentiles([])
        assert result.p50 == 0.0
        assert result.p95 == 0.0
        assert result.p99 == 0.0
        assert result.mean == 0.0
        assert result.count == 0

    def test_single_value(self):
        result = latency_percentiles([42.0])
        assert result.p50 == result.p95 == result.p99 == result.mean == 42.0
        assert result.count == 1


class TestFaithfulness:
    def test_normal_case(self):
        # expected non-stopword tokens: {run, make, deploy, staging, from,
        # infra, directory} (7 tokens after stripping stopwords/punct).
        # all appear in the context.
        expected_answer = "Run make deploy-staging from the infra directory."
        context = "To deploy the staging environment, run `make deploy-staging` from the infra directory."
        score = faithfulness(expected_answer, context)
        assert score == pytest.approx(1.0)

    def test_zero_hit_case(self):
        expected_answer = "The sky is blue today."
        context = "Rust is a systems programming language."
        assert faithfulness(expected_answer, context) == 0.0

    def test_nan_when_expected_answer_empty(self):
        assert math.isnan(faithfulness("", "some context"))
        assert math.isnan(faithfulness("   ", "some context"))

    def test_nan_when_expected_answer_all_stopwords(self):
        assert math.isnan(faithfulness("the a an is", "some context"))

    def test_partial_hit_worked_example(self):
        # expected tokens (non-stopword): {cat, sat, mat} -> 3 tokens.
        # context has "cat" and "mat" but not "sat" -> 2/3.
        expected_answer = "the cat sat on the mat"
        context = "a cat near the mat"
        assert faithfulness(expected_answer, context) == pytest.approx(2 / 3)
