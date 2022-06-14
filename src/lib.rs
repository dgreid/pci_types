#![no_std]

pub mod capability;
pub mod device_type;
mod register;

pub use register::{DevselTiming, StatusRegister};

use crate::capability::CapabilityIterator;
use bit_field::BitField;
use core::fmt;

/// PCIe supports 65536 segments, each with 256 buses, each with 32 slots, each with 8 possible functions. We cram this into a `u32`:
///
/// ```ignore
/// 32                              16               8         3      0
///  +-------------------------------+---------------+---------+------+
///  |            segment            |      bus      | device  | func |
///  +-------------------------------+---------------+---------+------+
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct PciAddress(u32);

impl PciAddress {
    const DEV_SHIFT: u32 = 3;
    const BUS_SHIFT: u32 = 8;
    const SEG_SHIFT: u32 = 16;

    pub fn new(segment: u16, bus: u8, device: u8, function: u8) -> PciAddress {
        let mut result = 0;
        result.set_bits(0..3, function as u32);
        result.set_bits(3..8, device as u32);
        result.set_bits(8..16, bus as u32);
        result.set_bits(16..32, segment as u32);
        PciAddress(result)
    }

    pub fn segment(&self) -> u16 {
        self.0.get_bits(16..32) as u16
    }

    pub fn bus(&self) -> u8 {
        self.0.get_bits(8..16) as u8
    }

    pub fn device(&self) -> u8 {
        self.0.get_bits(3..8) as u8
    }

    pub fn function(&self) -> u8 {
        self.0.get_bits(0..3) as u8
    }

    /// Increment the address to the increment sequential PCI function.
    /// Panics on bus overflow.
    pub fn increment_function(&mut self) {
        self.0.checked_add(1).unwrap();
    }

    /// Increment the address to the first function of the increment device.
    /// Panics on bus overflow.
    pub fn increment_device(&mut self) {
        self.0.checked_add(1 << Self::DEV_SHIFT).unwrap();
        self.0 &= (1 << Self::DEV_SHIFT) - 1;
    }

    /// Increment the address to the first device of the increment bus.
    /// Panics on bus overflow.
    pub fn increment_bus(&mut self) {
        self.0.checked_add(1 << Self::BUS_SHIFT).unwrap();
        self.0 &= (1 << Self::BUS_SHIFT) - 1;
    }

    /// Increment the address to the first bus of the increment segment.
    /// Panics on bus overflow.
    pub fn increment_segment(&mut self) {
        self.0.checked_add(1 << Self::SEG_SHIFT).unwrap();
        self.0 &= (1 << Self::SEG_SHIFT) - 1;
    }
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}-{:02x}:{:02x}.{}",
            self.segment(),
            self.bus(),
            self.device(),
            self.function()
        )
    }
}

pub type VendorId = u16;
pub type DeviceId = u16;
pub type DeviceRevision = u8;
pub type BaseClass = u8;
pub type SubClass = u8;
pub type Interface = u8;
pub type HeaderType = u8;

// TODO: documentation
pub trait ConfigRegionAccess: Send {
    fn function_exists(&self, address: PciAddress) -> bool;
    fn read(&self, address: PciAddress, offset: u16) -> u32;
    fn write(&self, address: PciAddress, offset: u16, value: u32);
}

pub const HEADER_TYPE_ENDPOINT: HeaderType = 0x00;
pub const HEADER_TYPE_PCI_PCI_BRIDGE: HeaderType = 0x01;
pub const HEADER_TYPE_CARDBUS_BRIDGE: HeaderType = 0x02;

/// Every PCI configuration region starts with a header made up of two parts:
///    - a predefined region that identify the function (bytes `0x00..0x10`)
///    - a device-dependent region that depends on the Header Type field
///
/// The predefined region is of the form:
/// ```ignore
///     32                            16                              0
///      +-----------------------------+------------------------------+
///      |       Device ID             |       Vendor ID              | 0x00
///      |                             |                              |
///      +-----------------------------+------------------------------+
///      |         Status              |       Command                | 0x04
///      |                             |                              |
///      +-----------------------------+---------------+--------------+
///      |               Class Code                    |   Revision   | 0x08
///      |                                             |      ID      |
///      +--------------+--------------+---------------+--------------+
///      |     BIST     |    Header    |    Latency    |  Cacheline   | 0x0c
///      |              |     type     |     timer     |    size      |
///      +--------------+--------------+---------------+--------------+
/// ```
pub struct PciHeader(PciAddress);

impl PciHeader {
    pub fn new(address: PciAddress) -> PciHeader {
        PciHeader(address)
    }

    pub fn id(&self, access: &impl ConfigRegionAccess) -> (VendorId, DeviceId) {
        let id = access.read(self.0, 0x00);
        (
            id.get_bits(0..16) as VendorId,
            id.get_bits(16..32) as DeviceId,
        )
    }

    pub fn header_type(&self, access: &impl ConfigRegionAccess) -> HeaderType {
        /*
         * Read bits 0..=6 of the Header Type. Bit 7 dictates whether the device has multiple functions and so
         * isn't returned here.
         */
        access.read(self.0, 0x0c).get_bits(16..23) as HeaderType
    }

    pub fn has_multiple_functions(&self, access: &impl ConfigRegionAccess) -> bool {
        /*
         * Reads bit 7 of the Header Type, which is 1 if the device has multiple functions.
         */
        access.read(self.0, 0x0c).get_bit(23)
    }

