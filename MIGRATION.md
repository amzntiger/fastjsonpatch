# Migrating to fastjsonpatch 1.0

1.0 is the first release that does not copy jsonpatch behaviour for its own sake. This
page lists what changes for code that used jsonpatch or fastjsonpatch 0.x, with the
fix for each. Changes are added here as they land; the full list is in the
[changelog](CHANGELOG.md).

Nothing else changes: the patches, documents and error messages stay as in jsonpatch
1.35 unless a section below says so.

## From jsonpatch

```diff
-import jsonpatch
+import fastjsonpatch as jsonpatch
```

The same calls work. The commands are `fastjsondiff` and `fastjsonpatch` (jsonpatch's
are `jsondiff` and `jsonpatch`), so both can be installed together.

## Errors from JSON pointers: `JsonPointerException`

fastjsonpatch no longer depends on the `jsonpointer` package; it contains its own copy
of jsonpointer 3.2.1. The exception for a bad or unresolvable pointer is now
`fastjsonpatch.JsonPointerException` (also `fastjsonpatch.pointer.JsonPointerException`),
a different class from `jsonpointer.JsonPointerException`.

If you catch it by the jsonpointer name, the exception is no longer caught:

```diff
-import jsonpointer
+import fastjsonpatch

 try:
-    jsonpatch.apply_patch(doc, patch)
-except jsonpointer.JsonPointerException:
+    fastjsonpatch.apply_patch(doc, patch)
+except fastjsonpatch.JsonPointerException:
     ...
```

The same applies to `from jsonpointer import JsonPointer` as `pointer_cls`: it still
works as the `pointer_cls` argument, but the errors it raises are
`jsonpointer.JsonPointerException`, which fastjsonpatch does not treat as its own. Use
`fastjsonpatch.JsonPointer` instead.

Code that only uses `jsonpatch.JsonPointerException` through the jsonpatch name (as in
the diff above) keeps working after the import change.

## Numbers in `make_patch`: 1 and 1.0 are the same

RFC 6902 (4.6) compares numbers by value, and so does `make_patch` now. jsonpatch 1.35
treats `1` and `1.0` as different and patches one to the other.

```python
make_patch({"a": 1}, {"a": 1.0}).patch
# jsonpatch 1.35, fastjsonpatch 0.x:  [{"op": "replace", "path": "/a", "value": 1.0}]
# fastjsonpatch 1.0:                  []
```

Applying the patch gives a document that is equal to the target as JSON, but a number
may keep the type it had in the source: `apply_patch({"a": 1}, make_patch({"a": 1}, {"a": 1.0}))`
is `{"a": 1}`, an `int`, where it used to be `{"a": 1.0}`. Python compares the two as
equal (`1 == 1.0`). The same holds for `-0.0` and `0`, and for whole floats such as `1e20`
and `10**20`. `1` and `True` stay different, as does any pair that is not the same number
(`2**53 + 1` and `float(2**53)`).

Equal numbers in lists are now matched up, so `make_patch([1, 2], [2.0, 1])` is one
`move`, not an `add` and a `remove`. If you need the old result, use
`JsonPatch.from_diff(src, dst, dumps=lambda value: json.dumps(value))`: a `dumps` of your
own compares serialized values as before (this is slower, as the Rust code is not used).
If code depends on the number type of the result, convert the values after applying.

## Error messages

The message for a pointer that goes through the end of an array (`/a/-/b`) names
`fastjsonpatch.pointer.EndOfList`, where jsonpatch's names `jsonpointer.EndOfList`. Code
that matches on this text needs the new name.
