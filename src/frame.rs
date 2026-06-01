//! The minimal CAN frame view used by the sans-io state machine.
//!
//! SDO only ever uses 11-bit standard identifiers with an 8-byte
//! payload, so we don't depend on the full `can-transport::CanFrame`
//! here — that conversion happens in the async glue layer.

/// CANopen COB-IDs that participate in SDO.
pub const COB_RSDO_BASE: u16 = 0x600; // client → server (request)
pub const COB_TSDO_BASE: u16 = 0x580; // server → client (response)

/// A single CAN frame as seen by the SDO state machine.
///
/// Always 8 data bytes, standard 11-bit COB-ID, no RTR, no CAN-FD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdoFrame {
    pub cob_id: u16,
    pub data: [u8; 8],
}

impl SdoFrame {
    pub fn new(cob_id: u16, data: [u8; 8]) -> Self {
        Self { cob_id, data }
    }

    /// COB-ID an SDO client uses to *send* a request to `node_id`.
    pub fn rsdo_id(node_id: u8) -> u16 {
        COB_RSDO_BASE | (node_id as u16 & 0x7F)
    }

    /// COB-ID a server uses to *respond* to `node_id`.
    pub fn tsdo_id(node_id: u8) -> u16 {
        COB_TSDO_BASE | (node_id as u16 & 0x7F)
    }

    /// Extract the node ID from a COB-ID. Returns `None` if the COB-ID
    /// is not an SDO frame for this node space.
    pub fn node_of_tsdo(cob_id: u16) -> Option<u8> {
        if cob_id & 0x780 == COB_TSDO_BASE {
            Some((cob_id & 0x7F) as u8)
        } else {
            None
        }
    }
}
