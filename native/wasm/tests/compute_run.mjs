// A Run to compute, for the tests (node_compute.mjs, node_stored_runs.mjs): beside a Project's
// computed Run, a Run of the same Scenario that is not computed. Its records are the source Run's
// with its own id and Python's status of a Run just created ("queued"), and its stored Run holds
// the source's "run" line and its "export" piece (the minute-0 world Python built), the compressed
// bytes copied as they are. Its branch `main` is at minute 0, its head the "export" line.
//
// The site offered such a Run to compute beside the sample until #411 (js/compute.mjs's
// ensureComputeRun); the tests keep making one, as it computes to the source's digests without a
// Scenario the WASM builds a world from (the small_replay world has none).

import { readHeads } from "../js/compute.mjs";
import * as store from "../js/project_store.mjs";
import { RunReader, RunWriter } from "../js/run_store.mjs";

/// The id the tests give the Run to compute (the stored Runs' fixtures hold it).
export const COMPUTE_RUN = "run-browser-compute";

/// The first Run (by id) other than `runId` whose branch `main` has a stored Run with an exported
/// world.
async function sourceRun(wasm, tree, projectId, runId) {
  const { runs } = JSON.parse(await store.runsResponse(wasm, tree, projectId));
  for (const run of runs) {
    if (run.run_id === runId || !run.branches.some((branch) => branch.branch_id === "main" && branch.checkpoint_hash !== null)) continue;
    const backend = await store.branchBackend(tree, projectId, run.run_id, "main");
    if ((await readHeads(wasm, backend)).some((head) => head.kind === "export")) return run;
  }
  return null;
}

/// Makes the Run `runId` to compute in Project `projectId`; `{ runId, source }`, or null when the
/// Project has no Run to start from.
export async function makeComputeRun({ wasm, tree, projectId, runId = COMPUTE_RUN }) {
  const source = await sourceRun(wasm, tree, projectId, runId);
  if (!source) return null;
  const reader = await RunReader.open({ wasm, backend: await store.branchBackend(tree, projectId, source.run_id, "main") });
  let head;
  try {
    const compressed = await reader.compressed(reader.journal.find("export"));
    const writer = await RunWriter.create(wasm, await store.branchBackend(tree, projectId, runId, "main"));
    try {
      writer.core.sameStart(reader.journal);
      head = await writer.flush(compressed);
    } finally {
      writer.close();
    }
  } finally {
    reader.close();
  }
  const evidence = await tree.read([projectId, "runs", source.run_id, "evidence.json"]);
  if (evidence === null) throw new store.ProjectStoreError("saved artifact is missing or corrupt");
  await store.createRun(tree, projectId, {
    runId,
    scenarioId: source.scenario_id,
    scenarioVersion: source.scenario_version,
    seed: source.seed,
    modelVersions: source.model_versions,
    evidenceBytes: evidence,
    createdAt: source.created_at,
  });
  await store.writeBranch(tree, projectId, runId, { branch_id: "main", parent_branch_id: null, fork_minute: 0, computed_until: 0, checkpoint_hash: head, status: "queued" });
  return { runId, source: source.run_id };
}
