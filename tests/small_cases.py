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


# Calls on the API (`m` is the module: jsonpatch to make tests/golden/parity.json,
# fastjsonpatch in the tests) whose result or error is compared.
PROBES = {
    "from_string bad json": lambda m: m.JsonPatch.from_string("{"),
    "from_string non-list": lambda m: m.JsonPatch.from_string('{"op":"add"}').patch,
    "apply str doc": lambda m: m.apply_patch("abc", [{"op": "replace", "path": "", "value": 1}]),
    "apply None doc": lambda m: m.apply_patch(None, [{"op": "add", "path": "", "value": 1}]),
    "empty patch": lambda m: m.apply_patch({"a": 1}, []),
    "patch tuple": lambda m: m.apply_patch({"a": 1}, ({"op": "add", "path": "/b", "value": 2},)),
    "generator patch": lambda m: m.apply_patch({"a": 1}, (x for x in [{"op": "add", "path": "/b", "value": 2}])),
    "bytes patch": lambda m: m.apply_patch({"a": 1}, b'[{"op":"add","path":"/b","value":2}]'),
    "to_string": lambda m: m.make_patch({"a": 1}, {"a": 2}).to_string(),
    "eq": lambda m: m.make_patch({"a": 1}, {"a": 2}) == m.make_patch({"a": 1}, {"a": 2}),
    "ne": lambda m: m.make_patch({"a": 1}, {"a": 2}) != m.make_patch({"a": 1}, {"a": 3}),
    "hash": lambda m: hash(m.make_patch({"a": 1}, {"a": 2})) == hash(m.make_patch({"a": 1}, {"a": 2})),
    "add list append": lambda m: m.apply_patch({"l": [1]}, [{"op": "add", "path": "/l/-", "value": 2}]),
    "add list mid": lambda m: m.apply_patch({"l": [1, 3]}, [{"op": "add", "path": "/l/1", "value": 2}]),
    "add past end": lambda m: m.apply_patch({"l": [1]}, [{"op": "add", "path": "/l/3", "value": 2}]),
    "remove negative": lambda m: m.apply_patch({"l": [1]}, [{"op": "remove", "path": "/l/-1"}]),
    "test ok": lambda m: m.apply_patch({"a": [1, {"b": 2}]}, [{"op": "test", "path": "/a", "value": [1, {"b": 2}]}]),
    "test float": lambda m: m.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 1.0}]),
    "test bool": lambda m: m.apply_patch({"a": 1}, [{"op": "test", "path": "/a", "value": True}]),
    "copy then change": lambda m: m.apply_patch(
        {"a": {"x": [1]}}, [{"op": "copy", "from": "/a", "path": "/b"}, {"op": "add", "path": "/b/x/-", "value": 2}]),
    "move into list": lambda m: m.apply_patch({"a": 1, "l": [0]}, [{"op": "move", "from": "/a", "path": "/l/0"}]),
    "unicode key": lambda m: m.apply_patch({"é": 1}, [{"op": "replace", "path": "/é", "value": 2}]),
    "escaped keys": lambda m: m.apply_patch(
        {"a/b": 1, "c~d": 2},
        [{"op": "replace", "path": "/a~1b", "value": 9}, {"op": "replace", "path": "/c~0d", "value": 8}]),
    "empty key": lambda m: m.apply_patch({"": 1}, [{"op": "replace", "path": "/", "value": 2}]),
    "make_patch scalars": lambda m: m.make_patch(1, 2).patch,
    "make_patch list top": lambda m: m.make_patch([1, 2], [2, 1]).patch,
    "make_patch None": lambda m: m.make_patch(None, {"a": 1}).patch,
    "dict subclass": lambda m: m.make_patch({"a": _D(x=1)}, {"a": _D(x=2)}).patch,
    "list subclass": lambda m: m.make_patch({"a": _L([1, 2])}, {"a": _L([2, 1])}).patch,
    "ordered dict": lambda m: m.make_patch(_OD(a=1, b=2), _OD(b=2, a=1)).patch,
    "decimal": lambda m: m.make_patch({"a": _DEC("1.5")}, {"a": _DEC("2.5")}).patch,
    "bytes value": lambda m: m.make_patch({"a": b"x"}, {"a": b"y"}).patch,
    "huge int": lambda m: m.make_patch({"a": 10**5000}, {"a": 10**5000 + 1}).patch,
}


class _D(dict):
    pass


class _L(list):
    pass


import collections  # noqa: E402
import decimal  # noqa: E402

_OD = collections.OrderedDict
_DEC = decimal.Decimal
