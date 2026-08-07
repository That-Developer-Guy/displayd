use crate::checksum;
use crate::{ Error, Result };

pub const HOST_ADDRESS: u8 = 0x51;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub command: u8,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(command: u8, payload: Vec<u8>) -> Self {
        Self {
            command,
            payload,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let length = 0x80 | ((self.payload.len() + 1) as u8);

        let mut frame = vec![0x51, length, self.command];

        frame.extend_from_slice(&self.payload);

        let mut checksum_input = vec![0x6e];
        checksum_input.extend_from_slice(&frame);

        let checksum = checksum::calculate(&checksum_input);

        frame.push(checksum);

        frame
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 3 {
            return Err(Error::InvalidPacket);
        }

        if bytes[0] == 0x6e {
            if !checksum::verify(bytes) {
                return Err(Error::InvalidChecksum);
            }
        } else {
            if !checksum::verify(bytes) {
                return Err(Error::InvalidChecksum);
            }
        }

        let command = bytes[2];

        let payload = if bytes.len() > 4 { bytes[3..bytes.len() - 1].to_vec() } else { Vec::new() };

        Ok(Self {
            command,
            payload,
        })
    }
}
