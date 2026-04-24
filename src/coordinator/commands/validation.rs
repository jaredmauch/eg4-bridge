use crate::prelude::*;

/// Size of a register block. Both hold and input registers are organized in blocks of 40.
pub const BLOCK_SIZE: u16 = 40;

/// Validates that a register read operation does not cross block boundaries.
/// Both hold and input registers are organized in blocks of 40 registers each.
/// Reading across block boundaries is not allowed by the protocol.
///
/// # Arguments
/// * `register` - The starting register number
/// * `count` - Number of registers to read
///
/// # Returns
/// * `Ok(())` if the read operation is valid
/// * `Err` with descriptive message if the operation would cross block boundaries
///
/// # Examples
/// ```
/// # use eg4_bridge::coordinator::commands::validation::validate_register_block_boundary;
/// // Valid: Reading 5 registers starting at 35 (all within block 0)
/// validate_register_block_boundary(35, 5).unwrap();
///
/// // Invalid: Reading 11 registers starting at 35 (crosses from block 0 to 1)
/// assert!(validate_register_block_boundary(35, 11).is_err());
/// ```
pub fn validate_register_block_boundary(register: u16, count: u16) -> Result<()> {
    // Calculate the block number for start and end registers
    let start_block = register / BLOCK_SIZE;
    let end_register = register + count - 1;
    let end_block = end_register / BLOCK_SIZE;

    // Check if the read operation crosses a block boundary
    if start_block != end_block {
        bail!(
            "Invalid read operation: Cannot read across block boundary. Register {} count {} would cross from block {} to block {}. \
            Each block is {} registers. Please limit your read to within a single block.",
            register,
            count,
            start_block,
            end_block,
            BLOCK_SIZE
        );
    }

    // Validate that count doesn't exceed remaining registers in the block
    let remaining_in_block = BLOCK_SIZE - (register % BLOCK_SIZE);
    if count > remaining_in_block {
        bail!(
            "Invalid read operation: Count {} exceeds remaining registers in block {}. \
            Maximum readable registers from position {} is {}.",
            count,
            start_block,
            register,
            remaining_in_block
        );
    }

    Ok(())
} 