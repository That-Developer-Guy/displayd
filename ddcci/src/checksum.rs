// xoring all bytes should result in zero
pub fn calculate(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, byte| acc ^ byte)
}

pub fn verify(packet: &[u8]) -> bool {
    let mut xor = 0x50; // host receive address

    for b in packet {
        xor ^= *b;
    }

    xor == 0
}
