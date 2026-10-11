"""Speed of fastjsonpatch vs jsonpatch on this machine; prints one JSON line.

Run: python bench/compare.py   (needs jsonpatch installed as the reference)
Used by CI to compare every OS / Python combination.
"""
import copy
import json
import platform
import random
import sys
import time

import jsonpatch

import fastjsonpatch


def best(f, budget=0.4, min_runs=3):
    b, spent, runs = float("inf"), 0.0, 0
    while spent < budget or runs < min_runs:
        s = time.perf_counter()
        f()
        e = time.perf_counter() - s
        b, spent, runs = min(b, e), spent + e, runs + 1
    return b


def plain(x):
    """Results with patch objects replaced by their operations, for comparison."""
    if isinstance(x, list):
        return [plain(i) for i in x]
    return x.patch if isinstance(x, (jsonpatch.JsonPatch, fastjsonpatch.JsonPatch)) else x


def records(n, rng):
    return [{"id": i, "name": f"user{i}", "score": rng.randint(0, 100), "tags": ["a", "b"],
             "meta": {"x": i, "ok": i % 2 == 0}} for i in range(n)]


def main():
    rng = random.Random(0)
    big = {"items": records(10000, rng)}
    big2 = copy.deepcopy(big)
    for r in rng.sample(big2["items"], 100):
        r["score"] += 1
    small_patch = [{"op": "replace", "path": "/items/5/score", "value": 1},
                   {"op": "add", "path": "/items/7/tags/-", "value": "c"}]
    resource = {"id": 42, "labels": {"env": "prod"}, "spec": {"replicas": 3, "ports": [{"port": 80}, {"port": 443}]}}
    req = [{"op": "replace", "path": "/spec/replicas", "value": 5},
           {"op": "add", "path": "/labels/canary", "value": "true"},
           {"op": "remove", "path": "/spec/ports/1"}]
    resource2 = jsonpatch.apply_patch(resource, req)

    cases = {
        "make_patch_10k": (lambda m: m.make_patch(big, big2)),
        "apply_small_patch_10k": (lambda m: m.apply_patch(big, small_patch)),
        "make_patch_request_x1000": (lambda m: [m.make_patch(resource, resource2) for _ in range(1000)]),
        "apply_patch_request_x1000": (lambda m: [m.apply_patch(resource, req) for _ in range(1000)]),
    }
    out = {"python": platform.python_version(), "os": platform.system(), "machine": platform.machine(),
           "libc": "-".join(platform.libc_ver()) or "-", "results": {}}
    for name, fn in cases.items():
        assert repr(plain(fn(fastjsonpatch))) == repr(plain(fn(jsonpatch))), name
        ref, fast = best(lambda: fn(jsonpatch)), best(lambda: fn(fastjsonpatch))
        out["results"][name] = {"jsonpatch_ms": round(ref * 1e3, 3), "fastjsonpatch_ms": round(fast * 1e3, 3),
                                "speedup": round(ref / fast, 1)}
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
