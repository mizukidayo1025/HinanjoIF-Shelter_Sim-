"""A Scenario's seed is any int from 0, past 64 bits too.

`CounterRNG` keys its draws with `str(seed)`, and the arrivals of `add_evacuees` take their IDs
from `f"{world.seed}..."`. The core reads such a seed as its text; before, some of the kernels read
it as an i64 and refused, and the Python rule ran instead. These Runs of the gate's Scenarios (the
gate's builders, with only the seed changed) give the results pinned below.

The digests were first recorded from the Python rules (87b6f10), re-recorded for model version 2
(step 4, L11), and recorded again from the core's hashes when the gate moved to the core (#430):
the Runs of the core gave, label by label, the core's digests of the worlds the Python path ended
with. The population Scenario's Branches scheduled ahead are recorded at the gate's own seed
(`SCHEDULED`), so only its other runs change with the seed here.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import pytest
from support.paths import BENCHMARKS

# The gate Scenarios that draw: toilets in all of them, behavior, information, social propagation
# and proximity. The first has the generated Scenario's own models (sleep-aware-utility-softmax-v2
# and evidence-aware-rumor-v1).
SCENARIOS = (
    "model__behavior__sleep-aware-utility-softmax-v2",
    "model__behavior__utility-softmax-v1",
    "model__information__hazard-information-v1",
    "model__information__sparse-information-contact-v1",
    "model__social__degroot-contact-v1",
    "model__social_proximity__distance-sampled-proximity-v1",
)
# The arrivals and departures of the population Scenario: a test of its own, a long one.
SEEDS = (2**64 + 7, 2**63, 10**30)

# The first 16 hex digits of the SHA-256 of each Scenario's `{"refused": ..., "runs": ...}`
# (`_digest`). The same on every OS, as the gate's hashes.
DIGESTS: dict[int, dict[str, str]] = {
    2**64 + 7: {
        "model__behavior__sleep-aware-utility-softmax-v2": "fb33ad86a299662e",
        "model__behavior__utility-softmax-v1": "e35e2281d31084d0",
        "model__information__hazard-information-v1": "110e87dd00c44af8",
        "model__information__sparse-information-contact-v1": "56466f7db6575860",
        "model__social__degroot-contact-v1": "719b9063a227fd5d",
        "model__social_proximity__distance-sampled-proximity-v1": "367edae2324fd82e",
        "population": "762ad082654ed2bb",
    },
    2**63: {
        "model__behavior__sleep-aware-utility-softmax-v2": "a9ce63f32e031191",
        "model__behavior__utility-softmax-v1": "94e24acee1ed9f37",
        "model__information__hazard-information-v1": "cddc91b08ca1be05",
        "model__information__sparse-information-contact-v1": "4a2f22198e9ca33d",
        "model__social__degroot-contact-v1": "d0c58146e43a0aa7",
        "model__social_proximity__distance-sampled-proximity-v1": "aa26e854f123e9a3",
        "population": "456a04efd3b24e01",
    },
    10**30: {
        "model__behavior__sleep-aware-utility-softmax-v2": "d2d2f13fd1d20d69",
        "model__behavior__utility-softmax-v1": "4134246519f5b001",
        "model__information__hazard-information-v1": "6c7e1eaceb83678f",
        "model__information__sparse-information-contact-v1": "b5b23a9d044fdf18",
        "model__social__degroot-contact-v1": "ae1e66e69ff3f413",
        "model__social_proximity__distance-sampled-proximity-v1": "a4470782e85bbb77",
        "population": "bea35e32d50367b4",
    },
}


def _digest(outcome: Any) -> str:
    document = {"refused": outcome.refused, "runs": outcome.runs}
    text = json.dumps(document, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def _gate_scenarios() -> Any:
    sys.path.insert(0, str(BENCHMARKS))
    try:
        import gate_scenarios
    finally:
        sys.path.remove(str(BENCHMARKS))
    return gate_scenarios


def _run(seed: int, names: tuple[str, ...], directory: Path) -> dict[str, str]:
    """Each gate Scenario's digest with this seed."""
    gate = _gate_scenarios()
    with pytest.MonkeyPatch.context() as patch:
        patch.setitem(gate.generated.__kwdefaults__, "seed", seed)
        scenarios = gate.all_scenarios()
        return {name: _digest(scenarios[name].run(directory / name)) for name in names}


@pytest.mark.parametrize("seed", SEEDS)
def test_a_seed_past_64_bits_gives_the_pinned_results(seed: int, tmp_path: Path) -> None:
    assert _run(seed, SCENARIOS, tmp_path) == {name: DIGESTS[seed][name] for name in SCENARIOS}


@pytest.mark.long_simulation
def test_arrivals_and_departures_with_a_seed_past_64_bits_give_the_pinned_results(
    tmp_path: Path,
) -> None:
    """The gate's population Scenario, ten runs, for each seed."""
    for seed in SEEDS:
        digests = _run(seed, ("population",), tmp_path / str(seed))
        assert digests == {"population": DIGESTS[seed]["population"]}
