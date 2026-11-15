// The browser's answers to comparing branches (Issue #307, under #292), as the Python server
// answers them:
//
//   GET runs/{run}/compare?branch_ids=a,b&minute=m      src/sheltersim/web/workspace.py `comparison`
//   GET runs/{run}/replay-frames?branch_ids=a,b&start=&end=   web/replay.py `replay_frames` of two
//       branches: each minute's comparison
//
// The checks and the answers are the core's (`sheltersim_core::answers::frames`, #449): a
// comparison holds each branch's frame at the minute (the core's, as text), the common computed
// range, each branch's fork receipt (`fork_origin`: the "fork" line of a child's stored Run, #306;
// null for a branch not made by a fork), whether they share their origin, and each branch's KPI
// differences from the first. This module reads the branches' journals and makes their frames.
//
// Where the browser differs: a receipt's `checkpoint_hash` is the parent's head when the child was
// made (its journal line's sha256), not a Checkpoint's hash.

import { core } from "./api_answers.mjs";
import * as store from "./project_store.mjs";

/// The journals of a Run's branches, for the core (`wasm.Files`, which the caller frees): each
/// one's `journal.jsonl`, empty where it has none.
async function journals(api, projectId, runId, branchIds) {
  const files = new api.wasm.Files();
  for (const id of branchIds) files.add(id, (await api.tree.read([projectId, "runs", runId, "branches", id, "journal.jsonl"])) ?? new Uint8Array(0));
  return files;
}

/// Runs `use(object)` on WASM objects, then frees them.
async function using(objects, use) {
  try {
    return await use(...objects);
  } finally {
    for (const object of objects) object.free();
  }
}

/// `ProjectWorkspace.comparison_context`'s checks of comparing `branchIds` at `minute` (the core's
/// `comparison_range`), and the branches' fork receipts read: a refusal where they do not pass.
export async function comparisonContext(api, projectId, runId, branchIds, minute) {
  await using([await store.runFiles(api.wasm, api.tree, projectId, runId)], (files) =>
    core(() => api.wasm.comparisonRange(branchIds, Number.isInteger(minute) ? minute : undefined, projectId, runId, files)),
  );
  // The receipts are read as a comparison reads them (a stored Run of an older format is refused).
  await using([await journals(api, projectId, runId, branchIds)], (files) => core(() => api.wasm.forkReceipts(files)));
}

/// `GET runs/{run}/compare?branch_ids=&minute=`: `search` the request's query.
export async function compareResponse(api, projectId, runId, search) {
  const files = await store.runFiles(api.wasm, api.tree, projectId, runId);
  let plan;
  try {
    plan = core(() => api.wasm.comparePlan(search, projectId, runId, files));
  } finally {
    files.free();
  }
  return using([plan], async () => {
    const branchIds = plan.branchIds();
    await using([await journals(api, projectId, runId, branchIds)], (read) => core(() => plan.receipts(read)));
    const records = JSON.parse(plan.recordsJson());
    return using([new api.wasm.Files()], async (frames) => {
      for (const record of records) frames.addText(record.branch_id, await api.frameText(projectId, runId, record, plan.minute()));
      return core(() => plan.answer(runId, frames));
    });
  });
}

/// `replay-frames` of two branches: the comparison of each minute from the plan's start to its
/// end, after the checks of `BrowserApi.replay` (the core's `ReplayPlan`).
export async function replayPair(api, projectId, runId, plan) {
  const branchIds = plan.branchIds();
  await using([await store.runFiles(api.wasm, api.tree, projectId, runId), await journals(api, projectId, runId, branchIds)], (files, read) =>
    core(() => plan.receipts(projectId, runId, files, read)),
  );
  const records = JSON.parse(plan.recordsJson());
  return using([new api.wasm.Files()], async (samples) => {
    for (let minute = plan.start(); minute <= plan.end(); minute += 1) {
      const text = await using([new api.wasm.Files()], async (frames) => {
        for (const record of records) frames.addText(record.branch_id, await api.frameText(projectId, runId, record, minute));
        return core(() => plan.compare(runId, minute, frames));
      });
      samples.addText("", text);
    }
    return core(() => plan.answer(samples));
  });
}
