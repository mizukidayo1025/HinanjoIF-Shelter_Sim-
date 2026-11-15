"""Record the corpus of Scenarios the core's minute-0 builder is checked on again (#429).

Runs the offline suite (`tests/unit` and `tests/integration`) with the recording plugin
`support.initial_corpus`, which writes every Scenario `WorldState.from_scenario` is given to a
temporary directory, then writes `fixtures/initial_corpus.json.xz`: one Scenario per
fingerprint of its configs, with the digest of the world Python builds of it
(`tests/support/initial_corpus.py`).

Run from the repository root with the native extension built:
    SHELTERSIM_LLM_MODE=offline python benchmarks/initial_corpus.py [pytest options ...]
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main(options: list[str]) -> None:
    sys.path.insert(0, str(ROOT / "tests"))
    from support.initial_corpus import RECORD_DIRECTORY, write_corpus

    with tempfile.TemporaryDirectory(prefix="initial-corpus-") as directory:
        environment = {**os.environ, RECORD_DIRECTORY: directory, "SHELTERSIM_LLM_MODE": "offline"}
        command = [sys.executable, "-m", "pytest", "-q", "-p", "support.initial_corpus"]
        command += [*options, "tests/unit", "tests/integration"]
        subprocess.run(command, cwd=ROOT, env=environment, check=True)
        path = write_corpus(Path(directory))
    print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main(sys.argv[1:])
