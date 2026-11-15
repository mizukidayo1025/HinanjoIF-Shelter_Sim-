//! The answers of a branch's routes that read only records and kept documents: `llm-status`,
//! `conversations`, `pdf-report` and `delete` (#376, #309).

use serde_json::Value;

use super::query::no_query;
use super::records::RunFiles;
use super::{check_fields, js_integer, post_document, write_value, Answer, Failure, Obj, Refusal};
use crate::error::Error;
use crate::json::write_json_string;

/// `GET …/branches/{branch}/llm-status`: an offline Run's answer (its router holds no records),
/// from the branch's record (`branch.json`, None where missing).
pub fn llm_status_answer(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<&[u8]>) -> Answer {
    no_query(query)?;
    let files = RunFiles { run: None, branches: branch.map(|bytes| vec![(branch_id.to_owned(), bytes.to_vec())]).unwrap_or_default() };
    let record = files.branch_record(project_id, run_id, branch_id)?;
    let mut out = b"{\"branch_id\":".to_vec();
    write_json_string(&mut out, branch_id);
    out.extend_from_slice(b",\"checkpoint_hash\":");
    write_value(&mut out, record.get("checkpoint_hash").unwrap_or(&Value::Null));
    out.extend_from_slice(b",\"computed_until\":");
    write_integer(&mut out, record.get("computed_until"))?;
    out.extend_from_slice(
        b",\"mode\":\"offline\",\"outcomes\":{\"accepted_count\":0,\"call_count\":0,\"fallback_by_error_kind\":{},\"fallback_by_prompt_version\":{},\
\"fallback_by_reason\":{},\"fallback_count\":0,\"first_fallback_call_index\":null,\"offline_rule_count\":0,\"status\":\"not_requested\",\
\"summary_schema_version\":1},\"record_scope\":\"router records in the branch head checkpoint; a child branch includes the parent's records \
before its fork\",\"schema_version\":1}",
    );
    Ok(out)
}

/// `GET …/branches/{branch}/llm-status` of a Run computed with an LLM (#463): the Run's LLM mode
/// (`record`, `replay`, from its `llm.json`) and the outcomes of the calls its stored tape holds
/// (its `llm_tape` records, from the Run's start: a child branch's include its parent's before the
/// fork), counted as `summarize_call_outcomes` (src/sheltersim/llm/outcomes.py) counts a router's
/// records. A Run without an LLM answers `llm_status_answer`'s.
pub fn llm_run_status_answer(query: &str, project_id: &str, run_id: &str, branch_id: &str, branch: Option<&[u8]>, mode: &str, tape: &[Vec<u8>]) -> Answer {
    no_query(query)?;
    let files = RunFiles { run: None, branches: branch.map(|bytes| vec![(branch_id.to_owned(), bytes.to_vec())]).unwrap_or_default() };
    let record = files.branch_record(project_id, run_id, branch_id)?;
    let mut accepted = 0i64;
    let mut offline = 0i64;
    let mut calls = 0i64;
    let mut by_reason = std::collections::BTreeMap::<String, i64>::new();
    let mut by_prompt = std::collections::BTreeMap::<String, i64>::new();
    let mut first: Option<i64> = None;
    for raw in tape {
        let entry = crate::llm::Entry::from_json(raw).map_err(Failure::from)?;
        calls += 1;
        let outcome = entry.reply.outcome.as_str();
        if outcome == "offline" {
            offline += 1;
            continue;
        }
        if matches!(outcome, "recorded" | "cache_hit") && !entry.reply.fallback_used {
            accepted += 1;
            continue;
        }
        *by_reason.entry(outcome.to_owned()).or_default() += 1;
        *by_prompt.entry(entry.prompt_version.clone()).or_default() += 1;
        first.get_or_insert(calls - 1);
    }
    let fallback: i64 = by_reason.values().sum();
    let status = if calls == 0 {
        "not_requested"
    } else if accepted + fallback == 0 {
        "offline_rules"
    } else if fallback == 0 {
        "all_accepted"
    } else if accepted == 0 {
        "all_fallback"
    } else {
        "degraded"
    };
    let counts = |counts: &std::collections::BTreeMap<String, i64>| {
        let mut object = Obj::new();
        for (key, count) in counts {
            object.int(key, *count);
        }
        object.finish()
    };
    let mut outcomes = Obj::new();
    outcomes.int("accepted_count", accepted).int("call_count", calls).raw("fallback_by_error_kind", counts(&by_reason));
    outcomes.raw("fallback_by_prompt_version", counts(&by_prompt)).raw("fallback_by_reason", counts(&by_reason)).int("fallback_count", fallback);
    outcomes.raw("first_fallback_call_index", first.map_or(b"null".to_vec(), |index| index.to_string().into_bytes()));
    outcomes.int("offline_rule_count", offline).str("status", status).int("summary_schema_version", 1);
    let mut answer = Obj::new();
    let mut hash = Vec::new();
    write_value(&mut hash, record.get("checkpoint_hash").unwrap_or(&Value::Null));
    let mut until = Vec::new();
    write_integer(&mut until, record.get("computed_until"))?;
    answer.str("branch_id", branch_id).raw("checkpoint_hash", hash).raw("computed_until", until).str("mode", mode).object("outcomes", outcomes);
    answer.str("record_scope", "LLM tape entries of the branch's stored Run; a child branch includes the parent's entries before its fork");
    answer.int("schema_version", 1);
    Ok(answer.finish())
}

