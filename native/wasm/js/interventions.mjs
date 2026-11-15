// The browser's answers to an intervention (Issue #306, under #292): the routes the screen's
// 「ここから別の『もしも』を試す」 calls, as the Python server answers them without an API key.
//
//   POST runs/{run}/branches/{branch}/intervention-preview   {minute, text}
//       src/sheltersim/web/workspace.py `intervention`: the branch at the minute, the summary of its
//       world (Simulation.scenarioSummary, #302), the offline compiler's result
//       (compileInterventionOffline, #303), and for a compiled plan its EffectProgram
//       (effectProgramDump, #304), the program's checks on the world (validateEffectProgram),
//       the approval's hash and a draft (`preview_id`).
//   POST runs/{run}/branches/{branch}/intervention-apply     {preview_id, approved_program_hash,
//       approved, child_id}
//       the draft's checks, then ProjectAPI.fork as the server's process worker answers it:
//       {"status": "running", "branch_id": child, "minute": m}. The child is made at once (the
//       branch at the minute, the program applied, the child's stored Run written: format v5 of
//       run_store.mjs, which reads the parent's pieces up to the saved point the fork's world was
//       restored from, its base) and the outcome is the Run's `last_fork` (GET runs/{run}), as the
//       server's is once its worker ends: {"status": "created", "branch_id", "minute",
//       "state_hash"} or {"status": "failed", "error", "code"}.
//
// The checks, the answers, the draft and the outcome are the core's (`answers::interventions`,
// #449), which the local and VM versions call through PyO3; this module reads the files and the
// branch's Run, keeps the draft, and makes the child.
//
// A Run computed with other model versions is view-only: both routes refuse it as the server does
// (409 `run_model_view_only`, compute.mjs `requireCurrentModel`).
//
// Where the browser differs from the server (docs/project-persistence.md lists these):
// - A typed `program` (the screen never sends one) is refused, 409 `browser_unsupported`, after the
//   checks of the branch and the minute: pydantic's texts for a program it refuses are not made here.
// - `preview_id` is random, as the server's (uuid4); a draft is kept in the Project's folder
//   (`drafts/<preview_id>.json`) and is not part of a Project's bundle.
// - The fork is not a job: it is done before the answer, so the Run never shows `running` for it,
//   and another branch computing does not count against the server's two workers.
// - `last_fork` is kept by the page's Worker, as the server keeps it in its process: it is gone
//   when the page closes.

import { core } from "./api_answers.mjs";
import { requireCurrentModel } from "./compute.mjs";
import * as store from "./project_store.mjs";
import { RunWriter } from "./run_store.mjs";

const encoder = new TextEncoder();

const draftPath = (projectId, previewId) => [projectId, "drafts", `${previewId}.json`];

/// A uuid4's hex, as the server's `preview_id`.
function previewId() {
  return crypto.randomUUID().replaceAll("-", "");
}

/// `POST …/intervention-preview` (`body` the request's text): the answer's JSON text. The core
/// checks the request and writes the answer and the draft (`answers::interventions`); this reads
/// the branch's Run at the minute and keeps the draft.
export async function interventionPreview(api, projectId, runId, branchId, body) {
  const { wasm, tree } = api;
  const files = await store.runFiles(wasm, tree, projectId, runId);
  let plan;
  try {
    plan = core(() => wasm.previewPlan(body ?? undefined, projectId, runId, files, branchId));
  } finally {
    files.free();
  }
  try {
    await requireCurrentModel(wasm, tree, projectId, runId, branchId);
    core(() => plan.checkTyped());
    const simulation = await api.simulationAt(projectId, runId, JSON.parse(plan.recordJson()), plan.minute());
    const id = previewId();
    const answer = core(() => plan.answer(simulation, id));
    const draft = plan.draft();
    if (draft !== undefined) await tree.write(draftPath(projectId, id), draft);
    return answer;
  } finally {
    plan.free();
  }
}

