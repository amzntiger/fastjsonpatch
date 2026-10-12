# Changelog

## Unreleased

- Python 3.10 and 3.11 are supported: the wheels now use the stable ABI from 3.10
  (`abi3-py310`), so one wheel per platform covers Python 3.10 and later. CI tests
  3.10 to 3.14.

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
