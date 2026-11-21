//! What the API's KPI history and offline report read of a Run's metric samples in the browser
//! (Issue #266): `GET …/branches/{branch}/history?kind=kpis` answers from the stored samples
//! (`MetricSamples`, #252) instead of the history index of the Python server (removed in #430),
//! and the report reads the last sample.
//!
//! A row (`kpi_row`) is the index's: the sample's `MetricSnapshot` fields without
//! `source_ids_json`, the ledger's columns where the sample holds them (`Balances`),
//! `time_semantics`, `record_kind`, `run_id` and `branch_id`, as canonical JSON (keys sorted, the
//! floats as Python writes them). `KpiTable` holds a stored Run's samples read back and pages them
//! as the index's query does: the minutes from `start` to `end` that are `start` plus a multiple of
//! `step`, after the cursor's row, `limit` of them. The JS side (`js/api_answers.mjs`) checks the
//! query, makes the cursor and writes the rest of the answer.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::events::packed::Packer;
use crate::frame::{kpis, RESOURCE_COLUMNS};
use crate::json::Encoder;
use crate::plain::{unpack_sample, Balances, UnpackedSample};
use crate::rules::metrics::{recent_toilet_wait_mean, MetricSample, RECENT_WAIT_MINUTES};
use crate::value::V;

fn corrupt(what: &str) -> Error {
    Error::Value(format!("not stored samples: {what}"))
}

/// The history index's row of a sample (its KPIs with the recent toilet wait since `before`, the
/// sample at the window's start (`metrics::recent_toilet_wait_mean`, #448), then the answer's
/// `record_kind`, `run_id` and `branch_id`), as canonical JSON.
pub fn kpi_row(
    sample: &MetricSample,
    minute: i64,
    before: Option<&MetricSample>,
    balances: Option<&Balances>,
    run_id: &str,
    branch_id: &str,
) -> Result<Vec<u8>> {
    let mut row = kpis(sample, minute, Some(recent_toilet_wait_mean(sample, before)));
    let mut put = |name: &str, value: V| {
        row.insert(Arc::from(name), value);
    };
    for (names, values) in RESOURCE_COLUMNS.iter().zip(balances.into_iter().flatten()) {
        for (name, value) in names.iter().zip(values) {
            put(name, V::Int(*value));
        }
    }
    put("time_semantics", V::Str(Arc::from("end_of_tick")));
    put("record_kind", V::Str(Arc::from("kpis")));
    put("run_id", V::Str(Arc::from(run_id)));
    put("branch_id", V::Str(Arc::from(branch_id)));
    let mut encoder = Encoder::with_capacity(1024);
    V::Map(row).write_json(&mut encoder)?;
    Ok(encoder.out)
}

/// The final metrics of the offline report's evidence (`answers::report`): each field of the
/// sample's `MetricSnapshot` but `minute_index` and `source_ids_json`, as a float, as canonical
/// JSON.
pub fn review_metrics(sample: &MetricSample) -> Result<Vec<u8>> {
    let mut row = kpis(sample, 0, None);
    row.shift_remove("minute_index");
    for value in row.values_mut() {
        if let V::Int(number) = value {
            *value = V::Float(*number as f64);
        }
    }
    let mut encoder = Encoder::with_capacity(1024);
    V::Map(row).write_json(&mut encoder)?;
    Ok(encoder.out)
}

/// A page of KPI rows (`KpiTable::page`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KpiPage {
    /// The rows, as a JSON list.
    pub items: Vec<u8>,
    /// The minute of the last row, which the next page's cursor names.
    pub last: Option<i64>,
    /// Whether rows follow the page.
    pub more: bool,
}

/// What a page asks for: the index's query of `kind=kpis`.
#[derive(Clone, Debug)]
pub struct KpiQuery<'a> {
    pub start: i64,
    pub end: i64,
    pub step: i64,
    pub limit: usize,
    /// The cursor's row: its minute and the key after `kpis:`; rows come after it in the order
    /// of (minute, key), the key a row's minute as text.
    pub after: Option<(i64, &'a str)>,
    /// A filter no KPI row passes (an identity, a type, a source, a status or a text term): no rows.
    pub filtered: bool,
    pub run_id: &'a str,
    pub branch_id: &'a str,
}

