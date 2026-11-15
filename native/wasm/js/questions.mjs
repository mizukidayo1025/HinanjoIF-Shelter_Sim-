// The browser's answer to a question about a branch's results (Issue #308, under #292):
// `POST runs/{run}/branches/{branch}/questions`, as the Python server answers it without an API key
// (src/sheltersim/web/workspace.py `question`): the request's checks, the
// evidence's checks (a completed branch, the minute, the selected person, the branches compared),
// the fixed answer that no question can be answered without the Claude API, the context of the
// question and its id, and the turn kept beside the branch.
//
// The checks, the answer and the turn are the core's (`answers::questions`, #449), which the local
// and VM versions call through PyO3; this module reads the branch's record, the Run's Scenario and
// the frame, gives the random digits and the time, and keeps the turn in the branch's stored Run
// (a "conversations" piece after its "end" line, the core's `run_store`, #368).
//
// The context's id is the server's: "c" + the first 32 hex digits of the sha256 of the context's
// JSON, "-", and 24 random hex digits, or the id the request gave when it is of the same context.
// The question is kept as the server keeps it (contact details and entity names redacted).
//
// A `history_query` (questions citing saved history pages) is an unknown field to both since #376
// (#368 decision A).

import { core } from "./api_answers.mjs";
import { comparisonContext } from "./comparison.mjs";
import * as store from "./project_store.mjs";

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// `POST …/questions` (`body` the request's text): the answer's JSON text.
export async function questionResponse(api, projectId, runId, branchId, body) {
  const { wasm, tree } = api;
  const plan = core(() => wasm.questionPlan(body ?? undefined, projectId, runId, branchId));
  try {
    // _project_evidence
    const files = await store.runFiles(wasm, tree, projectId, runId);
    try {
      core(() => plan.checkBranch(files));
    } finally {
      files.free();
    }
    const scenario = await store.runScenarioBytes(tree, projectId, runId);
    core(() => plan.evidence(scenario));
    const minute = plan.minute();
    const frame = await api.frameText(projectId, runId, await api.branch(projectId, runId, branchId), minute);
    const compared = core(() => plan.checkFrame(frame));
    if (compared.length > 0) await comparisonContext(api, projectId, runId, compared, minute);
    const answer = core(() => plan.answer(randomHex(12), `t-${Date.now()}000000-${randomHex(4)}`));
    await store.keepTurn(wasm, tree, projectId, runId, branchId, plan.turn());
    return answer;
  } finally {
    plan.free();
  }
}
