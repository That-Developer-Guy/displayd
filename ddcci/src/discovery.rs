use crate::Result;
use crate::transport::LinuxI2cTransport;

use std::path::PathBuf;

pub fn find_monitors() -> Result<Vec<PathBuf>> {
    let mut monitors = Vec::new();

    for entry in std::fs::read_dir("/dev")? {
        let path = entry?.path();

        if let Some(name) = path.file_name() {
            if name.to_string_lossy().starts_with("i2c-") {
                if LinuxI2cTransport::probe(&path)? {
                    monitors.push(path);
                }
            }
        }
    }

    Ok(monitors)
}
