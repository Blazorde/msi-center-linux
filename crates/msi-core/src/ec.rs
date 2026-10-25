//! EC register map, as typed data. No I/O: the daemon talks to the EC only through the msi-ec sysfs interface,
//! these constants document what msi-ec writes and let the daemon check read-backs (S7) and firmware (S8).
//!
//! Source: upstream msi-ec `CONF_G2_5` (see `docs/hardware/stealth-14-a1vgg.md`). Fan curve tables are not
//! here yet: TODO(ec-spike) add them from `docs/hardware/ec-map.md`.

/// A single bit of an EC register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EcBit {
    /// Register address.
    pub address: u8,
    /// Bit index (0 = LSB).
    pub bit: u8,
}

impl EcBit {
    /// Mask of the bit inside its register.
    #[must_use]
    pub const fn mask(self) -> u8 {
        1 << self.bit
    }
}

/// A register field selected by a mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaskedRegister {
    /// Register address.
    pub address: u8,
    /// Mask of the field.
    pub mask: u8,
}

/// EC performance ("shift") mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShiftMode {
    /// Lowest power.
    Eco,
    /// Balanced.
    Comfort,
    /// Highest performance.
    Turbo,
}

impl ShiftMode {
    /// Name used by the msi-ec `shift_mode` sysfs attribute.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eco => "eco",
            Self::Comfort => "comfort",
            Self::Turbo => "turbo",
        }
    }
}

/// EC fan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FanMode {
    /// Firmware curve. The safe fallback (S5, S7).
    Auto,
    /// Firmware silent curve.
    Silent,
    /// Custom curve tables.
    Advanced,
}

impl FanMode {
    /// Name used by the msi-ec `fan_mode` sysfs attribute.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Silent => "silent",
            Self::Advanced => "advanced",
        }
    }
}

/// Shift mode register and its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShiftModeMap {
    /// Register address.
    pub address: u8,
    /// Value for [`ShiftMode::Eco`].
    pub eco: u8,
    /// Value for [`ShiftMode::Comfort`].
    pub comfort: u8,
    /// Value for [`ShiftMode::Turbo`].
    pub turbo: u8,
}

impl ShiftModeMap {
    /// Register value for `mode`.
    #[must_use]
    pub const fn value(&self, mode: ShiftMode) -> u8 {
        match mode {
            ShiftMode::Eco => self.eco,
            ShiftMode::Comfort => self.comfort,
            ShiftMode::Turbo => self.turbo,
        }
    }

    /// Mode for a register value, `None` if unknown.
    #[must_use]
    pub fn mode(&self, value: u8) -> Option<ShiftMode> {
        [ShiftMode::Eco, ShiftMode::Comfort, ShiftMode::Turbo]
            .into_iter()
            .find(|&m| self.value(m) == value)
    }
}

/// Fan mode register and its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FanModeMap {
    /// Register address.
    pub address: u8,
    /// Value for [`FanMode::Auto`].
    pub auto: u8,
    /// Value for [`FanMode::Silent`].
    pub silent: u8,
    /// Value for [`FanMode::Advanced`].
    pub advanced: u8,
}

impl FanModeMap {
    /// Register value for `mode`.
    #[must_use]
    pub const fn value(&self, mode: FanMode) -> u8 {
        match mode {
            FanMode::Auto => self.auto,
            FanMode::Silent => self.silent,
            FanMode::Advanced => self.advanced,
        }
    }

    /// Mode for a register value, `None` if unknown.
    #[must_use]
    pub fn mode(&self, value: u8) -> Option<FanMode> {
        [FanMode::Auto, FanMode::Silent, FanMode::Advanced]
            .into_iter()
            .find(|&m| self.value(m) == value)
    }
}

/// Real-time temperature and fan registers of one chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SensorMap {
    /// Temperature register (°C).
    pub temp: u8,
    /// Fan speed register.
    pub fan_speed: u8,
}

/// Webcam switch registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WebcamMap {
    /// Enable bit.
    pub enable: EcBit,
    /// Hardware block bit.
    pub block: EcBit,
}

/// One msi-ec configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EcConfig {
    /// msi-ec configuration name.
    pub name: &'static str,
    /// EC firmware versions this configuration is known to be valid for (S8).
    pub allowed_firmware: &'static [&'static str],
    /// Shift mode.
    pub shift_mode: ShiftModeMap,
    /// Fan mode.
    pub fan_mode: FanModeMap,
    /// Cooler Boost (fans at full speed).
    pub cooler_boost: EcBit,
    /// Battery charge limit register.
    pub charge_limit: u8,
    /// Super battery field.
    pub super_battery: MaskedRegister,
    /// CPU sensors.
    pub cpu: SensorMap,
    /// GPU sensors.
    pub gpu: SensorMap,
    /// Webcam.
    pub webcam: WebcamMap,
    /// Fn ↔ Win key swap.
    pub fn_win_swap: EcBit,
}