    pub fn revision_and_class(
        &self,
        access: &impl ConfigRegionAccess,
    ) -> (DeviceRevision, BaseClass, SubClass, Interface) {
        let field = access.read(self.0, 0x08);
        (
            field.get_bits(0..8) as DeviceRevision,
            field.get_bits(24..32) as BaseClass,
            field.get_bits(16..24) as SubClass,
            field.get_bits(8..16) as Interface,
        )
    }

    pub fn status(&self, access: &impl ConfigRegionAccess) -> StatusRegister {
        let data = access.read(self.0, 0x4).get_bits(16..32);
        StatusRegister::new(data as u16)
    }
}

/// Endpoints have a Type-0 header, so the remainder of the header is of the form:
/// ```ignore
///     32                           16                              0
///     +-----------------------------------------------------------+ 0x00
///     |                                                           |
///     |                Predefined region of header                |
///     |                                                           |
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 0                  | 0x10
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 1                  | 0x14
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 2                  | 0x18
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 3                  | 0x1c
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 4                  | 0x20
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  Base Address Register 5                  | 0x24
///     |                                                           |
///     +-----------------------------------------------------------+
///     |                  CardBus CIS Pointer                      | 0x28
///     |                                                           |
///     +----------------------------+------------------------------+
///     |       Subsystem ID         |    Subsystem vendor ID       | 0x2c
///     |                            |                              |
///     +----------------------------+------------------------------+
///     |               Expansion ROM Base Address                  | 0x30
///     |                                                           |
///     +--------------------------------------------+--------------+
///     |                 Reserved                   | Capabilities | 0x34
///     |                                            |   Pointer    |
///     +--------------------------------------------+--------------+
///     |                         Reserved                          | 0x38
///     |                                                           |
///     +--------------+--------------+--------------+--------------+
///     |   Max_Lat    |   Min_Gnt    |  Interrupt   |  Interrupt   | 0x3c
///     |              |              |   line       |   line       |
///     +--------------+--------------+--------------+--------------+
/// ```
pub struct EndpointHeader(PciAddress);

impl EndpointHeader {
    pub fn from_header(
        header: PciHeader,
        access: &impl ConfigRegionAccess,
    ) -> Option<EndpointHeader> {
        match header.header_type(access) {
            0x00 => Some(EndpointHeader(header.0)),
            _ => None,
        }
    }

    pub fn status(&self, access: &impl ConfigRegionAccess) -> StatusRegister {
        let data = access.read(self.0, 0x4).get_bits(16..32);
        StatusRegister::new(data as u16)
    }

    pub fn header(&self) -> PciHeader {
        PciHeader(self.0)
    }

    pub fn capability_pointer(&self, access: &impl ConfigRegionAccess) -> u16 {
        let status = self.status(access);
        if status.has_capability_list() {
            access.read(self.0, 0x34).get_bits(0..8) as u16
        } else {
            0
        }
    }

    pub fn capabilities<'a, T: ConfigRegionAccess>(
        &self,
        access: &'a T,
    ) -> CapabilityIterator<'a, T> {
        let pointer = self.capability_pointer(access);
        CapabilityIterator::new(self.0, pointer, access)
    }

    /// Get the contents of a BAR in a given slot. Empty bars will return `None`.
    ///
    /// ### Note
    /// 64-bit memory BARs use two slots, so if one is decoded in e.g. slot #0, this method should not be called
    /// for slot #1
    pub fn bar(&self, slot: u8, access: &impl ConfigRegionAccess) -> Option<Bar> {
        let offset = 0x10 + (slot as u16) * 4;
        let bar = access.read(self.0, offset);

        /*
         * If bit 0 is `0`, the BAR is in memory. If it's `1`, it's in I/O.
         */
        if bar.get_bit(0) == false {
            let prefetchable = bar.get_bit(3);
            let address = bar.get_bits(4..32) << 4;

            // TODO: if the bar is 64-bits, do we need to do this on both BARs?
            let size = {
                access.write(self.0, offset, 0xffffffff);
                let mut readback = access.read(self.0, offset);
                access.write(self.0, offset, address);

                /*
                 * If the entire readback value is zero, the BAR is not implemented, so we return `None`.
                 */
                if readback == 0x0 {
                    return None;
                }

                readback.set_bits(0..4, 0);
                1 << readback.trailing_zeros()
            };

            match bar.get_bits(1..3) {
                0b00 => Some(Bar::Memory32 {
                    address,
                    size,
                    prefetchable,
                }),
                0b10 => {
                    let address = {
                        let mut address = address as u64;
                        // TODO: do we need to mask off the lower bits on this?
                        address.set_bits(32..64, access.read(self.0, offset + 4) as u64);
                        address
                    };
                    Some(Bar::Memory64 {
                        address,
                        size: size as u64,
                        prefetchable,
                    })
                }
                // TODO: should we bother to return an error here?
                _ => panic!("BAR Memory type is reserved!"),
            }
        } else {
            Some(Bar::Io {
                port: bar.get_bits(2..32),
            })
        }
    }
}

pub const MAX_BARS: usize = 6;

#[derive(Clone, Copy, Debug)]
pub enum Bar {
    Memory32 {
        address: u32,
        size: u32,
        prefetchable: bool,
    },
    Memory64 {
        address: u64,
        size: u64,
        prefetchable: bool,
    },
    Io {
        port: u32,
    },
}
