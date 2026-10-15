# Roadmap

Direction: stayed compatible with jsonpatch through 0.x (done, 0.3.0), becomes an
independent library at 1.0. Dates are not promised; the order is.

## 0.x: compatible with jsonpatch

The rule in `MAINTAINING.md` stands: same patches, documents and exceptions as
jsonpatch 1.35 (or the newest jsonpatch we have ported). A difference is a bug unless
the README lists it.

- Follow new jsonpatch releases: port behaviour changes, bump the test pin.
- Move more of the hot paths into Rust where the result can be reproduced exactly.
- Keep the API stable between PATCH releases; MINOR releases may change it (0.x).
- Collect what we would change if we were free to (behaviour we copy only because
  jsonpatch does) in issues labelled `1.0`. Nothing from that list lands in 0.x.

Done when: we have used it long enough without behaviour bugs to commit to 1.0.

## 1.0: independent

1.0 is the first release that may differ from jsonpatch on purpose. It is a MAJOR
release and has a migration guide.

- ✅ **Own the code:** the operation classes, exceptions and CLI move out of the copied
  jsonpatch module into this package.
- ✅ **Own the pointer:** a JSON Pointer (RFC 6901) implementation in Rust replaces
  `jsonpointer`; the package then has no runtime Python dependency.
- ✅ **Own the tests:** parity tests compare with stored expected results instead of
  running jsonpatch. jsonpatch stays an optional check, not a requirement. RFC 6902
  conformance (`tests/rfc6902/`) stays required.
- ✅ **Decide the differences:** go through the `1.0` issues and choose, one by one,
  keep or change. Each change is listed in the changelog and the migration guide.
- ✅ **Say what we are:** README and `MAINTAINING.md` stop describing the package as
  jsonpatch with a Rust core; "The rule" is rewritten as our own compatibility policy.

Done when: no runtime dependencies, tests pass without jsonpatch installed, every
difference from jsonpatch is documented.

## After 1.0

Not committed; candidates only. New behaviour is added without breaking 1.x.

- Options for choosing the array diff strategy and for smaller patches.
- Large documents and lower memory use.
- Whether to keep a jsonpatch compatibility mode, and for how long.

## Not goals

- Copying jsonpatch's bugs after 1.0.
- Formats other than RFC 6902 patches and RFC 6901 pointers.
- Supporting Python versions that are end of life.

## Open questions

- Name: "fastjsonpatch" says "jsonpatch, faster". Decide before 1.0 whether to keep it.
- Whether the compatibility mode is a flag, a separate import path, or dropped.
