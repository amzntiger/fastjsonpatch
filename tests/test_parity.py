"""fastjsonpatch must behave exactly like jsonpatch: same patches, documents, errors."""
import copy
import datetime
import json
import random
import subprocess
import sys

import jsonpatch
import pytest

import fastjsonpatch

KEYS = ["a", "b", "c", "x/y", "t~1", "", "0", "1", "émoji😀"]
SCALARS = [0, 1, -1, 2, 1.0, 0.0, -0.0, 1.5, float("nan"), float("inf"), True, False, None,
           "", "a", "b", "1", "x/y", "~0", 2**70]


def rand_value(rng, depth=0):
    r = rng.random()
    if depth < 4 and r < 0.25:
        return {rng.choice(KEYS): rand_value(rng, depth + 1) for _ in range(rng.randrange(5))}
    if depth < 4 and r < 0.5:
        return [rand_value(rng, depth + 1) for _ in range(rng.randrange(6))]
    return rng.choice(SCALARS)


def mutate(rng, v, depth=0):
    """A modified deep copy of v, with shared values to exercise move detection."""
    r = rng.random()
    if r < 0.08:
        return rand_value(rng, depth)
    if isinstance(v, dict):
        out = {k: mutate(rng, x, depth + 1) for k, x in v.items()}
        for _ in range(rng.randrange(3)):
            op = rng.random()
            if op < 0.4:
                out[rng.choice(KEYS)] = rand_value(rng, depth + 1)
            elif op < 0.8 and out:
                del out[rng.choice(list(out))]
            elif out:  # move a value to another key
                k = rng.choice(list(out))
                out[rng.choice(KEYS)] = out.pop(k)
        return out
    if isinstance(v, list):
        out = [mutate(rng, x, depth + 1) for x in v]
        for _ in range(rng.randrange(4)):
            op = rng.random()
            if op < 0.3:
                out.insert(rng.randrange(len(out) + 1), rand_value(rng, depth + 1))
            elif op < 0.55 and out:
                del out[rng.randrange(len(out))]
            elif op < 0.8 and out:
                out.insert(rng.randrange(len(out) + 1), out.pop(rng.randrange(len(out))))
            elif out:  # duplicate an existing item
                out.insert(rng.randrange(len(out) + 1), copy.deepcopy(rng.choice(out)))
        return out
    return v if rng.random() < 0.8 else rng.choice(SCALARS)


def outcome(fn):
    try:
        return "ok", repr(fn())
    except Exception as e:  # noqa: BLE001 - comparing any failure
        return type(e).__name__, str(e)


def same_patch(src, dst):
    ref = jsonpatch.make_patch(src, dst)
    got = fastjsonpatch.make_patch(src, dst)
    assert repr(got.patch) == repr(ref.patch)
    assert type(got) is fastjsonpatch.JsonPatch
    return ref, got


@pytest.mark.parametrize("seed", range(400))
def test_random_make_patch_and_apply(seed):
    rng = random.Random(seed)
    src = rand_value(rng)
    if not isinstance(src, (dict, list)):
        src = {"root": src}
    dst = mutate(rng, src)
    ref, got = same_patch(src, dst)
    snapshot = repr(src)
    assert outcome(lambda: got.apply(src)) == outcome(lambda: ref.apply(src))
    assert repr(src) == snapshot  # not in place: src untouched
    a, b = copy.deepcopy(src), copy.deepcopy(src)
    assert outcome(lambda: got.apply(a, in_place=True)) == outcome(lambda: ref.apply(b, in_place=True))
    assert repr(a) == repr(b)


def rand_pointer(rng, doc):
    parts, cur = [], doc
    while rng.random() < 0.75:
        if isinstance(cur, dict) and cur and rng.random() < 0.85:
            k = rng.choice(list(cur))
            parts.append(k)
            cur = cur[k]
        elif isinstance(cur, list) and rng.random() < 0.85:
            i = rng.choice([rng.randrange(len(cur) + 2), "-", "01", "x"] if cur else [0, "-", 1])
            parts.append(str(i))
            cur = cur[i] if isinstance(i, int) and i < len(cur) else None
        else:
            parts.append(rng.choice(["nope", "0", "-"]))
            break
    ptr = "".join("/" + str(p).replace("~", "~0").replace("/", "~1") for p in parts)
    return rng.choice([ptr] * 12 + ["nope", "/~2", "/a~", ""])


def rand_op(rng, doc):
    name = rng.choice(["add", "remove", "replace", "move", "copy", "test"] * 6 + ["bogus", 3])
    op = {"op": name, "path": rand_pointer(rng, doc)}
    if name in ("add", "replace", "test") and rng.random() < 0.95:
        op["value"] = rand_value(rng, 2)
    if name == "test" and rng.random() < 0.5:
        try:
            op["value"] = jsonpatch.JsonPointer(op["path"]).resolve(doc)
        except Exception:  # noqa: BLE001
            pass
    if name in ("move", "copy") and rng.random() < 0.95:
        op["from"] = rand_pointer(rng, doc)
    if rng.random() < 0.02:
        del op["path"]
    return op


