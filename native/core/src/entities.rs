//! The entities of a world as rows: agents with their needs, facilities, staff, support requests,
//! active tasks, and the zones and zone edges (docs/rust-port/lanes/p6-entity-windows.md).
//!
//! Each row is a struct named like the entity it holds, and `write_json` writes the bytes of its
//! canonical JSON in a world's document of a version (`canonical`). A `Table` holds
//! the rows of one `IdMap`, keys in the dict's order; `Entities` holds the seven tables of a world.
//!
//! The rows are the truth (decision 1 of docs/rust-port/step5-plan.md, PR 6-1), and since #430
//! removed the Python objects, all there is.

use std::sync::Arc;

use crate::error::Result;
use crate::json::{write_json_string, Encoder};
use crate::row::IdMap;
use crate::str_enum;
use crate::value::V;

// ---- Typed fields ----------------------------------------------------------------------------

/// A field of an entity row. Not the namespaces' `row::Field`: a WorldState document of v5 leaves
/// out some fields of v6, so writing takes the document's version.
pub trait EntityField: Sized {
    /// Whether the two values are the same: the bits of a float.
    fn same(&self, other: &Self) -> bool;
    /// The bytes `canonical_json` writes for this value in a WorldState document of this version.
    fn write_json(&self, encoder: &mut Encoder, version: i64) -> Result<()>;
    /// Sort the keys of every map inside, as a copy through canonical JSON does (`row::Field`).
    fn canonicalize(&mut self) {}
}

/// A row of a table: what orders the rows of a document.
pub trait EntityRow: EntityField {
    /// The entity's own ID, by which the world's canonical document sorts the rows.
    fn id(&self) -> &str;
}

fn write_strings(encoder: &mut Encoder, items: &[Arc<str>]) {
    encoder.out.push(b'[');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            encoder.out.push(b',');
        }
        write_json_string(&mut encoder.out, item);
    }
    encoder.out.push(b']');
}

impl EntityField for f64 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        encoder.try_float(*self)
    }
}

impl EntityField for i64 {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        encoder.int(*self);
        Ok(())
    }
}

impl EntityField for bool {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        encoder.bool(*self);
        Ok(())
    }
}

impl EntityField for Arc<str> {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        write_json_string(&mut encoder.out, self);
        Ok(())
    }
}

impl<T: EntityField> EntityField for Option<T> {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(mine), Some(theirs)) => mine.same(theirs),
            (None, None) => true,
            _ => false,
        }
    }
    fn write_json(&self, encoder: &mut Encoder, version: i64) -> Result<()> {
        match self {
            Some(value) => value.write_json(encoder, version),
            None => {
                encoder.null();
                Ok(())
            }
        }
    }
}

/// An ID list: a list of a facility or a staff member.
impl EntityField for Vec<Arc<str>> {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        write_strings(encoder, self);
        Ok(())
    }
}

/// A staff member's `skills`: a tuple of str, which a document writes as a list.
#[derive(Clone, PartialEq)]
pub struct Skills(pub Vec<Arc<str>>);

impl EntityField for Skills {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        write_strings(encoder, &self.0);
        Ok(())
    }
}

impl EntityField for V {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
        V::write_json(self, encoder)
    }
    fn canonicalize(&mut self) {
        V::canonicalize(self);
    }
}

/// An enum field: a member of the enum, as its number in the order the enum lists them, and its
/// text in a document.
macro_rules! member_field {
    ($name:ident, $all:ident, [$($member:ident = $text:literal),+ $(,)?]) => {
        str_enum! { pub enum $name { $($member = $text),+ } }

        /// The members in the order of the enum.
        pub const $all: &[$name] = &[$($name::$member),+];

        impl $name {
            /// The member's place in the enum.
            pub const fn index(self) -> usize {
                self as usize
            }
        }

        impl EntityField for $name {
            fn same(&self, other: &Self) -> bool {
                self == other
            }
            fn write_json(&self, encoder: &mut Encoder, _version: i64) -> Result<()> {
                write_json_string(&mut encoder.out, self.text());
                Ok(())
            }
        }
    };
}

member_field!(
    Activity,
    ACTIVITIES,
    [Idle = "idle", Moving = "moving", Queued = "queued", InService = "in_service", SeekingHelp = "seeking_help", Sleeping = "sleeping", Resting = "resting",]
);
member_field!(Mobility, MOBILITIES, [Independent = "independent", Assisted = "assisted", Wheelchair = "wheelchair"]);
member_field!(RequestStatus, STATUSES, [Pending = "pending", Assigned = "assigned", Resolved = "resolved", Unresolved = "unresolved",]);