/// A stored Run's metric samples read back (the pieces `sample_tables` and `sample_block` of
/// `run_store.mjs`, in the order they were written), kept packed, with each one's minute.
#[derive(Clone, Debug, Default)]
pub struct KpiTable {
    packer: Packer,
    blocks: Vec<Vec<u8>>,
    /// Each sample's block and offset in it.
    starts: Vec<(usize, usize)>,
    minutes: Vec<i64>,
}

impl KpiTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The tables' new entries (`Packer::write_tables`).
    pub fn read_tables(&mut self, bytes: &[u8]) -> Result<()> {
        self.packer.read_tables(bytes)
    }

    /// The samples of a block: how many it held. An error for bytes that are not samples, or
    /// samples whose minutes do not go up.
    pub fn read_block(&mut self, bytes: &[u8]) -> Result<usize> {
        let index = self.blocks.len();
        let (mut at, mut read) = (0, 0);
        while at < bytes.len() {
            let unpacked = unpack_sample(&self.packer, &bytes[at..])?;
            if unpacked.used == 0 {
                return Err(corrupt("an empty sample"));
            }
            if self.minutes.last().is_some_and(|last| *last >= unpacked.minute) {
                return Err(corrupt("the minutes do not go up"));
            }
            self.starts.push((index, at));
            self.minutes.push(unpacked.minute);
            at += unpacked.used;
            read += 1;
        }
        self.blocks.push(bytes.to_vec());
        Ok(read)
    }

    /// How many samples were read.
    pub fn len(&self) -> usize {
        self.minutes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.minutes.is_empty()
    }

    /// The sample `index`, or None past the last.
    pub fn sample(&self, index: usize) -> Result<Option<UnpackedSample>> {
        let Some(&(block, offset)) = self.starts.get(index) else {
            return Ok(None);
        };
        unpack_sample(&self.packer, &self.blocks[block][offset..]).map(Some)
    }

    /// The last sample whose minute is at most `minute`, or None where there is none: where the
    /// window of the recent toilet wait starts. A fork's table holds its parent's samples up to its
    /// base, so the window reaches back past the fork.
    fn last_at_or_before(&self, minute: i64) -> Result<Option<MetricSample>> {
        match self.minutes.partition_point(|at| *at <= minute).checked_sub(1) {
            Some(index) => Ok(self.sample(index)?.map(|unpacked| unpacked.sample)),
            None => Ok(None),
        }
    }

    /// The first index whose minute is at least `minute`.
    fn first_at(&self, minute: i64) -> usize {
        self.minutes.partition_point(|at| *at < minute)
    }

    /// How many samples there are from `start` to `end`, and the first's and the last's minutes
    /// (the index's `coverage_by_kind`, which no step, cursor or filter narrows).
    pub fn coverage(&self, start: i64, end: i64) -> Option<(usize, i64, i64)> {
        let first = self.first_at(start);
        let past = self.first_at(end.saturating_add(1));
        (first < past).then(|| (past - first, self.minutes[first], self.minutes[past - 1]))
    }

    /// A page of rows (see `KpiQuery`).
    pub fn page(&self, query: &KpiQuery<'_>) -> Result<KpiPage> {
        let mut page = KpiPage { items: vec![b'['], ..KpiPage::default() };
        if query.filtered || query.step < 1 || query.start > query.end {
            page.items.push(b']');
            return Ok(page);
        }
        let from = match query.after {
            Some((minute, _)) => query.start.max(minute),
            None => query.start,
        };
        let mut taken = 0;
        for index in self.first_at(from)..self.minutes.len() {
            let minute = self.minutes[index];
            if minute > query.end {
                break;
            }
            if (minute - query.start) % query.step != 0 {
                continue;
            }
            if let Some((after, key)) = query.after {
                if minute == after && minute.to_string().as_bytes() <= key.as_bytes() {
                    continue;
                }
            }
            if taken == query.limit {
                page.more = true;
                break;
            }
            let unpacked = self.sample(index)?.ok_or_else(|| corrupt("a sample is missing"))?;
            if taken > 0 {
                page.items.push(b',');
            }
            let before = self.last_at_or_before(minute - RECENT_WAIT_MINUTES)?;
            page.items.extend(kpi_row(&unpacked.sample, minute, before.as_ref(), unpacked.balances.as_ref(), query.run_id, query.branch_id)?);
            page.last = Some(minute);
            taken += 1;
        }
        page.items.push(b']');
        Ok(page)
    }

    /// What each of `keys` (a history row's KPIs, the recent toilet wait among them) did over the
    /// samples from `start` to `end` (#612): the whole range a report summarises, not one minute.
    pub fn summary(&self, start: i64, end: i64, keys: &[&'static str]) -> Result<KpiSummary> {
        let mut summary = KpiSummary { metrics: keys.iter().map(|key| MetricSummary::new(key)).collect(), ..KpiSummary::default() };
        if start > end {
            return Ok(summary);
        }
        // The minute of the sample before and, for each key, whether it was above 0.
        let mut previous: Option<(i64, Vec<bool>)> = None;
        for index in self.first_at(start)..self.minutes.len() {
            let minute = self.minutes[index];
            if minute > end {
                break;
            }
            let unpacked = self.sample(index)?.ok_or_else(|| corrupt("a sample is missing"))?;
            let before = self.last_at_or_before(minute - RECENT_WAIT_MINUTES)?;
            let row = kpis(&unpacked.sample, minute, Some(recent_toilet_wait_mean(&unpacked.sample, before.as_ref())));
            let joined = previous.as_ref().filter(|(at, _)| at + 1 == minute);
            if joined.is_some() {
                summary.covered_minutes += 1;
            }
            let mut positive = Vec::with_capacity(keys.len());
            for (index, metric) in summary.metrics.iter_mut().enumerate() {
                if joined.is_some_and(|(_, above)| above[index]) {
                    metric.positive_minutes += 1;
                }
                let value = row.get(metric.key).cloned().unwrap_or(V::Null);
                positive.push(metric.add(minute, value));
            }
            summary.samples += 1;
            summary.first = summary.first.or(Some(minute));
            summary.last = Some(minute);
            previous = Some((minute, positive));
        }
        Ok(summary)
    }
}

