#!/usr/bin/env python3
"""License summary of the crates linked into `rustxterm`, from `cargo metadata` JSON.

Usage: third-party-notices.py <metadata.json>  (prints to stdout)
Only normal (non-dev, non-build) dependencies reachable from the `rustxterm` package count.
"""
import collections
import json
import sys

meta = json.load(open(sys.argv[1]))
packages = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
root = next(p["id"] for p in meta["packages"] if p["name"] == "rustxterm")

seen, todo = set(), [root]
while todo:
    node = nodes[todo.pop()]
    for dep in node["deps"]:
        if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
            seen.add(dep["pkg"])
            todo.append(dep["pkg"])

# Registry crates plus the patched copies under vendor/ (path dependencies, same upstream license).
crates = sorted(
    (packages[i] for i in seen if packages[i]["source"] or "/vendor/" in packages[i]["manifest_path"]),
    key=lambda p: (p["name"], p["version"]),
)
missing = [p["name"] for p in crates if not p.get("license")]
if missing:
    sys.exit("crates without a license field: " + ", ".join(missing))
counts = collections.Counter(p["license"] for p in crates)

print("Third-party Rust crates compiled into rustxterm")
print("Generated from `cargo metadata` (Cargo.lock); license texts are in each crate's source.\n")
print("License summary:")
for lic, n in counts.most_common():
    print(f"  {n:4}  {lic}")
print(f"\n{len(crates)} crates:\n")
for p in crates:
    print(f"{p['name']} {p['version']}  {p['license']}  {p.get('repository') or ''}".rstrip())
