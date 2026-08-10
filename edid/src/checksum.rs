use crate::consts::EDID_BLOCK_SIZE;

pub fn valid_checksum(block: &[u8]) -> bool {
    block.len() == EDID_BLOCK_SIZE &&
        block.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)) == 0
}
