# fastjsonpatch

A drop-in replacement for [jsonpatch](https://github.com/stefankoegl/python-json-patch)
([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902)) with `make_patch` and `apply`
written in Rust. It returns the same patches and documents as jsonpatch 1.35 and raises
exceptions with the same types and messages. (fastjsonpatch 0.1.x matches jsonpatch 1.34.)

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

The package includes type hints (`py.typed`) and jsonpatch's `jsondiff` and
`jsonpatch` commands as `fastjsondiff` and `fastjsonpatch`, renamed so that they do
not clash with jsonpatch's when both are installed:

```sh
fastjsondiff old.json new.json > patch.json    # exits with 1 if the files differ
fastjsonpatch old.json patch.json              # or -i to modify old.json in place
```

Requires CPython 3.12+. Wheels are published for Linux (x86_64, aarch64; glibc and
musl), macOS (arm64, x86_64) and Windows (x64). There are no dependencies.

## Performance

fastjsonpatch 0.2.0 vs jsonpatch 1.35, Linux x86_64, Python 3.13, best of several runs:

| workload | jsonpatch | fastjsonpatch | speedup |
|---|---|---|---|
| `make_patch`, Kubernetes OpenAPI v1.33 → v1.34 (3.7 → 3.8 MB, 259 ops) | 157.3 ms | 11.4 ms | **14x** |
| `apply`, the same 259 ops | 54.8 ms | 7.6 ms | **7x** |
| `make_patch`, 10k records with 100 field edits | 163.3 ms | 10.9 ms | **15x** |
| `apply`, a 2-op patch to the 10k records | 57.7 ms | 5.5 ms | **10x** |
| `make_patch`, small REST resource ×1000 | 75.6 ms | 4.4 ms | **17x** |
| `apply_patch`, small REST resource ×1000 | 26.3 ms | 4.8 ms | **5x** |

How the time is spent is explained in
[BENCHMARKS.md](https://github.com/amzntiger/fastjsonpatch/blob/main/BENCHMARKS.md);
`python bench/compare.py` (in the source distribution) measures on your machine.

## Compatibility

fastjsonpatch is jsonpatch 1.35's module with `JsonPatch.__init__` (validation),
`JsonPatch.from_diff` (`make_patch`) and `JsonPatch.apply` (`apply_patch`) routed to
Rust; every other public name is jsonpatch's own code. Inputs the Rust code does not
handle exactly fall back to jsonpatch's original code before anything is modified:
non-JSON values, dict or list subclasses, nesting deeper than 1000, a custom `dumps=`
or `json_dumper`, a custom `pointer_cls`, `operations` or `_get_operation` (for
`apply`), and operations that would fail.

Known differences (see [MIGRATION.md](https://github.com/amzntiger/fastjsonpatch/blob/main/MIGRATION.md)):

- `make_patch` compares numbers by value, as RFC 6902 (4.6) does: `1`, `1.0` and `-0.0`
  are the numbers 1, 1 and 0, so `{"a": 1}` to `{"a": 1.0}` is an empty patch (jsonpatch
  makes a `replace`). `1` and `true` are still different.
- `JsonPointerException` is fastjsonpatch's own class, not jsonpointer's (there is no
  dependency on jsonpointer), and the error message for a pointer through the end of an
  array names `fastjsonpatch.pointer.EndOfList`.
- The exception classes are fastjsonpatch's own, so
  `except fastjsonpatch.JsonPatchConflict` does not catch an exception raised by the `jsonpatch` package, and vice versa.
- Values in a document that are not dicts, lists or JSON scalars are copied with
  `copy.deepcopy` one by one, so aliasing between such values is not preserved.
- `__version__` is fastjsonpatch's version; `__jsonpatch_version__` is `"1.35"`.
  jsonpatch's `__author__` and `__website__` are not defined.

## Testing

About 2,200 tests: comparisons with what jsonpatch 1.35 returned for random and
hand-written inputs (patches, documents and errors; stored, so jsonpatch is not needed), jsonpatch's own test suite (its command-line tests run against
`fastjsondiff` and `fastjsonpatch`) and doctests, and the
[json-patch-tests](https://github.com/json-patch/json-patch-tests) RFC 6902 suite.
CI runs them with Python 3.12-3.14 on Linux (x86_64, aarch64; glibc), macOS (arm64,
x86_64) and Windows (x64); the musl wheels are built but not tested in CI.

## License

BSD-3-Clause. The Python module and the diff algorithm come from jsonpatch
(Copyright (c) 2011 Stefan Kögl, BSD). Licenses of the Rust crates compiled into the
wheels are in `THIRD_PARTY_LICENSES.md`. Development and release notes:
[MAINTAINING.md](https://github.com/amzntiger/fastjsonpatch/blob/main/MAINTAINING.md).
