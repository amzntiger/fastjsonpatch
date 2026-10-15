# Changelog

## 0.3.0 (2026-10-14)

- `fastjsondiff` and `fastjsonpatch` commands: jsonpatch's `jsondiff` and `jsonpatch`,
  renamed so that they do not clash with jsonpatch's when both are installed.
- Type hints for the public API (`py.typed`).

## 0.2.0 (2026-10-12)

Now matches jsonpatch 1.35 instead of 1.34: the same patches, documents and exceptions.
Code written against 0.1.x can see different results:

- `make_patch` matches up equal items of two lists before comparing them, so a value
  inserted or removed in the middle of a list is one `add` or `remove` instead of a
  change to every item after it (e.g. 259 instead of 283 operations for the
  Kubernetes OpenAPI v1.33 → v1.34 diff). Moves are detected by JSON value, so `1`
  is not moved where `true` is added, and objects match regardless of key order.
- `test` compares like RFC 6902: `1` and `true` are different.
- `replace` with the path `/-` on an object replaces its `"-"` member.
- `add` with the path `""` replaces any document, not only objects and arrays.
- New error messages for removing the whole document, moving an array item into
  itself or its children, `from` ending in `-` for an array, a `from` that is not a
  string, and an operation that is not an object.
- Operations with arrays or objects as values can be hashed; operations with `1` and
  `true` as values are no longer equal.
- Requires `jsonpointer` 3.2 or later (like jsonpatch 1.35).
- `make_patch` is slower than in 0.1.4 on large lists (10.9 ms instead of 5.3 ms for
  10k records with 100 edits), as matching up items takes time; it is still about
  15x faster than jsonpatch 1.35.

## 0.1.4 (2026-10-12)

- `import fastjsonpatch` takes about 11 ms instead of about 53 ms: `__version__` now
  comes from the Rust module instead of `importlib.metadata`, which is no longer
  imported. (jsonpatch 1.34 takes about 11 ms.)
- CI: the macOS and Windows test jobs installed the wheel from PyPI instead of the
  one built in the job whenever both had the same version; they now install the
  built wheel by path. No change to the package.

## 0.1.3 (2026-10-12)

- Fix: `replace` with a path ending in `-` that names a dict key (`{"-": 1}`,
  path `/-`) was applied; jsonpatch 1.34 raises `InvalidJsonPatch` for it, and
  fastjsonpatch now does too.
- Shorter README; full benchmark tables moved to BENCHMARKS.md.

## 0.1.2 (2026-10-11)

Documentation only: the README now states only measured facts (including exactly
which platforms CI tests), and its link to MAINTAINING.md works on PyPI. No code
changes. (0.1.1 was tagged but not published.)

## 0.1.0 (2026-10-11)

First release. Behaves like jsonpatch 1.34 with `make_patch`, `apply`/`apply_patch`
and patch validation in Rust:

- `make_patch` 9-57x faster, `apply` 4-27x faster (see BENCHMARKS.md).
- Standalone: depends only on `jsonpointer`.
- Wheels for Linux (glibc and musl, x86_64 and aarch64), macOS (arm64, x86_64) and
  Windows (x64); one abi3 wheel per platform covers Python 3.12+.
