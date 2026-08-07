use crate::{ packet::Packet, protocol::{ Command, Response }, transport::Transport, Result };

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
}
