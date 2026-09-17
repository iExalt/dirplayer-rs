"""Select the exact test artifact from a successful Cargo JSON build receipt."""
import json
from pathlib import Path
import sys

messages = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines() if line.strip()]
if not any(m.get("reason") == "build-finished" and m.get("success") is True for m in messages):
    raise SystemExit("Cargo receipt does not record a successful build")
artifacts = {name for m in messages if m.get("reason") == "compiler-artifact"
             and m.get("target", {}).get("name") == "mod"
             and "test" in m.get("target", {}).get("kind", [])
             and m.get("profile", {}).get("test") is True
             for name in m.get("filenames", []) if name.endswith(".wasm")}
if len(artifacts) != 1:
    raise SystemExit(f"Expected one mod test WASM artifact, found {len(artifacts)}")
print(artifacts.pop())
