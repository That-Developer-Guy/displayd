use thiserror::Error;
use i2cdev::linux::LinuxI2CError;
use crate::feature::Feature;

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

    #[error("Value {value} exceeds maximum {maximum} for {feature:?}")] ValueOutOfRange {
        value: u16,
        maximum: u16,
        feature: Feature,
    },

    #[error("{0}")] Other(String),
}
