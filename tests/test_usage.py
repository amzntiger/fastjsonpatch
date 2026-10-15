"""Using the package the way programs do: threads, big and deep documents, many calls."""
import copy
import json
import threading

import fastjsonpatch
import small_cases


def as_text(value):
    return json.dumps(small_cases.canon(value), sort_keys=True)


def test_threads_share_documents():
    doc = {"items": [{"id": i, "v": [i, i + 1]} for i in range(200)], "meta": {"x": 1}}
    dst = copy.deepcopy(doc)
    dst["items"].insert(5, {"id": -1, "v": []})
    del dst["items"][100]
    dst["meta"]["x"] = 2.5
    expected = repr(fastjsonpatch.make_patch(doc, dst).patch)
    errors = []

    def work():
        for _ in range(150):
            patch = fastjsonpatch.make_patch(doc, dst)
            if repr(patch.patch) != expected:
                errors.append("patch")
            if as_text(patch.apply(doc)) != as_text(dst):
                errors.append("document")

    threads = [threading.Thread(target=work) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert errors == []


def nested(depth, leaf):
    doc = cur = {}
    for _ in range(depth):
        cur["n"] = {}
        cur = cur["n"]
    if leaf:
        cur["leaf"] = 1
    return doc


def test_deep_documents():
    # jsonpatch raises RecursionError from about 500 levels; here a document that deep is
    # diffed and patched, and at any depth the result is the patch or a RecursionError.
    for depth in (10, 400, 900):
        src, dst = nested(depth, False), nested(depth, True)
        try:
            patch = fastjsonpatch.make_patch(src, dst)
        except RecursionError:
            assert depth > 400
            continue
        assert len(patch.patch) == 1 and patch.patch[0]["op"] == "add"
        assert patch.apply(src) == dst


def test_large_document_round_trip():
    src = {"users": [{"id": i, "name": f"u{i}", "tags": ["a", "b"], "addr": {"zip": i}} for i in range(20000)]}
    dst = copy.deepcopy(src)
    for i in range(0, 20000, 97):
        dst["users"][i]["name"] += "x"
    dst["users"].insert(100, {"id": -1})
    del dst["users"][15000]
    patch = fastjsonpatch.make_patch(src, dst)
    assert as_text(patch.apply(src)) == as_text(dst)
    assert len(patch.patch) < 250


def test_many_calls_do_not_change_the_inputs():
    src = {"a": [1, {"b": [2, 3]}], "c": {"d": None}}
    dst = {"a": [{"b": [3, 2]}, 1], "c": {"d": 1.0, "e": []}}
    snapshot = (repr(src), repr(dst))
    for _ in range(2000):
        patch = fastjsonpatch.make_patch(src, dst)
        patch.apply(src)
        fastjsonpatch.apply_patch(src, patch.patch)
    assert (repr(src), repr(dst)) == snapshot
