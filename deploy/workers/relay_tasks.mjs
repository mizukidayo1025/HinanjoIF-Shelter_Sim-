// The tasks the LLM relay (relay.mjs, #419) answers at `/llm/v1/tasks/{name}`: each one builds its
// prompt here, in the relay, from the page's `input`, so the page can send data only, never a
// prompt of its own.
//
// A task: {
//   model,            a model of relay.mjs's PRICES (Python's model ids)
//   promptVersion,    recorded with the answer, as Python's tasks record theirs
//   maxOutputTokens,  the answer's size (thinking's room is added on Opus)
//   maxPromptBytes,   the UTF-8 bytes of { system, messages } it accepts
//   effort?,          output_config.effort (Python asks "low" for preparation)
//   disableThinking?, thinking: { type: "disabled" } (Haiku's runtime tasks)
//   timeoutMs?,       how long the call may take (120 s by default)
//   build(input),     → { system, messages }, or throws relay.mjs's TaskInputError
// }
//
// scenario_text (#421): the Scenario screen's text read as Python's `parse_scenario_text` asks the
// LLM to read it (llm/region_prepare.py). The task is Python's, written by
// native/core/tools/scenario_text_task.py into scenario_text_task.json: the model and effort of
// the PREPARE stage, the answer's size, the system prompt with the schema as Python writes it,
// and the instruction the text follows after a blank line. The core checks the answer against the
// same file's schema (`sheltersim_core::scenario::text::parse_answer`); the relay only passes the
// answer's text on.

import scenarioText from "../../native/core/src/scenario/scenario_text_task.json" with { type: "json" };
import { TaskInputError } from "./relay.mjs";

/// Python's `str.isspace` of a character (`text.strip()` takes these off).
const SPACE = /^[\s\x1c-\x1f\x85]$/u;

/// The input of `scenario_text`: `{ text }`, 1 to 4,000 code points, not only white space.
function scenarioPrompt(input) {
  const text = input?.text;
  if (typeof text !== "string") throw new TaskInputError("読み取る文がありません。");
  const chars = Array.from(text);
  if (chars.length > scenarioText.max_input_characters) throw new TaskInputError("文が長すぎます。4,000字以内にしてください。");
  if (chars.every((char) => SPACE.test(char) && char !== "﻿")) throw new TaskInputError("読み取る文がありません。");
  return {
    system: scenarioText.system,
    messages: [{ role: "user", content: `${scenarioText.instruction}\n\n${text}` }],
  };
}

export const TASKS = Object.freeze({
  [scenarioText.task]: Object.freeze({
    model: scenarioText.model,
    promptVersion: scenarioText.prompt_version,
    maxOutputTokens: scenarioText.max_output_tokens,
    // The system prompt (about 23 KB) and the instruction, and 4,000 characters of 4 bytes each.
    maxPromptBytes: 48 * 1024,
    effort: scenarioText.effort,
    build: scenarioPrompt,
  }),
});
