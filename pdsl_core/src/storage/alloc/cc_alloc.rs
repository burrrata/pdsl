// Copyright 2018-2019 Parity Technologies (UK) Ltd.
// This file is part of pDSL.
//
// pDSL is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// pDSL is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with pDSL.  If not, see <http://www.gnu.org/licenses/>.

use super::*;
use crate::{
	storage::{
		self,
		Key,
	},
};

use parity_codec::{Encode, Decode};

const CC_ALLOC_LOG_TARGET: &'static str = "cc_alloc";

/// An allocator for the contract storage.
///
/// Specialized to efficiently allocate and deallocate cells and chunks.
///
/// # Note
///
/// This allocator allows for two types of allocations:
///
/// 1. Single cell allocation
/// 2. Cell chunk allocation (2^32 cells)
///
/// Allocating and deallocating are always O(1) operations.
#[derive(Debug, Encode, Decode)]
pub struct CellChunkAlloc {
	/// Allocator stash for single cells.
	cells: storage::Stash<()>,
	/// Allocator stash for cell chunks.
	chunks: storage::Stash<()>,
	/// Cells key offset.
	cells_off: storage::Key,
	/// Chunks key offset.
	chunks_off: storage::Key,
}

impl CellChunkAlloc {
	/// Creates a new cell & chunks allocator using the given allocator.
	///
	/// # Note
	///
	/// At first it might seem strange to initialize the one allocator
	/// with another. Normally a `CellChunkAllocator` should be allocated
	/// using a `ForwardAllocator`. The `ForwardAllocator` cannot be
	/// stored in the contract storage and is not useful for dynamic
	/// memory allocations but only for compile time allocations. The
	/// `CellChunkAllocator`, however, is made especially for the purpose
	/// of dynamic contract storage allocations and can and should be itself
	/// stored in the contract storage.
	pub unsafe fn new_using_alloc<A>(alloc: &mut A) -> Self
	where
		A: storage::Allocator
	{
		Self {
			cells: storage::Stash::new_using_alloc(alloc),
			chunks: storage::Stash::new_using_alloc(alloc),
			cells_off: alloc.alloc(u32::max_value()),
			chunks_off:
				// We need `u64::max_value()` here.
				// This depends on work on the Key API
				// to allow for `core::ops::Add<u64>`.
				//
				// As first iteration this should suffice our needs
				// as long as we allocate the `CellChunkAlloc` at last.
				alloc.alloc(u32::max_value()),
		}
	}

	/// Returns the key to the first cell allocation.
	///
	/// # Note
	///
	/// This key is then used to determine the key for every
	/// other cell allocation using its allocation index.
	pub(crate) fn cells_offset_key(&self) -> Key {
		self.cells_off
	}

	/// Returns the key to the first chunk allocation.
	///
	/// # Note
	///
	/// This key is then used to determine the key for every
	/// other chunk allocation using its allocation index.
	pub(crate) fn chunks_offset_key(&self) -> Key {
		self.chunks_off
	}

	/// Allocates a new storage region that fits for a single cell.
	fn alloc_cell(&mut self) -> Key {
		let index = self.cells.put(());
		let key = self.cell_index_to_key(index);
		log::info!(
			target: CC_ALLOC_LOG_TARGET,
			"allocated cell at {:?}",
			key,
		);
		key
	}

	/// Allocates a new storage region that fits for a whole chunk.
	fn alloc_chunk(&mut self) -> Key {
		let index = self.chunks.put(());
		let key = self.chunk_index_to_key(index);
		log::info!(
			target: CC_ALLOC_LOG_TARGET,
			"allocated chunk at {:?}",
			key,
		);
		key
	}

	/// Deallocates a storage region fit for a single cell.
	fn dealloc_cell(&mut self, key: Key) {
		let index = self.key_to_cell_index(key);
		log::info!(
			target: CC_ALLOC_LOG_TARGET,
			"deallocate cell at {:?}",
			key,
		);
		self.cells.take(index)
			.expect(
				"[pdsl_core::CellChunkAlloc::dealloc_cell] Error: \
				 key was not allocated by the allocator"
			)
	}

	/// Deallocates a storage region fit for a whole chunk.
	fn dealloc_chunk(&mut self, key: Key) {
		let index = self.key_to_chunk_index(key);
		log::info!(
			target: CC_ALLOC_LOG_TARGET,
			"deallocate chunk at {:?}",
			key,
		);
		self.chunks.take(index)
			.expect(
				"[pdsl_core::CellChunkAlloc::dealloc_chunk] Error: \
				 key was not allocated by the allocator"
			)
	}

	/// Converts cell indices to keys.
	///
	/// The reverse of `key_to_cell_index`.
	fn cell_index_to_key(&self, index: u32) -> Key {
		self.cells_offset_key() + index
	}

	/// Converts keys to cell indices.
	///
	/// The reverse of `cell_index_to_key`.
	fn key_to_cell_index(&self, key: Key) -> u32 {
		let diff = key - self.cells_offset_key();
		diff.try_to_u32()
			.expect(
				"if allocated by this allocator the difference between
				 the given key and offset key must be less-than or equal
				 to u32::MAX."
			)
	}

	/// Converts chunk indices to keys.
	///
	/// The reverse of `key_to_chunk_index`.
	fn chunk_index_to_key(&self, index: u32) -> Key {
		let chunk_offset: u64 = (1 << 32) * (index as u64);
		self.chunks_offset_key() + chunk_offset
	}

	/// Converts keys to chunk indices.
	///
	/// The reverse of `chunk_index_to_key`.
	fn key_to_chunk_index(&self, key: Key) -> u32 {
		let diff = key - self.cells_offset_key();
		let index = diff.try_to_u64()
			.expect(
				"if allocated by this allocator the difference between
				 the given key and offset key must be less-than or equal
				 to u64::MAX."
			);
		(index >> 32) as u32
	}
}

impl Allocator for CellChunkAlloc {
	fn alloc(&mut self, size: u32) -> Key {
		if size == 0 {
			log::warn!(
				target: CC_ALLOC_LOG_TARGET,
				"tried to allocate size zero (0)",
			);
		}
		debug_assert!(size != 0);
		log::debug!(
			target: CC_ALLOC_LOG_TARGET,
			"allocate for size {:?}",
			size,
		);
		if size <= 1 {
			self.alloc_cell()
		} else {
			self.alloc_chunk()
		}
	}

	fn dealloc(&mut self, key: Key) {
		// This assumes that the given key was previously
		// generated by the associated call to `Allocator::alloc`
		// of this same allocator implementor.
		assert!(key >= self.cells_offset_key());
		// This condition requires cells offset key
		// to be always smaller than chunks offset key.
		//
		// This must either be an invariant or we need
		// another more safe condition in the future.
		if key < self.chunks_offset_key() {
			// The key was allocated as a cell
			self.dealloc_cell(key)
		} else {
			// The key was allocated as a chunk
			self.dealloc_chunk(key)
		}
	}
}
