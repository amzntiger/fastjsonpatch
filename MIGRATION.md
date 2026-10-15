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
