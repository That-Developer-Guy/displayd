use ddcci::packet::Packet;

fn main() {
    let packet = Packet::new(0x01, vec![0x10]);

    let bytes = packet.encode();

    println!("Packet:");
    for b in &bytes {
        print!("{:02X} ", b);
    }
    println!();

    let decoded = Packet::decode(&bytes).expect("Failed to decode packet");

    println!("Decoded:");
    println!("command: {:02X}", decoded.command);
    println!("payload: {:02X?}", decoded.payload);
}
