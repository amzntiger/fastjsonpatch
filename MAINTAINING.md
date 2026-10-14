# Maintaining fastjsonpatch

## What the package is

- `python/fastjsonpatch/__init__.py`: jsonpatch 1.35's module (BSD, notice kept), with
  `JsonPatch.__init__`, `from_diff` and `apply` routed to the Rust core when they can
  be reproduced exactly. Everything else is jsonpatch's code, unchanged.
- `python/fastjsonpatch/cli.py`: jsonpatch 1.35's `jsonpatch_cli.py`, installed as
  the `fastjsondiff` and `fastjsonpatch` commands.
- `python/fastjsonpatch/__init__.pyi`: type hints for the public API (with `py.typed`).
- `src/lib.rs`: the Rust core (PyO3). `diff` ports `DiffBuilder`; `apply` runs
  operations on plain dicts/lists and hands anything unusual back to the Python
  operation classes before mutating; `deepcopy` replaces `copy.deepcopy` for JSON.
- Runtime dependency: `jsonpointer` only. `jsonpatch` is a test dependency: it is the
  reference the parity tests compare against.

**The rule:** fastjsonpatch must return the same patches, documents and exceptions as
jsonpatch 1.35. A behaviour difference is a bug, unless it is listed in the README.

## Tests

```sh
maturin develop --release
pip install "jsonpatch==1.35" pytest
pytest tests                 # ~1650 tests, a few seconds
python bench/compare.py      # speed vs jsonpatch
```

- `tests/test_parity.py`: random documents, mutations and operations (valid and
  invalid), compared with jsonpatch by `repr` and by exception type and message.
- `tests/upstream/`: jsonpatch's own test suite and doctests, run against us.
- `tests/rfc6902/`: the json-patch-tests conformance suite (Apache-2.0).
- `scripts/test_linux_matrix.sh`: installs the built wheels in clean Docker images
  (Debian/Alpine, Python 3.12-3.14) and runs everything there.

CI runs the tests and the speed comparison on Linux x86_64/arm64, macOS arm64/x86_64
and Windows for Python 3.12, 3.13 and 3.14; the run's summary page has the table.

## Making changes

Every change goes through a pull request; nothing is pushed to `main` directly.

1. Branch from `main` (`fix/...`, `feat/...`, `chore/...`), commit, push the branch.
2. Open a pull request. CI runs on it; the `ci-ok` check passes only when every
   test, wheel and sdist job passed.
3. Review the diff on the pull request page, then merge (squash or merge commit).

`main` has a branch rule that requires a pull request and the `ci-ok` check. It does
not require an approving review: GitHub does not let the author approve their own
pull request, and the maintainer pushes with their own account.

## Versions

[Semantic versioning](https://semver.org): `MAJOR.MINOR.PATCH`.

| change | bump |
|---|---|
| bug fix, speed-up, dependency update, same behaviour | PATCH (0.1.0 → 0.1.1) |
| new feature, existing behaviour unchanged (e.g. an `optimize=` option) | MINOR (0.1 → 0.2) |
| existing behaviour changes, Python version dropped | MAJOR (after 1.0) |

While on 0.x the API may still change between minor versions. Go to 1.0 once the
package has had real-world use without behaviour bugs.

The published version comes from `pyproject.toml` (`[project] version`);
`fastjsonpatch.__version__` reads it from the installed package. Keep `Cargo.toml`
and `Cargo.lock` at the same version (they ship in the sdist).

## Releasing

1. In a pull request, update the version in `pyproject.toml`, `Cargo.toml` (and
   `Cargo.lock`) and `CHANGELOG.md`; merge it with a green `ci-ok`.
2. Optional dry run: Actions → CI → "Run workflow" with target `testpypi`, then
   `pip install -i https://test.pypi.org/simple/ --extra-index-url https://pypi.org/simple/ fastjsonpatch`.
   A version number can be uploaded to TestPyPI only once as well, so use a dev
   version (e.g. `0.1.1.dev1`) for dry runs.
3. On the merged `main` commit: `git tag v0.1.1 && git push origin v0.1.1`. CI checks
   that the tag equals the version and builds every wheel and the sdist; the publish
   job then waits for approval in the `pypi` environment (Actions → the run →
   Review deployments) and uploads to PyPI.

A version uploaded to PyPI can be yanked (hidden from new installs) but its number can
never be reused. Fix mistakes with a new PATCH release.

## Routine upkeep

- **New jsonpatch release:** install it, run the parity tests against it, port any
  behaviour changes (the Python module, `cli.py` and `src/lib.rs`), bump the test pin
  `jsonpatch==X` in `pyproject.toml`, and update `__jsonpatch_version__`. Update
  `__init__.pyi` for changed signatures and check it with
  `python -m mypy.stubtest fastjsonpatch` (with `ignore_missing_imports` for
  jsonpointer, which has no type hints).
- **New Python version:** the wheels use the stable ABI (abi3, Python ≥ 3.12), so they
  install on new Pythons without a rebuild. Add the version to the CI test matrix.
- **Dropping a Python version:** change `requires-python` and the `abi3-py3XX`
  feature in `Cargo.toml` together, in a MINOR (0.x) or MAJOR release.
- **Rust dependencies (PyO3, ahash):** `cargo update`, run the tests, then
  `python scripts/third_party_licenses.py` to refresh `THIRD_PARTY_LICENSES.md`.
- **Bug reports:** reproduce with jsonpatch first. If jsonpatch behaves the same,
  it is not our bug; if not, add the case to `tests/test_parity.py` before fixing.

## Licenses

- Package: BSD-3-Clause (`LICENSE`), keeping jsonpatch's copyright notice because
  the Python module and the diff algorithm come from it.
- Wheels include `THIRD_PARTY_LICENSES.md` for the Rust crates compiled into the
  extension (all MIT/Apache-2.0 dual licensed).
- `tests/rfc6902/` data: Apache-2.0, text in that folder; ships in the sdist only.
