//! Hardware detection — a faithful port of DrvmgrNetInstaller.exe's `sub_4025C0`.
//!
//! The original:
//!   1. `SetupDiGetClassDevsW(NULL, NULL, NULL, DIGCF_PRESENT | DIGCF_ALLCLASSES)`
//!      — every device currently present on the machine, any class.
//!   2. For each device, read SPDRP_COMPATIBLEIDS (and SPDRP_HARDWAREID).
//!   3. Keep it only if a compatible/hardware ID equals `PCI\CC_0200`
//!      (Ethernet) or `PCI\CC_0280` (other network controller, incl. Wi-Fi) —
//!      the PCI base-class/sub-class for network controllers — AND the device
//!      reports a CM problem code (i.e. it has no working driver).
//!   4. Collect the hardware IDs to look up in drivers.dat.

use anyhow::{Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW, CM_DEVNODE_STATUS_FLAGS, CM_PROB,
    DIGCF_ALLCLASSES, DIGCF_PRESENT, SETUP_DI_REGISTRY_PROPERTY, SPDRP_COMPATIBLEIDS,
    SPDRP_DEVICEDESC, SPDRP_HARDWAREID, SP_DEVINFO_DATA,
};
use windows::Win32::Foundation::HWND;

const DN_HAS_PROBLEM: u32 = 0x0000_0400;

#[derive(Debug, Clone)]
pub struct NicDevice {
    pub description: String,
    pub hardware_ids: Vec<String>,
    pub compatible_ids: Vec<String>,
    pub status: u32,
    pub problem: u32,
}

impl NicDevice {
    /// True when Windows has no working driver bound (a CM problem is reported).
    pub fn needs_driver(&self) -> bool {
        self.problem != 0 || (self.status & DN_HAS_PROBLEM) != 0
    }

    /// Hardware IDs first (most specific), then compatible IDs — the order to
    /// probe drivers.dat with.
    pub fn match_ids(&self) -> Vec<String> {
        let mut v = self.hardware_ids.clone();
        v.extend(self.compatible_ids.iter().cloned());
        v
    }
}

fn is_network(ids: &[String]) -> bool {
    ids.iter().any(|s| {
        let u = s.to_ascii_uppercase();
        u.contains("CC_0200") || u.contains("CC_0280")
    })
}

pub fn enumerate_nics() -> Result<Vec<NicDevice>> {
    let mut out = Vec::new();
    unsafe {
        let hdev = SetupDiGetClassDevsW(
            None,
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT | DIGCF_ALLCLASSES,
        )
        .context("SetupDiGetClassDevsW failed")?;

        let mut index = 0u32;
        loop {
            let mut did = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(hdev, index, &mut did).is_err() {
                break; // ERROR_NO_MORE_ITEMS
            }
            index += 1;

            let hwids = get_property(hdev, &did, SPDRP_HARDWAREID)
                .map(|b| parse_multi_sz(&b))
                .unwrap_or_default();
            let cids = get_property(hdev, &did, SPDRP_COMPATIBLEIDS)
                .map(|b| parse_multi_sz(&b))
                .unwrap_or_default();

            if !is_network(&hwids) && !is_network(&cids) {
                continue;
            }

            let description = get_property(hdev, &did, SPDRP_DEVICEDESC)
                .map(|b| parse_multi_sz(&b))
                .and_then(|v| v.into_iter().next())
                .unwrap_or_default();

            let mut status = CM_DEVNODE_STATUS_FLAGS(0);
            let mut problem = CM_PROB(0);
            let _ = CM_Get_DevNode_Status(&mut status, &mut problem, did.DevInst, 0);

            out.push(NicDevice {
                description,
                hardware_ids: hwids,
                compatible_ids: cids,
                status: status.0,
                problem: problem.0,
            });
        }
        let _ = SetupDiDestroyDeviceInfoList(hdev);
    }
    Ok(out)
}

/// Two-call SetupDiGetDeviceRegistryPropertyW: size probe, then read.
unsafe fn get_property(
    hdev: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    did: &SP_DEVINFO_DATA,
    prop: SETUP_DI_REGISTRY_PROPERTY,
) -> Option<Vec<u8>> {
    let mut required: u32 = 0;
    // First call with an empty buffer just to learn the size.
    let _ = SetupDiGetDeviceRegistryPropertyW(hdev, did, prop, None, None, Some(&mut required));
    if required == 0 {
        return None;
    }
    let mut buf = vec![0u8; required as usize];
    match SetupDiGetDeviceRegistryPropertyW(hdev, did, prop, None, Some(&mut buf), Some(&mut required))
    {
        Ok(()) => Some(buf),
        Err(_) => None,
    }
}

/// Decode a REG_MULTI_SZ / REG_SZ byte buffer (UTF-16LE) into strings.
fn parse_multi_sz(buf: &[u8]) -> Vec<String> {
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    units
        .split(|&c| c == 0)
        .filter(|p| !p.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}
