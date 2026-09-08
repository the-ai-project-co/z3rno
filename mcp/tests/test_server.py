"""Functional tests: call the MCP tool functions directly (no stdio
round-trip) against a real embedded z3rno engine at a temp SQLite path."""

import pytest

from z3rno_mcp import server
from z3rno_mcp.embed import hash_embed


@pytest.fixture(autouse=True)
def fresh_client(tmp_path, monkeypatch):
    """Point the server at a fresh temp SQLite db and reset its singleton
    client so each test gets an isolated, freshly-built engine."""
    monkeypatch.setenv("Z3RNO_SQLITE_PATH", str(tmp_path / "z3rno.db"))
    monkeypatch.setenv("Z3RNO_TENANT_ID", "test-tenant")
    monkeypatch.delenv("Z3RNO_DATABASE_URL", raising=False)
    server._client = None
    yield
    server._client = None


def test_store_recall_forget_roundtrip():
    memory_id = server.z3rno_store(content="the sky is blue")
    assert isinstance(memory_id, str) and memory_id

    results = server.z3rno_recall(query="sky")
    assert any(m["id"] == memory_id for m in results)
    found = next(m for m in results if m["id"] == memory_id)
    assert found["content"] == "the sky is blue"
    assert found["tier"] == "semantic"
    assert "created_at" in found

    confirmation = server.z3rno_forget(id=memory_id)
    assert memory_id in confirmation
    assert "audit event" in confirmation

    results_after = server.z3rno_recall(query="sky")
    assert not any(m["id"] == memory_id for m in results_after)


def test_forget_missing_id_is_not_an_error():
    fake_id = "00000000-0000-0000-0000-000000000000"
    message = server.z3rno_forget(id=fake_id)
    assert "no memory found" in message


def test_store_rejects_invalid_tier():
    with pytest.raises(ValueError, match="invalid tier"):
        server.z3rno_store(content="x", tier="bogus")


def test_audit_records_store_and_forget():
    memory_id = server.z3rno_store(content="audit me", tier="episodic")
    server.z3rno_forget(id=memory_id)

    events = server.z3rno_audit()
    operations = [(e["operation"], e["memory_id"]) for e in events]
    assert ("store", memory_id) in operations
    assert ("forget", memory_id) in operations


def test_recall_and_store_fall_back_to_matching_hash_embedding():
    # If store and recall both fall back to hash_embed, a query that shares
    # words with the stored content should find it without an explicit
    # embedding on either call.
    memory_id = server.z3rno_store(content="quarterly revenue exceeded forecasts")
    results = server.z3rno_recall(query="quarterly revenue")
    assert any(m["id"] == memory_id for m in results)


def test_hash_embed_conformance():
    # Reference vectors computed from the real Rust hash-embed implementation.
    v = hash_embed("hello world")
    for i, x in enumerate(v):
        if i == 11:
            assert x == pytest.approx(0.7071068, abs=1e-6)
        elif i == 115:
            assert x == pytest.approx(0.7071068, abs=1e-6)
        else:
            assert x == 0.0

    assert hash_embed("") == [0.0] * 128
