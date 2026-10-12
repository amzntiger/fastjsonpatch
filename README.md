# fastjsonpatch

A drop-in replacement for [jsonpatch](https://github.com/stefankoegl/python-json-patch)
([RFC 6902](https://www.rfc-editor.org/rfc/rfc6902)) with `make_patch` and `apply`
written in Rust. It returns the same patches and documents as jsonpatch 1.34 and raises
exceptions with the same types and messages.

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

Requires CPython 3.10+. Wheels are published for Linux (x86_64, aarch64; glibc and
musl), macOS (arm64, x86_64) and Windows (x64). The only dependency is `jsonpointer`.

## Performance

Linux x86_64, Python 3.13, best of several runs:

| workload | jsonpatch | fastjsonpatch | speedup |
|---|---|---|---|
| `make_patch`, Kubernetes OpenAPI v1.33 → v1.34 (3.7 → 3.8 MB, 283 ops) | 136.4 ms | 9.6 ms | **14x** |
| `apply`, the same 283 ops | 78.0 ms | 11.4 ms | **7x** |
| `make_patch`, 10k records with 100 field edits | 292.61 ms | 5.26 ms | **56x** |
| `apply`, the same 100 ops | 89.38 ms | 7.90 ms | **11x** |
| `make_patch`, small REST resource ×1000 | 74.4 ms | 5.6 ms | **13x** |
| `apply_patch`, small REST resource ×1000 | 47.4 ms | 7.5 ms | **6x** |

Across all measured workloads, `make_patch` is 9-57x faster and `apply` 4-27x (4x is a
3-operation patch applied `in_place` to a 3.8 MB document, where no copy is made). On
Python 3.14, whose `copy.deepcopy` is faster, the `apply` speedup is smaller (about 6x
instead of 11x on the 10k-record workload). Full tables and an explanation of where
the time goes are in
[BENCHMARKS.md](https://github.com/amzntiger/fastjsonpatch/blob/main/BENCHMARKS.md);
`python bench/compare.py` (in the source distribution) measures on your machine.

## Compatibility

fastjsonpatch is jsonpatch 1.34's module with `JsonPatch.__init__` (validation),
`JsonPatch.from_diff` (`make_patch`) and `JsonPatch.apply` (`apply_patch`) routed to
Rust; every other public name is jsonpatch's own code. Inputs the Rust code does not
handle exactly fall back to jsonpatch's original code before anything is modified:
non-JSON values, dict or list subclasses, nesting deeper than 1000, a custom `dumps=`
or `json_dumper`, a custom `pointer_cls`, `operations` or `_get_operation` (for
`apply`), and operations that would fail.

Known differences:

- The exception classes are fastjsonpatch's own, so
  `except fastjsonpatch.JsonPatchConflict` does not catch an exception raised by the `jsonpatch` package, and vice versa.
- Values in a document that are not dicts, lists or JSON scalars are copied with
  `copy.deepcopy` one by one, so aliasing between such values is not preserved.
- `__version__` is fastjsonpatch's version; `__jsonpatch_version__` is `"1.34"`.
  jsonpatch's `__author__` and `__website__` are not defined.

## Testing

About 1,580 tests: randomized comparisons against jsonpatch 1.34 (patches, documents
and errors), jsonpatch's own test suite and doctests, and the
[json-patch-tests](https://github.com/json-patch/json-patch-tests) RFC 6902 suite.
CI runs them with Python 3.10-3.14 on Linux (x86_64, aarch64; glibc), macOS (arm64,
x86_64) and Windows (x64); the musl wheels are built but not tested in CI.

## License

BSD-3-Clause. The Python module and the diff algorithm come from jsonpatch
(Copyright (c) 2011 Stefan Kögl, BSD). Licenses of the Rust crates compiled into the
wheels are in `THIRD_PARTY_LICENSES.md`. Development and release notes:
[MAINTAINING.md](https://github.com/amzntiger/fastjsonpatch/blob/main/MAINTAINING.md).
