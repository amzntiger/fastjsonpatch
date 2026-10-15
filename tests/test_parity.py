"""fastjsonpatch must behave exactly like jsonpatch 1.35: same patches, documents, errors.

The expected results are stored in tests/golden/parity.json, made by
scripts/make_golden.py from jsonpatch 1.35; these tests do not need jsonpatch."""
import copy
import json
import pathlib
import subprocess
import sys

import pytest

import fastjsonpatch
import small_cases

GOLDEN = json.loads((pathlib.Path(__file__).parent / "golden" / "parity.json").read_text(encoding="utf-8"))


def outcome(fn):
    """["ok", repr(result)], or [exception class name, message]."""
    try:
        return ["ok", repr(fn())]
    except Exception as e:  # noqa: BLE001 - comparing any failure
        return [type(e).__name__, str(e)]


@pytest.mark.parametrize("case", GOLDEN["make_patch"], ids=lambda c: None)
def test_make_patch_and_apply(case):
    src, dst = case["src"], case["dst"]
    got = fastjsonpatch.make_patch(src, dst)
    assert repr(got.patch) == case["patch"]
    assert type(got) is fastjsonpatch.JsonPatch
    snapshot = repr(src)
    assert outcome(lambda: got.apply(src)) == case["apply"]
    assert repr(src) == snapshot  # not in place: src untouched
    doc = copy.deepcopy(src)
    assert outcome(lambda: got.apply(doc, in_place=True)) == case["in_place"]
    assert repr(doc) == case["in_place_doc"]


@pytest.mark.parametrize("case", GOLDEN["operations"], ids=lambda c: None)
def test_operations(case):
    doc, ops = case["doc"], case["ops"]
    assert outcome(lambda: fastjsonpatch.apply_patch(doc, copy.deepcopy(ops))) == case["outcome"]
    target = copy.deepcopy(doc)
    assert outcome(lambda: fastjsonpatch.apply_patch(target, copy.deepcopy(ops), in_place=True)) == case["in_place"]
    assert repr(target) == case["in_place_doc"]


def test_one_op_at_a_time():
    """Each op alone, so failing ops early in a patch don't hide later ones."""
    for case in GOLDEN["single_ops"]:
        doc = copy.deepcopy(SINGLE_DOC)
        got = outcome(lambda: fastjsonpatch.apply_patch(doc, [copy.deepcopy(case["op"])], in_place=True))
        assert got == case["outcome"], case["op"]
        assert repr(doc) == case["doc_after"], case["op"]


SINGLE_DOC = {"a": [1, {"b": [2, 3]}, "s"], "c": {"d": None, "e~f": {"/": 1}}, "l": [[0], []]}


@pytest.mark.parametrize("patch, expected", zip(small_cases.INVALID_PATCHES, GOLDEN["invalid_patches"]),
                         ids=range(len(small_cases.INVALID_PATCHES)))
def test_invalid_patches(patch, expected):
    assert outcome(lambda: fastjsonpatch.JsonPatch(patch)) == expected


@pytest.mark.parametrize("case, expected", zip(small_cases.DASH_KEY, GOLDEN["dash_key"]),
                         ids=range(len(small_cases.DASH_KEY)))
def test_dash_key(case, expected):
    doc, patch = case
    for in_place, want in zip((False, True), expected):
        target = copy.deepcopy(doc)
        assert outcome(lambda: fastjsonpatch.apply_patch(target, patch, in_place=in_place)) == want["outcome"]
        assert repr(target) == want["doc_after"]


def test_exceptions_have_the_same_hierarchy():
    with pytest.raises(fastjsonpatch.JsonPatchConflict):
        fastjsonpatch.apply_patch({}, [{"op": "remove", "path": "/a"}])
    with pytest.raises(fastjsonpatch.JsonPatchTestFailed):
        fastjsonpatch.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 2}])
    with pytest.raises(AssertionError):  # JsonPatchTestFailed is also an AssertionError
        fastjsonpatch.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 2}])
    with pytest.raises(fastjsonpatch.JsonPointerException):
        fastjsonpatch.apply_patch({}, [{"op": "add", "path": "/a/b", "value": 1}])
    assert _mro(fastjsonpatch.JsonPatchException) == ["JsonPatchException", "Exception", "BaseException", "object"]
    assert _mro(fastjsonpatch.InvalidJsonPatch) == ["InvalidJsonPatch", "JsonPatchException", "Exception",
                                                    "BaseException", "object"]
    assert _mro(fastjsonpatch.JsonPatchConflict) == ["JsonPatchConflict", "JsonPatchException", "Exception",
                                                     "BaseException", "object"]
    assert _mro(fastjsonpatch.JsonPatchTestFailed) == ["JsonPatchTestFailed", "JsonPatchException", "AssertionError",
                                                       "Exception", "BaseException", "object"]


