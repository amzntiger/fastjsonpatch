# Changelog

## Unreleased

- `import fastjsonpatch` takes about 11 ms instead of about 53 ms: `__version__` now
  comes from the Rust module instead of `importlib.metadata`, which is no longer
  imported. (jsonpatch 1.34 takes about 11 ms.)

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
