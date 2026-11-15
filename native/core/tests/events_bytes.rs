//! The core's events: the queue, the writer's checks and the log's records. The expected values
//! were recorded before #430 and pin the core's output now. The queue's order is: due minute,
//! priority, sequence, and for equal keys the order scheduled.

use std::sync::Arc;

use indexmap::IndexMap;
use sheltersim_core::error::Error;
use sheltersim_core::events::log::{fields_of_bytes, EventLog};
use sheltersim_core::events::queue::Queue;
use sheltersim_core::events::writer::{self, WriteError};
use sheltersim_core::events::{EventKind, EventRow, EventStatus};
use sheltersim_core::value::V;

fn row(event_id: &str, due: i64, priority: i64, sequence: i64) -> EventRow {
    let mut payload = IndexMap::new();
    payload.insert(Arc::from("agent_id"), V::Str(Arc::from("a-1")));
    EventRow {
        event_id: Arc::from(event_id),
        kind: EventKind::Endogenous,
        due_minute: due,
        priority,
        sequence,
        event_type: Arc::from("test_event"),
        payload: V::Map(payload),
        source_id: None,
        causation_id: None,
        provenance: V::Map(IndexMap::new()),
    }
    .checked()
    .expect("a valid row")
}

#[test]
fn the_queue_orders_by_due_minute_priority_and_sequence() {
    let mut queue = Queue::new();
    for (id, due, priority, sequence) in [("c", 5, 0, 3), ("a", 1, 2, 4), ("b", 1, 0, 9), ("d", 5, 0, 1), ("e", 1, 2, 2)] {
        assert_eq!(queue.schedule_row(row(id, due, priority, sequence)), 0);
    }
    let ids = |queue: &Queue, minute| -> Vec<String> { queue.pending_ids(minute).iter().map(|id| id.to_string()).collect() };
    assert_eq!(ids(&queue, None), ["b", "e", "a", "d", "c"]);
    assert_eq!(ids(&queue, Some(1)), ["b", "e", "a"]);
    assert_eq!(queue.schedule_row(row("a", 0, 0, 10)), 1);
    assert_eq!(queue.schedule_row(row("f", 0, 0, 9)), 2);

    // Equal keys keep the order scheduled; a replaced event goes after its equals.
    let generation = queue.generation();
    let first = queue.add(Arc::from("g"), (5, 0, 7), None);
    let second = queue.add(Arc::from("h"), (5, 0, 7), None);
    assert!(queue.generation() > generation);
    assert_eq!(ids(&queue, None), ["b", "e", "a", "d", "c", "g", "h"]);
    queue.rekey(first, (5, 0, 7));
    assert_eq!(ids(&queue, None), ["b", "e", "a", "d", "c", "h", "g"]);
    assert!(queue.slot(first).row.is_none() && queue.slot(second).key == (5, 0, 7));

    let (_, slot) = queue.take_slot("e").expect("pending");
    assert_eq!(slot.key, (1, 2, 2));
    assert!(!queue.has_event("e") && !queue.has_sequence(2));
    assert_eq!(queue.len(), 6);
    assert_eq!(queue.validate(), 0);
}

#[test]
fn a_repeated_identity_from_the_constructor_is_refused() {
    let mut queue = Queue::new();
    queue.add_loaded(Arc::from("a"), (0, 0, 1));
    queue.add_loaded(Arc::from("a"), (0, 0, 2));
    assert_eq!(queue.validate(), 1);
    let mut queue = Queue::new();
    queue.add_loaded(Arc::from("a"), (0, 0, 1));
    queue.add_loaded(Arc::from("b"), (0, 0, 1));
    assert_eq!(queue.validate(), 2);
}

#[test]
fn the_writer_schedules_and_records_with_the_recorded_errors() {
    let mut queue = Queue::new();
    let mut log = EventLog::new();
    assert_eq!(writer::schedule(&mut queue, &log, 60, row("e-1", 10, 0, 0)), Ok(1));
    assert_eq!(
        writer::schedule(&mut queue, &log, 60, row("e-2", 60, 0, 1)),
        Err(WriteError::Error(Error::Value("event 'e-2' is outside the scenario".into())))
    );
    assert_eq!(writer::schedule(&mut queue, &log, 60, row("e-1", 20, 0, 5)), Err(WriteError::Duplicate("duplicate event ID 'e-1'".into())));
    let event = row("e-1", 10, 0, 0);
    writer::record(&mut log, &event, 10, EventStatus::Applied, None).unwrap();
    assert_eq!(writer::record(&mut log, &event, 10, EventStatus::Applied, None), Err(WriteError::Duplicate("event 'e-1' already realized".into())));
    assert_eq!(
        writer::record(&mut log, &row("e-3", 10, 0, 0), 10, EventStatus::Applied, None),
        Err(WriteError::Duplicate("event sequence 0 already realized".into()))
    );
    assert_eq!(
        writer::record(&mut log, &row("e-4", 10, 0, 4), -1, EventStatus::Applied, None),
        Err(WriteError::Error(Error::Value("realized_minute must be a non-negative integer".into())))
    );
    assert_eq!(writer::schedule(&mut queue, &log, 60, row("e-5", 20, 0, 0)), Err(WriteError::Duplicate("event sequence 0 already realized".into())));

    // The bytes written are a saved record that reads back to the same event.
    assert_eq!(log.len(), 1);
    let written = log.written(0).expect("written here").to_vec();
    assert_eq!(
        written,
        br#"{"detail":null,"event":{"causation_id":null,"due_minute":10,"event_id":"e-1","event_type":"test_event","kind":"endogenous","payload":{"agent_id":"a-1"},"priority":0,"provenance":{},"sequence":0,"source_id":null},"realized_minute":10,"status":"applied"}"#
    );
    let fields = fields_of_bytes(&written).expect("a saved record");
    assert_eq!((fields.event_id.as_str(), fields.sequence, fields.minute, fields.applied), ("e-1", 0, 10, true));
    let (read, minute, status, detail) = EventRow::from_saved_record(&written).expect("a saved record");
    assert_eq!((read, minute, status, detail), (event, 10, EventStatus::Applied, None));
    assert!(log.all_encoded());
}
