#!/usr/bin/env python3
"""Checks that the declared rollout is the one flagd applies.

Two properties, and the second is the one that matters: the percentage comes
close to the declared one, and the SAME entity always gets the same answer.
Without the second, a payment would take the new path on one call and the old one
on the next, and end up half migrated.
"""
import json
import sys
import urllib.request

host = sys.argv[1] if len(sys.argv) > 1 else "localhost:8016"
flag = sys.argv[2] if len(sys.argv) > 2 else "charge_v2"
expected = float(sys.argv[3]) if len(sys.argv) > 3 else 10.0
n = 300


def evaluate(tenant):
    # OFREP: OpenFeature's standard REST protocol, not flagd's own API
    req = urllib.request.Request(
        f"http://{host}/ofrep/v1/evaluate/flags/{flag}",
        data=json.dumps({"context": {"tenant_id": tenant}}).encode(),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as r:
        return bool(json.load(r).get("value"))


tenants = [f"tenant-{i}" for i in range(n)]
first = {t: evaluate(t) for t in tenants}
on = sum(first.values())
measured = on * 100 / n

# the same entity, again: if it changes, the rollout is not sticky
unstable = [t for t in tenants[:40] if evaluate(t) != first[t]]

print(f"  declared {expected:.0f}%  measured {measured:.1f}%  ({on} of {n})")
assert not unstable, f"the rollout is not stable for: {unstable[:5]}"
# a deliberately wide margin: with 300 samples the variance is real, and what is
# checked is that the percentage applies, not the hash's quality
assert abs(measured - expected) < max(5.0, expected * 0.6), (
    f"the measured rollout ({measured:.1f}%) does not resemble the declared one ({expected:.0f}%)"
)
print("  OK: sticky per tenant, and the percentage applies")
