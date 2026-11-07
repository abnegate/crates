//! Inventory bookkeeping for the widget warehouse.
//!
//! Tracks stock levels and reservations per bin.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::error::StockError;

/// Default number of bins a warehouse starts with.
pub const DEFAULT_BINS: usize = 16;

static GLOBAL_COUNTER: AtomicCounter = AtomicCounter::new();

pub type BinId = u32;

/// A single storage bin and its contents.
#[derive(Debug, Clone, PartialEq)]
pub struct Bin {
    pub id: BinId,
    pub label: String,
    quantity: u64,
    reserved: u64,
}

/// Why a reservation could not be made.
#[derive(Debug)]
pub enum ReservationError {
    UnknownBin(BinId),
    Insufficient { requested: u64, available: u64 },
    Locked,
}

impl fmt::Display for ReservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBin(id) => write!(formatter, "unknown bin {id}"),
            Self::Insufficient { requested, available } => {
                write!(formatter, "requested {requested}, only {available} available")
            }
            Self::Locked => write!(formatter, "warehouse is locked"),
        }
    }
}

/// Anything that can report a stock level.
pub trait StockSource {
    fn available(&self, bin: BinId) -> Option<u64>;
    fn label(&self, bin: BinId) -> Option<&str>;
}

/// The warehouse holds every bin behind a lock.
pub struct Warehouse {
    bins: Arc<Mutex<HashMap<BinId, Bin>>>,
    locked: bool,
}

impl Warehouse {
    /// Create an empty warehouse with the default bin capacity.
    pub fn new() -> Self {
        Self {
            bins: Arc::new(Mutex::new(HashMap::with_capacity(DEFAULT_BINS))),
            locked: false,
        }
    }

    /// Add stock to a bin, creating it when missing.
    pub fn restock(&self, id: BinId, label: &str, quantity: u64) -> Result<u64, StockError> {
        let mut bins = self.bins.lock().map_err(|_| StockError::Poisoned)?;
        let bin = bins.entry(id).or_insert_with(|| Bin {
            id,
            label: label.to_string(),
            quantity: 0,
            reserved: 0,
        });
        bin.quantity = bin.quantity.saturating_add(quantity);
        Ok(bin.quantity)
    }

    /// Reserve stock for an order without removing it yet.
    pub fn reserve(&self, id: BinId, requested: u64) -> Result<u64, ReservationError> {
        if self.locked {
            return Err(ReservationError::Locked);
        }
        let mut bins = self.bins.lock().map_err(|_| ReservationError::Locked)?;
        let bin = bins.get_mut(&id).ok_or(ReservationError::UnknownBin(id))?;
        let available = bin.quantity - bin.reserved;
        if requested > available {
            return Err(ReservationError::Insufficient { requested, available });
        }
        bin.reserved += requested;
        Ok(available - requested)
    }

    fn lock(&mut self) {
        self.locked = true;
    }
}

impl StockSource for Warehouse {
    fn available(&self, bin: BinId) -> Option<u64> {
        let bins = self.bins.lock().ok()?;
        bins.get(&bin).map(|bin| bin.quantity - bin.reserved)
    }

    fn label(&self, _bin: BinId) -> Option<&str> {
        None
    }
}

mod audit {
    pub(crate) fn summarize(totals: &[u64]) -> u64 {
        totals.iter().copied().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_reduces_availability() {
        let warehouse = Warehouse::new();
        warehouse.restock(7, "bolts", 10).unwrap();
        assert_eq!(warehouse.reserve(7, 4).unwrap(), 6);
        assert_eq!(warehouse.available(7), Some(6));
    }
}
