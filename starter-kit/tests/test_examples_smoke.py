"""Smoke tests for every example in `starter_kit`.

Each example runs against a local embedded `z3rno.Client` with zero
external services — unlike the old starter kit's import-only gate, which
needed a live server it couldn't assume was up in CI, these tests
actually call each example's `main()` and assert it runs to completion.
Each runs inside a fresh temp directory so its SQLite file doesn't leak
into the repo or collide between tests.
"""

from __future__ import annotations

import importlib
from pathlib import Path

import pytest

EXAMPLE_MODULES = [
    "starter_kit.chat_memory",
    "starter_kit.customer_support",
    "starter_kit.sql_copilot",
    "starter_kit.code_memory",
    "starter_kit.research_notebook",
]


def test_starter_kit_has_five_examples() -> None:
    """The repo promise: five worked examples."""
    assert len(EXAMPLE_MODULES) == 5


def _short_id(module_name: str) -> str:
    return module_name.rsplit(".", 1)[-1]


@pytest.mark.parametrize("module_name", EXAMPLE_MODULES, ids=_short_id)
def test_example_has_callable_main(module_name: str) -> None:
    module = importlib.import_module(module_name)
    assert hasattr(module, "main"), f"{module_name} is missing a top-level main()"
    assert callable(module.main)


@pytest.mark.parametrize("module_name", EXAMPLE_MODULES, ids=_short_id)
def test_example_runs_end_to_end(
    module_name: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    module = importlib.import_module(module_name)
    module.main()
