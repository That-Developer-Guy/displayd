use thiserror::Error;
use i2cdev::linux::LinuxI2CError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error")] Io(#[from] std::io::Error),

    #[error("Linux I²C error")] I2c(#[from] LinuxI2CError),

    #[error("Invalid checksum")]
    InvalidChecksum,

    #[error("Invalid packet")]
    InvalidPacket,

    #[error("Unexpected response")]
    UnexpectedResponse,

    #[error("Timed out")]
    Timeout,

    #[error("Monitor did not provide a response")]
    NoResponse,

    #[error("{0}")] Other(String),
}