// ---- Rows ------------------------------------------------------------------------------------

/// A document holds this field only in WorldState v6.
fn v6<T>(_value: &T, version: i64) -> bool {
    version == 6
}

/// A document leaves this field out while it is None.
fn set<T>(value: &Option<T>, _version: i64) -> bool {
    value.is_some()
}

macro_rules! keeps_field {
    ($value:expr, $version:expr) => {
        true
    };
    ($value:expr, $version:expr, $keep:ident) => {
        $keep($value, $version)
    };
}

/// A struct of fields, each named as its key in the document. `name[rule]` says when a document
/// holds the field (`v6`, `set`). `NAMES` lists the fields in their order.
macro_rules! entity_row {
    ($name:ident { $($field:ident $([$keep:ident])?: $kind:ty),* $(,)? }) => {
        #[derive(Clone)]
        pub struct $name {
            $(pub $field: $kind,)*
        }

        impl $name {
            /// The fields in the order they are listed.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($field)),*];
        }

        impl EntityField for $name {
            fn same(&self, other: &Self) -> bool {
                true $(&& self.$field.same(&other.$field))*
            }
            fn canonicalize(&mut self) {
                $(EntityField::canonicalize(&mut self.$field);)*
            }
            fn write_json(&self, encoder: &mut Encoder, version: i64) -> Result<()> {
                const ORDER: [usize; $name::NAMES.len()] = crate::row::sorted_order($name::NAMES);
                type Writer = fn(&$name, &mut Encoder, i64, &mut bool) -> Result<()>;
                const WRITERS: &[Writer] = &[$(|row, encoder, version, first| {
                    if !keeps_field!(&row.$field, version $(, $keep)?) {
                        return Ok(());
                    }
                    if !*first {
                        encoder.out.push(b',');
                    }
                    *first = false;
                    write_json_string(&mut encoder.out, stringify!($field));
                    encoder.out.push(b':');
                    EntityField::write_json(&row.$field, encoder, version)
                }),*];
                let mut first = true;
                encoder.out.push(b'{');
                for index in ORDER {
                    WRITERS[index](self, encoder, version, &mut first)?;
                }
                encoder.out.push(b'}');
                Ok(())
            }
        }
    };
}

entity_row!(NeedRow { toilet_urgency: f64, hunger: f64, fatigue: f64, hydration_need[v6]: f64 });

entity_row!(ProfileRow {
    id: Arc<str>,
    group_id: Option<Arc<str>>,
    initial_zone_id: Arc<str>,
    mobility: Mobility,
    support_need: f64,
    stress_sensitivity: f64,
    rng_key: Arc<str>,
    initial_needs: NeedRow,
    provenance: V,
});

entity_row!(AgentRow {
    profile: ProfileRow,
    zone_id: Arc<str>,
    activity: Activity,
    target_id: Option<Arc<str>>,
    needs: NeedRow,
    stress: f64,
    trust: f64,
    next_decision_at: i64,
    support_request_id: Option<Arc<str>>,
    support_cooldown_until: i64,
});

entity_row!(FacilityRow {
    id: Arc<str>,
    kind: Arc<str>,
    zone_id: Arc<str>,
    capacity: i64,
    working_units: i64,
    active_agent_ids: Vec<Arc<str>>,
    queue_agent_ids: Vec<Arc<str>>,
    resource_active_agent_ids[v6]: Vec<Arc<str>>,
    resource_queue_agent_ids[v6]: Vec<Arc<str>>,
});

entity_row!(StaffRow {
    id: Arc<str>,
    zone_id: Arc<str>,
    skills: Skills,
    work_queue: Vec<Arc<str>>,
    busy_until: i64,
    active_task_id: Option<Arc<str>>,
});

entity_row!(RequestRow {
    id: Arc<str>,
    agent_id: Arc<str>,
    requested_minute: i64,
    timeout_minute: i64,
    status: RequestStatus,
    assigned_staff_id: Option<Arc<str>>,
    task_id: Option<Arc<str>>,
    resolved_minute: Option<i64>,
    cooldown_until: i64,
    last_assignment_attempt_minute[set]: Option<i64>,
    last_assignment_wait_reason_code[set]: Option<Arc<str>>,
});

