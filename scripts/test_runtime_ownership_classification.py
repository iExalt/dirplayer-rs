#!/usr/bin/env python3
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('classification', Path(__file__).with_name('check-runtime-ownership-classification.py'))
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)

class CoverageTests(unittest.TestCase):
    def setUp(self):
        self.finding = dict(category='rust-static-mut', path='vm-rust/src/lib.rs', symbol='STATE', text_sha256='abc')
        self.entry = dict(self.finding, occurrence=1, disposition='unresolved-state', target_owner='session', requirement_owner='2.9', rationale='Move state into the owning session.')
        self.manifest = dict(schema_version=1, findings=[self.entry])

    def test_reviewed_unresolved_is_inventory_success(self):
        self.assertEqual([], CHECK.validate([self.finding], self.manifest))

    def test_new_same_symbol_occurrence_is_unclassified(self):
        self.assertTrue(any('unclassified' in e for e in CHECK.validate([self.finding, self.finding], self.manifest)))

    def test_removed_or_changed_finding_fails(self):
        self.assertTrue(any('stale' in e for e in CHECK.validate([], self.manifest)))
        changed = dict(self.finding, text_sha256='changed')
        self.assertTrue(any('changed' in e for e in CHECK.validate([changed], self.manifest)))

    def test_duplicate_and_missing_owner_fail(self):
        manifest = copy.deepcopy(self.manifest)
        manifest['findings'].append(copy.deepcopy(self.entry))
        del manifest['findings'][0]['target_owner']
        errors = CHECK.validate([self.finding], manifest)
        self.assertTrue(any('duplicate' in e for e in errors))
        self.assertTrue(any('target_owner' in e for e in errors))

    def test_bad_shape_and_incompatible_disposition_fail(self):
        self.assertTrue(CHECK.validate([self.finding], []))
        self.entry['disposition'] = 'immutable-data'
        self.assertTrue(CHECK.validate([self.finding], self.manifest))

    def test_new_source_without_findings_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'vm-rust/src').mkdir(parents=True)
            (root / 'vm-rust/src/empty.rs').write_text('// No lexical finding')
            errors = CHECK.check_tree(root, self.manifest)
            self.assertTrue(any('new unreviewed source: vm-rust/src/empty.rs' in e for e in errors))

    def test_empty_tree_and_missing_ruffle_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            errors = CHECK.check_tree(Path(temp), self.manifest)
            self.assertTrue(any('ruffle/core/src' in e for e in errors))
            self.assertTrue(any('scanner changed' in e for e in errors))
        self.assertTrue(CHECK.validate([], dict(schema_version=1, findings=[])))

if __name__ == '__main__':
    unittest.main()