def _mro(cls):
    return [c.__name__ for c in cls.__mro__]


def test_public_api_matches_jsonpatch():
    assert set(GOLDEN["public_names"]) <= set(dir(fastjsonpatch))


def test_standalone_does_not_import_jsonpatch():
    code = "import sys, fastjsonpatch; fastjsonpatch.make_patch({}, {'a': 1}); print('jsonpatch' in sys.modules)"
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True)
    assert out.stdout.strip() == "False"


def test_version_matches_the_installed_package():
    # __version__ comes from Cargo.toml; the wheel's version from pyproject.toml.
    import importlib.metadata
    assert fastjsonpatch.__version__ == importlib.metadata.version("fastjsonpatch")


_SPY_IMPORTLIB_METADATA = """
import sys, traceback
preloaded = "importlib.metadata" in sys.modules
for name in [m for m in sys.modules if m == "importlib.metadata" or m.startswith("importlib.metadata.")]:
    del sys.modules[name]
importers = []

class Spy:
    def find_spec(self, name, path=None, target=None):
        if name == "importlib.metadata":
            importers.append("".join(traceback.format_stack(limit=15)))

sys.meta_path.insert(0, Spy())
import fastjsonpatch
print("preloaded:", preloaded)
print(importers[0] if importers else "")
"""


def test_import_does_not_load_importlib_metadata():
    # importlib.metadata alone used to add ~30 ms to `import fastjsonpatch`. Drop it
    # from sys.modules first, in case the interpreter loaded it at startup, and
    # record who imports it again.
    out = subprocess.run([sys.executable, "-I", "-c", _SPY_IMPORTLIB_METADATA],
                         capture_output=True, text=True, check=True)
    preloaded, _, importer = out.stdout.partition("\n")
    assert not importer.strip(), f"{preloaded}; importlib.metadata imported by:\n{importer}"


def test_aliasing_preserved_like_deepcopy():
    shared = [1, 2]
    doc = {"a": shared, "b": shared}
    out = fastjsonpatch.apply_patch(doc, [{"op": "add", "path": "/a/-", "value": 3}])
    assert out == {"a": [1, 2, 3], "b": [1, 2, 3]}  # as with jsonpatch: copy.deepcopy keeps the sharing
    assert doc == {"a": [1, 2], "b": [1, 2]}


@pytest.mark.parametrize("case, expected", zip(small_cases.NON_JSON, GOLDEN["non_json"]),
                         ids=range(len(small_cases.NON_JSON)))
def test_non_json_values(case, expected):
    src, dst = case
    assert outcome(lambda: fastjsonpatch.make_patch(src, dst).patch) == expected["make_patch"]
    assert outcome(lambda: fastjsonpatch.apply_patch(src, small_cases.NON_JSON_ADD)) == expected["add"]


def test_custom_dumps_and_subclass_fall_back():
    src, dst = {"a": 1}, {"a": 1.0}
    dumps = lambda o: json.dumps(o, sort_keys=True)  # noqa: E731
    assert fastjsonpatch.JsonPatch.from_diff(src, dst, dumps=dumps).patch == \
        [{"op": "replace", "path": "/a", "value": 1.0}]

    class Mine(fastjsonpatch.JsonPatch):
        pass
    p = Mine.from_diff({"a": [1, 2]}, {"a": [2]})
    assert type(p) is Mine and p.patch == [{"op": "remove", "path": "/a/0"}]


def test_deep_nesting_falls_back():
    src, _ = small_cases.deep(3000)
    assert outcome(lambda: len(fastjsonpatch.make_patch(src, [1]).patch)) == GOLDEN["deep"]


def test_string_patch_and_equality():
    s = json.dumps([{"op": "add", "path": "/a", "value": 1}])
    assert fastjsonpatch.apply_patch({}, s) == {"a": 1}
    assert fastjsonpatch.JsonPatch.from_string(s) == fastjsonpatch.JsonPatch.from_string(s)
    assert fastjsonpatch.JsonPatch.from_string(s).patch == [{"op": "add", "path": "/a", "value": 1}]
    assert str(fastjsonpatch.make_patch({}, {"a": 1})) == '[{"op": "add", "path": "/a", "value": 1}]'


@pytest.mark.parametrize("case", GOLDEN["shuffles"], ids=lambda c: None)
def test_shuffles_exercise_move_detection(case):
    got = fastjsonpatch.make_patch(case["src"], case["dst"])
    assert repr(got.patch) == case["patch"]
    assert outcome(lambda: got.apply(case["src"])) == case["apply"]


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