entity_row!(TaskRow {
    id: Arc<str>,
    kind: Arc<str>,
    agent_id: Option<Arc<str>>,
    resource_id: Option<Arc<str>>,
    start_minute: i64,
    due_minute: i64,
    staff_id: Option<Arc<str>>,
});

// A zone. A world document writes its fields, the same in v5 and v6.
entity_row!(ZoneRow {
    id: Arc<str>,
    name: Arc<str>,
    capacity: Option<i64>,
    display_x: Option<f64>,
    display_y: Option<f64>,
});

// An edge between two zones.
entity_row!(EdgeRow {
    id: Arc<str>,
    from_zone_id: Arc<str>,
    to_zone_id: Arc<str>,
    travel_minutes: i64,
    closed: bool,
});

impl EntityRow for AgentRow {
    fn id(&self) -> &str {
        &self.profile.id
    }
}

macro_rules! own_id {
    ($($name:ident),+) => {
        $(impl EntityRow for $name {
            fn id(&self) -> &str {
                &self.id
            }
        })+
    };
}

own_id!(FacilityRow, StaffRow, RequestRow, TaskRow, ZoneRow, EdgeRow);

// ---- Tables ----------------------------------------------------------------------------------

/// The rows of one `IdMap`: a stable slot per row, and the keys in the dict's order. The rows are
/// the truth (Issue #152, PR 6-1).
#[derive(Clone)]
pub struct Table<R> {
    rows: Vec<Option<R>>,
    free: Vec<u32>,
    ids: IdMap<u32>,
    /// The slots written or filled since `take_marked`, each once and in the order of their first
    /// write: what the end-of-tick check's memo reads again (lane P8 d4).
    /// Not state.
    marked: Vec<u32>,
    is_marked: Vec<bool>,
}

impl<R> Default for Table<R> {
    fn default() -> Self {
        Self { rows: Vec::new(), free: Vec::new(), ids: IdMap::default(), marked: Vec::new(), is_marked: Vec::new() }
    }
}

/// Mark a slot written for the end-of-tick check's memo (`Table::marked`), once.
#[inline]
fn mark(marked: &mut Vec<u32>, is_marked: &mut [bool], slot: u32) {
    if !is_marked[slot as usize] {
        is_marked[slot as usize] = true;
        marked.push(slot);
    }
}

