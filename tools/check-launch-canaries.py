#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check a synthetic launch audit's request records for the fixture's canaries.

This is deliberately a small, independently readable literal/encoding check.
It is not Declass's broader adversarial canary matcher or a network observer.
Usage: python3 tools/check-launch-canaries.py AUDIT_JSONL FIXTURE_DIR
Exit 1 means a planted value was found. Output is a reproducible JSON report.
"""
import base64
import csv
import hashlib
import json
from pathlib import Path
import sys
import urllib.parse


def main():
    audit, fixture = map(Path, sys.argv[1:])
    rows = list(csv.DictReader((fixture / 'data/customers.csv').open()))
    values = [r[k] for r in rows for k in ['customer_id', 'name', 'email', 'amount']]
    env = (fixture / 'env.example').read_text().strip().split('=', 1)[1]
    values.append(urllib.parse.urlsplit(env).password)
    records = [json.loads(line) for line in audit.read_text().splitlines()]
    requests = [r['request'] for r in records if 'request' in r]
    text = json.dumps(requests, ensure_ascii=False)
    checks = []
    for value in values:
        variants = {'literal': value, 'base64': base64.b64encode(value.encode()).decode(),
                    'hex': value.encode().hex(), 'url': urllib.parse.quote(value, safe='')}
        seen = set()
        counts = {}
        for label, encoded in variants.items():
            if encoded not in seen:
                counts[label] = text.count(encoded)
                seen.add(encoded)
        checks.append({'canary': value, 'matches': counts})
    report = {
        'audit_sha256': hashlib.sha256(audit.read_bytes()).hexdigest(),
        'scope': 'Recorded outbound request bodies only; no packet capture or provider receipt. '
                 'Complete planted values in literal, base64, hex and URL forms; duplicate encodings counted once. '
                 'No claim about fragments, paraphrase or all encodings.',
        'request_count': len(requests), 'audit_records': len(records),
        'canary_count': len(values), 'checks': checks,
        'total_matches': sum(sum(c['matches'].values()) for c in checks),
    }
    print(json.dumps(report, indent=2))
    return int(report['total_matches'] != 0)


if __name__ == '__main__':
    raise SystemExit(main())
