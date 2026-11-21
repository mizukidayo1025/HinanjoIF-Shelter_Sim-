//! The KPI history of a Run's stored samples (#266; `sheltersim_core::history`): the rows the
//! history index gives, paged as its query pages them. `native/wasm/tests/node_api_answers.mjs`
//! checks the whole answer against the Python server's on a Run; this checks the paging's edges.

use sheltersim_core::events::packed::Packer;
use sheltersim_core::history::{kpi_row, review_metrics, KpiQuery, KpiTable};
use sheltersim_core::plain::MetricSamples;
use sheltersim_core::rules::metrics::MetricSample;
use sheltersim_core::value::V;

fn sample(stress: f64) -> MetricSample {
    MetricSample {
        toilet_queue_length: 2,
        toilet_active_users: 1,
        toilet_working_units: 3,
        toilet_wait_count: 1.0,
        toilet_wait_total_minutes: 4.5,
        toilet_wait_mean_minutes: 4.5,
        toilet_wait_max_minutes: 4.5,
        stress_mean: stress,
        stress_high_count: 0,
        staff_assigned_task_count: 1,
        staff_busy_count: 1,
        staff_load_ratio: 0.5,
        unresolved_support_request_count: 0,
        source_ids_json: r#"{"busy_staff":["staff_001"]}"#.to_owned(),
        support_no_qualified_worker_count: 0,
        support_no_qualified_worker_request_minutes: 0.0,
        support_fairness_promoted_assignment_count: 0.0,
        support_wait_staff_busy_request_minutes: 0.0,
        support_wait_staff_off_shift_request_minutes: 0.0,
    }
}

/// A table of samples at the minutes 0 to 29, stored in blocks of the given size, as a Run in the
/// browser stores them, and read back.
fn table() -> KpiTable {
    let mut samples = MetricSamples::default();
    for minute in 0..30 {
        samples.push(&sample(minute as f64 / 100.0), minute, Some(&[[minute; 6], [-minute; 6]])).unwrap();
    }
    let (packer, blocks) = samples.store();
    let mut tables = Vec::new();
    packer.write_tables(0, 0, &mut tables);
    let mut table = KpiTable::new();
    table.read_tables(&tables).unwrap();
    for block in blocks.blocks() {
        table.read_block(block).unwrap();
    }
    assert_eq!(table.len(), 30);
    table
}

fn query<'a>(start: i64, end: i64, step: i64, limit: usize, after: Option<(i64, &'a str)>) -> KpiQuery<'a> {
    KpiQuery { start, end, step, limit, after, filtered: false, run_id: "r", branch_id: "main" }
}

fn minutes(items: &[u8]) -> Vec<i64> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(items).unwrap();
    rows.iter().map(|row| row["minute_index"].as_i64().unwrap()).collect()
}

#[test]
fn a_page_is_the_index_s() {
    let table = table();
    let page = table.page(&query(3, 20, 4, 3, None)).unwrap();
    assert_eq!(minutes(&page.items), [3, 7, 11]);
    assert_eq!((page.last, page.more), (Some(11), true));
    let next = table.page(&query(3, 20, 4, 3, Some((11, "11")))).unwrap();
    assert_eq!(minutes(&next.items), [15, 19]);
    assert!(!next.more);
    // The cursor's key compares as text with the row's minute: "5" comes after "10".
    assert_eq!(minutes(&table.page(&query(0, 29, 1, 3, Some((5, "10")))).unwrap().items), [5, 6, 7]);
    assert_eq!(minutes(&table.page(&query(0, 29, 1, 3, Some((5, "5")))).unwrap().items), [6, 7, 8]);
    // Past the samples, a step past the range, a filter.
    assert_eq!(table.page(&query(30, 40, 1, 5, None)).unwrap().items, b"[]");
    assert_eq!(minutes(&table.page(&query(2, 29, 1000, 5, None)).unwrap().items), [2]);
    let filtered = KpiQuery { filtered: true, ..query(0, 29, 1, 5, None) };
    assert_eq!(table.page(&filtered).unwrap().items, b"[]");
    assert_eq!(table.coverage(3, 40), Some((27, 3, 29)));
    assert_eq!(table.coverage(30, 40), None);
}

