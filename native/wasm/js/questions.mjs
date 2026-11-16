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
//
// With the relay's passphrase (`llm`, the home's 「API を有効にする」), the question is asked of the
// LLM as the Python server asks it with an API key (#470): the core builds the evidence packet and
// the task (`QuestionPlan.ask`: the frame, the comparison's answer, the Run's `evidence.json`, the
// Scenario, the branch's earlier turns), the site's relay answers it (relay_link.mjs), and the core
// checks the answer (`Asking.answer`, mode `record`). A relay that does not let the passphrase in
// leaves the answer without an API key.

import { core } from "./api_answers.mjs";
import { comparisonContext, compareResponse } from "./comparison.mjs";
import * as store from "./project_store.mjs";
import { askRelay } from "./relay_link.mjs";

function randomHex(bytes) {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/// `POST …/questions` (`body` the request's text, `llm` the relay's passphrase or null): the
/// answer's JSON text.
export async function questionResponse(api, projectId, runId, branchId, body, llm = null) {
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
    const random = randomHex(12);
    const turnId = `t-${Date.now()}000000-${randomHex(4)}`;
    const asked = llm ? await askLlm(api, projectId, runId, branchId, plan, { frame, compared, minute, scenario, random, turnId, passphrase: llm }) : null;
    const answer = asked?.answer ?? core(() => plan.answer(random, turnId));
    await store.keepTurn(wasm, tree, projectId, runId, branchId, asked?.turn ?? plan.turn());
    return answer;
  } finally {
    plan.free();
  }
}

/// The question asked through the relay: `{ answer, turn }`, or null where the relay is no link.
async function askLlm(api, projectId, runId, branchId, plan, { frame, compared, minute, scenario, random, turnId, passphrase }) {
  const { wasm, tree } = api;
  const comparison = compared.length > 0 ? await compareResponse(api, projectId, runId, `branch_ids=${compared.join(",")}&minute=${minute}`) : undefined;
  const evidence = (await tree.read([projectId, "runs", runId, "evidence.json"])) ?? undefined;
  const turns = await store.conversationPieces(wasm, tree, projectId, runId, branchId);
  let asking;
  try {
    asking = core(() => plan.ask(frame, comparison, evidence, scenario, turns, random));
  } finally {
    turns.free();
  }
  try {
    const reply = await askRelay(api.relay, passphrase, core(() => asking.task()));
    if (reply === null) return null;
    const answer = core(() => asking.answer(reply, "record", turnId));
    return { answer, turn: asking.turn() };
  } finally {
    asking.free();
  }
}
