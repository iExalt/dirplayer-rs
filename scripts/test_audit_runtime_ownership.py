#!/usr/bin/env python3
"""Focused tests for the lexical runtime ownership audit."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("audit-runtime-ownership.py")


def run_audit(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--root", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )


class AuditRuntimeOwnershipTests(unittest.TestCase):
    def fixture(self, rust: str = "", js: str = "") -> tempfile.TemporaryDirectory[str]:
        directory = tempfile.TemporaryDirectory()
        root = Path(directory.name)
        (root / "vm-rust/src").mkdir(parents=True)
        (root / "src").mkdir(parents=True)
        (root / "vm-rust/src/runtime.rs").write_text(rust, encoding="utf-8")
        (root / "src/runtime.ts").write_text(js, encoding="utf-8")
        return directory

    def test_inventory_is_deterministic_and_masks_comments_and_raw_strings(self) -> None:
        directory = self.fixture(
            rust=r'''/* static mut COMMENTED: bool = false; */
const FAKE = r#"static mut RAW: bool = false;"#;
static mut BAD: Option<u8> = None;
thread_local! {
    static TLS: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(Vec::new());
}
static MULTI:
    std::sync::Mutex<Option<u8>> = std::sync::Mutex::new(Some(
        reserve_player_ref(|p| p)
    ));
static TABLE: [u8; 2] = [1, 2];
''',
            js=r'''// const fake = new Map();
const text = `const fake = new Map();`;
const pending =
  new Map();
(() => {
  const players = new Map();
  const local = new Map();
})();
let vmCallbacks = undefined;
globalThis.dirplayer = {};
window['legacyGlobal'] = 1;
''',
        )
        try:
            root = Path(directory.name)
            first = run_audit(root, "--format", "json")
            second = run_audit(root, "--format", "json")
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertEqual(first.stdout, second.stdout)
            findings = json.loads(first.stdout)
            pairs = {(item["category"], item["symbol"]) for item in findings}
            self.assertIn(("rust-static-mut", "BAD"), pairs)
            self.assertIn(("rust-thread-local", "TLS"), pairs)
            self.assertIn(("rust-mutable-singleton", "MULTI"), pairs)
            self.assertIn(("legacy-accessor", "reserve_player_ref"), pairs)
            self.assertIn(("rust-immutable", "TABLE"), pairs)
            self.assertIn(("js-module-registry", "pending"), pairs)
            self.assertIn(("js-module-registry", "players"), pairs)
            self.assertNotIn(("js-module-registry", "local"), pairs)
            self.assertIn(("js-module-binding", "vmCallbacks"), pairs)
            self.assertIn(("js-global-write", "globalThis"), pairs)
            self.assertEqual(sum(item["category"] == "js-global-write" for item in findings), 2)
            self.assertNotIn(("rust-static-mut", "COMMENTED"), pairs)
            self.assertNotIn(("rust-static-mut", "RAW"), pairs)
            self.assertNotIn(("js-module-registry", "fake"), pairs)
        finally:
            directory.cleanup()

    def test_check_requires_hash_pinned_diagnostic_allowlist(self) -> None:
        directory = self.fixture(
            rust="static COUNTER: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(Some(\"seed\"));\n"
        )
        try:
            root = Path(directory.name)
            initial = run_audit(root, "--format", "json")
            findings = json.loads(initial.stdout)
            finding = next(item for item in findings if item["symbol"] == "COUNTER")
            allowlist = root / "allowlist.json"
            allowlist.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "entries": [
                            {
                                "category": finding["category"],
                                "path": finding["path"],
                                "symbol": finding["symbol"],
                                "text_sha256": finding["text_sha256"],
                                "disposition": "diagnostic",
                                "reason": "test-only counter",
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            passed = run_audit(root, "--check", "--allowlist", str(allowlist))
            self.assertEqual(passed.returncode, 0, passed.stderr)
            duplicate = json.loads(allowlist.read_text(encoding="utf-8"))
            duplicate["entries"].append(duplicate["entries"][0].copy())
            allowlist.write_text(json.dumps(duplicate), encoding="utf-8")
            duplicate_result = run_audit(root, "--check", "--allowlist", str(allowlist))
            self.assertNotEqual(duplicate_result.returncode, 0)
            self.assertIn("duplicate key", duplicate_result.stderr)
            stale_entry = json.loads(allowlist.read_text(encoding="utf-8"))
            stale_entry["entries"] = [stale_entry["entries"][0] | {"symbol": "MISSING"}]
            allowlist.write_text(json.dumps(stale_entry), encoding="utf-8")
            stale_entry_result = run_audit(root, "--check", "--allowlist", str(allowlist))
            self.assertNotEqual(stale_entry_result.returncode, 0)
            self.assertIn("stale entry", stale_entry_result.stderr)

            # Restore one entry, then change a string in the declaration. The
            # allowlist hash covers original source, including initializer data.
            duplicate["entries"] = duplicate["entries"][:1]
            allowlist.write_text(json.dumps(duplicate), encoding="utf-8")
            source = root / "vm-rust/src/runtime.rs"
            source.write_text(source.read_text(encoding="utf-8").replace("seed", "changed"), encoding="utf-8")
            stale = run_audit(root, "--check", "--allowlist", str(allowlist))
            self.assertNotEqual(stale.returncode, 0)
            self.assertIn("hash mismatch", stale.stderr)
        finally:
            directory.cleanup()

    def test_static_mut_and_legacy_categories_cannot_be_allowlisted(self) -> None:
        directory = self.fixture(rust="static mut PLAYER_OPT: Option<u8> = None;\nfn f() { reserve_player_mut(|p| p); }\n")
        try:
            root = Path(directory.name)
            inventory = json.loads(run_audit(root, "--format", "json").stdout)
            entries = []
            for finding in inventory:
                if finding["category"] in {"rust-static-mut", "legacy-accessor"}:
                    entries.append(
                        {
                            **{key: finding[key] for key in ("category", "path", "symbol", "text_sha256")},
                            "disposition": "diagnostic",
                            "reason": "must be rejected",
                        }
                    )
            allowlist = root / "allowlist.json"
            allowlist.write_text(json.dumps({"version": 1, "entries": entries}), encoding="utf-8")
            result = run_audit(root, "--check", "--allowlist", str(allowlist))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("never allowlisted", result.stderr)
        finally:
            directory.cleanup()


if __name__ == "__main__":
    unittest.main()
