# fastjsonpatch

A drop-in, faster [jsonpatch](https://github.com/stefankoegl/python-json-patch)
([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902) JSON Patch) for Python.
`make_patch` and `apply` run in Rust and return the **same patches and the same
documents, and raise exceptions with the same types and messages,** as jsonpatch 1.34.
In the benchmarks below, making a patch is 9-57x faster and applying one 6-27x faster.

```sh
pip install fastjsonpatch
```

```python
import fastjsonpatch as jsonpatch      # the only line you change

src = {"name": "web", "replicas": 3, "ports": [80, 443]}
dst = {"name": "web", "replicas": 5, "ports": [80, 443, 8080]}

patch = jsonpatch.make_patch(src, dst)
patch.patch
# [{'op': 'replace', 'path': '/replicas', 'value': 5},
#  {'op': 'add', 'path': '/ports/2', 'value': 8080}]

patch.apply(src) == dst                # True; src is not modified
jsonpatch.apply_patch(src, [{"op": "remove", "path": "/ports/0"}])
# {'name': 'web', 'replicas': 3, 'ports': [443]}
```

Python 3.12+ (CPython). Prebuilt wheels for Linux (x86_64 and aarch64, glibc and
musl), macOS (Apple Silicon and Intel) and Windows (x64); one wheel per platform covers
every Python from 3.12 on. The only dependency is `jsonpointer`.

## Why

JSON Patch describes a change to a JSON document as a list of operations (`add`,
`remove`, `replace`, `move`, `copy`, `test`). It is a media type for HTTP `PATCH`
requests (`application/json-patch+json`), Kubernetes accepts it
(`kubectl patch --type=json`), and it can store or send the difference between two
versions of a document.

jsonpatch is written in pure Python. In the measurements below:

- `make_patch` on a 10,000-record document takes 290 ms, and diffing two Kubernetes
  OpenAPI specs (3.7 MB and 3.8 MB) takes 136 ms.
- `apply` deep-copies the whole document first, so a 3-operation patch on the
  3.8 MB Kubernetes spec takes 71 ms, against 21 µs when applied in place.
- Applying or computing a patch on a small REST resource takes 47-74 µs per call.

fastjsonpatch keeps jsonpatch's behaviour exactly and moves these hot paths to Rust,
so existing code gets faster without changes.

## Speed

Measured on Linux x86_64, Python 3.13 (times are the best of several runs).

Real-world data:

| scenario | size | jsonpatch | fastjsonpatch | speedup |
|---|---|---|---|---|
| Kubernetes OpenAPI v1.33 (3.7 MB) → v1.34 (3.8 MB): make_patch | 283 ops | 136.4 ms | 9.6 ms | **14x** |
| Kubernetes OpenAPI v1.33 → v1.34: apply | 283 ops | 78.0 ms | 11.4 ms | **7x** |
| 3-op patch on the v1.34 spec | 3.8 MB | 71.1 ms | 10.1 ms | **7x** |
| 3-op patch on the v1.34 spec, `in_place=True` | 3.8 MB | 21.2 µs | 4.7 µs | **4x** |
| PyPI metadata snapshots (8k records): make_patch | 2,487 ops | 255.2 ms | 12.1 ms | **21x** |
| PyPI metadata snapshots: apply | 2,487 ops | 91.7 ms | 14.0 ms | **7x** |
| REST resource, apply_patch per request | ×1000 | 47.4 ms | 7.5 ms | **6x** |
| REST resource, make_patch per request | ×1000 | 74.4 ms | 5.6 ms | **13x** |

Synthetic records shaped like `{"id", "name", "score", "tags": [..], "meta": {..}}`:

| workload | ops | make_patch: jsonpatch | fastjsonpatch | speedup | apply: jsonpatch | fastjsonpatch | speedup |
|---|---|---|---|---|---|---|---|
| 10k records, 100 field edits | 100 | 292.61 ms | 5.26 ms | **56x** | 89.38 ms | 7.90 ms | **11x** |
| 10k records, insert 1 at front | 49911 | 669.77 ms | 42.78 ms | **16x** | 535.21 ms | 72.35 ms | **7x** |
| 10k records, delete 50 | 44900 | 636.53 ms | 40.46 ms | **16x** | 563.20 ms | 63.45 ms | **9x** |
| 10k records, move 20 | 26305 | 434.93 ms | 24.38 ms | **18x** | 325.64 ms | 41.03 ms | **8x** |
| 2k ints, insert 1 at front | 1 | 17.04 ms | 1.91 ms | **9x** | 581 µs | 21 µs | **27x** |
| small config (200 keys), 1 edit | 1 | 1.98 ms | 35 µs | **57x** | 464 µs | 32 µs | **15x** |

Python 3.14 made `copy.deepcopy` about 1.5x faster, so on 3.14 jsonpatch's `apply` on
a large document is faster too; on the "10k records, 100 field edits" workload the
`apply` speedup drops from about 11x to about 6x. The `make_patch` speedup stays about
the same. Run `python bench/compare.py` (from the source distribution) to measure on
your own machine.

## Why it is faster

**make_patch.** jsonpatch walks both documents in Python and compares every scalar
with `json.dumps(a) == json.dumps(b)`. Profiling the "10k records" workload with
cProfile puts about 40% of the time in `json.dumps` and about a third in `isinstance`
checks against abstract base classes (`MutableMapping`, `MutableSequence`). fastjsonpatch walks the documents in Rust over the
CPython objects directly (no conversion to a Rust data structure), compares values
natively with the same rules as the `json.dumps` comparison (`1`, `1.0` and `True`
are all different, `-0.0 != 0.0`, NaN equals NaN), keeps the current path on a stack,
and takes a single lockstep pass when two dicts have the same keys in the same order.
It is a port of jsonpatch's `DiffBuilder`, so it produces the same operations in the
same order, including detected moves.

**apply.** jsonpatch first runs `copy.deepcopy` on the document (so the original is
not modified), then builds a `JsonPointer` and an operation object for each operation.
For a small patch on a large document the copy is nearly all of the time.
fastjsonpatch copies each dict and list with one C call and only recurses into values
that are themselves containers; it keeps `deepcopy`'s semantics (shared and cyclic
references are copied once) but skips the bookkeeping for containers referenced only
once. It then resolves the paths and runs the operations without creating
`JsonPointer` or operation objects.

