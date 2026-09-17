"""Prepare exact historical source and compatible harness without changing live code."""
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

PACKAGE = Path(__file__).resolve().parent
REPO = PACKAGE.parents[2]
SOURCE = "297da4a410495a1116e2c1b93ee57f9f1b5c8d79"
RUFFLE = "79d1ca0f45d79e28c3a0658bbc6b8430d26ef308"
RUFFLE_JS = "704f8d3004e402b46a8781496be8be3bbb7f334e3327e6085ac36b1a9aa206b5"


def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: prepare-historical.py NEW_SOURCE RUFFLE_BUNDLE | SOURCE --runner-only")
    stage = Path(sys.argv[1]).resolve()
    if sys.argv[2] == "--runner-only":
        receipt = json.loads((stage / "stage1-preparation.json").read_text())
        if receipt["source_revision"] != SOURCE:
            raise SystemExit("unexpected source revision")
        runner = stage / "vm-rust/target/browser_runner"
        runner.mkdir(parents=True, exist_ok=True)
        for original, target in [("vm-rust/tests/browser_templates/dirplayer-js-api.js", "dirplayer-js-api.js"), ("dirplayer-js-api/index.js", "dirplayer-js-api-real.js")]:
            shutil.copyfile(stage / original, runner / target)
        return
    bundle = Path(sys.argv[2]).resolve()
    if digest(bundle / "dirplayer_ruffle.js") != RUFFLE_JS:
        raise SystemExit("Ruffle JavaScript does not match the retained historical baseline")
    if not list(bundle.glob("*.wasm")):
        raise SystemExit("Ruffle companion WebAssembly is missing")
    stage.mkdir(parents=True, exist_ok=False)
    for repository, revision, destination in [(REPO, SOURCE, stage), (REPO / "ruffle", RUFFLE, stage / "ruffle")]:
        destination.mkdir(exist_ok=True)
        archive = subprocess.check_output(["git", "-C", str(repository), "archive", revision])
        with tarfile.open(fileobj=io.BytesIO(archive)) as entries:
            entries.extractall(destination, filter="data")
    (stage / "node_modules").symlink_to(REPO / "node_modules", target_is_directory=True)
    shutil.copytree(bundle, stage / "public/ruffle")
    original = PACKAGE / "run-historical-audio-baseline.mjs"
    adapted = stage / "scripts/run-historical-audio-baseline.mjs"
    code = original.read_text()
    lookup = 'execFileSync("git", ["-C", "ruffle", "rev-parse", "HEAD"], {cwd: repoRoot, encoding: "utf8"}).trim()'
    assert code.count(lookup) == 1
    adapted.write_text(code.replace(lookup, json.dumps(RUFFLE)))
    shutil.copyfile(PACKAGE / "test-sound-playback.mjs", stage / "scripts/test-sound-playback.mjs")
    receipt = {"source_revision": SOURCE, "ruffle_revision": RUFFLE,
               "historical_harness_sha256": digest(original), "adapted_harness_sha256": digest(adapted),
               "ruffle_bundle_source": str(bundle), "ruffle_bundle": {p.name: digest(p) for p in bundle.iterdir() if p.is_file()},
               "node_modules": str(REPO / "node_modules")}
    (stage / "stage1-preparation.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
