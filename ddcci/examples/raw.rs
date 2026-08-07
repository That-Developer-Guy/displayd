use ddcci::packet::Packet;
use ddcci::transport::LinuxI2cTransport;
use ddcci::DdcDevice;

fn main() -> anyhow::Result<()> {
    // change depending on actual path
    let transport = LinuxI2cTransport::open("/dev/i2c-8")?;

    let mut device = DdcDevice::new(transport);

    let packet = Packet::new(0x01, vec![0x10]);

    let reply = device.transact(packet)?;

    println!("{reply:#?}");

    Ok(())
}
