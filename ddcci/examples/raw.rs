use ddcci::{ feature::Feature, transport::LinuxI2cTransport, DdcDevice };

fn main() -> anyhow::Result<()> {
    // change depending on actual path
    let transport = LinuxI2cTransport::open("/dev/i2c-8")?;

    let mut device = DdcDevice::new(transport);

    let brightness = device.get_vcp(Feature::Brightness)?;

    println!("Current brightness: {}/{}", brightness.current, brightness.maximum);

    let new_value = if brightness.current >= 50 { 25 } else { 75 };

    device.set_vcp(Feature::Brightness, new_value)?;

    let brightness = device.get_vcp(Feature::Brightness)?;

    println!(
        "New brightness: {}/{} ({:.0}%)",
        brightness.current,
        brightness.maximum,
        brightness.percentage()
    );

    Ok(())
}
