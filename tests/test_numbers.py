"""Numbers are compared by value (RFC 6902, 4.6): 1, 1.0 and -0.0 are 1, 1 and 0, but
1 is not true. The Rust diff and the Python DiffBuilder must agree on it."""
import copy
import json
import random

import pytest

import fastjsonpatch
import small_cases

NUMBERS = [0, 0.0, -0.0, 1, 1.0, 2, 2.0, -1, -1.0, 1.5, 2**53, 2.0**53, 2**53 + 1, 10**20, 1e20, 2**70,
           float(2**70), float("inf"), float("nan")]
OTHERS = [True, False, None, "1", "a", ""]


def rand_value(rng, depth=0):
    r = rng.random()
    if depth < 3 and r < 0.2:
        return {rng.choice("abc"): rand_value(rng, depth + 1) for _ in range(rng.randrange(4))}
    if depth < 3 and r < 0.5:
        return [rand_value(rng, depth + 1) for _ in range(rng.randrange(6))]
    return rng.choice(NUMBERS if rng.random() < 0.8 else OTHERS)


def as_text(value):
    return json.dumps(small_cases.canon(value), sort_keys=True)


@pytest.mark.parametrize("seed", range(500))
def test_rust_and_python_diff_agree_on_numbers(seed):
    rng = random.Random(seed)
    src = {"x": rand_value(rng), "l": [rand_value(rng) for _ in range(rng.randrange(8))]}
    dst = copy.deepcopy(src)
    for _ in range(rng.randrange(1, 4)):
        dst["l"].insert(rng.randrange(len(dst["l"]) + 1), rand_value(rng))
        if dst["l"] and rng.random() < 0.5:
            dst["l"].insert(rng.randrange(len(dst["l"]) + 1), dst["l"].pop(rng.randrange(len(dst["l"]))))
        dst["x"] = rand_value(rng) if rng.random() < 0.5 else dst["x"]
    fast = fastjsonpatch.make_patch(src, dst)
    slow = fastjsonpatch.JsonPatch.from_diff(src, dst, dumps=json.dumps)  # the Python DiffBuilder
    assert repr(fast.patch) == repr(slow.patch)
    # applying it gives a document that is dst, as RFC 6902 compares them
    assert as_text(fast.apply(src)) == as_text(dst)


@pytest.mark.parametrize("src, dst", [
    ({"a": 1}, {"a": 1.0}), ({"a": 1.0}, {"a": 1}), ({"a": 0.0}, {"a": -0.0}), ({"a": 0}, {"a": -0.0}),
    ({"a": [1]}, {"a": [1.0]}), ({"a": 10**20}, {"a": 1e20}), ({"a": 2**70}, {"a": float(2**70)}),
    ({"a": {"x": [1, {"y": 2}]}}, {"a": {"x": [1.0, {"y": 2.0}]}}),
    ([1, 2, 3], [1.0, 2.0, 3.0]),
])
def test_equal_numbers_need_no_patch(src, dst):
    assert fastjsonpatch.make_patch(src, dst).patch == []


@pytest.mark.parametrize("src, dst", [
    ({"a": 1}, {"a": 1.5}), ({"a": 2**53 + 1}, {"a": float(2**53)}), ({"a": 1}, {"a": True}),
    ({"a": 0}, {"a": False}), ({"a": 1.0}, {"a": True}), ({"a": 1}, {"a": "1"}), ({"a": [1]}, {"a": [True]}),
    ({"a": 1}, {"a": float("inf")}),
])
def test_different_values_are_patched(src, dst):
    patch = fastjsonpatch.make_patch(src, dst)
    assert [op["op"] for op in patch.patch] == ["replace"]
    assert as_text(patch.apply(src)) == as_text(dst)


def test_equal_numbers_in_lists_are_moved_not_replaced():
    assert fastjsonpatch.make_patch([1, 2], [2.0, 1]).patch == [{"op": "move", "from": "/1", "path": "/0"}]
    assert fastjsonpatch.make_patch({"a": [1]}, {"b": [1.0]}).patch == [{"op": "move", "from": "/a", "path": "/b"}]


def test_custom_dumps_decides_by_itself():
    dumps = lambda o: json.dumps(o, sort_keys=True)  # noqa: E731
    assert fastjsonpatch.JsonPatch.from_diff({"a": 1}, {"a": 1.0}, dumps=dumps).patch == \
        [{"op": "replace", "path": "/a", "value": 1.0}]


def test_own_dumps_gives_the_old_result_documented_in_migration_md():
    patch = fastjsonpatch.JsonPatch.from_diff({"a": 1}, {"a": 1.0}, dumps=lambda value: json.dumps(value))
    assert patch.patch == [{"op": "replace", "path": "/a", "value": 1.0}]