#[test]
fn a_row_is_canonical_json_with_the_ledger_columns() {
    let row = kpi_row(&sample(0.25), 7, None, Some(&[[1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12]]), "r", "main").unwrap();
    let text = String::from_utf8(row).unwrap();
    assert!(text.starts_with(r#"{"branch_id":"main","food_meal_units_delivered":8,"food_meal_units_initial":7,"#), "{text}");
    assert!(
        text.contains(r#""minute_index":7,"record_kind":"kpis","run_id":"r","staff_assigned_task_count":1,"staff_busy_count":1,"staff_load_ratio":0.5,"#),
        "{text}"
    );
    assert!(text.contains(r#""time_semantics":"end_of_tick","toilet_active_users":1,"#), "{text}");
    assert!(text.ends_with(r#""water_ml_reserved":6,"water_ml_served":3}"#), "{text}");
    assert!(!text.contains("source_ids_json"));
    let without = String::from_utf8(kpi_row(&sample(0.25), 7, None, None, "r", "main").unwrap()).unwrap();
    assert!(!without.contains("water_ml"));
    let metrics = String::from_utf8(review_metrics(&sample(0.25)).unwrap()).unwrap();
    assert!(metrics.contains(r#""toilet_working_units":3.0,"#) && !metrics.contains("minute_index"), "{metrics}");
}

#[test]
fn stored_samples_that_do_not_read_back_are_errors() {
    let mut table = KpiTable::new();
    assert!(table.read_block(&[3, 0, 0]).is_err());
    assert!(table.read_block(&[1]).is_err());
    let mut tables = Vec::new();
    Packer::new().write_tables(0, 0, &mut tables);
    table.read_tables(&tables).unwrap();
    assert!(table.read_block(&[0xff; 4]).is_err());
}

/// A sample whose cumulative toilet waits are these.
fn waited(count: f64, total: f64) -> MetricSample {
    MetricSample { toilet_wait_count: count, toilet_wait_total_minutes: total, ..sample(0.0) }
}

fn recent(items: &[u8]) -> Vec<f64> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(items).unwrap();
    rows.iter().map(|row| row["toilet_wait_mean_24h_minutes"].as_f64().unwrap()).collect()
}

/// A table of these samples, read back as a stored Run's.
fn table_of(samples: &[(i64, MetricSample)]) -> KpiTable {
    let mut packed = MetricSamples::default();
    for (minute, sample) in samples {
        packed.push(sample, *minute, None).unwrap();
    }
    let (packer, blocks) = packed.store();
    let mut tables = Vec::new();
    packer.write_tables(0, 0, &mut tables);
    let mut table = KpiTable::new();
    table.read_tables(&tables).unwrap();
    for block in blocks.blocks() {
        table.read_block(block).unwrap();
    }
    table
}

#[test]
fn a_row_s_recent_toilet_wait_is_of_those_who_began_in_the_last_24_hours() {
    // A wait of 100 minutes ends at minute 10, then one of 2 minutes every 100 minutes from 100 on.
    let samples: Vec<(i64, MetricSample)> = (0..3100)
        .map(|minute| {
            let short = if minute >= 100 { (minute / 100) as f64 } else { 0.0 };
            let long = if minute >= 10 { 1.0 } else { 0.0 };
            (minute, waited(long + short, 100.0 * long + 2.0 * short))
        })
        .collect();
    let table = table_of(&samples);
    let at = |minute: i64| recent(&table.page(&query(minute, minute, 1, 1, None)).unwrap().items)[0];
    // Before 24 hours, the window starts with the Run: none waited before minute 10.
    assert_eq!(at(5), 0.0);
    assert_eq!(at(10), 100.0);
    assert_eq!(at(1000), 120.0 / 11.0);
    // The window is the last 1440 minutes: the long wait leaves it when its minute does.
    assert_eq!(at(1449), 128.0 / 15.0);
    assert_eq!(at(1450), 2.0);
    assert_eq!(at(3000), 2.0);
    // It falls as well as rises, where the cumulative mean only drifts.
    let mean = samples[3000].1.toilet_wait_total_minutes / samples[3000].1.toilet_wait_count;
    assert!(mean > 2.0, "{mean}");
    // None began in the window: 0.
    let quiet = table_of(&[(0, waited(0.0, 0.0)), (100, waited(3.0, 30.0)), (2000, waited(3.0, 30.0))]);
    assert_eq!(recent(&quiet.page(&query(0, 2000, 1, 5, None)).unwrap().items), [0.0, 10.0, 0.0]);
    // A minute without a sample: the window starts at the last sample before it.
    let sparse = table_of(&[(0, waited(1.0, 50.0)), (1000, waited(2.0, 60.0)), (2500, waited(4.0, 70.0))]);
    assert_eq!(recent(&sparse.page(&query(2500, 2500, 1, 1, None)).unwrap().items), [5.0]);
}

#[test]
fn the_samples_find_where_the_window_starts() {
    let mut samples = MetricSamples::default();
    for minute in [3, 5, 9] {
        samples.push(&waited(minute as f64, 0.0), minute, None).unwrap();
    }
    let count = |minute: i64| samples.last_at_or_before(minute).unwrap().map(|sample| sample.toilet_wait_count);
    assert_eq!(
        [count(2), count(3), count(4), count(5), count(8), count(9), count(100)],
        [None, Some(3.0), Some(3.0), Some(5.0), Some(5.0), Some(9.0), Some(9.0)]
    );
    assert_eq!(MetricSamples::default().last_at_or_before(0).unwrap(), None);
}

#[test]
fn a_summary_is_of_every_sample_of_the_range() {
    // #612: the main KPIs over the range, not at one minute.
    let queue = |length: usize| MetricSample { toilet_queue_length: length, ..sample(0.0) };
    // The minutes 0 to 5 and 8 to 9: the gap 5..8 is not covered.
    let samples: Vec<(i64, MetricSample)> =
        [(0, 0), (1, 2), (2, 3), (3, 0), (4, 0), (5, 1), (8, 3), (9, 0)].into_iter().map(|(minute, length)| (minute, queue(length))).collect();
    let table = table_of(&samples);
    let summary = table.summary(0, 9, &["toilet_queue_length", "stress_mean", "no_such_metric"]).unwrap();
    assert_eq!((summary.samples, summary.first, summary.last, summary.covered_minutes), (8, Some(0), Some(9), 6));
    let [length, stress, missing] = &summary.metrics[..] else { panic!("three metrics") };
    assert_eq!(length.samples, 8);
    assert_eq!(length.mean(), Some(9.0 / 8.0));
    assert_eq!((&length.min, &length.max, length.max_minute, &length.last), (&V::Int(0), &V::Int(3), Some(2), &V::Int(0)));
    // [1, 2], [2, 3] and [8, 9] are above 0 with both ends sampled; [5, 6] has no end.
    assert_eq!((length.positive_minutes, length.first_positive_minute), (3, Some(1)));
    assert_eq!((stress.samples, stress.positive_minutes, &stress.max, stress.max_minute), (8, 0, &V::Float(0.0), Some(0)));
    assert_eq!((missing.samples, missing.mean(), &missing.max), (0, None, &V::Null));
    // A part of the range, and a range with no sample.
    let part = table.summary(2, 5, &["toilet_queue_length"]).unwrap();
    assert_eq!((part.samples, part.covered_minutes, part.metrics[0].positive_minutes, part.metrics[0].mean()), (4, 3, 1, Some(1.0)));
    let none = table.summary(6, 7, &["toilet_queue_length"]).unwrap();
    assert_eq!((none.samples, none.first, none.metrics[0].mean()), (0, None, None));
}
