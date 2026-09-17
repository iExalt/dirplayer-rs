#!/usr/bin/env python3
"""Inventory process and module state relevant to native session ownership.

This is deliberately a conservative lexical audit.  It is useful as a review
index and a CI tripwire, but it is not an AST or semantic ownership proof:
macro expansion, aliases, generated code, computed property names, and state
assembled indirectly still require manual inventory.

The default invocation prints a deterministic inventory.  ``--check`` fails
for every non-immutable finding unless it has a matching, hash-pinned entry in
the JSON allowlist.  Allowlist entries are for reviewed process diagnostics
only; ``static mut`` and legacy current-player/accessor/task findings can never
be allowlisted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


DEFAULT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ALLOWLIST = Path(__file__).with_name("runtime-ownership-allowlist.json")

RUST_ROOTS = ("vm-rust/src", "xtra-sdk/src")
RUST_GLOB_ROOTS = ("xtras",)
JS_ROOTS = ("src", "extension/src", "polyfill/src", "dirplayer-js-api", "public")
EXCLUDED_PARTS = {"target", "node_modules", "dist", "build", ".git", "ruffle"}

STATIC_RE = re.compile(
    r"\bstatic\s+(?P<mut>mut\s+)?(?P<ref>ref\s+)?(?P<symbol>[A-Za-z_]\w*)"
    r"(?:\s*:\s*(?P<decl>.*))?\Z",
    re.DOTALL,
)
RUST_LEGACY_RE = re.compile(
    r"\b(?:reserve_player_[A-Za-z_]\w*|WithActivePlayer|"
    r"PLAYER_OPT|ACTIVE_PLAYER_ID|NESTED_PLAYERS|NESTED_PLAYER_KEYS|"
    r"NESTED_EVENT_TX|PLAYER_EVENT_TX|PLAYER_TX|PLAYER_GENERATION)\b"
)
RUST_ACCESSOR_RE = re.compile(r"\b(?:player_mut|player_ref|player_is_playing)\s*\(")
RUST_SPAWN_RE = re.compile(
    r"\b(?:WithActivePlayer|spawn_player_[A-Za-z_]\w*|spawn_local)\b"
    r"|\b(?:async_std::task|wasm_bindgen_futures)::spawn(?:_local)?\b"
)
JS_DECL_RE = re.compile(
    r"\b(?P<binding>const|let|var)\s+(?P<symbol>[A-Za-z_$][\w$]*)"
    r"\s*=\s*(?P<init>.*)\Z",
    re.DOTALL,
)
JS_COLLECTION_RE = re.compile(
    r"^\s*(?:new\s+(?:Map|Set|WeakMap|WeakSet)\b|\[|\{)"
)
JS_GLOBAL_WRITE_RE = re.compile(
    r"\b(?P<global>window|globalThis|global)\s*(?:\.\s*[A-Za-z_$][\w$]*|\s*\[[^\]]*\])\s*="
)
JS_REGISTRY_NAME_RE = re.compile(
    r"(?:registry|instance|pending|handler|callback|player|plugin|runtime|object|"
    r"inflight|in_flight|cache|listener|store|resolver|tabs?)",
    re.IGNORECASE,
)
RUST_RAW_START_RE = re.compile(r"(?:br|rb|r)(#+)?\"")


@dataclass(frozen=True)
class Finding:
    category: str
    path: str
    line: int
    symbol: str
    source: str
    match: str
    text_sha256: str

    @property
    def key(self) -> tuple[str, str, str]:
        return self.category, self.path, self.symbol

    def as_dict(self) -> dict[str, object]:
        return {
            "category": self.category,
            "path": self.path,
            "line": self.line,
            "symbol": self.symbol,
            "source": self.source,
            "match": self.match,
            "text_sha256": self.text_sha256,
        }


def _masked_code(text: str, language: str) -> str:
    """Replace comments and string bodies with spaces while keeping newlines.

    This intentionally handles the common Rust/JS lexical forms only.  It is
    enough to avoid reporting prose and fixture strings while keeping line
    numbers stable; it must not be mistaken for a parser.
    """

    out = list(text)
    i = 0
    state = "code"
    block_depth = 0
    quote = ""
    raw_end: str | None = None
    n = len(text)
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if state == "line-comment":
            if c != "\n":
                out[i] = " "
            else:
                state = "code"
            i += 1
            continue
        if state == "block-comment":
            if text.startswith("/*", i):
                block_depth += 1
                out[i] = out[i + 1] = " "
                i += 2
            elif text.startswith("*/", i):
                block_depth -= 1
                out[i] = out[i + 1] = " "
                i += 2
                if block_depth == 0:
                    state = "code"
            else:
                if c != "\n":
                    out[i] = " "
                i += 1
            continue
        if state == "string":
            if c == "\\":
                out[i] = " "
                if i + 1 < n and text[i + 1] != "\n":
                    out[i + 1] = " "
                    i += 2
                else:
                    i += 1
            elif c == quote:
                out[i] = " "
                state = "code"
                i += 1
            else:
                if c != "\n":
                    out[i] = " "
                i += 1
            continue
        if state == "raw-string":
            assert raw_end is not None
            if text.startswith(raw_end, i):
                for j in range(i, min(n, i + len(raw_end))):
                    if text[j] != "\n":
                        out[j] = " "
                i += len(raw_end)
                state = "code"
                raw_end = None
            else:
                if c != "\n":
                    out[i] = " "
                i += 1
            continue

        if text.startswith("//", i):
            out[i] = out[i + 1] = " "
            state = "line-comment"
            i += 2
            continue
        if text.startswith("/*", i):
            out[i] = out[i + 1] = " "
            state = "block-comment"
            block_depth = 1
            i += 2
            continue
        if language == "rust":
            # Rust raw strings: r#"..."#, br#"..."#, and rb#"..."#.
            raw = RUST_RAW_START_RE.match(text, i)
            if raw:
                hashes = raw.group(1) or ""
                prefix_len = raw.end() - i
                raw_end = '"' + hashes if hashes else '"'
                for j in range(i, min(n, i + prefix_len)):
                    if text[j] != "\n":
                        out[j] = " "
                i += prefix_len
                state = "raw-string"
                continue
            if c == '"':
                out[i] = " "
                quote = c
                state = "string"
                i += 1
                continue
        else:
            if c in "'\"`":
                out[i] = " "
                quote = c
                state = "string"
                i += 1
                continue
        i += 1
    return "".join(out)


def _normalise(value: str) -> str:
    return " ".join(value.split())


def _sha(value: str) -> str:
    return hashlib.sha256(_normalise(value).encode("utf-8")).hexdigest()


def _relative(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def _files(root: Path, roots: Iterable[str], suffixes: set[str]) -> list[Path]:
    found: set[Path] = set()
    for rel in roots:
        base = root / rel
        if not base.exists():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in suffixes:
                continue
            if any(part in EXCLUDED_PARTS for part in path.relative_to(root).parts):
                continue
            found.add(path)
    return sorted(found)


def _finding(
    category: str,
    root: Path,
    path: Path,
    line: int,
    symbol: str,
    source: str,
    match: str,
    hash_text: str | None = None,
) -> Finding:
    return Finding(
        category,
        _relative(root, path),
        line,
        symbol,
        source,
        _normalise(match),
        _sha(hash_text if hash_text is not None else match),
    )


def _joined_statement(masked_lines: list[str], start: int, max_lines: int = 24) -> tuple[str, int]:
    """Return a bounded declaration, stopping at its first semicolon."""

    statement = masked_lines[start]
    end = start
    while ";" not in statement and end + 1 < len(masked_lines) and end - start < max_lines:
        end += 1
        statement += " " + masked_lines[end]
    return statement, end


def _parse_static(statement: str) -> re.Match[str] | None:
    # A declaration is required to occupy the meaningful part of the line;
    # this avoids treating `foo(static BAR)` as a global declaration.
    return STATIC_RE.search(statement.strip().rstrip(";"))


def _rust_findings(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    masked = _masked_code(text, "rust").splitlines()
    findings: list[Finding] = []
    brace_depth = 0
    thread_base: int | None = None
    seen_legacy: set[tuple[int, str]] = set()
    declaration_continuations: set[int] = set()
    for index, masked_line in enumerate(masked):
        original_line = lines[index] if index < len(lines) else ""
        before = brace_depth
        if "thread_local!" in masked_line:
            thread_base = before
        in_thread_local = thread_base is not None and before > thread_base
        if index not in declaration_continuations and re.search(r"\bstatic\b", masked_line):
            statement, _ = _joined_statement(masked, index)
            parsed = _parse_static(statement)
            if parsed:
                symbol = parsed.group("symbol")
                decl = parsed.group("decl") or ""
                full_source, end = _joined_statement(lines, index)
                if parsed.group("mut"):
                    category = "rust-static-mut"
                elif in_thread_local or "thread_local!" in masked_line:
                    category = "rust-thread-local"
                elif parsed.group("ref") or re.search(
                    r"\b(?:Mutex|RwLock|RefCell|UnsafeCell|Cell|Atomic[A-Za-z0-9_]*|"
                    r"Once(?:Lock|Cell)|Lazy(?:Lock)?|(?:Hash)?Map|(?:Hash)?Set|Vec|"
                    r"Option|Sender|Receiver|Arc|Rc)\b",
                    decl,
                ):
                    category = "rust-mutable-singleton"
                else:
                    category = "rust-immutable"
                findings.append(
                    _finding(category, root, path, index + 1, symbol, full_source, statement, full_source)
                )
                # Do not re-scan continuation lines as independent declarations,
                # but retain them for brace depth and legacy-reference scanning.
                declaration_continuations.update(range(index + 1, min(end + 1, len(masked))))

        for match in RUST_LEGACY_RE.finditer(masked_line):
            token = match.group(0)
            key = (index + 1, token)
            if key not in seen_legacy:
                findings.append(_finding("legacy-accessor", root, path, index + 1, token, original_line, token))
                seen_legacy.add(key)
        for regex, category in ((RUST_ACCESSOR_RE, "legacy-accessor"), (RUST_SPAWN_RE, "legacy-task")):
            for match in regex.finditer(masked_line):
                token = match.group(0).strip()
                key = (index + 1, category + ":" + token)
                if key not in seen_legacy:
                    findings.append(_finding(category, root, path, index + 1, token.rstrip("("), original_line, token))
                    seen_legacy.add(key)

        brace_depth += masked_line.count("{") - masked_line.count("}")
        if thread_base is not None and brace_depth <= thread_base:
            thread_base = None
    return findings


def _js_findings(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    masked = _masked_code(text, "js").splitlines()
    findings: list[Finding] = []
    brace_depth = 0
    declaration_continuations: set[int] = set()
    for index, masked_line in enumerate(masked):
        original_line = lines[index] if index < len(lines) else ""
        before = brace_depth
        decl_match = None if index in declaration_continuations else JS_DECL_RE.search(masked_line.strip())
        if decl_match:
            symbol = decl_match.group("symbol")
            init = decl_match.group("init")
            statement, end = _joined_statement(masked, index)
            parsed = JS_DECL_RE.search(statement.strip().rstrip(";"))
            if parsed:
                init = parsed.group("init")
                collection = bool(JS_COLLECTION_RE.search(init))
                module_binding = parsed.group("binding") in {"let", "var"}
                known_registry = bool(JS_REGISTRY_NAME_RE.search(symbol))
                # A closure-level registry is a process/page singleton too;
                # only report nested collections with an ownership-shaped name.
                if (collection and (before == 0 or known_registry)) or (module_binding and before == 0):
                    category = "js-module-registry" if collection else "js-module-binding"
                    full_source, _ = _joined_statement(lines, index)
                    findings.append(
                        _finding(category, root, path, index + 1, symbol, full_source, statement, full_source)
                    )
                    # Preserve continuation lines for scope tracking and other
                    # references while suppressing duplicate declarations.
                    declaration_continuations.update(range(index + 1, min(end + 1, len(masked))))

        global_match = JS_GLOBAL_WRITE_RE.search(masked_line)
        if global_match:
            target = global_match.group("global")
            findings.append(
                _finding("js-global-write", root, path, index + 1, target, original_line, global_match.group(0))
            )

        brace_depth += masked_line.count("{") - masked_line.count("}")
    return findings


def scan(root: Path) -> list[Finding]:
    rust_files = _files(root, RUST_ROOTS, {".rs"})
    for base in RUST_GLOB_ROOTS:
        rust_files.extend(_files(root, (base,), {".rs"}))
    rust_files = sorted(set(rust_files))
    js_files = _files(root, JS_ROOTS, {".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"})
    findings: list[Finding] = []
    for path in rust_files:
        findings.extend(_rust_findings(root, path))
    for path in js_files:
        findings.extend(_js_findings(root, path))
    return sorted(findings, key=lambda item: (item.path, item.line, item.category, item.symbol, item.match))


def _load_allowlist(path: Path) -> list[dict[str, object]]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return []
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read allowlist {path}: {exc}") from exc
    if not isinstance(payload, dict) or payload.get("version") != 1 or not isinstance(payload.get("entries"), list):
        raise ValueError("allowlist must be an object with version 1 and an entries array")
    entries = payload["entries"]
    if not all(isinstance(entry, dict) for entry in entries):
        raise ValueError("allowlist entries must be objects")
    return entries  # type: ignore[return-value]


FORBIDDEN_ALLOWLIST_CATEGORIES = {"rust-static-mut", "legacy-accessor", "legacy-task"}


def _check_allowlist(findings: list[Finding], entries: list[dict[str, object]]) -> list[str]:
    errors: list[str] = []
    by_key: dict[tuple[str, str, str], list[tuple[int, Finding]]] = {}
    for finding in findings:
        if finding.category != "rust-immutable":
            by_key.setdefault(finding.key, []).append((len(by_key.get(finding.key, [])) + 1, finding))
    entry_keys: set[tuple[str, str, str, int | None]] = set()
    matched: set[int] = set()
    for entry_index, entry in enumerate(entries):
        try:
            category = str(entry["category"])
            path = str(entry["path"])
            symbol = str(entry["symbol"])
            digest = str(entry["text_sha256"])
            disposition = str(entry["disposition"])
            reason = str(entry["reason"]).strip()
        except (KeyError, TypeError) as exc:
            errors.append(f"allowlist entry {entry_index + 1}: missing required field ({exc})")
            continue
        occurrence = entry.get("occurrence")
        if occurrence is not None and (not isinstance(occurrence, int) or occurrence < 1):
            errors.append(f"allowlist entry {entry_index + 1}: occurrence must be a positive integer")
            continue
        key = (category, path, symbol, occurrence)
        if key in entry_keys:
            errors.append(f"allowlist entry {entry_index + 1}: duplicate key {key}")
        entry_keys.add(key)
        if category in FORBIDDEN_ALLOWLIST_CATEGORIES:
            errors.append(f"allowlist entry {entry_index + 1}: {category} is never allowlisted")
            continue
        if category == "rust-immutable":
            errors.append(f"allowlist entry {entry_index + 1}: immutable inventory entries need no allowlist entry")
            continue
        if disposition != "diagnostic":
            errors.append(f"allowlist entry {entry_index + 1}: disposition must be diagnostic")
        if not reason:
            errors.append(f"allowlist entry {entry_index + 1}: reason must be non-empty")
        candidates = by_key.get((category, path, symbol), [])
        if occurrence is None and len(candidates) > 1:
            errors.append(f"allowlist entry {entry_index + 1}: ambiguous key {category}:{path}:{symbol}; add occurrence")
            continue
        selected = occurrence or 1
        if selected > len(candidates):
            errors.append(f"allowlist entry {entry_index + 1}: stale entry {category}:{path}:{symbol}")
            continue
        finding_number, finding = candidates[selected - 1]
        if finding.text_sha256 != digest:
            errors.append(
                f"allowlist entry {entry_index + 1}: hash mismatch for {category}:{path}:{symbol}"
            )
        matched.add(id(finding))
    for finding in findings:
        if finding.category == "rust-immutable":
            continue
        if id(finding) not in matched:
            errors.append(f"unallowlisted {finding.category}: {finding.path}:{finding.line} {finding.symbol}")
    return errors


def _print_text(findings: list[Finding]) -> None:
    print("runtime ownership inventory (lexical; manual review required)")
    for finding in findings:
        print(
            f"{finding.category}\t{finding.path}:{finding.line}\t{finding.symbol}"
            f"\tsha256={finding.text_sha256}\t{finding.match}"
        )
    counts: dict[str, int] = {}
    for finding in findings:
        counts[finding.category] = counts.get(finding.category, 0) + 1
    print("counts\t" + " ".join(f"{key}={counts[key]}" for key in sorted(counts)))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Inventory Rust and JS/TS runtime ownership candidates. This lexical "
            "scan cannot prove AST ownership; review macro expansion, aliases, "
            "generated code, and indirectly assembled registries manually."
        )
    )
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="repository root (default: script parent repository)")
    parser.add_argument("--allowlist", type=Path, default=DEFAULT_ALLOWLIST, help="reviewed JSON diagnostics allowlist")
    parser.add_argument("--format", choices=("text", "json"), default="text")
    parser.add_argument("--check", action="store_true", help="fail on unallowlisted non-immutable findings")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    findings = scan(root)
    if args.format == "json":
        print(json.dumps([finding.as_dict() for finding in findings], indent=2, sort_keys=True))
    elif not args.check:
        _print_text(findings)
    if not args.check:
        return 0
    try:
        errors = _check_allowlist(findings, _load_allowlist(args.allowlist.resolve()))
    except ValueError as exc:
        print(f"audit-runtime-ownership: {exc}", file=sys.stderr)
        return 2
    if errors:
        print(f"audit-runtime-ownership: {len(errors)} check failure(s)", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
