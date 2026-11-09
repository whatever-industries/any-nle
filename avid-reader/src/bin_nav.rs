//! Read-only equivalents of pyavb `Bin`'s mob navigation helpers.
//! Source: pinned pyavb `src/avb/bin.py` at ff64e06 (MIT; see NOTICE).
//! This traverses only items stored in this bin, in their stored order.

use crate::file::AvbFile;
use crate::io::MobId;
use crate::objects::{Node, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinQueryError {
    MissingRoot,
    NotBinRoot,
    MissingItems,
    MalformedItem(usize),
    MissingMobReference(usize),
    UndecodedMob(usize, u32),
    NotCompositionObject(usize, u32),
    MissingMobType(usize, u32),
    UnknownMobType(usize, u32, i64),
    MissingUsageCode(usize, u32),
    MissingMobId(usize, u32),
}

#[derive(Clone, Copy, Debug)]
pub struct BinMob<'a> {
    /// Zero-based position in the bin's stored item list.
    pub item_index: usize,
    /// One-based AVB object-table reference.
    pub object_index: u32,
    pub mob: &'a Node,
}

pub struct BinNavigator<'a> {
    file: &'a AvbFile,
    root: &'a Node,
}

impl<'a> BinNavigator<'a> {
    pub fn new(file: &'a AvbFile) -> Result<Self, BinQueryError> {
        let root = file.root().ok_or(BinQueryError::MissingRoot)?;
        if root.class != *b"ABIN" && root.class != *b"BINF" {
            return Err(BinQueryError::NotBinRoot);
        }
        Ok(Self { file, root })
    }

    /// pyavb `mobs`: preserve bin item order and duplicate references.
    pub fn mobs(&self) -> Result<Vec<BinMob<'a>>, BinQueryError> {
        let Value::List(items) = self.root.get("items").ok_or(BinQueryError::MissingItems)? else {
            return Err(BinQueryError::MissingItems);
        };
        let mut mobs = Vec::with_capacity(items.len());
        for (item_index, item) in items.iter().enumerate() {
            let Value::Node(item) = item else {
                return Err(BinQueryError::MalformedItem(item_index));
            };
            let object_index = item
                .get("mob")
                .and_then(Value::as_index)
                .ok_or(BinQueryError::MissingMobReference(item_index))?;
            let mob = self
                .file
                .node(object_index)
                .ok_or(BinQueryError::UndecodedMob(item_index, object_index))?;
            mobs.push(BinMob {
                item_index,
                object_index,
                mob,
            });
        }
        Ok(mobs)
    }

    fn typed_mobs(&self, kind: i64, toplevel: bool) -> Result<Vec<BinMob<'a>>, BinQueryError> {
        let mut output = Vec::new();
        for entry in self.mobs()? {
            if entry.mob.class != *b"CMPO" {
                return Err(BinQueryError::NotCompositionObject(
                    entry.item_index,
                    entry.object_index,
                ));
            }
            let id = entry.mob.get("mob_type_id").and_then(Value::as_int).ok_or(
                BinQueryError::MissingMobType(entry.item_index, entry.object_index),
            )?;
            if !(1..=3).contains(&id) {
                return Err(BinQueryError::UnknownMobType(
                    entry.item_index,
                    entry.object_index,
                    id,
                ));
            }
            if id != kind {
                continue;
            }
            if toplevel {
                let usage = entry.mob.get("usage_code").and_then(Value::as_int).ok_or(
                    BinQueryError::MissingUsageCode(entry.item_index, entry.object_index),
                )?;
                if usage != 0 {
                    continue;
                }
            }
            output.push(entry);
        }
        Ok(output)
    }

    /// pyavb `toplevel`: CompositionMob with usage_code 0.
    pub fn toplevel(&self) -> Result<Vec<BinMob<'a>>, BinQueryError> {
        self.typed_mobs(1, true)
    }

    /// pyavb `compositionmobs`: all CompositionMob items, including subclips/groups.
    pub fn compositionmobs(&self) -> Result<Vec<BinMob<'a>>, BinQueryError> {
        self.typed_mobs(1, false)
    }

    /// pyavb `mastermobs`: all MasterMob items.
    pub fn mastermobs(&self) -> Result<Vec<BinMob<'a>>, BinQueryError> {
        self.typed_mobs(2, false)
    }

    /// pyavb `find_by_mob_id`: the last stored item wins when IDs repeat.
    /// A missing ID returns `Ok(None)`; a malformed item/ID is an error.
    pub fn find_by_mob_id(&self, mob_id: &MobId) -> Result<Option<BinMob<'a>>, BinQueryError> {
        let mut found = None;
        for entry in self.mobs()? {
            let id = match entry.mob.get("mob_id") {
                Some(Value::MobId(id)) => id,
                _ => {
                    return Err(BinQueryError::MissingMobId(
                        entry.item_index,
                        entry.object_index,
                    ))
                }
            };
            if id == mob_id {
                found = Some(entry);
            }
        }
        Ok(found)
    }
}
