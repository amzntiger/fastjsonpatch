#!/usr/bin/env bash
# Install the built wheels (dist/) into clean Linux containers and run the full test
# suite plus the jsonpatch speed comparison. One JSON result line per image goes to
# the file given as $1 (default: linux-matrix.jsonl).
#
#   maturin build ... --out dist     # build the wheels first
#   scripts/test_linux_matrix.sh [results.jsonl] [image ...]
#
# Images this script pulls are removed again afterwards (disk space); images that
# were already present are kept.
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-linux-matrix.jsonl}"
shift || true
images=("$@")
if [ ${#images[@]} -eq 0 ]; then
  images=(python:3.12-slim python:3.13-slim python:3.14-slim
          python:3.12-alpine python:3.13-alpine python:3.14-alpine
          python:3.12-slim-bullseye)
fi
: > "$out"
for image in "${images[@]}"; do
  had_image=$(docker image inspect "$image" >/dev/null 2>&1 && echo yes || echo no)
  echo "=== $image" >&2
  if docker run --rm -v "$PWD/dist:/dist:ro" -v "$PWD/tests:/src/tests:ro" -v "$PWD/bench:/src/bench:ro" \
      "$image" sh -c '
        set -e
        pip install -q --no-cache-dir --disable-pip-version-check --root-user-action=ignore \
          --only-binary=:all: pytest "jsonpatch==1.35" >&2
        # --no-index: with PyPI, pip prefers its wheel of the same version over /dist
        pip install -q --no-cache-dir --disable-pip-version-check --root-user-action=ignore \
          --no-index --find-links /dist fastjsonpatch >&2
        cp -r /src /work && cd /work
        result=$(python -m pytest -q -p no:cacheprovider tests 2>&1 | tail -1)
        echo "$result" >&2
        python bench/compare.py | python -c "
import json, sys
r = json.load(sys.stdin); r[\"image\"] = sys.argv[1]; r[\"tests\"] = sys.argv[2]
print(json.dumps(r))" "'"$image"'" "$result"
      ' >> "$out"; then
    :
  else
    echo "{\"image\": \"$image\", \"tests\": \"FAILED to run\"}" >> "$out"
  fi
  if [ "$had_image" = no ]; then docker image rm -f "$image" >/dev/null 2>&1 || true; fi
done
echo "results: $out" >&2
