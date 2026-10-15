"""Make or check tests/golden/*.json: what jsonpatch 1.35 returns for the test inputs.

    python scripts/make_golden.py          # new random inputs, results from jsonpatch
    python scripts/make_golden.py --check  # stored inputs, results from jsonpatch must
                                           # equal the stored ones (no random numbers)

The tests compare fastjsonpatch with these files and do not need jsonpatch. Needs
`pip install jsonpatch==1.35`."""
import copy
import json
import pathlib
import random
import sys

import jsonpatch
import jsonpointer

ROOT = pathlib.Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "tests" / "golden"
sys.path[:0] = [str(ROOT / "tests"), str(ROOT / "tests" / "rfc6902")]
import small_cases  # noqa: E402
import test_rfc6902  # noqa: E402

KEYS = ["a", "b", "c", "x/y", "t~1", "", "0", "1", "émoji😀"]
SCALARS = [0, 1, -1, 2, 1.0, 0.0, -0.0, 1.5, float("nan"), float("inf"), True, False, None,
           "", "a", "b", "1", "x/y", "~0", 2**70]
SINGLE_DOC = {"a": [1, {"b": [2, 3]}, "s"], "c": {"d": None, "e~f": {"/": 1}}, "l": [[0], []]}


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
            value = jsonpointer.JsonPointer(op["path"]).resolve(doc)
            if not isinstance(value, jsonpointer.EndOfList):  # the end of an array: not JSON
                op["value"] = value
        except Exception:  # noqa: BLE001
            pass
    if name in ("move", "copy") and rng.random() < 0.95:
        op["from"] = rand_pointer(rng, doc)
    if rng.random() < 0.02:
        del op["path"]
    return op


def as_container(rng, v):
    return v if isinstance(v, (dict, list)) else {"root": v}


def outcome(fn):
    """["ok", repr(result)], or [exception class name, message]."""
    try:
        return ["ok", repr(fn())]
    except Exception as e:  # noqa: BLE001 - recording any failure
        return [type(e).__name__, str(e)]


def make_patch_case(src, dst):
    ref = jsonpatch.make_patch(src, dst)
    in_place = copy.deepcopy(src)
    return {
        "src": src, "dst": dst, "patch": repr(ref.patch),
        "apply": outcome(lambda: ref.apply(src)),
        "in_place": outcome(lambda: ref.apply(in_place, in_place=True)),
        "in_place_doc": repr(in_place),
    }


def operations_case(doc, ops):
    a = copy.deepcopy(doc)
    return {
        "doc": doc, "ops": ops,
        "outcome": outcome(lambda: jsonpatch.apply_patch(doc, copy.deepcopy(ops))),
        "in_place": outcome(lambda: jsonpatch.apply_patch(a, copy.deepcopy(ops), in_place=True)),
        "in_place_doc": repr(a),
    }


def single_op_case(op):
    doc = copy.deepcopy(SINGLE_DOC)
    return {"op": op, "outcome": outcome(lambda: jsonpatch.apply_patch(doc, [copy.deepcopy(op)], in_place=True)),
            "doc_after": repr(doc)}


def shuffle_case(src, dst):
    ref = jsonpatch.make_patch(src, dst)
    return {"src": src, "dst": dst, "patch": repr(ref.patch), "apply": outcome(lambda: ref.apply(src))}


def invalid_patch_outcome(patch):
    return outcome(lambda: jsonpatch.JsonPatch(patch))


def dash_key_case(doc, patch):
    out = []
    for in_place in (False, True):
        d = copy.deepcopy(doc)
        out.append({"outcome": outcome(lambda: jsonpatch.apply_patch(d, patch, in_place=in_place)),
                    "doc_after": repr(d)})
    return out


def non_json_case(src, dst):
    return {"make_patch": outcome(lambda: jsonpatch.make_patch(src, dst).patch),
            "add": outcome(lambda: jsonpatch.apply_patch(src, small_cases.NON_JSON_ADD))}


