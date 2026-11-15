// The browser's answers to an intervention (Issue #306, under #292): the routes the screen's
// 「ここから別の『もしも』を試す」 calls, as the Python server answers them without an API key.
//
//   POST runs/{run}/branches/{branch}/intervention-preview   {minute, text}
//       src/sheltersim/web/workspace.py `intervention_preview`: the branch at the minute, the summary of its
//       world (Simulation.scenarioSummary, #302), the offline compiler's result
//       (compileInterventionOffline, #303), and for a compiled plan its EffectProgram
//       (effectProgramDump, #304), the program's checks on the world (validateEffectProgram),
//       the approval's hash and a draft (`preview_id`).
//   POST runs/{run}/branches/{branch}/intervention-apply     {preview_id, approved_program_hash,
//       approved, child_id}
//       src/sheltersim/web/workspace.py `intervention_apply`: the draft's checks, then the child
//       made at once, as the server's `ProjectAPI.fork` makes it (the branch at the minute, the
//       program applied, the child's stored Run written: format v6 of run_store.mjs, which reads
//       the parent's pieces up to the saved point the fork's world was restored from, its base).
//       The answer is {"status": "running", "branch_id": child, "minute": m}, and the outcome is
//       the Run's `last_fork` (GET runs/{run}): {"status": "created", "branch_id", "minute",
//       "state_hash"} or {"status": "failed", "error", "code"}.
//
// The checks, the answers, the draft and the outcome are the core's (`answers::interventions`,
// #449), which the local and VM versions call through PyO3; so is making the child (the core's
// `Procedure.fork`, #476, whose steps run_store.mjs's `StepHost` does). This module reads the files
// and the branch's Run for the preview and keeps the draft.
//
// A Run computed with other model versions is view-only: both routes refuse it as the server does
// (409 `run_model_view_only`, compute.mjs `requireCurrentModel`).
//
// The rest is as the server does it too (#427, #449; docs/project-persistence.md): a typed
// `program` (the screen never sends one) is refused, 409 `browser_unsupported`, after the checks of
// the branch, the minute and the model versions (the core's `check_typed`); `preview_id` is random
// (a uuid4's hex); the draft is kept in the Project's folder (`drafts/<preview_id>.json`) and is not
// part of a Project's bundle; the fork is not a computing job, so it never makes the Run `running`
// and does not count against the parallel computations. `last_fork` is kept in memory, by the
// page's Worker (`BrowserApi.forkOutcomes`) as by the server's process
// (`ProjectAPI._fork_outcomes`): it is gone when the page closes.

import { core } from "./api_answers.mjs";
import { requireCurrentModel } from "./compute.mjs";
import * as store from "./project_store.mjs";

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

/// The fork (the core's `Procedure.fork`): the parent at the minute, checked against the state
/// the preview saw, the program applied, and the child's stored Run and record. The child's stored
/// Run reads the parent's pieces up to the line the parent's world was restored from (its base),
/// and stores the rest from there: the records to the fork's minute, and a saved point there,
/// after the program was applied. Returns `last_fork` (the core's JSON).
async function fork(api, projectId, runId, parent, plan) {
  const { wasm, tree } = api;
  const minute = plan.minute();
  const childId = plan.childId();
  // The kept worlds go first: WASM's memory does not shrink.
  api.freeLive();
  // A draft holds the state its preview saw; one without it matches no state.
  const procedure = wasm.Procedure.fork(projectId, runId, parent.branch_id, childId, minute, plan.programText(), plan.hash(), plan.stateHash() ?? "");
  try {
    await store.stepHost(wasm, tree).finish(procedure);
    return wasm.createdOutcome(childId, minute, procedure.stateHash());
  } finally {
    procedure.free();
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
