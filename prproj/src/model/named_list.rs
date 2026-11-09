//! `NamedList` (port of `py_premiere/models/named_list.py`): a snapshot list of model handles
//! that is also indexable by name.
//!
//! Python's `NamedList` asks each element for its key attributes lazily (`name`, or
//! `display_name`/`match_name` for components); here the project reads each element's keys
//! when it builds the list, keeping a key that raised as its error. A lookup walks the elements
//! in order and each element's keys in order, exactly like Python's `any(...)`: the first key
//! equal to the name wins, and a key that raised before any match makes the lookup raise.

use crate::convert::model::py_str_repr;
use crate::error::{Error, Result};

/// A list of handles, also indexable by name. A snapshot: changing it does not touch the model.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedList<T> {
    items: Vec<T>,
    keys: Vec<Vec<Result<String>>>,
}

impl<T: Copy + PartialEq> NamedList<T> {
    /// A list whose element `i` answers to the names `keys[i]` (in key order).
    pub fn new(items: Vec<T>, keys: Vec<Vec<Result<String>>>) -> Self {
        assert_eq!(items.len(), keys.len(), "one key set per element");
        NamedList { items, keys }
    }

    fn position(&self, name: &str) -> Result<Option<usize>> {
        for (index, keys) in self.keys.iter().enumerate() {
            for key in keys {
                match key {
                    Ok(k) if k == name => return Ok(Some(index)),
                    Ok(_) => {}
                    Err(e) => return Err(e.clone()),
                }
            }
        }
        Ok(None)
    }

    /// `collection.get(name)`: the first element named `name`.
    pub fn get(&self, name: &str) -> Result<Option<T>> {
        Ok(self.position(name)?.map(|i| self.items[i]))
    }

    /// `collection[name]`: the first element named `name`, or `KeyError(repr(name))`.
    pub fn by_name(&self, name: &str) -> Result<T> {
        self.get(name)?.ok_or_else(|| Error::Key(py_str_repr(name)))
    }

    /// `name in collection`.
    pub fn contains_name(&self, name: &str) -> Result<bool> {
        Ok(self.position(name)?.is_some())
    }

    /// `item in collection` (identity).
    pub fn contains(&self, item: T) -> bool {
        self.items.contains(&item)
    }

    /// `collection.index(item)`.
    pub fn index_of(&self, item: T) -> Option<usize> {
        self.items.iter().position(|&i| i == item)
    }

    /// The key values of element `index`, in key order.
    pub fn keys_of(&self, index: usize) -> &[Result<String>] {
        &self.keys[index]
    }

    /// The handles.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// The handles, owned.
    pub fn into_vec(self) -> Vec<T> {
        self.items
    }
}

impl<T> std::ops::Deref for NamedList<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<'a, T> IntoIterator for &'a NamedList<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}
