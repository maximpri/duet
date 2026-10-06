#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Independently inspect this launch packet's JSONL chain and supplied anchor.

Usage: python3 tools/verify-launch-audit.py AUDIT_JSONL ANCHOR_JSON
This checks the published fixture's serialization, not every possible JSON
number representation. Declass's own audit verifier is authoritative for runs.
A bundled anchor checks consistency, not independent custody or authorship.
"""
import hashlib
import json
from pathlib import Path
import sys


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    audit, anchor_file = map(Path, sys.argv[1:])
    anchor = json.loads(anchor_file.read_bytes())
    previous = '0' * 64
    count = 0
    errors = []
    for line in audit.read_bytes().splitlines():
        if not line:
            continue
        count += 1
        record = json.loads(line)
        if record['seq'] != count or record['prev'] != previous:
            errors.append(f'chain mismatch at record {count}')
        if 'request' in record:
            # This workspace enables serde_json's preserve_order feature.
            body = json.dumps(record['request'], ensure_ascii=False,
                              separators=(',', ':')).encode()
            if digest(body) != record['request_sha256']:
                errors.append(f'request body digest mismatch at record {count}')
        previous = digest(line)
    matches = count == anchor['records'] and previous == anchor['head']
    print(json.dumps({'audit_sha256': digest(audit.read_bytes()),
                      'records': count, 'chain_and_body_errors': errors,
                      'supplied_anchor_matches': matches,
                      'scope': 'Artifact consistency only; no independent custody, '
                               'provider receipt or disclosure-safety claim.'}, indent=2))
    return int(bool(errors) or not matches)


if __name__ == '__main__':
    raise SystemExit(main())
