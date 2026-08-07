use crate::Result;

use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;

use std::path::Path;
use std::thread;
use std::time::Duration;

pub trait Transport {
    fn transact(&mut self, request: &[u8]) -> Result<Vec<u8>>;
}

pub struct LinuxI2cTransport {
    dev: LinuxI2CDevice,
}

impl LinuxI2cTransport {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            dev: LinuxI2CDevice::new(path.as_ref(), 0x37)?,
        })
    }
    pub fn probe(path: impl AsRef<Path>) -> Result<bool> {
        let mut transport = Self::open(path)?;

        let request = [0x51, 0x82, 0xf3, 0x00, 0x00, 0x00];

        match transport.transact(&request) {
            Ok(response) => Ok(!response.is_empty()),
            Err(_) => Ok(false),
        }
    }
}

impl Transport for LinuxI2cTransport {
    fn transact(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        self.dev.write(request)?;

        // wait for the slow monitor
        thread::sleep(Duration::from_millis(50));

        // most likely too big
        let mut response = vec![0u8; 32];

        let mut last_error = None;

        for _attempt in 0..5 {
            match self.dev.read(&mut response) {
                Ok(_) => {
                    if response.len() >= 3 {
                        let length = (response[1] & 0x7f) as usize;

                        let total_len = length + 3;

                        if total_len <= response.len() {
                            response.truncate(total_len);
                        }
                    }

                    return Ok(response);
                }
                Err(e) => {
                    last_error = Some(e);
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }

        Err(last_error.unwrap().into())
    }
}