/// What the metrics did over a range of samples (`KpiTable::summary`).
#[derive(Clone, Debug, Default)]
pub struct KpiSummary {
    /// How many samples there were, and the first's and the last's minutes.
    pub samples: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// The one-minute intervals with both ends sampled.
    pub covered_minutes: i64,
    pub metrics: Vec<MetricSummary>,
}

/// What one metric did over a range of samples: the mean of its samples, its least and greatest
/// values (the first minute of the greatest), its last value, and the one-minute intervals with
/// both ends sampled whose start is above 0 (as the PDF report's problem windows count them).
#[derive(Clone, Debug)]
pub struct MetricSummary {
    pub key: &'static str,
    pub samples: usize,
    pub sum: f64,
    pub min: V,
    pub max: V,
    pub max_minute: Option<i64>,
    pub last: V,
    pub positive_minutes: i64,
    pub first_positive_minute: Option<i64>,
}

impl MetricSummary {
    fn new(key: &'static str) -> Self {
        MetricSummary {
            key,
            samples: 0,
            sum: 0.0,
            min: V::Null,
            max: V::Null,
            max_minute: None,
            last: V::Null,
            positive_minutes: 0,
            first_positive_minute: None,
        }
    }

    /// The sample's value at `minute`; whether it is above 0.
    fn add(&mut self, minute: i64, value: V) -> bool {
        let number = match value {
            V::Int(number) => number as f64,
            V::Float(number) if number.is_finite() => number,
            _ => return false,
        };
        let of = |value: &V| match value {
            V::Int(number) => Some(*number as f64),
            V::Float(number) => Some(*number),
            _ => None,
        };
        if of(&self.min).is_none_or(|least| number < least) {
            self.min = value.clone();
        }
        if of(&self.max).is_none_or(|greatest| number > greatest) {
            self.max = value.clone();
            self.max_minute = Some(minute);
        }
        if number > 0.0 && self.first_positive_minute.is_none() {
            self.first_positive_minute = Some(minute);
        }
        self.samples += 1;
        self.sum += number;
        self.last = value;
        number > 0.0
    }

    /// The mean of its samples, or None without one.
    pub fn mean(&self) -> Option<f64> {
        (self.samples > 0).then(|| self.sum / self.samples as f64)
    }
}
