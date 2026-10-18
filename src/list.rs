//! List operations and iterator handle definitions
//!
//! Ref: `DESIGN.md` §7.4

use crate::engine::Engine;
use crate::error::TurboResult;
use crate::ticket::TurboTicket;
use std::sync::Arc;

/// Handle representing a 1-to-N list structure in TurboFox
#[derive(Debug, Clone)]
pub struct TurboList {
    engine: Arc<Engine>,
    key: Vec<u8>,
}

impl TurboList {
    #[inline(always)]
    pub(crate) fn new(engine: Arc<Engine>, key: Vec<u8>) -> Self {
        Self { engine, key }
    }

    /// Target key associated with this list handle
    #[inline(always)]
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// Appends an item payload to the tail of the list
    #[inline(always)]
    pub fn append(&self, item: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.list_append(&self.key, item)
    }

    /// Pops and removes the item payload at the tail of the list
    #[inline(always)]
    pub fn pop(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_pop(&self.key)
    }

    /// Retrieves the first item payload in the list without removing it
    #[inline(always)]
    pub fn first(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_first(&self.key)
    }

    /// Retrieves the last item payload in the list without removing it
    #[inline(always)]
    pub fn last(&self) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_last(&self.key)
    }

    /// Retrieves the item payload at the specified 0-based index
    #[inline(always)]
    pub fn get_at(&self, idx: usize) -> TurboResult<Option<Vec<u8>>> {
        self.engine.list_get_at(&self.key, idx)
    }

    /// Total number of item entries in this list
    #[inline(always)]
    pub fn len(&self) -> TurboResult<usize> {
        self.engine.list_len(&self.key)
    }

    /// Returns true if this list contains zero items
    #[inline(always)]
    pub fn is_empty(&self) -> TurboResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Returns an iterator over all item payloads in the list
    #[inline(always)]
    pub fn iter(&self) -> TurboResult<TurboListIter> {
        let items = self.engine.list_items(&self.key)?;
        Ok(TurboListIter { items, index: 0 })
    }

    /// Removes this list and frees all associated blocks and buffers
    #[inline(always)]
    pub fn clear(&self) -> TurboResult<bool> {
        self.engine.list_del(&self.key)
    }
}

/// Iterator over items in a TurboList
#[derive(Debug, Clone)]
pub struct TurboListIter {
    items: Vec<Vec<u8>>,
    index: usize,
}

impl Iterator for TurboListIter {
    type Item = Vec<u8>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.items.len() {
            let item = self.items[self.index].clone();
            self.index += 1;
            Some(item)
        } else {
            None
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.items.len().saturating_sub(self.index);
        (rem, Some(rem))
    }
}

impl ExactSizeIterator for TurboListIter {}