def deep_outcome(n):
    src, _ = small_cases.deep(n)
    return outcome(lambda: len(jsonpatch.make_patch(src, [1]).patch))


def rfc_outcome(case):
    return outcome(lambda: jsonpatch.apply_patch(copy.deepcopy(case["doc"]), copy.deepcopy(case["patch"])))


def public_names():
    return sorted(n for n in dir(jsonpatch) if not n.startswith("__"))


def new_inputs():
    data = {"make_patch": [], "operations": [], "shuffles": [], "single_ops": []}
    for seed in range(400):
        rng = random.Random(seed)
        src = as_container(rng, rand_value(rng))
        data["make_patch"].append((src, mutate(rng, src)))
    for seed in range(600):
        rng = random.Random(seed)
        doc = as_container(rng, rand_value(rng))
        data["operations"].append((doc, [rand_op(rng, doc) for _ in range(rng.randrange(1, 6))]))
    for seed in range(3000):
        data["single_ops"].append(rand_op(random.Random(seed), copy.deepcopy(SINGLE_DOC)))
    for seed in range(300):
        rng = random.Random(seed)
        pool = ([rng.choice(SCALARS[:12]) for _ in range(6)] + [{"k": i % 3} for i in range(4)]
                + [[i % 2] for i in range(3)])
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
        data["shuffles"].append(({"l": src, "n": {"m": list(reversed(src))}}, {"l": dst, "n": {"m": dst[::2]}}))
    # What is stored is what the tests read back: no shared lists or dicts (JSON has none).
    return json.loads(json.dumps(data))


def build(inputs):
    return {
        "jsonpatch": "1.35",
        "make_patch": [make_patch_case(s, d) for s, d in inputs["make_patch"]],
        "operations": [operations_case(d, o) for d, o in inputs["operations"]],
        "single_ops": [single_op_case(o) for o in inputs["single_ops"]],
        "shuffles": [shuffle_case(s, d) for s, d in inputs["shuffles"]],
        "invalid_patches": [invalid_patch_outcome(p) for p in small_cases.INVALID_PATCHES],
        "dash_key": [dash_key_case(d, p) for d, p in small_cases.DASH_KEY],
        "non_json": [non_json_case(s, d) for s, d in small_cases.NON_JSON],
        "deep": deep_outcome(3000),
        "public_names": public_names(),
    }


def stored_inputs(data):
    return {
        "make_patch": [(c["src"], c["dst"]) for c in data["make_patch"]],
        "operations": [(c["doc"], c["ops"]) for c in data["operations"]],
        "single_ops": [c["op"] for c in data["single_ops"]],
        "shuffles": [(c["src"], c["dst"]) for c in data["shuffles"]],
    }


def rfc_build():
    return [rfc_outcome(case) for case, _ in test_rfc6902.CASES]


def dump(path, data):
    path.write_text(json.dumps(data, separators=(",", ":"), ensure_ascii=False) + "\n", encoding="utf-8")


def main():
    parity, rfc = GOLDEN / "parity.json", GOLDEN / "rfc6902.json"
    if "--check" in sys.argv:
        stored = json.loads(parity.read_text(encoding="utf-8"))
        now = build(stored_inputs(stored))
        bad = [k for k in now if repr(now[k]) != repr(stored[k])]
        if repr(rfc_build()) != repr(json.loads(rfc.read_text(encoding="utf-8"))):
            bad.append("rfc6902")
        if bad:
            sys.exit(f"jsonpatch {jsonpatch.__version__ if hasattr(jsonpatch, '__version__') else ''} "
                     f"disagrees with the stored results: {', '.join(bad)}")
        print("golden files match jsonpatch")
        return
    GOLDEN.mkdir(exist_ok=True)
    dump(parity, build(new_inputs()))
    dump(rfc, rfc_build())
    print("wrote", parity, rfc)


if __name__ == "__main__":
    main()