impl EcConfig {
    /// `true` if `fw_version` (as read from msi-ec, surrounding whitespace ignored) is in
    /// [`EcConfig::allowed_firmware`]. Anything else must leave the daemon read-only (S8).
    #[must_use]
    pub fn supports_firmware(&self, fw_version: &str) -> bool {
        let fw = fw_version.trim();
        self.allowed_firmware.contains(&fw)
    }
}

/// msi-ec `CONF_G2_5`: MSI Stealth 14 AI Studio A1VGG / A1VFG.
pub const CONF_G2_5: EcConfig = EcConfig {
    name: "CONF_G2_5",
    allowed_firmware: &["14K2EMS1.104", "14K2EMS1.107", "14K2EMS1.108", "14K2EMS1.109"],
    shift_mode: ShiftModeMap {
        address: 0xd2,
        eco: 0xc2,
        comfort: 0xc1,
        turbo: 0xc4,
    },
    fan_mode: FanModeMap {
        address: 0xd4,
        auto: 0x0d,
        silent: 0x1d,
        advanced: 0x8d,
    },
    cooler_boost: EcBit {
        address: 0x98,
        bit: 7,
    },
    charge_limit: 0xd7,
    super_battery: MaskedRegister {
        address: 0xeb,
        mask: 0x0f,
    },
    cpu: SensorMap {
        temp: 0x68,
        fan_speed: 0x71,
    },
    gpu: SensorMap {
        temp: 0x80,
        fan_speed: 0x89,
    },
    webcam: WebcamMap {
        enable: EcBit {
            address: 0x2e,
            bit: 1,
        },
        block: EcBit {
            address: 0x2f,
            bit: 1,
        },
    },
    fn_win_swap: EcBit {
        address: 0xe8,
        bit: 4,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_g2_5_matches_the_hardware_doc() {
        let c = CONF_G2_5;
        assert_eq!(c.shift_mode.address, 0xd2);
        assert_eq!(c.shift_mode.value(ShiftMode::Turbo), 0xc4);
        assert_eq!(c.shift_mode.value(ShiftMode::Eco), 0xc2);
        assert_eq!(c.shift_mode.value(ShiftMode::Comfort), 0xc1);
        assert_eq!(c.fan_mode.address, 0xd4);
        assert_eq!(c.fan_mode.value(FanMode::Auto), 0x0d);
        assert_eq!(c.fan_mode.value(FanMode::Silent), 0x1d);
        assert_eq!(c.fan_mode.value(FanMode::Advanced), 0x8d);
        assert_eq!(c.cooler_boost.address, 0x98);
        assert_eq!(c.cooler_boost.mask(), 0x80);
        assert_eq!(c.charge_limit, 0xd7);
        assert_eq!(
            c.super_battery,
            MaskedRegister {
                address: 0xeb,
                mask: 0x0f
            }
        );
        assert_eq!(
            c.cpu,
            SensorMap {
                temp: 0x68,
                fan_speed: 0x71
            }
        );
        assert_eq!(
            c.gpu,
            SensorMap {
                temp: 0x80,
                fan_speed: 0x89
            }
        );
        assert_eq!(c.webcam.enable.mask(), 0x02);
        assert_eq!(c.fn_win_swap.mask(), 0x10);
    }

    #[test]
    fn mode_values_round_trip() {
        for m in [ShiftMode::Eco, ShiftMode::Comfort, ShiftMode::Turbo] {
            assert_eq!(CONF_G2_5.shift_mode.mode(CONF_G2_5.shift_mode.value(m)), Some(m));
        }
        for m in [FanMode::Auto, FanMode::Silent, FanMode::Advanced] {
            assert_eq!(CONF_G2_5.fan_mode.mode(CONF_G2_5.fan_mode.value(m)), Some(m));
        }
        assert_eq!(CONF_G2_5.shift_mode.mode(0x00), None);
        assert_eq!(CONF_G2_5.fan_mode.mode(0xff), None);
    }

    #[test]
    fn s8_only_known_firmware_is_supported() {
        assert!(CONF_G2_5.supports_firmware("14K2EMS1.109"));
        assert!(CONF_G2_5.supports_firmware("14K2EMS1.104\n"));
        assert!(!CONF_G2_5.supports_firmware("14K2EMS1.110"));
        assert!(!CONF_G2_5.supports_firmware("14K2EMS1.10"));
        assert!(!CONF_G2_5.supports_firmware(""));
    }

    #[test]
    fn sysfs_names() {
        assert_eq!(ShiftMode::Comfort.as_str(), "comfort");
        assert_eq!(FanMode::Auto.as_str(), "auto");
    }
}
