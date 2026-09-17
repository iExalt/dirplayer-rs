#!/usr/bin/env python3
"""Check Stage 1 review coverage, not the Stage 2 singleton-removal gate."""
from __future__ import annotations
import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('ownership_scan', ROOT / 'scripts/audit-runtime-ownership.py')
SCANNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SCANNER
SPEC.loader.exec_module(SCANNER)
REQUIRED_ROOTS = ('vm-rust/src', 'xtra-sdk/src', 'xtras', 'src', 'extension/src',
                  'polyfill/src', 'dirplayer-js-api', 'public', 'ruffle/core/src', 'ruffle/web/src')
DISPOSITIONS = {'unresolved-state', 'unresolved-access', 'unresolved-task',
                'immutable-data', 'diagnostic', 'test-harness', 'host-boundary', 'lexical-candidate'}

def identities(findings):
    counts = Counter()
    result = {}
    for finding in findings:
        item = finding.as_dict() if hasattr(finding, 'as_dict') else finding
        base = (item['category'], item['path'], item['symbol'])
        counts[base] += 1
        result[(*base, counts[base])] = item
    return result

def validate(findings, manifest):
    errors = []
    if not isinstance(manifest, dict) or not isinstance(manifest.get('findings'), list):
        return ['manifest must be an object with a findings array']
    current = identities(findings)
    recorded = {}
    if manifest.get('schema_version') != 1:
        errors.append('unsupported classification schema')
    for number, item in enumerate(manifest.get('findings', []), 1):
        try:
            key = (item['category'], item['path'], item['symbol'], item['occurrence'])
            if type(item['occurrence']) is not int or item['occurrence'] < 1:
                raise ValueError('invalid occurrence')
            if key in recorded:
                errors.append(f'duplicate classification: {key}')
            recorded[key] = item
            for field in ('target_owner', 'requirement_owner', 'rationale'):
                if not isinstance(item.get(field), str) or not item[field].strip():
                    errors.append(f'{key}: missing {field}')
            if item['category'] in ('rust-static-mut', 'rust-thread-local', 'legacy-accessor', 'legacy-task') and item.get('disposition') == 'immutable-data':
                errors.append(f'{key}: category cannot be immutable-data')
            if item.get('disposition') not in DISPOSITIONS:
                errors.append(f'{key}: invalid disposition')
            if key in current and item.get('text_sha256') != current[key]['text_sha256']:
                errors.append(f'changed source: {key}')
        except (KeyError, ValueError, TypeError) as exc:
            errors.append(f'invalid entry {number}: {exc}')
    errors.extend(f'unclassified: {key}' for key in current.keys() - recorded.keys())
    errors.extend(f'stale classification: {key}' for key in recorded.keys() - current.keys())
    if not current:
        errors.append('empty scan cannot establish review coverage')
    return errors

def check_tree(root, manifest):
    errors = []
    if not isinstance(manifest, dict):
        return ['manifest must be an object']
    errors.extend(f'missing acceptance root: {name}' for name in SCANNER.coverage(root)['missing_roots'])
    scanned = {str(p.relative_to(root)) for group in SCANNER.source_files(root) for p in group}
    hashes = manifest.get('reviewed_source_sha256', {})
    if not isinstance(hashes, dict):
        return errors + ['reviewed_source_sha256 must be an object']
    recorded = set(hashes)
    errors.extend(f'new unreviewed source: {name}' for name in scanned - recorded)
    errors.extend(f'removed acceptance source: {name}' for name in recorded - scanned)
    for relative in REQUIRED_ROOTS:
        folder = root / relative
        if not folder.is_dir() or not any(folder.rglob('*.*')):
            errors.append(f'missing or empty acceptance source root: {relative}')
    scanner_file = root / 'scripts/audit-runtime-ownership.py'
    if not scanner_file.is_file() or hashlib.sha256(scanner_file.read_bytes()).hexdigest() != manifest.get('scanner_sha256'):
        errors.append('scanner changed: review coverage and regenerate classifications')
    for relative, digest in manifest.get('reviewed_source_sha256', {}).items():
        source = root / relative
        if not source.is_file() or hashlib.sha256(source.read_bytes()).hexdigest() != digest:
            errors.append(f'reviewed source changed: {relative}')
    if not manifest.get('reviewed_source_sha256'):
        errors.append('missing reviewed source provenance')
    return errors

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--manifest', type=Path, default=ROOT / 'docs/native-ownership-classification.json')
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text())
        errors = check_tree(args.root, manifest)
        errors.extend(validate(SCANNER.scan(args.root), manifest))
    except (OSError, ValueError, TypeError) as exc:
        print(f'ownership classification: {exc}', file=sys.stderr)
        return 2
    if errors:
        print('\n'.join(sorted(errors)), file=sys.stderr)
        return 1
    print(f"Stage 1 classification coverage: {len(manifest['findings'])} findings; unresolved owners retained. Not a Stage 2 removal pass.")
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
