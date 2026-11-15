//! The Project routes' answers (#449; `sheltersim_core::answers`): the edges of the parts the
//! browser's Node tests and `tests/integration/test_project_answers.py` do not reach: a
//! StorageError's code, the query as `parse_qs` reads it, the route's ints, a POST's body, and a
//! stored Run of an older format as a refusal.

use sheltersim_core::answers::query::{route_int, Query};
use sheltersim_core::answers::records::{is_id, parse_record, RunFiles};
use sheltersim_core::answers::{check_fields, post_document, storage_code, Failure, Refusal};
use sheltersim_core::run_store::outdated;

#[test]
fn a_storage_error_s_code_is_its_leading_word() {
    assert_eq!(storage_code("pdf_invalid_scope: 範囲"), "pdf_invalid_scope");
    assert_eq!(storage_code("invalid artifact identifier"), "storage_conflict");
    assert_eq!(storage_code("a: one letter"), "storage_conflict");
    assert_eq!(storage_code("Upper: case"), "storage_conflict");
    assert_eq!(Refusal::storage("stale_or_foreign_cursor").body(), br#"{"code":"stale_or_foreign_cursor","error":"stale_or_foreign_cursor"}"#);
    assert_eq!(Refusal::invalid_request().body(), br#"{"error":"invalid project request"}"#);
}

#[test]
fn the_query_is_read_as_parse_qs_reads_it() {
    let query = Query::parse("?a=1&b=&c&a=%E3%81%82+x&d=%zz&=e&f=%E3%81");
    let fields: Vec<(&str, Vec<&str>)> = query.fields.iter().map(|(name, values)| (name.as_str(), values.iter().map(String::as_str).collect())).collect();
    assert_eq!(fields, vec![("a", vec!["1", "あ x"]), ("d", vec!["%zz"]), ("", vec!["e"]), ("f", vec!["\u{fffd}"])]);
    assert_eq!(query.one("a"), None);
    assert_eq!(query.one("d"), Some("%zz"));
    assert!(Query::parse("x").is_empty());
}

#[test]
fn a_route_s_int_is_the_server_s() {
    assert_eq!(route_int(" +1_0 ", "bad"), Ok(10));
    assert_eq!(route_int("-3", "bad"), Ok(-3));
    for text in ["", "1.5", "abc", "1e3", "99999999999999999999"] {
        assert_eq!(route_int(text, "bad"), Err(Refusal::storage("bad")), "{text}");
    }
}

fn refused<T: std::fmt::Debug>(result: Result<T, Failure>) -> (u16, String) {
    match result {
        Err(Failure::Refused(refusal)) => (refusal.status, refusal.error),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_post_s_body_and_its_fields_are_checked_in_the_server_s_order() {
    assert_eq!(refused(post_document(None)), (409, "project request exceeds the body limit".to_owned()));
    assert_eq!(refused(post_document(Some(b"{bad"))), (400, "invalid project request".to_owned()));
    assert_eq!(refused(post_document(Some(b"[1]"))), (409, "project request must be an object".to_owned()));
    let document = post_document(Some(br#"{"a":1,"b":2}"#)).unwrap();
    assert!(check_fields(&document, &["a"], &["b"]).is_ok());
    assert_eq!(refused(check_fields(&document, &["a"], &[])), (409, "request fields do not match this operation".to_owned()));
    assert_eq!(refused(check_fields(&document, &["a", "b", "c"], &[])), (409, "request fields do not match this operation".to_owned()));
}

#[test]
fn records_and_ids_are_the_store_s() {
    assert!(is_id("a-1.b:c_d"));
    assert!(!is_id("../x") && !is_id("") && !is_id(".a") && !is_id(&"a".repeat(65)));
    assert!(parse_record(br#"{"schema_version":1.0}"#).is_ok());
    let refused = |bytes: &[u8]| match parse_record(bytes) {
        Err(Failure::Refused(refusal)) => refusal.error,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused(b"{"), "saved artifact is missing or corrupt");
    assert_eq!(refused(b"\xff"), "saved artifact is missing or corrupt");
    assert_eq!(refused(br#"{"schema_version":2}"#), "unsupported saved artifact version; migration required");
    // A branch is found by its record's id; its folder's name is not checked.
    let files = RunFiles { run: Some(br#"{"schema_version":1}"#.to_vec()), branches: vec![] };
    match sheltersim_core::answers::records::branch_of("p", "r", &files, "../x") {
        Err(Failure::Refused(refusal)) => assert_eq!(refusal.error, "saved artifact is missing or corrupt"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_stored_run_of_an_older_format_is_refused() {
    match Failure::from(outdated("sheltersim-browser-run-v4")) {
        Failure::Refused(refusal) => {
            assert_eq!((refusal.status, refusal.code.as_deref()), (409, Some("stored_run_outdated")));
            assert!(refusal.error.starts_with("stored_run_outdated: "));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fork_s_outcome_is_the_server_s() {
    use sheltersim_core::answers::interventions::{created_outcome, failed_outcome};
    assert_eq!(created_outcome("c1", 400, "h"), br#"{"branch_id":"c1","minute":400,"state_hash":"h","status":"created"}"#);
    let refusal = Refusal::new(409, "source state changed; preview the intervention again", None);
    assert_eq!(
        failed_outcome(&refusal.error, Some(&refusal)),
        br#"{"code":"storage_conflict","error":"source state changed; preview the intervention again","status":"failed"}"#
    );
    assert_eq!(failed_outcome("EffectProgram could not be scheduled: x", None), br#"{"code":null,"error":"invalid project request","status":"failed"}"#);
    assert_eq!(failed_outcome("boom", None), br#"{"code":null,"error":"project operation failed","status":"failed"}"#);
}

#[test]
fn a_question_is_checked_before_its_branch() {
    use sheltersim_core::answers::questions::QuestionPlan;
    for body in
        [&br#"{"question":"   "}"#[..], br#"{"question":"x","extra":1}"#, br#"{"question":"x","minute":1.5}"#, br#"{"question":"x","demo_branch_id":"d"}"#]
    {
        assert_eq!(refused(QuestionPlan::new(Some(body), "p", "r", "b")), (400, "invalid project request".to_owned()), "{}", String::from_utf8_lossy(body));
    }
    let mut plan = QuestionPlan::new(Some(br#"{"question":"x","minute":5.0}"#), "p", "r", "b").unwrap();
    assert_eq!(refused(plan.check_branch(&RunFiles::default())), (409, "saved artifact is missing or corrupt".to_owned()));
}
