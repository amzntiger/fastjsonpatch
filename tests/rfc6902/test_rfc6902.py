"""The json-patch-tests conformance suite (RFC 6902) and cases for the parts of the RFC
that jsonpatch 1.34 did not follow, checked against the expected results and, when
jsonpatch is installed, against jsonpatch's own behaviour."""
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


def runnable(case):
    # The disabled cases with two 'op' members cannot be represented as a dict; the
    # other disabled ones (a scalar document, testing the whole document) can.
    return "doc" in case and "two 'op' members" not in case.get("error", "")


# The parts of RFC 6902 (and RFC 6901) that jsonpatch 1.34 did not follow.
RFC_CASES = [
    {"comment": "4.6 test: 1 is not true", "doc": {"a": 1},
     "patch": [{"op": "test", "path": "/a", "value": True}], "error": "not equal"},
    {"comment": "4.6 test: true is not 1", "doc": {"a": True},
     "patch": [{"op": "test", "path": "/a", "value": 1}], "error": "not equal"},
    {"comment": "4.6 test: nested 0 is not false", "doc": {"a": [0]},
     "patch": [{"op": "test", "path": "/a", "value": [False]}], "error": "not equal"},
    {"comment": "4.6 test: numbers are compared by value", "doc": {"a": 1},
     "patch": [{"op": "test", "path": "/a", "value": 1.0}], "expected": {"a": 1}},
    {"comment": "4.6 test: the order of members does not matter", "doc": {"a": {"x": 1, "y": 2}},
     "patch": [{"op": "test", "path": "/a", "value": {"y": 2, "x": 1}}], "expected": {"a": {"x": 1, "y": 2}}},
    {"comment": "RFC 6901: '-' is an ordinary member name of an object", "doc": {"-": 1},
     "patch": [{"op": "replace", "path": "/-", "value": 2}], "expected": {"-": 2}},
    {"comment": "4.1 add: path '' replaces a scalar document", "doc": 5,
     "patch": [{"op": "add", "path": "", "value": [1]}], "expected": [1]},
    {"comment": "4.4 move: not into the array item's own children", "doc": {"a": [[1]]},
     "patch": [{"op": "move", "from": "/a/0", "path": "/a/0/0"}], "error": "own children"},
    {"comment": "4.4 move: not the whole document into its children", "doc": {"a": {}},
     "patch": [{"op": "move", "from": "", "path": "/a/b"}], "error": "own children"},
    {"comment": "4.4 move: the whole document onto itself is a no-op", "doc": {"a": 1},
     "patch": [{"op": "move", "from": "", "path": ""}], "expected": {"a": 1}},
    {"comment": "4.5 copy: from '-' of an array does not exist", "doc": {"a": [1]},
     "patch": [{"op": "copy", "from": "/a/-", "path": "/b"}], "error": "invalid array index"},
    {"comment": "4.4 move: from '-' of an array does not exist", "doc": {"a": [1]},
     "patch": [{"op": "move", "from": "/a/-", "path": "/b"}], "error": "invalid array index"},
]

CASES = [
    pytest.param(case, id=f"{name}:{i}:{case.get('comment', '')[:50]}")
    for name in ("tests.json", "spec_tests.json")
    for i, case in enumerate(json.load(open(os.path.join(HERE, name), encoding="utf-8")))
    if runnable(case)
] + [pytest.param(case, id=f"rfc:{i}:{case['comment'][:50]}") for i, case in enumerate(RFC_CASES)]


def json_equal(a, b):
    """Equality of JSON values as RFC 6902, 4.6 defines it: true and false only equal
    themselves (Python has True == 1), numbers are compared by value, objects member
    by member in any order, arrays item by item."""
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(json_equal(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(map(json_equal, a, b))
    numbers = (int, float)
    if isinstance(a, numbers) and isinstance(b, numbers):
        return a == b
    return type(a) is type(b) and not isinstance(a, (dict, list)) and a == b


def outcome(mod, case):
    try:
        return "ok", mod.apply_patch(copy.deepcopy(case["doc"]), copy.deepcopy(case["patch"]))
    except Exception as e:  # noqa: BLE001 - comparing any failure
        return type(e).__name__, str(e)


@pytest.mark.parametrize("case", CASES)
def test_case(case):
    kind, result = outcome(fastjsonpatch, case)
    if "expected" in case:
        assert kind == "ok" and json_equal(result, case["expected"]), (kind, result)
    elif "error" in case:
        assert kind != "ok"
    if jsonpatch is not None:
        assert repr((kind, result)) == repr(outcome(jsonpatch, case))
