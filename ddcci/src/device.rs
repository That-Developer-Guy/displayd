use crate::{
    packet::Packet,
    protocol::{ Command, Response, VcpValue },
    transport::Transport,
    feature::Feature,
    error::Error,
    Result,
};

pub struct DdcDevice<T: Transport> {
    transport: T,
}

impl<T: Transport> DdcDevice<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
        }
    }

    pub fn transact(&mut self, packet: Packet) -> Result<Packet> {
        let bytes = packet.encode();

        let response = self.transport.transact(&bytes)?;

        Packet::decode(&response)
    }

    pub fn execute(&mut self, command: Command) -> Result<Response> {
        let request = command.into_packet();

        let response = self.transact(request)?;

        Response::from_packet(response)
    }

    pub fn get_vcp(&mut self, feature: Feature) -> Result<VcpValue> {
        match self.execute(Command::GetVcp(feature))? {
            Response::Vcp(value) => Ok(value),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub fn set_vcp(&mut self, feature: Feature, value: u16) -> Result<()> {
        let response = self.execute(Command::SetVcp {
            feature,
            value,
        })?;

        println!("Set response: {response:#?}");

        Ok(())
    }

    pub fn set_vcp_checked(&mut self, feature: Feature, value: u16) -> Result<()> {
        let current = self.get_vcp(feature)?;

        if value > current.maximum {
            return Err(Error::ValueOutOfRange {
                value,
                maximum: current.maximum,
                feature,
            });
        }

        self.set_vcp(feature, value)
    }
}
