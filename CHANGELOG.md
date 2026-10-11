# Changelog

## 0.1.1 (2026-10-11)

Documentation only: the README now states only measured facts, and its link to
MAINTAINING.md works on PyPI. No code changes.

## 0.1.0 (2026-10-11)

First release. Behaves like jsonpatch 1.34 with `make_patch`, `apply`/`apply_patch`
and patch validation in Rust:

- `make_patch` 9-57x faster, `apply` 6-27x faster (see README).
- Standalone: depends only on `jsonpointer`.
- Wheels for Linux (glibc and musl, x86_64 and aarch64), macOS (arm64, x86_64) and
  Windows (x64); one abi3 wheel per platform covers Python 3.12+.