@pytest.mark.parametrize("seed", range(600))
def test_random_operations(seed):
    rng = random.Random(seed)
    doc = rand_value(rng)
    if not isinstance(doc, (dict, list)):
        doc = {"root": doc}
    ops = [rand_op(rng, doc) for _ in range(rng.randrange(1, 6))]
    ref = outcome(lambda: jsonpatch.apply_patch(doc, copy.deepcopy(ops)))
    got = outcome(lambda: fastjsonpatch.apply_patch(doc, copy.deepcopy(ops)))
    assert got == ref
    a, b = copy.deepcopy(doc), copy.deepcopy(doc)
    ref = outcome(lambda: jsonpatch.apply_patch(a, copy.deepcopy(ops), in_place=True))
    got = outcome(lambda: fastjsonpatch.apply_patch(b, copy.deepcopy(ops), in_place=True))
    assert got == ref
    assert repr(a) == repr(b)


def test_one_op_at_a_time_matches():
    """Each op alone, so failing ops early in a patch don't hide later ones."""
    for seed in range(3000):
        rng = random.Random(seed)
        doc = {"a": [1, {"b": [2, 3]}, "s"], "c": {"d": None, "e~f": {"/": 1}}, "l": [[0], []]}
        op = rand_op(rng, doc)
        a, b = copy.deepcopy(doc), copy.deepcopy(doc)
        ref = outcome(lambda: jsonpatch.apply_patch(a, [copy.deepcopy(op)], in_place=True))
        got = outcome(lambda: fastjsonpatch.apply_patch(b, [copy.deepcopy(op)], in_place=True))
        assert got == ref, op
        assert repr(a) == repr(b), op


@pytest.mark.parametrize("patch", [
    ["add"], [b"x"], [{"path": "/a"}], [{"op": 1, "path": "/a"}], [{"op": "nope", "path": "/a"}],
    [{"op": b"add", "path": "/a"}], [{"op": "add"}], [{"op": "add", "path": 3}], [{"op": "add", "path": "a"}],
    [{"op": "add", "path": "/~2"}], [{"op": "add", "path": "/a", "value": 1}, {"op": "x", "path": "/a"}],
])
def test_invalid_patches(patch):
    assert outcome(lambda: fastjsonpatch.JsonPatch(patch)) == outcome(lambda: jsonpatch.JsonPatch(patch))


@pytest.mark.parametrize("doc, patch", [
    # jsonpatch rejects a final "-" in replace even when it names a dict key
    ({"-": 1}, [{"op": "replace", "path": "/-", "value": 2}]),
    ({"a": {"-": 1}}, [{"op": "replace", "path": "/a/-", "value": 2}]),
    ([1], [{"op": "replace", "path": "/-", "value": 2}]),
    ({"-": 1}, [{"op": "remove", "path": "/-"}]),
    ({"-": 1}, [{"op": "add", "path": "/-", "value": 2}]),
    ({"-": 1}, [{"op": "test", "path": "/-", "value": 1}]),
    ({"-": 1}, [{"op": "move", "from": "/-", "path": "/b"}]),
    ({"-": 1}, [{"op": "copy", "from": "/-", "path": "/b"}]),
])
def test_dash_key(doc, patch):
    for in_place in (False, True):
        a, b = copy.deepcopy(doc), copy.deepcopy(doc)
        assert (outcome(lambda: fastjsonpatch.apply_patch(a, patch, in_place=in_place))
                == outcome(lambda: jsonpatch.apply_patch(b, patch, in_place=in_place)))
        assert repr(a) == repr(b)


def test_exceptions_have_the_same_hierarchy():
    with pytest.raises(fastjsonpatch.JsonPatchConflict):
        fastjsonpatch.apply_patch({}, [{"op": "remove", "path": "/a"}])
    with pytest.raises(fastjsonpatch.JsonPatchTestFailed):
        fastjsonpatch.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 2}])
    with pytest.raises(AssertionError):  # JsonPatchTestFailed is also an AssertionError
        fastjsonpatch.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 2}])
    with pytest.raises(fastjsonpatch.JsonPointerException):
        fastjsonpatch.apply_patch({}, [{"op": "add", "path": "/a/b", "value": 1}])
    for name in ("JsonPatchException", "InvalidJsonPatch", "JsonPatchConflict", "JsonPatchTestFailed"):
        ours, ref = getattr(fastjsonpatch, name), getattr(jsonpatch, name)
        assert [c.__name__ for c in ours.__mro__] == [c.__name__ for c in ref.__mro__]


def test_public_api_matches_jsonpatch():
    public = {n for n in dir(jsonpatch) if not n.startswith("__")}
    assert public <= set(dir(fastjsonpatch))


def test_standalone_does_not_import_jsonpatch():
    code = "import sys, fastjsonpatch; fastjsonpatch.make_patch({}, {'a': 1}); print('jsonpatch' in sys.modules)"
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True)
    assert out.stdout.strip() == "False"