/// An integer of an answer the browser wrote with its keys sorted: a safe integer, else the
/// browser's error (500).
fn write_integer(out: &mut Vec<u8>, value: Option<&Value>) -> Answer<()> {
    match value.and_then(super::js_safe_integer) {
        Some(number) => {
            out.extend_from_slice(number.to_string().as_bytes());
            Ok(())
        }
        None => Err(Failure::Error(Error::Value("not an integer of an answer".to_owned()))),
    }
}

/// How many of a branch's questions and answers `conversations` gives: the latest.
pub const CONVERSATION_TURNS: usize = 100;

/// `GET …/branches/{branch}/conversations`, after the branch is found (`records::branch_of`): the
/// turns kept in the branch's stored Run, read from its "conversations" pieces (each piece's bytes
/// decompressed, in the order of its journal; oldest first), the latest 100 answered, each with
/// the schema version its store adds (2 for a turn with `claims`, else 1).
pub fn conversations_answer(pieces: &[&[u8]]) -> Answer {
    let mut turns = Vec::new();
    for piece in pieces {
        turns.extend(crate::run_store::records::read(piece)?);
    }
    let mut out = b"{\"turns\":[".to_vec();
    let start = turns.len().saturating_sub(CONVERSATION_TURNS);
    for (index, turn) in turns[start..].iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        let Ok(Value::Object(turn)) = serde_json::from_slice::<Value>(turn) else {
            return Err(Failure::Error(Error::Value("a kept turn is not a JSON object".to_owned())));
        };
        let version: &[u8] = if turn.contains_key("claims") { b"2" } else { b"1" };
        out.extend_from_slice(b"{\"schema_version\":");
        match turn.get("schema_version") {
            Some(kept) => write_value(&mut out, kept),
            None => out.extend_from_slice(version),
        }
        for (key, value) in &turn {
            if key == "schema_version" {
                continue;
            }
            out.push(b',');
            write_json_string(&mut out, key);
            out.push(b':');
            write_value(&mut out, value);
        }
        out.push(b'}');
    }
    out.extend_from_slice(b"],\"coverage\":\"latest_100_turns\",\"history_route\":\"history?kind=conversations\"}");
    Ok(out)
}

/// `POST runs/{run}/pdf-report` (src/sheltersim/web/pdf_report.py `export`): the server's checks
/// in its order (the fields, the scope, the Run, each branch, the computed range), then what it
/// answers without an API key: the analysis needs Opus, so no PDF is made. Always a refusal.
/// `minute` is taken as JS takes it: 5.0 is 5.
pub fn pdf_report_answer(project_id: &str, run_id: &str, body: Option<&[u8]>, files: &RunFiles) -> Failure {
    match pdf_report_checks(project_id, run_id, body, files) {
        Ok(()) => Refusal::storage("pdf_analysis_offline: Opusの記録応答またはAPI接続が必要です。").into(),
        Err(failure) => failure,
    }
}

fn pdf_report_checks(project_id: &str, run_id: &str, body: Option<&[u8]>, files: &RunFiles) -> Answer<()> {
    let document = post_document(body)?;
    check_fields(&document, &["branch_ids", "minute"], &[])?;
    let scope = || Refusal::storage("pdf_invalid_scope: 単一BranchまたはA/Bの2分岐と整数分を指定してください。");
    let ids: Vec<&str> = match &document["branch_ids"] {
        Value::Array(items) if (1..=2).contains(&items.len()) => items.iter().map(Value::as_str).collect::<Option<_>>().ok_or_else(scope)?,
        _ => return Err(scope().into()),
    };
    let distinct = ids.len() < 2 || ids[0] != ids[1];
    let minute = js_integer(&document["minute"]).filter(|minute| *minute >= 0.0);
    let (Some(minute), true) = (minute, distinct) else {
        return Err(scope().into());
    };
    files.document(project_id, run_id)?;
    let mut records = Vec::with_capacity(ids.len());
    for id in &ids {
        records.push(files.branch_record(project_id, run_id, id)?);
    }
    let within = |record: &super::records::Record| {
        let at = |key: &str| record.get(key).and_then(super::js_number);
        matches!((at("fork_minute"), at("computed_until")), (Some(fork), Some(until)) if fork <= minute && minute <= until)
    };
    if !records.iter().all(within) {
        return Err(Refusal::storage("pdf_uncomputed_range: 指定時刻は共通の計算済み範囲外です。").into());
    }
    Ok(())
}

/// `POST runs/{run}/delete` (`ProjectAPI.delete_run` of src/sheltersim/web/projects.py): the Run's
/// id as the confirmation, no computation of it running (`active`, which the caller knows), and a
/// status that is not running. The answer, once the caller has removed the Run's folder.
pub fn delete_answer(project_id: &str, run_id: &str, body: Option<&[u8]>, active: bool, files: &RunFiles) -> Answer {
    let document = post_document(body)?;
    check_fields(&document, &["confirmation"], &[])?;
    if active {
        return Err(Refusal::storage("cannot delete an active run").into());
    }
    let (run, _) = files.document(project_id, run_id)?;
    if document["confirmation"].as_str() != Some(run_id) || run.get("status").and_then(Value::as_str) == Some("running") {
        return Err(Refusal::storage("deletion needs run ID confirmation and an inactive run").into());
    }
    let mut out = b"{\"deleted\":".to_vec();
    write_json_string(&mut out, run_id);
    out.push(b'}');
    Ok(out)
}
