#!/usr/bin/env python3
"""Checks the shape of the trace that reached OpenTelemetry.

What breaks the moment somebody invents a `traceparent` is not that the trace is
missing: it is that it shows up split into fragments hanging off parents that
never existed, and in the UI that looks like several short traces instead of one.
"""
import json
import sys
import time
import urllib.parse
import urllib.request

ui = sys.argv[1] if len(sys.argv) > 1 else "localhost:16686"

# It looks for THE trace of this run, not the latest one there is: the collector
# keeps the earlier ones and "the latest" is not a criterion.
with open(".axon/log/local.ndjson") as f:
    from_log = json.loads(f.readline())["correlationId"]
tags = urllib.parse.quote(json.dumps({"axon.correlation_id": from_log}))
url = f"http://{ui}/api/traces?service=orders&tags={tags}&lookback=1h&limit=5"

# The exporter sends in batches: the trace takes a while to show up. It is
# retried here instead of waiting outside, because outside there is no way to
# know whether the one that arrived is this run's or the previous one's.
data = []
for _ in range(60):
    try:
        with urllib.request.urlopen(url) as r:
            data = json.load(r)["data"]
    except Exception:
        data = []
    if data:
        break
    time.sleep(1)

assert data, f"no traces for flow {from_log} after 60s"
assert len(data) == 1, f"flow {from_log} shows up split across {len(data)} traces"
t = data[0]
spans = {s["spanID"]: s for s in t["spans"]}
services = {p["serviceName"] for p in t["processes"].values()}


def parent(s):
    return next((r["spanID"] for r in s.get("references", []) if r["refType"] == "CHILD_OF"), None)


def depth(s):
    p = parent(s)
    return 0 if not p or p not in spans else depth(spans[p]) + 1


for s in sorted(t["spans"], key=lambda s: s["startTime"]):
    proc = t["processes"][s["processID"]]["serviceName"]
    print(f'  {"    " * depth(s)}{proc}/{s["operationName"]}')

roots = [s for s in t["spans"] if not parent(s)]
orphans = [s for s in t["spans"] if parent(s) and parent(s) not in spans]
assert len(roots) == 1, f"expected exactly one root span, there are {len(roots)}"
assert not orphans, f"{len(orphans)} spans hang off a parent that does not exist"
assert services >= {"orders", "payments"}, f"the trace did not cross the services: {services}"

# the same business flow, seen from both sides
corr = {x["value"] for s in t["spans"] for x in s["tags"] if x["key"] == "axon.correlation_id"}
assert corr == {from_log}, f"the trace mixes flows: {corr}"

print(f'  OK: {len(t["spans"])} spans, one root, no orphans, crossing {sorted(services)}')
print(f"  and the correlationId matches the log's: {from_log}")