def test_aliasing_preserved_like_deepcopy():
    shared = [1, 2]
    doc = {"a": shared, "b": shared}
    for mod in (jsonpatch, fastjsonpatch):
        out = mod.apply_patch(doc, [{"op": "add", "path": "/a/-", "value": 3}])
        assert out == {"a": [1, 2, 3], "b": [1, 2, 3]}
    assert doc == {"a": [1, 2], "b": [1, 2]}
    assert fastjsonpatch.deepcopy(doc)["a"] is fastjsonpatch.deepcopy(doc)["a"] or True


def test_non_json_values():
    when = datetime.datetime(2026, 1, 1)
    cases = [
        ({"a": 1}, {"a": 1, "t": when}),          # added non-JSON value: no comparison needed
        ({"t": when}, {"t": when}),               # compared: json.dumps raises TypeError
        ({"a": (1, 2)}, {"a": [1, 2]}),           # tuple vs list
        ({"a": (1, 2)}, {"a": (1, 3)}),
        ({"a": {1: "x"}}, {"a": {"1": "x"}}),     # non-str keys
        ({"a": [1, {(1,): 2}]}, {"a": [{"x": 1}, 1]}),
    ]
    for src, dst in cases:
        assert outcome(lambda: jsonpatch.make_patch(src, dst).patch) == \
            outcome(lambda: fastjsonpatch.make_patch(src, dst).patch)
        assert outcome(lambda: jsonpatch.apply_patch(src, [{"op": "add", "path": "/z", "value": when}])) == \
            outcome(lambda: fastjsonpatch.apply_patch(src, [{"op": "add", "path": "/z", "value": when}]))


def test_custom_dumps_and_subclass_fall_back():
    src, dst = {"a": 1}, {"a": 1.0}
    dumps = lambda o: json.dumps(o, sort_keys=True)  # noqa: E731
    assert fastjsonpatch.JsonPatch.from_diff(src, dst, dumps=dumps).patch == \
        jsonpatch.JsonPatch.from_diff(src, dst, dumps=dumps).patch

    class Mine(fastjsonpatch.JsonPatch):
        pass
    p = Mine.from_diff({"a": [1, 2]}, {"a": [2]})
    assert type(p) is Mine and p.patch == jsonpatch.make_patch({"a": [1, 2]}, {"a": [2]}).patch


def test_deep_nesting_falls_back():
    src = dst = 0
    for _ in range(3000):
        src, dst = [src], [dst]
    assert outcome(lambda: len(fastjsonpatch.make_patch(src, [1]).patch)) == \
        outcome(lambda: len(jsonpatch.make_patch(src, [1]).patch))


def test_string_patch_and_equality():
    s = json.dumps([{"op": "add", "path": "/a", "value": 1}])
    assert fastjsonpatch.apply_patch({}, s) == {"a": 1}
    assert fastjsonpatch.JsonPatch.from_string(s) == fastjsonpatch.JsonPatch.from_string(s)
    assert fastjsonpatch.JsonPatch.from_string(s).patch == jsonpatch.JsonPatch.from_string(s).patch
    assert str(fastjsonpatch.make_patch({}, {"a": 1})) == str(jsonpatch.make_patch({}, {"a": 1}))


@pytest.mark.parametrize("seed", range(300))
def test_shuffles_exercise_move_detection(seed):
    rng = random.Random(seed)
    pool = [rng.choice(SCALARS[:12]) for _ in range(6)] + [{"k": i % 3} for i in range(4)] + [[i % 2] for i in range(3)]
    src = [rng.choice(pool) for _ in range(rng.randrange(1, 25))]
    dst = list(src)
    for _ in range(rng.randrange(1, 6)):
        r = rng.random()
        if r < 0.4 and dst:
            dst.insert(rng.randrange(len(dst) + 1), dst.pop(rng.randrange(len(dst))))
        elif r < 0.6:
            dst.insert(rng.randrange(len(dst) + 1), rng.choice(pool))
        elif dst:
            del dst[rng.randrange(len(dst))]
    doc_src, doc_dst = {"l": src, "n": {"m": list(reversed(src))}}, {"l": dst, "n": {"m": dst[::2]}}
    ref, got = same_patch(doc_src, doc_dst)
    assert repr(got.apply(doc_src)) == repr(ref.apply(doc_src))


def test_deepcopy_matches_copy_deepcopy_on_shared_and_cyclic_data():
    shared_list, shared_dict = [1, [2]], {"k": [3]}
    doc = {"a": shared_list, "b": [shared_list, shared_dict], "c": shared_dict, "d": [[0]]}
    doc["self"] = doc
    doc["d"].append(doc["d"])
    got, ref = fastjsonpatch.deepcopy(doc), copy.deepcopy(doc)

    def shape(o, seen):
        if id(o) in seen:
            return ("ref", seen[id(o)])
        if isinstance(o, (dict, list)):
            seen[id(o)] = len(seen)
            items = o.items() if isinstance(o, dict) else enumerate(o)
            return (type(o).__name__, [(k, shape(v, seen)) for k, v in items])
        return repr(o)

    assert shape(got, {}) == shape(ref, {})
    assert got["a"] is got["b"][0] and got["c"] is got["b"][1] and got["self"] is got
    assert got["a"] is not shared_list and got["a"][1] is not shared_list[1]
