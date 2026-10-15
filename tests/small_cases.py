"""Small hand-written cases shared by tests/test_parity.py and scripts/make_golden.py.

The expected results are in tests/golden/parity.json, made from jsonpatch 1.35."""
import datetime

_WHEN = datetime.datetime(2026, 1, 1)

INVALID_PATCHES = [
    ["add"], [b"x"], [{"path": "/a"}], [{"op": 1, "path": "/a"}], [{"op": "nope", "path": "/a"}],
    [{"op": b"add", "path": "/a"}], [{"op": "add"}], [{"op": "add", "path": 3}], [{"op": "add", "path": "a"}],
    [{"op": "add", "path": "/~2"}], [{"op": "add", "path": "/a", "value": 1}, {"op": "x", "path": "/a"}],
]

# jsonpatch rejects a final "-" in replace even when it names a dict key
DASH_KEY = [
    ({"-": 1}, [{"op": "replace", "path": "/-", "value": 2}]),
    ({"a": {"-": 1}}, [{"op": "replace", "path": "/a/-", "value": 2}]),
    ([1], [{"op": "replace", "path": "/-", "value": 2}]),
    ({"-": 1}, [{"op": "remove", "path": "/-"}]),
    ({"-": 1}, [{"op": "add", "path": "/-", "value": 2}]),
    ({"-": 1}, [{"op": "test", "path": "/-", "value": 1}]),
    ({"-": 1}, [{"op": "move", "from": "/-", "path": "/b"}]),
    ({"-": 1}, [{"op": "copy", "from": "/-", "path": "/b"}]),
]

NON_JSON = [
    ({"a": 1}, {"a": 1, "t": _WHEN}),          # added non-JSON value: no comparison needed
    ({"t": _WHEN}, {"t": _WHEN}),              # compared: json.dumps raises TypeError
    ({"a": (1, 2)}, {"a": [1, 2]}),            # tuple vs list
    ({"a": (1, 2)}, {"a": (1, 3)}),
    ({"a": {1: "x"}}, {"a": {"1": "x"}}),      # non-str keys
    ({"a": [1, {(1,): 2}]}, {"a": [{"x": 1}, 1]}),
]
NON_JSON_ADD = [{"op": "add", "path": "/z", "value": _WHEN}]


def deep(n):
    src = dst = 0
    for _ in range(n):
        src, dst = [src], [dst]
    return src, dst


def as_jsonpatch(message):
    """An error message as jsonpatch 1.35 words it: the class of the end of an array is
    fastjsonpatch.pointer.EndOfList here and jsonpointer.EndOfList there."""
    return message.replace("fastjsonpatch.pointer.EndOfList", "jsonpointer.EndOfList")


def canon(value):
    """value with every float that has a whole value (1.0, -0.0, 1e20) as the int of that
    value: fastjsonpatch compares numbers by value (RFC 6902, 4.6), jsonpatch 1.35 does
    not, and jsonpatch gives fastjsonpatch's patch for canon(src), canon(dst)."""
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, dict):
        return {k: canon(v) for k, v in value.items()}
    if isinstance(value, list):
        return [canon(v) for v in value]
    return value


def has_whole_float(value):
    return repr(canon(value)) != repr(value)
