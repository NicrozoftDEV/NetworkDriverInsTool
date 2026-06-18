//! Host facts that mirror what DrvmgrNetInstaller.exe collects before querying
//! drivers.dat: the OS version tag `[major.minor]` and the platform tag
//! `[x32]`/`[x64]`. The original used GetVersionEx + a bitness flag; we use
//! RtlGetVersion (true version, not shimmed) and the *native* processor arch.

use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::System::SystemInformation::{
    GetNativeSystemInfo, OSVERSIONINFOW, PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM64,
    SYSTEM_INFO,
};
use windows::Win32::UI::Shell::IsUserAnAdmin;

/// e.g. "[10.0]" on Win10/11, "[6.1]" on Win7. Matches the `OS` column of
/// t_hidandpkg (which stores one or more such tags concatenated).
pub fn os_tag() -> String {
    let mut osvi = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = RtlGetVersion(&mut osvi);
    }
    format!("[{}.{}]", osvi.dwMajorVersion, osvi.dwMinorVersion)
}

/// "[x64]" or "[x32]", based on the native OS architecture (the kernel decides
/// which driver bitness loads, not the process bitness).
pub fn pf_tag() -> &'static str {
    let mut si = SYSTEM_INFO::default();
    unsafe { GetNativeSystemInfo(&mut si) };
    let arch = unsafe { si.Anonymous.Anonymous.wProcessorArchitecture };
    if arch == PROCESSOR_ARCHITECTURE_AMD64 || arch == PROCESSOR_ARCHITECTURE_ARM64 {
        "[x64]"
    } else {
        "[x32]"
    }
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}
