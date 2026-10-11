"""Write THIRD_PARTY_LICENSES.md: the license texts of the Rust crates linked into the
extension module. Run after changing Rust dependencies: python scripts/third_party_licenses.py

Only crates compiled into the binary are listed (proc-macros and build scripts run at
build time and are not shipped). For dual-licensed crates the MIT text is used.
"""
import glob
import json
import os
import subprocess

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PREFERRED = ["LICENSE-MIT", "LICENSE-MIT.md", "LICENSE", "LICENSE.md", "LICENSE.txt", "LICENSE-APACHE"]


def linked_crates():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", host_triple()], cwd=ROOT))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root, seen, todo = meta["resolve"]["root"], set(), [meta["resolve"]["root"]]
    while todo:
        node = nodes[todo.pop()]
        for dep in node["deps"]:
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            proc_macro = any("proc-macro" in t["kind"] for t in packages[dep["pkg"]]["targets"])
            if normal and not proc_macro and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    return sorted((packages[i] for i in seen if i != root), key=lambda p: p["name"])


def host_triple():
    out = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.split(": ")[1] for line in out.splitlines() if line.startswith("host: "))


def license_text(pkg):
    folder = os.path.dirname(pkg["manifest_path"])
    for name in PREFERRED:
        path = os.path.join(folder, name)
        if os.path.exists(path):
            return name, open(path, encoding="utf-8").read().strip()
    found = glob.glob(os.path.join(folder, "LICENSE*")) + glob.glob(os.path.join(folder, "COPYING*"))
    if found:
        return os.path.basename(found[0]), open(found[0], encoding="utf-8").read().strip()
    raise SystemExit(f"no license file in {pkg['name']} {pkg['version']}")


def main():
    parts = ["# Third-party licenses\n",
             "The compiled extension module (`fastjsonpatch/_core`) includes these Rust crates.\n"]
    for pkg in linked_crates():
        name, text = license_text(pkg)
        parts.append(f"## {pkg['name']} {pkg['version']}\n\nLicense: {pkg['license']} "
                     f"(text below: {name})\n\n```\n{text}\n```\n")
    with open(os.path.join(ROOT, "THIRD_PARTY_LICENSES.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(parts))
    print("\n".join(f"{p['name']} {p['version']}: {p['license']}" for p in linked_crates()))


if __name__ == "__main__":
    main()
