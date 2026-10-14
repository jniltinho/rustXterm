#!/usr/bin/env python3
"""Deterministic zip: zip.py OUT.zip DIR -> members under DIR's name, sorted, fixed time and modes.

Every member gets the time of SOURCE_DATE_EPOCH (UTC, at least 1980-01-01), mode 0644 (0755 for
directories) and deflate level 9, so the archive depends only on the file contents.
"""
import os
import sys
import time
import zipfile

out, root = sys.argv[1], sys.argv[2].rstrip("/")
stamp = time.gmtime(max(int(os.environ["SOURCE_DATE_EPOCH"]), 315532800))[:6]
base = os.path.dirname(root)
paths = []
for d, dirs, files in os.walk(root):
    paths += [os.path.join(d, n) for n in dirs + files]
with zipfile.ZipFile(out, "w") as z:
    for p in sorted([root] + paths):
        name = os.path.relpath(p, base).replace(os.sep, "/")
        info = zipfile.ZipInfo(name + "/" if os.path.isdir(p) else name, stamp)
        info.create_system = 3
        if os.path.isdir(p):
            info.external_attr = (0o40755 << 16) | 0x10
            z.writestr(info, b"")
        else:
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            with open(p, "rb") as f:
                z.writestr(info, f.read(), compresslevel=9)
