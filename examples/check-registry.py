#!/usr/bin/env python3
"""The registry from disk against the one from what is running.

A service can be deployed with a manifest other than the one in the repo, and
nothing says so: the contracts keep compiling because they were generated from
the old one. Comparing both registries is the only place that shows.
"""
import json
import sys

disk, live = (json.load(open(p)) for p in sys.argv[1:3])
for d in (disk, live):
    for svc in d.values():
        # a file path against a URL: it differs on purpose
        svc.pop("source", None)
# an external contract is frozen and serves nothing, so it is only on disk
disk.pop("stripe", None)

if disk != live:
    missing = sorted(set(disk) ^ set(live))
    print(f"  FAILED: the two registries disagree: {missing or 'contents'}")
    for k in sorted(set(disk) & set(live)):
        if disk[k] != live[k]:
            print(f"    {k}: disk={disk[k]}")
            print(f"    {k}: running={live[k]}")
    sys.exit(1)

print(f"  OK: {len(live)} services discovered live, and they declare what the repo says")
