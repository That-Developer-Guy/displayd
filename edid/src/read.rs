use std::path::Path;

use crate::error::Error;
use crate::checksum::valid_checksum;
use crate::consts::{ EDID_BLOCK_SIZE, EDID_ADDRESS, EDID_SEGMENT_ADDRESS };
use i2cdev::{ core::{ I2CMessage, I2CTransfer }, linux::LinuxI2CDevice };

pub fn read_edid(path: impl AsRef<Path>) -> Result<Vec<u8>, Error> {
    let path = path.as_ref();
    let mut edid_dev = LinuxI2CDevice::new(path, EDID_ADDRESS)?;
    let mut segment_dev = LinuxI2CDevice::new(path, EDID_SEGMENT_ADDRESS)?;

    let base = read_block(&mut edid_dev, &mut segment_dev, 0)?;

    if base[..8] != [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00] {
        return Err(Error::InvalidReturnData);
    }

    if !valid_checksum(&base) {
        return Err(Error::InvalidChecksum);
    }

    let extension_count = base[126] as usize;

    let mut edid = Vec::with_capacity(EDID_BLOCK_SIZE * (1 + extension_count));

    edid.extend_from_slice(&base);

    for block in 1..=extension_count {
        let data = read_block(&mut edid_dev, &mut segment_dev, block)?;

        if !valid_checksum(&data) {
            return Err(Error::InvalidChecksum);
        }

        edid.extend_from_slice(&data);
    }

    Ok(edid)
}

fn read_block(
    edid_dev: &mut LinuxI2CDevice,
    segment_dev: &mut LinuxI2CDevice,
    block: usize
) -> Result<[u8; EDID_BLOCK_SIZE], Error> {
    let segment = (block / 2) as u8;
    let offset = ((block % 2) * EDID_BLOCK_SIZE) as u8;

    if block >= 2 {
        let segment_buf = [segment];

        let mut messages = [I2CMessage::write(&segment_buf)];

        segment_dev.transfer(&mut messages)?;
    }

    let offset_buf = [offset];
    let mut data = [0u8; EDID_BLOCK_SIZE];

    let mut messages = [I2CMessage::write(&offset_buf), I2CMessage::read(&mut data)];

    edid_dev.transfer(&mut messages)?;

    Ok(data)
}
