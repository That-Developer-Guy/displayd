use crate::{ feature::Feature, packet::Packet, Error, Result };

#[derive(Debug)]
pub enum Command {
    GetVcp(Feature),

    SetVcp {
        feature: Feature,
        value: u16,
    },

    Capabilities,
}

#[derive(Debug)]
pub struct VcpValue {
    pub feature: Feature,
    pub current: u16,
    pub maximum: u16,
}

impl VcpValue {
    pub fn percentage(&self) -> f32 {
        if self.maximum == 0 {
            return 0.0;
        }

        ((self.current as f32) / (self.maximum as f32)) * 100.0
    }
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
            Command::GetVcp(feature) => { Packet::new(0x01, vec![feature.into()]) }

            Command::SetVcp { feature, value } => {
                Packet::new(0x03, vec![feature.into(), (value >> 8) as u8, value as u8])
            }

            Command::Capabilities => { Packet::new(0xf3, Vec::new()) }
        }
    }
}

impl Response {
    pub fn from_packet(packet: Packet) -> Result<Self> {
        match packet.command {
            0x02 => {
                if packet.payload.len() < 7 {
                    return Err(Error::InvalidPacket);
                }

                let result = packet.payload[0];

                if result != 0 {
                    return Err(Error::InvalidPacket);
                }

                let feature = Feature::from(packet.payload[1]);

                let maximum = u16::from_be_bytes([packet.payload[3], packet.payload[4]]);

                let current = u16::from_be_bytes([packet.payload[5], packet.payload[6]]);

                Ok(
                    Response::Vcp(VcpValue {
                        feature,
                        current,
                        maximum,
                    })
                )
            }

            0xe3 => {
                let caps = String::from_utf8_lossy(&packet.payload).into_owned();

                Ok(Response::Capabilities(caps))
            }

            _ => Ok(Response::Ack),
        }
    }
}
