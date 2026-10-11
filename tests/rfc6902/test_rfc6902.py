"""The json-patch-tests conformance suite (RFC 6902), checked against the expected
results and, when jsonpatch is installed, against jsonpatch's own behaviour."""
import copy
import json
import os

import pytest

import fastjsonpatch

try:
    import jsonpatch
except ImportError:  # the reference is optional for this suite
    jsonpatch = None

HERE = os.path.dirname(__file__)
CASES = [
    pytest.param(case, id=f"{name}:{i}:{case.get('comment', '')[:50]}")
    for name in ("tests.json", "spec_tests.json")
    for i, case in enumerate(json.load(open(os.path.join(HERE, name), encoding="utf-8")))
    if "doc" in case and not case.get("disabled")
]


def outcome(mod, case):
    try:
        return "ok", mod.apply_patch(copy.deepcopy(case["doc"]), copy.deepcopy(case["patch"]))
    except Exception as e:  # noqa: BLE001 - comparing any failure
        return type(e).__name__, str(e)


@pytest.mark.parametrize("case", CASES)
def test_case(case):
    kind, result = outcome(fastjsonpatch, case)
    if "expected" in case:
        assert kind == "ok" and result == case["expected"]
    elif "error" in case:
        assert kind != "ok"
    if jsonpatch is not None:
        assert repr((kind, result)) == repr(outcome(jsonpatch, case))
