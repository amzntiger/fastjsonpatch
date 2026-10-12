# fastjsonpatch benchmarks

## Results

fastjsonpatch 0.2.0 vs jsonpatch 1.35, measured on Linux x86_64, Python 3.13 (times
are the best of several runs).

| scenario | size | jsonpatch | fastjsonpatch | speedup |
|---|---|---|---|---|
| Kubernetes OpenAPI v1.33 (3.7 MB) → v1.34 (3.8 MB): make_patch | 259 ops | 157.3 ms | 11.4 ms | **14x** |
| Kubernetes OpenAPI v1.33 → v1.34: apply | 259 ops | 54.8 ms | 7.6 ms | **7x** |
| 10k records, 100 field edits: make_patch | 100 ops | 163.3 ms | 10.9 ms | **15x** |
| 10k records, 2-op patch: apply | 2 ops | 57.7 ms | 5.5 ms | **10x** |
| REST resource, make_patch per request | ×1000 | 75.6 ms | 4.4 ms | **17x** |
| REST resource, apply_patch per request | ×1000 | 26.3 ms | 4.8 ms | **5x** |

The records are shaped like `{"id", "name", "score", "tags": [..], "meta": {..}}`; the
last four rows are `python bench/compare.py`. The tables for fastjsonpatch 0.1.x vs
jsonpatch 1.34 are in this file's history.

## Where the time goes

**make_patch.** jsonpatch walks both documents in Python and compares every scalar
with `json.dumps(a) == json.dumps(b)`. Before comparing two lists it serializes each
item with `json.dumps` and matches up equal items with `difflib.SequenceMatcher`.
fastjsonpatch gives equal items equal ids from a structural hash instead of
serializing them, skips the equal items at both ends without hashing them, and runs a
port of SequenceMatcher on the ids. Otherwise it walks the documents in Rust over the
CPython objects directly (no conversion to a Rust data structure), compares values
natively with the same rules as the `json.dumps` comparison (`1`, `1.0` and `True` are
all different, `-0.0 != 0.0`, NaN equals NaN), keeps the current path on a stack,
and takes a single lockstep pass when two dicts have the same keys in the same order.
It is a port of jsonpatch's `DiffBuilder`, so it produces the same operations in the
same order, including detected moves. Matching up list items costs time that 1.34
did not spend (the 10k-records `make_patch` took 5.3 ms with fastjsonpatch 0.1.4), but
the patches are shorter: an item inserted at the front of a list is one `add` instead
of a change to every item after it.

**apply.** jsonpatch first runs `copy.deepcopy` on the document (so the original is
not modified), then builds a `JsonPointer` and an operation object for each operation.
For a small patch on a large document the copy is nearly all of the time.
fastjsonpatch copies each dict and list with one C call and only recurses into values
that are themselves containers; it keeps `deepcopy`'s semantics (shared and cyclic
references are copied once) but skips the bookkeeping for containers referenced only
once. It then resolves the paths and runs the operations without creating
`JsonPointer` or operation objects.
