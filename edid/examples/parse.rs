use edid::{ parse::{ ProductionDate, parse }, read::read_edid };

fn main() {
    // change depending on actual path
    let path = "/dev/i2c-8".to_string();

    match read_edid(&*path) {
        Ok(edid) => {
            if edid.len() >= 8 && edid[..8] == [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00] {
                match parse(&edid) {
                    Ok(edid_data) => {
                        println!("EDID Data ID:");
                        println!("ID: {}", edid_data.id);
                        println!("Name: {}", edid_data.name.as_deref().unwrap_or("Unknown"));
                        println!("Product code: 0x{:04x}", edid_data.product_code);
                        println!("Serial Number: 0x{:08x}", edid_data.serial_number);
                        match edid_data.production_date {
                            ProductionDate::Manufacture { week, year } => {
                                println!("Production date: Week {}, {}", week, year);
                            }
                            ProductionDate::ModelYear { year } => {
                                println!("Production date: Model year {}", year);
                            }
                        }
                        println!("EDID Version: {}", edid_data.edid_version);
                    }
                    Err(e) => { println!("Parsing failed: {}", e) }
                }
            }
        }

        Err(e) => {
            eprintln!("{path}: no EDID ({e})");
        }
    }
}
