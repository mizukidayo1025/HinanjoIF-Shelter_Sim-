//! `V`: a JSON value owned by Rust, for state without typed rows. It holds exactly what
//! `json.dumps` tells apart: `1`, `1.0` and `True` differ, and a map keeps its insertion order, as
//! a Python dict does, so a value read from JSON writes the same bytes back.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::Result;
use crate::json::Encoder;

/// How deep a value read from JSON may nest: `Field::from_value` and `value_bytes` refuse a deeper
/// one (`V::depth_fits`).
pub const MAX_DEPTH: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    Int(i64),
    /// An int outside i64, as `int.__repr__` writes it.
    Big(Box<str>),
    Float(f64),
    Str(Arc<str>),
    List(Vec<V>),
    Map(IndexMap<Arc<str>, V>),
}

// Rows and values must be usable without the GIL.
const _: fn() = || {
    fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<V>();
};

impl V {
    /// What `json.loads` reads, as a `V`: a number with a point or an exponent is a float, an int
    /// outside i64 is `Big`, and a map keeps the order `serde_json` gives its keys. None for a
    /// number Python's `float` would not read.
    pub fn from_json(value: &serde_json::Value) -> Option<V> {
        use serde_json::Value;
        Some(match value {
            Value::Null => V::Null,
            Value::Bool(flag) => V::Bool(*flag),
            Value::String(text) => V::Str(Arc::from(text.as_str())),
            Value::Number(number) => {
                let text = number.to_string();
                if text.contains(['.', 'e', 'E']) {
                    V::Float(text.parse().ok()?)
                } else {
                    match text.parse::<i64>() {
                        Ok(small) => V::Int(small),
                        Err(_) => V::Big(text.into()),
                    }
                }
            }
            Value::Array(items) => V::List(items.iter().map(V::from_json).collect::<Option<Vec<_>>>()?),
            Value::Object(object) => {
                let mut map = IndexMap::with_capacity(object.len());
                for (key, item) in object {
                    map.insert(Arc::from(key.as_str()), V::from_json(item)?);
                }
                V::Map(map)
            }
        })
    }

    /// `bool(value)`.
    pub fn is_truthy(&self) -> bool {
        match self {
            V::Null => false,
            V::Bool(flag) => *flag,
            V::Int(number) => *number != 0,
            V::Big(_) => true,
            V::Float(number) => *number != 0.0,
            V::Str(text) => !text.is_empty(),
            V::List(items) => !items.is_empty(),
            V::Map(items) => !items.is_empty(),
        }
    }

    /// Whether no container in the value is deeper than `MAX_DEPTH`, the value itself at depth 0.
    pub fn depth_fits(&self) -> bool {
        fn fits(value: &V, depth: usize) -> bool {
            if depth > MAX_DEPTH {
                return false;
            }
            match value {
                V::List(items) => items.iter().all(|item| fits(item, depth + 1)),
                V::Map(map) => map.values().all(|item| fits(item, depth + 1)),
                _ => true,
            }
        }
        fits(self, 0)
    }

    /// The canonical JSON of the value, keys sorted by bytes. NaN and infinity are `Unsupported`.
    pub fn write_json(&self, encoder: &mut Encoder) -> Result<()> {
        match self {
            V::Null => encoder.null(),
            V::Bool(flag) => encoder.bool(*flag),
            V::Int(number) => encoder.int(*number),
            V::Big(text) => encoder.out.extend_from_slice(text.as_bytes()),
            V::Float(number) => encoder.try_float(*number)?,
            V::Str(text) => encoder.string(text),
            V::List(items) => {
                encoder.out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        encoder.out.push(b',');
                    }
                    item.write_json(encoder)?;
                }
                encoder.out.push(b']');
            }
            V::Map(map) => {
                let mut entries: Vec<(&Arc<str>, &V)> = map.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                encoder.out.push(b'{');
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        encoder.out.push(b',');
                    }
                    encoder.string(key);
                    encoder.out.push(b':');
                    item.write_json(encoder)?;
                }
                encoder.out.push(b'}');
            }
        }
        Ok(())
    }

    /// Sort the keys of every map by their bytes, as a copy through canonical JSON does
    /// (`row::sort_keys`); lists keep their order.
    pub fn canonicalize(&mut self) {
        match self {
            V::List(items) => {
                for item in items {
                    item.canonicalize();
                }
            }
            V::Map(map) => {
                crate::row::sort_keys(map);
                for item in map.values_mut() {
                    item.canonicalize();
                }
            }
            _ => {}
        }
    }

    /// `==`, and every map in the same order (`==` of a map ignores its order).
    pub fn same_in_order(&self, other: &V) -> bool {
        match (self, other) {
            (V::List(mine), V::List(theirs)) => mine.len() == theirs.len() && mine.iter().zip(theirs).all(|(a, b)| a.same_in_order(b)),
            (V::Map(mine), V::Map(theirs)) => {
                mine.len() == theirs.len() && mine.iter().zip(theirs).all(|((key, a), (other, b))| key == other && a.same_in_order(b))
            }
            _ => self == other,
        }
    }

    /// The canonical JSON bytes of the value.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut encoder = Encoder::with_capacity(256);
        self.write_json(&mut encoder)?;
        Ok(encoder.out)
    }

    /// The bytes the value holds on the heap, as allocated (capacities, not lengths), without the
    /// allocator's own overhead: for measuring what a namespace costs (lane 5-3).
    pub fn heap_bytes(&self) -> usize {
        match self {
            V::Null | V::Bool(_) | V::Int(_) | V::Float(_) => 0,
            V::Big(text) => text.len(),
            // An `Arc<str>` holds its two counts next to the text.
            V::Str(text) => text.len() + 2 * std::mem::size_of::<usize>(),
            V::List(items) => items.capacity() * std::mem::size_of::<V>() + items.iter().map(V::heap_bytes).sum::<usize>(),
            V::Map(map) => {
                // An entry is its hash, key and value; the index table holds one usize per entry.
                let entry = std::mem::size_of::<(u64, Arc<str>, V)>() + std::mem::size_of::<usize>();
                map.capacity() * entry + map.iter().map(|(key, item)| key.len() + 2 * std::mem::size_of::<usize>() + item.heap_bytes()).sum::<usize>()
            }
        }
    }
}
