use crate::Result;
use crate::transport::{ LinuxI2cTransport, ProbeResult };

pub fn find_monitors() -> Result<Vec<ProbeResult>> {
    let mut monitors = Vec::new();

    for entry in std::fs::read_dir("/dev")? {
        let path = entry?.path();

        let Some(name) = path.file_name() else {
            continue;
        };

        if !name.to_string_lossy().starts_with("i2c-") {
            continue;
        }

        if let Some(result) = LinuxI2cTransport::probe(&path)? {
            monitors.push(result);
        }
    }

    Ok(monitors)
}

pub fn find_monitor() -> Result<Option<ProbeResult>> {
    Ok(find_monitors()?.into_iter().next())
}
