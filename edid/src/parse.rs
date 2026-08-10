use crate::error::Error;

pub enum ProductionDate {
    Manufacture {
        week: u8,
        year: u16,
    },
    ModelYear {
        year: u16,
    },
}

pub struct EdidData {
    pub id: String,
    pub name: Option<String>,
    pub product_code: u16,
    pub serial_number: u32,
    pub production_date: ProductionDate,
}

pub fn parse(edid: &[u8]) -> Result<EdidData, Error> {
    if edid.len() < 128 {
        return Err(Error::InvalidReturnData);
    }

    let manufacturer = u16::from_be_bytes([edid[8], edid[9]]);

    let first = ((manufacturer >> 10) & 0x1f) as u8;
    let second = ((manufacturer >> 5) & 0x1f) as u8;
    let third = (manufacturer & 0x1f) as u8;

    let id = format!(
        "{}{}{}",
        manufacturer_letter(first)?,
        manufacturer_letter(second)?,
        manufacturer_letter(third)?
    );

    let product_code = u16::from_le_bytes([edid[10], edid[11]]);

    let serial_number = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);

    let production_date = if edid[16] == 0xff {
        ProductionDate::ModelYear {
            year: (edid[17] as u16) + 1990,
        }
    } else {
        ProductionDate::Manufacture {
            week: edid[16],
            year: (edid[17] as u16) + 1990,
        }
    };

    let name = parse_name(edid);

    Ok(EdidData {
        id,
        name,
        product_code,
        serial_number,
        production_date,
    })
}

fn parse_name(edid: &[u8]) -> Option<String> {
    for offset in [54, 72, 90, 108] {
        if edid[offset + 3] == 0xfc {
            let name = &edid[offset + 5..offset + 18];

            let end = name
                .iter()
                .position(|&byte| (byte == 0x0a || byte == 0x00))
                .unwrap_or(name.len());

            return Some(
                String::from_utf8_lossy(&name[..end])
                    .trim()
                    .to_string()
            );
        }
    }

    None
}

fn manufacturer_letter(value: u8) -> Result<char, Error> {
    if !(1..=26).contains(&value) {
        return Err(Error::InvalidReturnData);
    }

    Ok((b'A' + value - 1) as char)
}
