//! SDO abort codes (CiA 301, section 7.2.4.3.17).

/// A standard CANopen SDO abort code.
///
/// The wire representation is a little-endian `u32` in bytes 4..8 of an
/// abort message. `From<u32>` / `Into<u32>` round-trip those raw codes;
/// unknown values are preserved as [`SdoAbortCode::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdoAbortCode {
    /// 0x05030000 — Toggle bit not alternated.
    ToggleBitNotAlternated,
    /// 0x05040000 — SDO protocol timed out.
    ProtocolTimeout,
    /// 0x05040001 — Client/server command specifier not valid or unknown.
    InvalidCommandSpecifier,
    /// 0x05040002 — Invalid block size (block mode only).
    InvalidBlockSize,
    /// 0x05040003 — Invalid sequence number (block mode only).
    InvalidSequenceNumber,
    /// 0x05040004 — CRC error (block mode only).
    CrcError,
    /// 0x05040005 — Out of memory.
    OutOfMemory,
    /// 0x06010000 — Unsupported access to an object.
    UnsupportedAccess,
    /// 0x06010001 — Attempt to read a write-only object.
    ReadWriteOnly,
    /// 0x06010002 — Attempt to write a read-only object.
    WriteReadOnly,
    /// 0x06020000 — Object does not exist in the object dictionary.
    ObjectDoesNotExist,
    /// 0x06040041 — Object cannot be mapped to the PDO.
    NotMappable,
    /// 0x06040042 — Number / length of objects to be mapped exceeds PDO length.
    PdoLengthExceeded,
    /// 0x06040043 — General parameter incompatibility reason.
    ParameterIncompatibility,
    /// 0x06040047 — General internal incompatibility in the device.
    InternalIncompatibility,
    /// 0x06060000 — Access failed due to a hardware error.
    HardwareError,
    /// 0x06070010 — Data type does not match, length of service parameter does not match.
    DataTypeLengthMismatch,
    /// 0x06070012 — Data type does not match, length of service parameter too high.
    DataTypeLengthHigh,
    /// 0x06070013 — Data type does not match, length of service parameter too low.
    DataTypeLengthLow,
    /// 0x06090011 — Sub-index does not exist.
    SubindexDoesNotExist,
    /// 0x06090030 — Invalid value for parameter.
    InvalidValue,
    /// 0x06090031 — Value of parameter written too high.
    ValueTooHigh,
    /// 0x06090032 — Value of parameter written too low.
    ValueTooLow,
    /// 0x06090036 — Maximum value is less than minimum value.
    MaxLessThanMin,
    /// 0x060A0023 — Resource not available.
    ResourceNotAvailable,
    /// 0x08000000 — General error.
    General,
    /// 0x08000020 — Data cannot be transferred or stored to the application.
    StorageError,
    /// 0x08000021 — …because of local control.
    StorageLocalControl,
    /// 0x08000022 — …because of the present device state.
    StorageDeviceState,
    /// 0x08000023 — Object dictionary dynamic generation fails or no object dictionary is present.
    NoObjectDictionary,
    /// 0x08000024 — No data available.
    NoData,
    /// Anything not recognised above.
    Unknown(u32),
}

impl SdoAbortCode {
    pub fn raw(self) -> u32 {
        match self {
            Self::ToggleBitNotAlternated => 0x0503_0000,
            Self::ProtocolTimeout => 0x0504_0000,
            Self::InvalidCommandSpecifier => 0x0504_0001,
            Self::InvalidBlockSize => 0x0504_0002,
            Self::InvalidSequenceNumber => 0x0504_0003,
            Self::CrcError => 0x0504_0004,
            Self::OutOfMemory => 0x0504_0005,
            Self::UnsupportedAccess => 0x0601_0000,
            Self::ReadWriteOnly => 0x0601_0001,
            Self::WriteReadOnly => 0x0601_0002,
            Self::ObjectDoesNotExist => 0x0602_0000,
            Self::NotMappable => 0x0604_0041,
            Self::PdoLengthExceeded => 0x0604_0042,
            Self::ParameterIncompatibility => 0x0604_0043,
            Self::InternalIncompatibility => 0x0604_0047,
            Self::HardwareError => 0x0606_0000,
            Self::DataTypeLengthMismatch => 0x0607_0010,
            Self::DataTypeLengthHigh => 0x0607_0012,
            Self::DataTypeLengthLow => 0x0607_0013,
            Self::SubindexDoesNotExist => 0x0609_0011,
            Self::InvalidValue => 0x0609_0030,
            Self::ValueTooHigh => 0x0609_0031,
            Self::ValueTooLow => 0x0609_0032,
            Self::MaxLessThanMin => 0x0609_0036,
            Self::ResourceNotAvailable => 0x060A_0023,
            Self::General => 0x0800_0000,
            Self::StorageError => 0x0800_0020,
            Self::StorageLocalControl => 0x0800_0021,
            Self::StorageDeviceState => 0x0800_0022,
            Self::NoObjectDictionary => 0x0800_0023,
            Self::NoData => 0x0800_0024,
            Self::Unknown(raw) => raw,
        }
    }
}

impl From<u32> for SdoAbortCode {
    fn from(raw: u32) -> Self {
        match raw {
            0x0503_0000 => Self::ToggleBitNotAlternated,
            0x0504_0000 => Self::ProtocolTimeout,
            0x0504_0001 => Self::InvalidCommandSpecifier,
            0x0504_0002 => Self::InvalidBlockSize,
            0x0504_0003 => Self::InvalidSequenceNumber,
            0x0504_0004 => Self::CrcError,
            0x0504_0005 => Self::OutOfMemory,
            0x0601_0000 => Self::UnsupportedAccess,
            0x0601_0001 => Self::ReadWriteOnly,
            0x0601_0002 => Self::WriteReadOnly,
            0x0602_0000 => Self::ObjectDoesNotExist,
            0x0604_0041 => Self::NotMappable,
            0x0604_0042 => Self::PdoLengthExceeded,
            0x0604_0043 => Self::ParameterIncompatibility,
            0x0604_0047 => Self::InternalIncompatibility,
            0x0606_0000 => Self::HardwareError,
            0x0607_0010 => Self::DataTypeLengthMismatch,
            0x0607_0012 => Self::DataTypeLengthHigh,
            0x0607_0013 => Self::DataTypeLengthLow,
            0x0609_0011 => Self::SubindexDoesNotExist,
            0x0609_0030 => Self::InvalidValue,
            0x0609_0031 => Self::ValueTooHigh,
            0x0609_0032 => Self::ValueTooLow,
            0x0609_0036 => Self::MaxLessThanMin,
            0x060A_0023 => Self::ResourceNotAvailable,
            0x0800_0000 => Self::General,
            0x0800_0020 => Self::StorageError,
            0x0800_0021 => Self::StorageLocalControl,
            0x0800_0022 => Self::StorageDeviceState,
            0x0800_0023 => Self::NoObjectDictionary,
            0x0800_0024 => Self::NoData,
            other => Self::Unknown(other),
        }
    }
}

impl From<SdoAbortCode> for u32 {
    fn from(code: SdoAbortCode) -> u32 {
        code.raw()
    }
}

impl core::fmt::Display for SdoAbortCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "0x{:08X} ({:?})", self.raw(), self)
    }
}