/// `POST …/intervention-apply` (`body` the request's text): the answer's JSON text. The child is
/// made before it answers; its outcome is the Run's `last_fork` (the core's JSON).
export async function interventionApply(api, projectId, runId, branchId, body) {
  const { wasm, tree } = api;
  const draftId = core(() => wasm.applyDraftId(body ?? undefined));
  const draft = (await tree.read(draftPath(projectId, draftId))) ?? undefined;
  const busy = api.computing.jobs.has(`${projectId}/${runId}/${branchId}`);
  const plan = core(() => wasm.applyPlan(body ?? undefined, draft, projectId, runId, branchId, busy));
  try {
    // A Run of another model version is view-only: no fork, no EffectProgram.
    await requireCurrentModel(wasm, tree, projectId, runId, branchId);
    const files = await store.runFiles(wasm, tree, projectId, runId);
    let record;
    try {
      record = JSON.parse(core(() => plan.forkChecks(files)));
    } finally {
      files.free();
    }
    api.forkOutcomes.delete(`${projectId}/${runId}`);
    let outcome;
    try {
      outcome = await fork(api, projectId, runId, record, plan);
    } catch (error) {
      if (error instanceof store.ProjectStoreError) {
        outcome = wasm.failedOutcome(error.message, 409, error.code ?? undefined);
      } else {
        if (!String(error?.message ?? "").startsWith("EffectProgram could not be scheduled")) console.error(error);
        outcome = wasm.failedOutcome(String(error?.message ?? error), undefined, undefined);
      }
    }
    api.forkOutcomes.set(`${projectId}/${runId}`, outcome);
    return core(() => plan.answer());
  } finally {
    plan.free();
  }
}

/// `TimelineStore.fork`: the parent at the minute, checked against the state the preview saw, the
/// program applied, and the child's stored Run and record. The child's stored Run reads the
/// parent's pieces up to the line the parent's world was restored from (its base), and stores the
/// rest from there: the records and decision records to the fork's minute, and a saved point there,
/// after the program was applied. Returns `last_fork` (the core's JSON).
async function fork(api, projectId, runId, parent, plan) {
  const { wasm, tree } = api;
  const minute = plan.minute();
  const childId = plan.childId();
  // The kept worlds go first: WASM's memory does not shrink.
  api.freeLive();
  const reader = await store.branchReader(wasm, tree, projectId, runId, parent.branch_id);
  try {
    const { simulation, base } = await reader.seek(minute, { decisions: true });
    try {
      const origin = simulation.stateDigest();
      store.answered(() => plan.checkOrigin(origin));
      const programText = plan.programText();
      const receipt = simulation.applyEffectProgram(encoder.encode(programText));
      const backend = await store.branchBackend(tree, projectId, runId, childId);
      const writer = await RunWriter.create(wasm, backend);
      let head;
      try {
        writer.core.forkStart(reader.journal, base, minute, parent.branch_id, origin, parent.checkpoint_hash, programText, plan.hash(), receipt);
        await writer.flush();
        head = await writer.save(simulation);
      } finally {
        writer.close();
      }
      await store.writeBranch(tree, projectId, runId, { branch_id: childId, parent_branch_id: parent.branch_id, fork_minute: minute, computed_until: minute, checkpoint_hash: head, status: "paused" });
      return wasm.createdOutcome(childId, minute, simulation.stateDigest());
    } finally {
      simulation.free();
    }
  } finally {
    reader.close();
  }
}

/// The fork's receipt and origin a child's stored Run holds (its "fork" line), or null for a
/// branch not made by a fork.
export async function forkLine(api, projectId, runId, branchId) {
  const file = await (await store.branchBackend(api.tree, projectId, runId, branchId)).open("journal.jsonl");
  let journal;
  try {
    journal = new api.wasm.Journal(file.read(0, file.size()));
  } finally {
    file.close();
  }
  try {
    return JSON.parse(journal.linesJson()).find((line) => line.kind === "fork") ?? null;
  } finally {
    journal.free();
  }
}