## Compatibility

fastjsonpatch is jsonpatch's own module (BSD licensed) with three methods routed to
Rust: `JsonPatch.__init__` (validation), `JsonPatch.from_diff` (`make_patch`) and
`JsonPatch.apply` (`apply_patch`). Everything else is jsonpatch's code, so the public
names are the same: `JsonPatch`, `make_patch`, `apply_patch`, `JsonPatch.from_string`,
the operation classes, `DiffBuilder`, `multidict`, and the exceptions
`JsonPatchException`, `InvalidJsonPatch`, `JsonPatchConflict`, `JsonPatchTestFailed`
(with the same hierarchy, and `JsonPointerException` from jsonpointer).

The Rust code handles the common case and hands anything else back to the original
Python code, so results never differ:

- **make_patch**: values it cannot reproduce exactly (non-JSON types that need
  comparing, dict or list subclasses, a custom `dumps=`, nesting deeper than 1000)
  are diffed by the original `DiffBuilder`.
- **apply**: works on plain dicts and lists. An operation that would fail, or that
  touches anything else, is run by the original operation classes before anything is
  mutated, so errors have the same type and message.
- `apply` and validation use the original code for a custom `pointer_cls`, a
  subclass with custom `operations` or an overridden `_get_operation`, and patches
  that are not lists or tuples. `make_patch` uses it for a custom `dumps=` or
  `json_dumper`.

Known differences:

- The exception classes belong to fastjsonpatch. An exception raised by the original
  `jsonpatch` package (for example inside another library that imports it) is not
  caught by `except fastjsonpatch.JsonPatchConflict`, and vice versa.
- A value that is not a dict, list or JSON scalar inside a document being applied to
  is copied with `copy.deepcopy` on its own, so aliasing between such values is not
  preserved.
- `__version__` is fastjsonpatch's version, and jsonpatch's `__author__` and
  `__website__` are not defined.

`fastjsonpatch.__jsonpatch_version__` says which jsonpatch release the behaviour
matches (currently `1.34`).

## How it is tested

About 1,570 tests, run on every supported OS and Python version:

- **Parity:** random documents, mutations and patches (valid and invalid) run through
  both fastjsonpatch and jsonpatch 1.34, comparing the resulting patches and documents
  by `repr` and errors by type and message. Also covers aliasing, cycles, non-JSON
  values, subclasses, deep nesting and every fallback path.
- **jsonpatch's own test suite** and doctests, run against fastjsonpatch.
- **RFC 6902 conformance:** the
  [json-patch-tests](https://github.com/json-patch/json-patch-tests) suite.

The source distribution includes the tests and the benchmark.

## Development

```sh
pip install maturin
maturin develop --release
pip install "jsonpatch==1.34" pytest   # jsonpatch is the reference for the parity tests
pytest tests
python bench/compare.py                # speed vs jsonpatch on this machine
```

See [MAINTAINING.md](https://github.com/amzntiger/fastjsonpatch/blob/main/MAINTAINING.md) for releases and upkeep.

## License

BSD-3-Clause. The Python module and the diff algorithm come from jsonpatch
(Copyright (c) 2011 Stefan Kögl, BSD). The licenses of the Rust crates compiled into
the wheels are in `THIRD_PARTY_LICENSES.md`.
