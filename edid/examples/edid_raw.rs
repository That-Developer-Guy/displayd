use edid::read::read_edid;

fn main() {
    // change depending on actual path
    let path = "/dev/i2c-8".to_string();

    match read_edid(&*path) {
        Ok(edid) => {
            if edid.len() >= 8 && edid[..8] == [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00] {
                println!("{path}: EDID found");

                for byte in &edid {
                    print!("{byte:02x} ");
                }
                println!();
            }
        }

        Err(e) => {
            eprintln!("{path}: no EDID ({e})");
        }
    }
}
