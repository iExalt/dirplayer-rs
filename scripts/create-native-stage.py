#!/usr/bin/env python3
"""Create a source-only native compiler stage with one shared Cargo cache."""

import argparse
import json
from pathlib import Path
import shlex
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    source = args.source.resolve()
    destination = args.destination.resolve()
    shared_target = Path("/tmp/dirplayer-parser-review-build/target").resolve()
    if destination.exists():
        parser.error("destination already exists; preserve or remove its source explicitly")
    if destination.is_relative_to(source) or source.is_relative_to(destination):
        parser.error("source and stage must be separate directories")
    for crate in ("vm-rust", "xtra-sdk"):
        if not (source / crate / "Cargo.toml").is_file():
            parser.error(f"missing source crate: {crate}")

    destination.mkdir(parents=True)
    ignored = shutil.ignore_patterns(
        "target", "node_modules", ".cache", ".git", "__pycache__", "pkg", ".DS_Store"
    )
    for crate in ("vm-rust", "xtra-sdk"):
        shutil.copytree(source / crate, destination / crate, ignore=ignored, symlinks=True)
    shutil.copy2(source / "mise.toml", destination / "mise.toml")
    config = destination / ".cargo"
    config.mkdir()
    (config / "config.toml").write_text(
        "[build]\ntarget-dir = " + json.dumps(str(shared_target)) + "\nincremental = false\n"
    )
    command = [
        "env", "CARGO_INCREMENTAL=0", "mise", "exec", "--", "cargo", "test", "--manifest-path",
        str(destination / "vm-rust/Cargo.toml"), "--lib", "--no-run", "--locked",
        "--offline", "--target-dir", str(shared_target),
    ]
    size = sum(p.stat().st_size for p in destination.rglob("*") if p.is_file() and not p.is_symlink())
    print(f"Created {destination} ({size / 1024**2:.1f} MiB of source)")
    print(f"Shared target: {shared_target}")
    print(shlex.join(command))


if __name__ == "__main__":
    main()
