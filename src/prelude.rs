//! The `everscale-types` prelude.
//!
//! This brings into scope a number of traits and commonly used type aliases.

pub use crate::boc::{Boc, BocRepr};
pub use crate::cell::{
    Cell, CellBuilder, CellContext, CellFamily, CellImpl, CellSlice, CellSliceParts,
    CellSlicePartsExt, CellSliceRange, CellType, DynCell, EquivalentRepr, ExactSize, HashBytes,
    Load, OwnedCellSlice, Size, Store, UsageTree, UsageTreeMode, WeakCell,
};
pub use crate::dict::{AugDict, Dict, RawDict};
