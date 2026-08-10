use thiserror::Error;
use i2cdev::linux::LinuxI2CError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error")] Io(#[from] std::io::Error),

    #[error("Linux I²C error")] I2c(#[from] LinuxI2CError),

    #[error("Operation not supported on transport endpoint")]
    UnsupportedTransfer,

    #[error("Invalid return data")]
    InvalidReturnData,

    #[error("Invalid checksum")]
    InvalidChecksum,

    #[error("{0}")] Other(String),
}
