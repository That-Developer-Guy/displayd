use crate::{ packet::Packet, Error, Result };

#[derive(Debug)]
pub enum Command {
    GetVcp(u8),

    SetVcp {
        feature: u8,
        value: u16,
    },

    Capabilities,
}

#[derive(Debug)]
pub struct VcpValue {
    pub current: u16,
    pub maximum: u16,
}

#[derive(Debug)]
pub enum Response {
    Vcp(VcpValue),

    Capabilities(String),

    Ack,
}

impl Command {
    pub fn into_packet(self) -> Packet {
        match self {
            Command::GetVcp(feature) => { Packet::new(0x01, vec![feature]) }

            Command::SetVcp { feature, value } => {
                Packet::new(0x03, vec![feature, (value >> 8) as u8, value as u8])
            }

            Command::Capabilities => { Packet::new(0xf3, vec![]) }
        }
    }
}

impl Response {
    pub fn from_packet(packet: Packet) -> Result<Self> {
        match packet.command {
            0x02 => {
                if packet.payload.len() < 6 {
                    return Err(Error::InvalidPacket);
                }

                let maximum = u16::from_be_bytes([packet.payload[2], packet.payload[3]]);

                let current = u16::from_be_bytes([packet.payload[4], packet.payload[5]]);

                Ok(
                    Response::Vcp(VcpValue {
                        current,
                        maximum,
                    })
                )
            }

            0xe3 => {
                let caps = String::from_utf8_lossy(&packet.payload).to_string();

                Ok(Response::Capabilities(caps))
            }

            _ => Ok(Response::Ack),
        }
    }
}
