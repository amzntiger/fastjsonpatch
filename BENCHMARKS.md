# fastjsonpatch benchmarks

## Results

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

## Where the time goes

**make_patch.** jsonpatch walks both documents in Python and compares every scalar
with `json.dumps(a) == json.dumps(b)`. Profiling the "10k records" workload with
cProfile puts about 40% of the time in `json.dumps` and about a third in `isinstance`
checks against abstract base classes (`MutableMapping`, `MutableSequence`).
fastjsonpatch walks the documents in Rust over the CPython objects directly (no
conversion to a Rust data structure), compares values natively with the same rules as the `json.dumps` comparison (`1`, `1.0` and `True`
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
