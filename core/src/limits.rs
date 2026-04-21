//! Block, message, and transport size limits.
//!
//! Collected here so cross-layer relationships are visible in one place.

/// 1 MiB — total transaction bytes in a single block.
pub const MAX_BLOCK_SIZE: usize = 1024 * 1024;

pub const MAX_BLOCK_TRANSACTIONS: usize = 1024;

/// 16 MiB — must fit a full block over gossipsub.
pub const MAX_GOSSIPSUB_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// 16 MiB — must fit a full block during chain sync.
pub const MAX_SYNC_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// 16 MiB — upper bound on transaction bytes the leader collects per proposal.
pub const MAX_BLOCK_PROPOSAL_FILL_SIZE: usize = 16 * 1024 * 1024;
