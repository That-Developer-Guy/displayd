// xoring all bytes should result in zero
pub fn calculate(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, byte| acc ^ byte)
}

pub fn verify(packet: &[u8]) -> bool {
    !packet.is_empty() && calculate(packet) == 0
}