impl<R> Table<R> {
    /// A free slot, with no row yet and no key.
    fn reserve_slot(&mut self) -> u32 {
        self.free.pop().unwrap_or_else(|| {
            self.rows.push(None);
            self.is_marked.push(false);
            u32::try_from(self.rows.len() - 1).expect("under 2**32 entities")
        })
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The row in a slot, for writing. The slot is marked (`marked`).
    pub fn slot_mut(&mut self, slot: u32) -> Option<&mut R> {
        let row = self.rows.get_mut(slot as usize)?.as_mut()?;
        mark(&mut self.marked, &mut self.is_marked, slot);
        Some(row)
    }

    /// The row in a slot.
    pub fn row_at(&self, slot: u32) -> Option<&R> {
        self.rows.get(slot as usize)?.as_ref()
    }

    /// The row of a key.
    pub fn get(&self, id: &str) -> Option<&R> {
        self.rows.get(*self.ids.get(id)? as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut R> {
        let slot = *self.ids.get(id)?;
        self.slot_mut(slot)
    }

    /// The slot of a key.
    pub fn slot_of(&self, id: &str) -> Option<u32> {
        self.ids.get(id).copied()
    }

    /// Whether the table has the key.
    pub fn contains_key(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    /// The keys and rows in the order of the dict.
    pub fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &R)> {
        self.ids.iter().filter_map(|(id, slot)| Some((id, self.rows[*slot as usize].as_ref()?)))
    }

    /// The keys and slots in the order of the dict.
    pub fn slots(&self) -> impl Iterator<Item = (&Arc<str>, u32)> {
        self.ids.iter().map(|(id, slot)| (id, *slot))
    }

    /// The slots written or filled since the last `take_marked`, in the order of their first write.
    /// A slot may have lost its row since.
    pub fn marked(&self) -> &[u32] {
        &self.marked
    }

    /// Whether the slot was written or filled since the last `take_marked`.
    pub fn is_marked(&self, slot: u32) -> bool {
        self.is_marked.get(slot as usize).copied().unwrap_or(false)
    }

    /// `marked`, and the marks cleared.
    pub fn take_marked(&mut self) -> Vec<u32> {
        let marked = std::mem::take(&mut self.marked);
        for slot in &marked {
            self.is_marked[*slot as usize] = false;
        }
        marked
    }
}

impl<R: Clone> Table<R> {
    /// Put a row under a key: a new key goes to the end of the order, a key already there keeps
    /// its place and loses its row.
    pub fn insert(&mut self, key: Arc<str>, row: R) -> u32 {
        if let Some(old) = self.ids.get(&*key).copied() {
            self.take_out(old);
        }
        let slot = self.reserve_slot();
        self.rows[slot as usize] = Some(row);
        mark(&mut self.marked, &mut self.is_marked, slot);
        self.ids.insert(key, slot);
        slot
    }

    /// Take the key and its row out (`shift_remove`: the others keep their order). False if the
    /// table has no such key.
    pub fn remove_row(&mut self, key: &str) -> bool {
        let Some(slot) = self.ids.shift_remove(key) else {
            return false;
        };
        self.take_out(slot);
        true
    }

    fn take_out(&mut self, slot: u32) {
        if self.rows[slot as usize].take().is_some() {
            self.free.push(slot);
        }
    }
}

impl<R: EntityRow> Table<R> {
    /// The slots of the rows in the order of the world's canonical document: by the entity's own
    /// ID, stably from the dict's order.
    pub fn document_slots(&self) -> Vec<u32> {
        let mut order: Vec<(&str, u32)> = self.ids.values().filter_map(|slot| Some((self.rows[*slot as usize].as_ref()?.id(), *slot))).collect();
        order.sort_by(|first, second| first.0.as_bytes().cmp(second.0.as_bytes()));
        order.into_iter().map(|(_, slot)| slot).collect()
    }

    /// The canonical order of the table (`World::canonicalize`; lane 6-2): the keys by
    /// the entity's own ID, stably from the dict's order (`document_slots`), and the keys of the
    /// maps inside each row sorted. The rows stay in their slots and nothing is marked.
    pub fn canonicalize(&mut self) {
        let sorted = self.ids.values().zip(self.ids.values().skip(1)).all(|(first, second)| {
            match (self.rows[*first as usize].as_ref(), self.rows[*second as usize].as_ref()) {
                (Some(first), Some(second)) => first.id().as_bytes() <= second.id().as_bytes(),
                _ => false,
            }
        });
        if !sorted {
            let slots = self.document_slots();
            let mut ids = IdMap::with_capacity_and_hasher(self.ids.len(), Default::default());
            let mut keys: Vec<Option<Arc<str>>> = vec![None; self.rows.len()];
            for (key, slot) in &self.ids {
                keys[*slot as usize] = Some(key.clone());
            }
            for slot in slots {
                if let Some(key) = keys[slot as usize].take() {
                    ids.insert(key, slot);
                }
            }
            // A key without a row (none in a store that follows its objects) keeps its place last.
            for (key, slot) in &self.ids {
                if !ids.contains_key(key) {
                    ids.insert(key.clone(), *slot);
                }
            }
            self.ids = ids;
        }
        for row in self.rows.iter_mut().flatten() {
            row.canonicalize();
        }
    }
}

/// All the rows of a world. `Clone` is for a snapshot's copy on write (lane 6-2).
#[derive(Clone, Default)]
pub struct Entities {
    pub agents: Table<AgentRow>,
    pub facilities: Table<FacilityRow>,
    pub staff: Table<StaffRow>,
    pub support_requests: Table<RequestRow>,
    pub active_tasks: Table<TaskRow>,
    pub zones: Table<ZoneRow>,
    pub zone_edges: Table<EdgeRow>,
}

impl Entities {
    /// The order a copy through canonical JSON gives (`Table::canonicalize`), for each table.
    pub fn canonicalize(&mut self) {
        self.agents.canonicalize();
        self.facilities.canonicalize();
        self.staff.canonicalize();
        self.support_requests.canonicalize();
        self.active_tasks.canonicalize();
        self.zones.canonicalize();
        self.zone_edges.canonicalize();
    }
}

#[allow(dead_code)]
fn assert_rows_are_shareable() {
    fn check<T: Send + Sync>() {}
    check::<Entities>();
}
