// The browser's answer to a question about a branch's results (Issue #308, under #292):
// `POST runs/{run}/branches/{branch}/questions`, as the Python server answers it
// (src/sheltersim/web/workspace.py `question`): the request's checks, the evidence's checks (a
// completed branch, the minute, the selected person, the branches compared), the question asked of
// the LLM, the context of the question and its id, and the turn kept beside the branch.
//
// The checks, the task and the answer are the core's (`answers::questions`, #449), which the local
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
// The question needs the relay's passphrase (`llm`, the home's 「API を有効にする」): the core
// builds the evidence packet and the task (`QuestionPlan.ask`: the frame, the comparison's answer,
// the Run's `evidence.json`, the Scenario, the branch's earlier turns), the site's relay answers it
// (relay_link.mjs), and the core checks the answer (`Asking.answer`, mode `record`), as the Python
// server asks it with an API key (#470). Without the passphrase the core refuses the question once
// it passes its checks (409 `question_offline`, as the server without an API key, #585). Where the
// relay does not answer (the passphrase not let in, no API key on the relay, the day's cap, the
// API busy or failing), the question fails with the reason (409 `question_llm_failed`): no
// fixed answer is made in its place and no turn is kept (#580).

import { ApiError, core } from "./api_answers.mjs";
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
    if (!llm) core(() => plan.offline());
    const random = randomHex(12);
    const turnId = `t-${Date.now()}000000-${randomHex(4)}`;
    const asked = await askLlm(api, projectId, runId, branchId, plan, { frame, compared, minute, scenario, random, turnId, passphrase: llm });
    await store.keepTurn(wasm, tree, projectId, runId, branchId, asked.turn);
    return asked.answer;
  } finally {
    plan.free();
  }
}

/// Why the relay gave no answer, by the router's outcome of its refusal (relay_link.mjs).
const RELAY_FAILURES = {
  budget_exceeded: "今日のAPIの上限に達しました。",
  rate_limited: "APIが混み合っています。しばらくしてから、もう一度質問してください。",
  input_token_limit: "質問の根拠が、AIに送れる大きさを超えました。",
};

function relayFailed(reason) {
  return new ApiError(409, `AIの中継から回答を得られませんでした。${reason}`, "question_llm_failed");
}

/// The question asked through the relay: `{ answer, turn }`; throws where the relay gave no answer.
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
    if (reply === null) throw relayFailed("合言葉が通らないか、中継にAPIキーがありません。ホームの「AIの機能（API）」で確かめてください。");
    const { fallback_used: failed, outcome } = JSON.parse(reply);
    if (failed) throw relayFailed(RELAY_FAILURES[outcome] ?? "中継かAPIで誤りが起きました。");
    const answer = core(() => asking.answer(reply, "record", turnId));
    return { answer, turn: asking.turn() };
  } finally {
    asking.free();
  }
}
