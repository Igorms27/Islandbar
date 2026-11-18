//! Leitura real de baterias sem fio no Windows.
//!
//! Fontes consultadas (sem precisar de administrador nem instalar drivers):
//! 1. **8BitDo** (`VID 0x2DC8`): canal de telemetria `FF7A`/`FFA0` dos dongles
//!    2.4 GHz (ex.: Ultimate 2 Wireless) + relatórios D-Input / Bluetooth.
//! 2. **PlayStation** (`VID 0x054C`): DualShock 4, DualSense e DualSense Edge.
//! 3. **Logitech HID++ 2.0 / 1.0** (`VID 0x046D`): receptores Unifying, Bolt e
//!    Lightspeed (mouses, teclados e headsets G).
//! 4. **Razer** (`VID 0x1532`): relatórios de 91 bytes (mouses, teclados e fones).
//! 5. **HyperX** (`VID 0x03F0` / `0x0951`): Cloud II / III / Alpha e Pulsefire Haste.
//! 6. **Compx** (`VID 0x3554, 0x373B, 0x3770, 0x3710, 0x33E4, 0x35BB`): ATK, VXE,
//!    Kysona, Pulsar, Redragon e G-Wolves.
//! 7. **Rongyuan** (lista `VID:PID`): receptores 2.4 GHz da Attack Shark, Ajazz e MSI.
//! 8. **SteelSeries** (`VID 0x1038`): headsets Arctis / Nova e mouses Aerox / Rival.
//! 9. **Corsair** (`VID 0x1B1C`): receptores Slipstream / Bragi e Compx.
//! 10. **HID Padrão**: qualquer receptor/periférico que exponha `Battery Strength`
//!     (`0x06:0x20`) ou `Absolute State of Charge` (`0x85:0x65`) — exceto mesas
//!     digitalizadoras (`0x0D`), que nunca são tocadas.
//! 11. **Bluetooth do Windows**: propriedade `DEVPKEY` de bateria (HFP e BLE GATT).
//! 12. **XInput**: controles e headsets sem fio de Xbox.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{GUID, HRESULT, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceInterfaceDetailW, SetupDiGetDevicePropertyW,
    CM_DEVNODE_STATUS_FLAGS, CM_PROB, CR_SUCCESS, DIGCF_ALLCLASSES, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    DN_HAS_PROBLEM, HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FlushQueue, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid, HidD_GetInputReport,
    HidD_GetPreparsedData, HidD_GetProductString, HidD_SetFeature, HidP_Feature, HidP_GetCaps, HidP_GetUsageValue,
    HidP_GetValueCaps, HidP_Input, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS,
    PHIDP_PREPARSED_DATA,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_DeviceContainer_Category, DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_ContainerId,
    DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPROPTYPE, DEVPROP_BOOLEAN, DEVPROP_TYPE_BOOLEAN,
    DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING, DEVPROP_TYPE_UINT32,
};
use windows::Win32::Foundation::{
    CloseHandle, DEVPROPKEY, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};
use windows::Win32::UI::Input::XboxController::{
    XInputGetBatteryInformation, XInputGetState, BATTERY_DEVTYPE_GAMEPAD, BATTERY_DEVTYPE_HEADSET, BATTERY_LEVEL_EMPTY,
    BATTERY_LEVEL_FULL, BATTERY_LEVEL_LOW, BATTERY_LEVEL_MEDIUM, BATTERY_TYPE_DISCONNECTED, BATTERY_TYPE_WIRED,
    XINPUT_BATTERY_INFORMATION, XINPUT_STATE,
};

use super::{
    compx_checksum, debug_enabled, guess_kind_from_name, li_ion_percent, strip_bt_service_suffix,
    strip_receiver_suffix, Kind, Peripheral, PollOutput, RawReading,
};

// Propriedades PnP de dispositivos Bluetooth no Windows.
const PKEY_BT_BATTERY: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5), pid: 2 };
const PKEY_IS_CONNECTED: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29), pid: 15 };
const PKEY_BT_COD: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a), pid: 10 };

pub(crate) fn poll() -> PollOutput {
    let colls = unsafe { enumerate_hid() };
    let mut ctx = PollCtx::new(colls);

    if debug_enabled() {
        for c in &ctx.colls {
            if c.vid != 0 {
                crate::perf::line(format!(
                    "battery: hid {:04x}:{:04x} page={:04x}:{:04x} mi={} in={} out={} feat={} name={:?}",
                    c.vid,
                    c.pid,
                    c.caps.UsagePage,
                    c.caps.Usage,
                    c.iface,
                    c.in_len(),
                    c.out_len(),
                    c.feat_len(),
                    c.name
                ));
            }
        }
    }

    poll_8bitdo(&mut ctx);
    poll_playstation(&mut ctx);
    poll_logitech(&mut ctx);
    poll_razer(&mut ctx);
    poll_hyperx(&mut ctx);
    poll_compx(&mut ctx);
    poll_rongyuan(&mut ctx);
    poll_steelseries(&mut ctx);
    poll_corsair(&mut ctx);
    poll_bluetooth(&mut ctx);
    poll_generic_hid(&mut ctx);
    poll_xinput(&mut ctx);

    ctx.out
}

// ---------------------------------------------------------------------------
// Infraestrutura HID e SetupAPI
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HidColl {
    path_wide: Vec<u16>,
    path_lower: String,
    container: Option<GUID>,
    vid: u16,
    pid: u16,
    iface: i32,
    caps: HIDP_CAPS,
    name: String,
}

impl HidColl {
    fn page(&self) -> u16 {
        self.caps.UsagePage
    }
    fn usage(&self) -> u16 {
        self.caps.Usage
    }
    fn in_len(&self) -> usize {
        self.caps.InputReportByteLength as usize
    }
    fn out_len(&self) -> usize {
        self.caps.OutputReportByteLength as usize
    }
    fn feat_len(&self) -> usize {
        self.caps.FeatureReportByteLength as usize
    }
    fn tag(&self) -> u32 {
        ((self.vid as u32) << 16) | (self.pid as u32)
    }
}

struct PollCtx {
    colls: Vec<HidColl>,
    claimed: HashSet<u32>,
    bt_containers: HashSet<GUID>,
    out: PollOutput,
}

impl PollCtx {
    fn new(colls: Vec<HidColl>) -> Self {
        let mut out = PollOutput::default();
        for c in &colls {
            if c.vid != 0 {
                out.present_tags.insert(c.tag());
                // A base do 8BitDo Ultimate 2 muda o dongle de 310B para 6013 ao encaixar.
                if c.vid == 0x2DC8 && c.pid == 0x6013 {
                    out.present_tags.insert((0x2DC8u32 << 16) | 0x310B);
                }
            }
        }
        Self { colls, claimed: HashSet::new(), bt_containers: HashSet::new(), out }
    }

    fn products(&self, vid: u16) -> Vec<(u16, u16)> {
        let mut list = Vec::new();
        for c in &self.colls {
            if c.vid == vid && !self.claimed.contains(&c.tag()) && !list.contains(&(c.vid, c.pid)) {
                list.push((c.vid, c.pid));
            }
        }
        list
    }

    fn claim(&mut self, vid: u16, pid: u16) {
        self.claimed.insert(((vid as u32) << 16) | (pid as u32));
    }

    fn is_claimed(&self, vid: u16, pid: u16) -> bool {
        self.claimed.contains(&(((vid as u32) << 16) | (pid as u32)))
    }

    fn add(&mut self, peripheral: Peripheral, receiver: Option<(u16, u16)>) {
        let receiver_tag = receiver.map(|(v, p)| ((v as u32) << 16) | (p as u32));
        if let Some((v, p)) = receiver {
            self.claim(v, p);
        }
        self.out.readings.push(RawReading { peripheral, receiver_tag, asleep: false });
    }

    fn add_asleep(&mut self, id: String, vid: u16, pid: u16) {
        self.claim(vid, pid);
        self.out.readings.push(RawReading {
            peripheral: Peripheral { id, name: String::new(), kind: Kind::Mouse, percent: 0, charging: false },
            receiver_tag: Some(((vid as u32) << 16) | (pid as u32)),
            asleep: true,
        });
    }

    fn kind_from_collections(&self, vid: u16, pid: u16) -> Option<Kind> {
        let mut saw_mouse = false;
        let mut saw_kbd = false;
        let mut saw_pad = false;
        let mut saw_headset = false;
        for c in &self.colls {
            if c.vid != vid || c.pid != pid {
                continue;
            }
            match (c.page(), c.usage()) {
                (0x01, 0x02) => saw_mouse = true,
                (0x01, 0x06) => saw_kbd = true,
                (0x01, 0x04 | 0x05) => saw_pad = true,
                (0x0B, _) => saw_headset = true,
                _ => {}
            }
        }
        // Muitos mouses gamer também expõem uma coleção de teclado para macros;
        // se houver headset de telefonia vence headset, depois controle, depois mouse.
        if saw_headset {
            Some(Kind::Headset)
        } else if saw_pad {
            Some(Kind::Gamepad)
        } else if saw_mouse {
            Some(Kind::Mouse)
        } else if saw_kbd {
            Some(Kind::Keyboard)
        } else {
            None
        }
    }
}

struct DevInfoGuard(HDEVINFO);
impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
}

struct HidDev {
    handle: HANDLE,
    event: Option<HANDLE>,
    in_len: usize,
    out_len: usize,
    feat_len: usize,
    overlapped: bool,
}

impl Drop for HidDev {
    fn drop(&mut self) {
        unsafe {
            if let Some(ev) = self.event {
                let _ = CloseHandle(ev);
            }
            if !self.handle.is_invalid() {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

impl HidDev {
    fn open_rw(c: &HidColl) -> Option<Self> {
        Self::open_with(c, GENERIC_READ.0 | GENERIC_WRITE.0, true)
    }

    fn open_ro(c: &HidColl) -> Option<Self> {
        Self::open_with(c, GENERIC_READ.0, true)
    }

    fn open_feature(c: &HidColl) -> Option<Self> {
        Self::open_with(c, 0, false)
    }

    fn open_with(c: &HidColl, access: u32, overlapped: bool) -> Option<Self> {
        let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { FILE_FLAGS_AND_ATTRIBUTES(0) };
        let handle = unsafe {
            CreateFileW(
                PCWSTR(c.path_wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )
        }
        .ok()?;
        let event = if overlapped {
            match unsafe { CreateEventW(None, true, false, None) } {
                Ok(ev) => Some(ev),
                Err(_) => {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    return None;
                }
            }
        } else {
            None
        };
        Some(Self { handle, event, in_len: c.in_len(), out_len: c.out_len(), feat_len: c.feat_len(), overlapped })
    }

    fn flush(&self) {
        unsafe {
            let _ = HidD_FlushQueue(self.handle);
        }
    }

    fn write(&self, data: &[u8], timeout_ms: u32) -> bool {
        if !self.overlapped || self.out_len == 0 || data.is_empty() {
            return false;
        }
        let Some(ev) = self.event else { return false };
        let mut buf = vec![0u8; self.out_len];
        let copy = buf.len().min(data.len());
        buf[..copy].copy_from_slice(&data[..copy]);

        let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
        let mut transferred = 0u32;
        let pending_hr = HRESULT::from_win32(ERROR_IO_PENDING.0);

        unsafe {
            let res = WriteFile(self.handle, Some(&buf), None, Some(&mut ov));
            if res.is_err() && res.as_ref().err().map(|e| e.code()) != Some(pending_hr) {
                return false;
            }
            if WaitForSingleObject(ev, timeout_ms) == WAIT_OBJECT_0 {
                GetOverlappedResult(self.handle, &ov, &mut transferred, false).is_ok()
            } else {
                // Cancela e espera o driver soltar `ov` na pilha antes de retornar.
                let _ = CancelIo(self.handle);
                let _ = GetOverlappedResult(self.handle, &ov, &mut transferred, true);
                false
            }
        }
    }

    fn read(&self, timeout_ms: u32) -> Option<Vec<u8>> {
        if !self.overlapped || self.in_len == 0 {
            return None;
        }
        let ev = self.event?;
        let mut buf = vec![0u8; self.in_len];
        let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
        let mut transferred = 0u32;
        let pending_hr = HRESULT::from_win32(ERROR_IO_PENDING.0);

        unsafe {
            let res = ReadFile(self.handle, Some(&mut buf), None, Some(&mut ov));
            if res.is_err() && res.as_ref().err().map(|e| e.code()) != Some(pending_hr) {
                return None;
            }
            let ok = if WaitForSingleObject(ev, timeout_ms) == WAIT_OBJECT_0 {
                GetOverlappedResult(self.handle, &ov, &mut transferred, false).is_ok()
            } else {
                let _ = CancelIo(self.handle);
                let _ = GetOverlappedResult(self.handle, &ov, &mut transferred, true);
                false
            };
            if ok && transferred > 0 {
                buf.truncate(transferred as usize);
                Some(buf)
            } else {
                None
            }
        }
    }

    fn get_feature(&self, report_id: u8) -> Option<Vec<u8>> {
        if self.feat_len == 0 {
            return None;
        }
        let mut buf = vec![0u8; self.feat_len];
        buf[0] = report_id;
        let ok = unsafe { HidD_GetFeature(self.handle, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
        ok.then_some(buf)
    }

    fn set_feature(&self, data: &[u8]) -> bool {
        if self.feat_len == 0 || data.is_empty() {
            return false;
        }
        let mut buf = vec![0u8; self.feat_len];
        let copy = buf.len().min(data.len());
        buf[..copy].copy_from_slice(&data[..copy]);
        unsafe { HidD_SetFeature(self.handle, buf.as_ptr() as *const c_void, buf.len() as u32) }
    }
}

unsafe fn enumerate_hid() -> Vec<HidColl> {
    let mut out = Vec::new();
    let guid = HidD_GetHidGuid();
    let Ok(di) = SetupDiGetClassDevsW(Some(&guid), None, None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
        return out;
    };
    let _guard = DevInfoGuard(di);

    let mut idx = 0u32;
    loop {
        let mut ifd = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if SetupDiEnumDeviceInterfaces(di, None, &guid, idx, &mut ifd).is_err() {
            break;
        }
        idx += 1;

        let mut need = 0u32;
        let _ = SetupDiGetDeviceInterfaceDetailW(di, &ifd, None, 0, Some(&mut need), None);
        if need < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
            continue;
        }

        // Alinhado em u64 para `SP_DEVICE_INTERFACE_DETAIL_DATA_W` em x86_64.
        let words = (need as usize).div_ceil(8);
        let mut backing = vec![0u64; words];
        let detail = backing.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        let mut dev_info =
            SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };

        if SetupDiGetDeviceInterfaceDetailW(di, &ifd, Some(detail), need, None, Some(&mut dev_info)).is_err() {
            continue;
        }

        let path_ptr = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
        let max_u16 = (need as usize - std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath)) / 2;
        let raw_slice = std::slice::from_raw_parts(path_ptr, max_u16);
        let nul = raw_slice.iter().position(|&c| c == 0).unwrap_or(raw_slice.len());
        let mut path_wide = raw_slice[..nul].to_vec();
        path_wide.push(0);
        let path_lower = String::from_utf16_lossy(&path_wide[..nul]).to_lowercase();

        let Ok(h) = CreateFileW(
            PCWSTR(path_wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) else {
            continue;
        };

        let mut attr = HIDD_ATTRIBUTES { Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
        let _ = HidD_GetAttributes(h, &mut attr);

        let mut caps = HIDP_CAPS::default();
        let mut pp = PHIDP_PREPARSED_DATA::default();
        if HidD_GetPreparsedData(h, &mut pp) {
            let _ = HidP_GetCaps(pp, &mut caps);
            let _ = HidD_FreePreparsedData(pp);
        }

        // Prefere o nome em cache do Windows para não acordar dispositivos em repouso.
        let mut name = prop_string(di, &dev_info, &DEVPKEY_Device_BusReportedDeviceDesc);
        if name.is_empty() {
            name = prop_string(di, &dev_info, &DEVPKEY_Device_FriendlyName);
        }
        if name.is_empty() || name.eq_ignore_ascii_case("usb input device") || name.contains("HID-compliant") {
            let mut prod = [0u16; 128];
            if HidD_GetProductString(h, prod.as_mut_ptr() as *mut c_void, (prod.len() * 2) as u32) {
                let end = prod.iter().position(|&c| c == 0).unwrap_or(prod.len());
                let s = String::from_utf16_lossy(&prod[..end]).trim().to_string();
                if !s.is_empty() {
                    name = s;
                }
            }
        }
        let _ = CloseHandle(h);

        let container = prop_guid(di, &dev_info, &DEVPKEY_Device_ContainerId);
        let iface = parse_interface(&path_lower);

        out.push(HidColl {
            path_wide,
            path_lower,
            container,
            vid: attr.VendorID,
            pid: attr.ProductID,
            iface,
            caps,
            name,
        });
    }
    out
}

fn parse_interface(path_lower: &str) -> i32 {
    let Some(pos) = path_lower.find("&mi_") else { return -1 };
    let hex = &path_lower[pos + 4..];
    let digits: String = hex.chars().take(2).take_while(|c| c.is_ascii_hexdigit()).collect();
    i32::from_str_radix(&digits, 16).unwrap_or(-1)
}

unsafe fn prop_string(di: HDEVINFO, dd: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> String {
    let mut ty = DEVPROPTYPE::default();
    let mut buf = [0u8; 512];
    let mut req = 0u32;
    if SetupDiGetDevicePropertyW(di, dd, key, &mut ty, Some(&mut buf), Some(&mut req), 0).is_err() {
        return String::new();
    }
    if ty != DEVPROP_TYPE_STRING && ty.0 != 0x00001012 {
        return String::new();
    }
    let u16_len = (req as usize / 2).min(buf.len() / 2);
    let wide = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
    let nul = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..nul]).trim().to_string()
}

unsafe fn prop_guid(di: HDEVINFO, dd: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<GUID> {
    let mut ty = DEVPROPTYPE::default();
    let mut guid = GUID::zeroed();
    let slice = std::slice::from_raw_parts_mut(&mut guid as *mut GUID as *mut u8, std::mem::size_of::<GUID>());
    SetupDiGetDevicePropertyW(di, dd, key, &mut ty, Some(slice), None, 0).ok()?;
    (ty == DEVPROP_TYPE_GUID).then_some(guid)
}

// ---------------------------------------------------------------------------
// 1. 8BitDo (VID 0x2DC8) — Dongles 2.4 GHz (telemetria FF7A/FFA0) + D-Input
// ---------------------------------------------------------------------------

fn eightbitdo_model(pid: u16) -> Option<(&'static str, bool)> {
    Some(match pid {
        0x310B | 0x6013 => ("8BitDo Ultimate 2 Wireless", false),
        0x6012 => ("8BitDo Ultimate 2", false),
        0x202F | 0x202E => ("8BitDo Ultimate 3-mode", false),
        0x301B | 0x301A | 0x310A => ("8BitDo Ultimate 2C", false),
        0x3012 | 0x3013 | 0x3106 | 0x3109 => ("8BitDo Ultimate 2.4G", false),
        0x6007 => ("8BitDo Ultimate", false),
        0x6009 => ("8BitDo Pro 3", true),
        0x6003 | 0x6006 => ("8BitDo Pro 2", true),
        0x6001 | 0x6101 => ("8BitDo SN30 Pro", true),
        0x6000 | 0x6100 => ("8BitDo SF30 Pro", true),
        _ => return None,
    })
}

fn eightbitdo_name(c: &HidColl) -> String {
    if let Some((known, _)) = eightbitdo_model(c.pid) {
        return known.to_string();
    }
    let clean = strip_receiver_suffix(&c.name);
    if !clean.is_empty() && !clean.contains("HID") && !clean.eq_ignore_ascii_case("usb input device") {
        if clean.to_lowercase().starts_with("8bitdo") {
            return clean;
        }
        return format!("8BitDo {clean}");
    }
    "Controle 8BitDo".to_string()
}

fn eightbitdo_vendor_query(dev: &HidDev, req: &[u8], cmd_lo: u8, cmd_hi: u8) -> Option<Vec<u8>> {
    dev.flush();
    if !dev.write(req, 250) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let ms = left.as_millis().min(300) as u32;
        if ms == 0 {
            break;
        }
        let Some(r) = dev.read(ms) else { break };
        if r.len() >= 19 && r[0] == 0x02 && r[1] == 0x04 && r[4] == cmd_lo && r[5] == cmd_hi {
            return Some(r);
        }
    }
    None
}

fn poll_8bitdo(ctx: &mut PollCtx) {
    const VID: u16 = 0x2DC8;
    for (_, pid) in ctx.products(VID) {
        let is_ult2 = matches!(pid, 0x310B | 0x6012 | 0x6013);
        let id = if is_ult2 { "8bitdo:ultimate2".to_string() } else { format!("8bitdo:{pid:04x}") };
        let tag_pid = if is_ult2 { 0x310B } else { pid };

        // 1) Canal de telemetria proprietário (dongle 2.4 GHz em modo XInput).
        let vendor_colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| {
                c.vid == VID
                    && c.pid == pid
                    && (c.page() == 0xFF7A || c.page() == 0xFFA0)
                    && c.usage() == 0x0001
                    && c.in_len() >= 64
                    && c.out_len() >= 64
            })
            .cloned()
            .collect();

        let mut handled = false;
        for c in &vendor_colls {
            let Some(dev) = HidDev::open_rw(c) else { continue };
            let linked = eightbitdo_vendor_query(&dev, &[0x81, 0x04, 0x20, 0x01], 0x20, 0x01).map(|r| r[18] != 0);
            if linked == Some(false) {
                // Controle desligado ou na base: se estiver na base (`6013`/`3109`), mantém repouso.
                if matches!(pid, 0x6013 | 0x3109) {
                    ctx.claim(VID, pid);
                    ctx.add_asleep(id.clone(), VID, tag_pid);
                } else {
                    ctx.claim(VID, pid);
                }
                handled = true;
                break;
            }
            if let Some(r) = eightbitdo_vendor_query(&dev, &[0x81, 0x04, 0x21, 0x01, 0x00, 0x00, 0x06], 0x21, 0x01) {
                let pct = r[18] & 0x7F;
                if (1..=100).contains(&pct) {
                    let charging = (r[18] & 0x80) != 0 && pct < 100;
                    ctx.claim(VID, pid);
                    ctx.add(
                        Peripheral {
                            id: id.clone(),
                            name: eightbitdo_name(c),
                            kind: Kind::Gamepad,
                            percent: pct,
                            charging,
                        },
                        Some((VID, tag_pid)),
                    );
                    handled = true;
                    break;
                }
            }
        }
        if handled {
            continue;
        }

        // 2) Relatórios D-Input / Bluetooth (byte 14: bits 0..6 = %, bit 7 = carregando).
        let dinput_colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| {
                c.vid == VID && c.pid == pid && c.page() == 0x01 && matches!(c.usage(), 0x04 | 0x05) && c.in_len() >= 15
            })
            .cloned()
            .collect();

        for c in &dinput_colls {
            let needs_feat06 = eightbitdo_model(pid).is_some_and(|(_, f)| f);
            let dev = if needs_feat06 && c.feat_len() > 0 {
                let d = HidDev::open_rw(c);
                if let Some(ref d) = d {
                    let _ = d.get_feature(0x06);
                }
                d.or_else(|| HidDev::open_ro(c))
            } else {
                HidDev::open_ro(c)
            };
            let Some(dev) = dev else { continue };
            let min_len = if pid == 0x6012 { 34 } else { 15 };
            let deadline = Instant::now() + Duration::from_millis(450);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                let ms = left.as_millis().min(100) as u32;
                if ms == 0 {
                    break;
                }
                let Some(r) = dev.read(ms) else { continue };
                if r.len() >= min_len && matches!(r[0], 0x01 | 0x04) {
                    let pct = r[14] & 0x7F;
                    if (1..=100).contains(&pct) {
                        let charging = (r[14] & 0x80) != 0 && pct < 100;
                        ctx.add(
                            Peripheral {
                                id: id.clone(),
                                name: eightbitdo_name(c),
                                kind: Kind::Gamepad,
                                percent: pct,
                                charging,
                            },
                            Some((VID, pid)),
                        );
                        break;
                    }
                }
            }
            if ctx.is_claimed(VID, pid) {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. PlayStation DualShock 4 / DualSense (VID 0x054C)
// ---------------------------------------------------------------------------

fn poll_playstation(ctx: &mut PollCtx) {
    let colls: Vec<HidColl> = ctx
        .colls
        .iter()
        .filter(|c| {
            c.vid == 0x054C
                && matches!(c.pid, 0x05C4 | 0x09CC | 0x0BA0 | 0x0CE6 | 0x0DF2)
                && c.page() == 0x01
                && c.usage() == 0x05
        })
        .cloned()
        .collect();

    for c in colls {
        let Some(dev) = HidDev::open_rw(&c).or_else(|| HidDev::open_ro(&c)) else { continue };
        let bt = c.in_len() > 64;
        if bt {
            let _ = dev.get_feature(0x05);
        }
        let is_ds4 = matches!(c.pid, 0x05C4 | 0x09CC | 0x0BA0);
        let deadline = Instant::now() + Duration::from_millis(400);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let ms = left.as_millis().min(100) as u32;
            if ms == 0 {
                break;
            }
            let Some(r) = dev.read(ms) else { continue };
            if is_ds4 {
                let at = match r.first().copied() {
                    Some(0x01) if !bt => 30,
                    Some(0x11) => 32,
                    _ => continue,
                };
                if r.len() <= at || (c.pid == 0x0BA0 && r[at] == 0) {
                    break;
                }
                let level = r[at] & 0x0F;
                let cable = (r[at] & 0x10) != 0;
                let pct = if cable {
                    if level >= 11 {
                        100
                    } else {
                        (level * 10).min(100)
                    }
                } else {
                    (level * 10 + 5).min(100)
                };
                ctx.add(
                    Peripheral {
                        id: format!("ps:{:04x}", c.pid),
                        name: "DualShock 4".into(),
                        kind: Kind::Gamepad,
                        percent: pct,
                        charging: cable && level < 11,
                    },
                    Some((c.vid, c.pid)),
                );
                break;
            } else {
                let at = match r.first().copied() {
                    Some(0x01) if !bt => 53,
                    Some(0x31) => 54,
                    _ => continue,
                };
                if r.len() <= at {
                    continue;
                }
                let level = r[at] & 0x0F;
                let state = r[at] >> 4;
                let (pct, charging) =
                    if state == 0x2 { (100, true) } else { ((level * 10 + 5).min(100), state == 0x1) };
                let name = if c.pid == 0x0DF2 { "DualSense Edge" } else { "DualSense" };
                ctx.add(
                    Peripheral {
                        id: format!("ps:{:04x}", c.pid),
                        name: name.into(),
                        kind: Kind::Gamepad,
                        percent: pct,
                        charging,
                    },
                    Some((c.vid, c.pid)),
                );
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Logitech HID++ 2.0 / 1.0 (VID 0x046D)
// ---------------------------------------------------------------------------

const LOGI_SW_ID: u8 = 0x0D;

struct Hidpp {
    short: Option<HidDev>,
    long: Option<HidDev>,
}

enum HidppReply {
    Ok(Vec<u8>),
    Err(u8),
    Timeout,
}

impl Hidpp {
    fn open(short_c: Option<&HidColl>, long_c: Option<&HidColl>) -> Option<Self> {
        let short = short_c.and_then(HidDev::open_rw);
        let long = long_c.and_then(HidDev::open_rw);
        if short.is_none() && long.is_none() {
            return None;
        }
        Some(Self { short, long })
    }

    fn request(&self, idx: u8, b2: u8, b3: u8, params: &[u8], force_short: bool, timeout_ms: u32) -> HidppReply {
        let use_long = self.long.is_some() && !force_short;
        let Some(writer) = (if use_long { self.long.as_ref() } else { self.short.as_ref() }) else {
            return HidppReply::Timeout;
        };
        let mut msg = vec![0u8; if use_long { 20 } else { 7 }];
        msg[0] = if use_long { 0x11 } else { 0x10 };
        msg[1] = idx;
        msg[2] = b2;
        msg[3] = b3;
        for (i, &v) in params.iter().enumerate() {
            if 4 + i < msg.len() {
                msg[4 + i] = v;
            }
        }
        if !writer.write(&msg, 200) {
            return HidppReply::Timeout;
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        while Instant::now() < deadline {
            for reader in [&self.long, &self.short].into_iter().flatten() {
                let Some(r) = reader.read(20) else { continue };
                if r.len() < 7 || (r[0] != 0x10 && r[0] != 0x11) || r[1] != idx {
                    continue;
                }
                if r[2] == b2 && r[3] == b3 {
                    return HidppReply::Ok(r[4..].to_vec());
                }
                if (r[2] == 0xFF || r[2] == 0x8F) && r[3] == b2 && r[4] == b3 {
                    return HidppReply::Err(r[5]);
                }
            }
        }
        HidppReply::Timeout
    }

    fn call(&self, idx: u8, feat_idx: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        match self.request(idx, feat_idx, (func << 4) | LOGI_SW_ID, params, false, 350) {
            HidppReply::Ok(p) => Some(p),
            _ => None,
        }
    }

    fn feature_index(&self, idx: u8, feat_id: u16) -> Option<u8> {
        let p = self.call(idx, 0x00, 0, &[(feat_id >> 8) as u8, (feat_id & 0xFF) as u8])?;
        p.first().copied().filter(|&f| f != 0)
    }
}

fn poll_logitech(ctx: &mut PollCtx) {
    for (_, pid) in ctx.products(0x046D) {
        let passthrough = pid == 0xC54F;
        let shorts: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| {
                c.vid == 0x046D
                    && c.pid == pid
                    && c.out_len() == 7
                    && ((c.page() == 0xFF00 && c.usage() == 0x0001)
                        || (passthrough && c.page() == 0xFF43 && c.usage() == 0x0301))
            })
            .cloned()
            .collect();
        let longs: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| {
                c.vid == 0x046D
                    && c.pid == pid
                    && c.out_len() == 20
                    && ((c.page() == 0xFF00 && c.usage() == 0x0002)
                        || (c.page() == 0xFF43 && (c.usage() == 0x0202 || (passthrough && c.usage() == 0x0302))))
            })
            .cloned()
            .collect();

        if shorts.is_empty() && longs.is_empty() {
            continue;
        }
        let Some(hpp) = Hidpp::open(shorts.first(), longs.first()) else { continue };
        let base_coll = longs.first().or(shorts.first()).unwrap();
        let prod_name = &base_coll.name;
        let is_receiver = prod_name.contains("Receiver");
        let is_bt = base_coll.path_lower.contains("{00001812-") || base_coll.path_lower.contains("{00001124-");

        let indices: &[u8] = if passthrough {
            &[1]
        } else if is_bt
            || (!is_receiver
                && matches!(
                    hpp.request(0xFF, 0x00, 0x10 | LOGI_SW_ID, &[0, 0, 0x5A], false, 300),
                    HidppReply::Ok(ref p) if p.len() >= 3 && p[0] >= 2
                ))
        {
            &[0xFF]
        } else {
            &[1, 2, 3, 4, 5, 6]
        };

        let mut any = false;
        for &idx in indices {
            let ping = hpp.request(idx, 0x00, 0x10 | LOGI_SW_ID, &[0, 0, 0x5A], false, 300);
            match ping {
                HidppReply::Ok(ref p) if p.len() >= 3 && p[0] >= 2 => {
                    let Some((pct, charging)) = logi_battery_20(&hpp, idx) else { continue };
                    let (dev_name, dev_kind) = logi_name_and_kind_20(&hpp, idx, prod_name, base_coll.page() == 0xFF43);
                    let id = logi_unit_id_20(&hpp, idx)
                        .map(|u| format!("logi:unit:{u}"))
                        .unwrap_or_else(|| format!("logi:{pid:04x}:{idx}"));
                    ctx.add(
                        Peripheral { id, name: dev_name, kind: dev_kind, percent: pct, charging },
                        Some((0x046D, pid)),
                    );
                    any = true;
                }
                HidppReply::Err(0x01) => {
                    // Dispositivo HID++ 1.0 (ex.: mouses/teclados Unifying mais antigos).
                    if let HidppReply::Ok(p) = hpp.request(idx, 0x81, 0x0D, &[], true, 250) {
                        if p.len() >= 3 && (1..=100).contains(&p[0]) {
                            let charging = (0x21..=0x26).contains(&p[2]);
                            let kind = ctx.kind_from_collections(0x046D, pid).unwrap_or(Kind::Mouse);
                            ctx.add(
                                Peripheral {
                                    id: format!("logi:{pid:04x}:{idx}"),
                                    name: "Logitech sem fio".into(),
                                    kind,
                                    percent: p[0],
                                    charging,
                                },
                                Some((0x046D, pid)),
                            );
                            any = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if any {
            ctx.claim(0x046D, pid);
        }
    }
}

fn logi_battery_20(hpp: &Hidpp, idx: u8) -> Option<(u8, bool)> {
    // 0x1004: Unified Battery
    if let Some(f) = hpp.feature_index(idx, 0x1004) {
        if let Some(p) = hpp.call(idx, f, 1, &[]) {
            if p.len() >= 3 {
                let (soc, level, st) = (p[0], p[1], p[2]);
                let pct = if (1..=100).contains(&soc) {
                    soc
                } else if level != 0 {
                    if level & 8 != 0 {
                        100
                    } else if level & 4 != 0 {
                        60
                    } else if level & 2 != 0 {
                        20
                    } else {
                        5
                    }
                } else {
                    return None;
                };
                return Some((pct, matches!(st, 1..=3)));
            }
        }
    }
    // 0x1000: Battery Status
    if let Some(f) = hpp.feature_index(idx, 0x1000) {
        if let Some(p) = hpp.call(idx, f, 0, &[]) {
            if p.len() >= 3 && p[0] <= 100 {
                let st = p[2];
                let pct = if p[0] == 0 && st == 3 { 100 } else { p[0] };
                if pct > 0 {
                    return Some((pct, (1..=4).contains(&st)));
                }
            }
        }
    }
    // 0x1001: Battery Voltage
    if let Some(f) = hpp.feature_index(idx, 0x1001) {
        if let Some(p) = hpp.call(idx, f, 0, &[]) {
            if p.len() >= 3 {
                let mv = u16::from_be_bytes([p[0], p[1]]);
                if mv > 0 {
                    return Some((li_ion_percent(mv), (p[2] & 0x80) != 0));
                }
            }
        }
    }
    // 0x1F20: ADC Measurement (headsets G)
    if let Some(f) = hpp.feature_index(idx, 0x1F20) {
        if let Some(p) = hpp.call(idx, f, 0, &[]) {
            if p.len() >= 3 {
                let mv = u16::from_be_bytes([p[0], p[1]]);
                if mv > 0 {
                    return Some((li_ion_percent(mv), p[2] == 0x03));
                }
            }
        }
    }
    None
}

fn logi_name_and_kind_20(hpp: &Hidpp, idx: u8, fallback: &str, audio_page: bool) -> (String, Kind) {
    let mut name = String::new();
    let mut kind = None;
    if let Some(f) = hpp.feature_index(idx, 0x0005) {
        if let Some(p) = hpp.call(idx, f, 0, &[]) {
            if let Some(&total) = p.first() {
                let total = total as usize;
                let mut bytes = Vec::new();
                while bytes.len() < total && bytes.len() < 64 {
                    let Some(chunk) = hpp.call(idx, f, 1, &[bytes.len() as u8]) else { break };
                    let before = bytes.len();
                    for &b in &chunk {
                        if bytes.len() >= total || b == 0 {
                            break;
                        }
                        bytes.push(b);
                    }
                    if bytes.len() == before {
                        break;
                    }
                }
                name = String::from_utf8_lossy(&bytes).trim().to_string();
            }
        }
        if let Some(p) = hpp.call(idx, f, 2, &[]) {
            kind = match p.first().copied() {
                Some(0x00) => Some(Kind::Keyboard),
                Some(0x03) => Some(Kind::Mouse),
                Some(0x08) => Some(Kind::Headset),
                Some(0x0B | 0x0C) => Some(Kind::Gamepad),
                _ => None,
            };
        }
    }
    if name.is_empty() {
        name = strip_receiver_suffix(fallback);
    }
    if name.is_empty() || name.contains("Receiver") {
        name = "Logitech sem fio".into();
    } else if !name.starts_with("Logitech") && !name.starts_with("G ") && !name.starts_with("MX") {
        name = format!("Logitech {name}");
    }
    let resolved_kind =
        kind.or_else(|| guess_kind_from_name(&name)).unwrap_or(if audio_page { Kind::Headset } else { Kind::Mouse });
    (name, resolved_kind)
}

fn logi_unit_id_20(hpp: &Hidpp, idx: u8) -> Option<String> {
    let f = hpp.feature_index(idx, 0x0003)?;
    let p = hpp.call(idx, f, 0, &[])?;
    if p.len() >= 5 && (p[1] | p[2] | p[3] | p[4]) != 0 {
        Some(format!("{:02x}{:02x}{:02x}{:02x}", p[1], p[2], p[3], p[4]))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 4. Razer (VID 0x1532) — Feature report de 91 bytes
// ---------------------------------------------------------------------------

enum RazerRes {
    Ok(Vec<u8>),
    Offline,
    NoReply,
}

fn razer_cmd(dev: &HidDev, tid: u8, cls: u8, cmd_id: u8, size: u8) -> RazerRes {
    let mut rep = [0u8; 91];
    rep[2] = tid;
    rep[6] = size;
    rep[7] = cls;
    rep[8] = cmd_id;
    let mut crc = 0u8;
    for &b in &rep[3..89] {
        crc ^= b;
    }
    rep[89] = crc;

    let mut send = true;
    let mut saw_offline = false;
    for attempt in 0..5 {
        if send && !dev.set_feature(&rep) {
            return RazerRes::NoReply;
        }
        std::thread::sleep(Duration::from_millis(if attempt == 0 { 15 } else { 30 }));
        send = true;
        let Some(resp) = dev.get_feature(0) else { continue };
        if resp.len() < 91 {
            continue;
        }
        let a = &resp[1..];
        let mut reply_crc = 0u8;
        for &b in &a[2..88] {
            reply_crc ^= b;
        }
        if a[1] != tid
            || a[2] != 0
            || a[3] != 0
            || a[4] != 0
            || a[5] != size
            || a[6] != cls
            || a[7] != cmd_id
            || a[88] != reply_crc
        {
            continue;
        }
        match a[0] {
            0x01 => send = false,
            0x02 => return RazerRes::Ok(a[8..88].to_vec()),
            0x04 => saw_offline = true,
            _ => {}
        }
    }
    if saw_offline {
        RazerRes::Offline
    } else {
        RazerRes::NoReply
    }
}

fn poll_razer(ctx: &mut PollCtx) {
    let mut seen_names: HashSet<String> = HashSet::new();
    for (_, pid) in ctx.products(0x1532) {
        let colls: Vec<HidColl> =
            ctx.colls.iter().filter(|c| c.vid == 0x1532 && c.pid == pid && c.feat_len() == 91).cloned().collect();
        for c in colls {
            let Some(dev) = HidDev::open_feature(&c) else { continue };
            let mut found = None;
            let mut offline = false;
            for tid in [0x1F, 0x3F, 0xFF, 0x08, 0x9F] {
                match razer_cmd(&dev, tid, 0x07, 0x80, 0x02) {
                    RazerRes::Ok(args) => {
                        found = Some((tid, args));
                        break;
                    }
                    RazerRes::Offline => offline = true,
                    RazerRes::NoReply => {}
                }
            }
            if let Some((tid, args)) = found {
                let raw = args.get(1).copied().unwrap_or(0) as u32;
                let pct = ((raw * 100 + 127) / 255).min(100) as u8;
                if pct == 0 {
                    continue;
                }
                let charging =
                    matches!(razer_cmd(&dev, tid, 0x07, 0x84, 0x02), RazerRes::Ok(ch) if ch.get(1) == Some(&1));
                let mut name = strip_receiver_suffix(&c.name);
                if name.is_empty() {
                    name = "Razer sem fio".into();
                }
                if !seen_names.insert(name.clone()) {
                    ctx.claim(0x1532, pid);
                    break;
                }
                let kind = guess_kind_from_name(&name)
                    .or_else(|| ctx.kind_from_collections(0x1532, pid))
                    .unwrap_or(Kind::Mouse);
                ctx.add(
                    Peripheral { id: format!("razer:{pid:04x}"), name, kind, percent: pct, charging },
                    Some((0x1532, pid)),
                );
                break;
            } else if offline {
                ctx.claim(0x1532, pid);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. HyperX (VID 0x03F0 / 0x0951) — Cloud II / III / Alpha
// ---------------------------------------------------------------------------

fn hyperx_query(dev: &HidDev, req: &[u8], timeout_ms: u32, check: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
    if !dev.write(req, 200) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let ms = left.as_millis().min(250) as u32;
        if ms == 0 {
            break;
        }
        let r = dev.read(ms)?;
        if check(&r) {
            return Some(r);
        }
    }
    None
}

fn poll_hyperx(ctx: &mut PollCtx) {
    // Cloud III / Cloud II Core Wireless (HP 0x03F0)
    for pid in [0x05B7u16, 0x0C9D, 0x069F, 0x0995] {
        if ctx.is_claimed(0x03F0, pid) {
            continue;
        }
        let colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| c.vid == 0x03F0 && c.pid == pid && c.out_len() >= 62 && c.in_len() >= 62)
            .cloned()
            .collect();
        for c in colls {
            let Some(dev) = HidDev::open_rw(&c) else { continue };
            let Some(conn) = hyperx_query(&dev, &[0x66, 0x82], 500, |r| r.len() >= 5 && r[0] == 0x66 && r[1] == 0x82)
            else {
                continue;
            };
            if conn[2] == 0 {
                ctx.claim(0x03F0, pid);
                break;
            }
            let charging = hyperx_query(&dev, &[0x66, 0x8A], 400, |r| r.len() >= 3 && r[0] == 0x66 && r[1] == 0x8A)
                .is_some_and(|r| r[2] == 1);
            if let Some(bat) = hyperx_query(&dev, &[0x66, 0x89], 400, |r| r.len() >= 5 && r[0] == 0x66 && r[1] == 0x89)
            {
                if (1..=100).contains(&bat[4]) {
                    let name = if matches!(pid, 0x069F | 0x0995) { "HyperX Cloud II Core" } else { "HyperX Cloud III" };
                    ctx.add(
                        Peripheral {
                            id: format!("hyperx:{pid:04x}"),
                            name: name.into(),
                            kind: Kind::Headset,
                            percent: bat[4],
                            charging,
                        },
                        Some((0x03F0, pid)),
                    );
                    break;
                }
            }
        }
    }

    // Cloud II Wireless (HP 0x03F0: 0696, 018B, 0D93)
    for pid in [0x0696u16, 0x018B, 0x0D93] {
        if ctx.is_claimed(0x03F0, pid) {
            continue;
        }
        let colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| c.vid == 0x03F0 && c.pid == pid && c.page() == 0xFF90 && c.usage() == 0x0303)
            .cloned()
            .collect();
        for c in colls {
            let Some(dev) = HidDev::open_rw(&c) else { continue };
            let Some(bat) = hyperx_query(&dev, &[0x06, 0xFF, 0xBB, 0x02], 500, |r| {
                r.len() >= 8 && r[0] == 0x06 && r[1] == 0xFF && r[2] == 0xBB && r[3] == 0x02
            }) else {
                continue;
            };
            if (1..=100).contains(&bat[7]) {
                let charging = hyperx_query(&dev, &[0x06, 0xFF, 0xBB, 0x03], 400, |r| {
                    r.len() >= 5 && r[0] == 0x06 && r[1] == 0xFF && r[2] == 0xBB && r[3] == 0x03
                })
                .is_some_and(|r| r[4] == 1);
                ctx.add(
                    Peripheral {
                        id: format!("hyperx:{pid:04x}"),
                        name: "HyperX Cloud II".into(),
                        kind: Kind::Headset,
                        percent: bat[7],
                        charging,
                    },
                    Some((0x03F0, pid)),
                );
                break;
            }
        }
    }

    // Cloud Alpha Wireless (HP 0x03F0: 098D)
    if !ctx.is_claimed(0x03F0, 0x098D) {
        let colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| c.vid == 0x03F0 && c.pid == 0x098D && c.page() >= 0xFF00 && c.out_len() >= 3)
            .cloned()
            .collect();
        for c in colls {
            let Some(dev) = HidDev::open_rw(&c) else { continue };
            let Some(st) =
                hyperx_query(&dev, &[0x21, 0xBB, 0x03], 500, |r| r.len() >= 4 && r[0] == 0x21 && r[2] == 0x03)
            else {
                continue;
            };
            if st[3] == 1 {
                ctx.claim(0x03F0, 0x098D);
                break;
            }
            let charging =
                hyperx_query(&dev, &[0x21, 0xBB, 0x0C], 400, |r| r.len() >= 4 && r[0] == 0x21 && r[2] == 0x0C)
                    .is_some_and(|r| r[3] == 1);
            if let Some(bat) =
                hyperx_query(&dev, &[0x21, 0xBB, 0x0B], 400, |r| r.len() >= 4 && r[0] == 0x21 && r[2] == 0x0B)
            {
                if (1..=100).contains(&bat[3]) {
                    ctx.add(
                        Peripheral {
                            id: "hyperx:098d".into(),
                            name: "HyperX Cloud Alpha".into(),
                            kind: Kind::Headset,
                            percent: bat[3],
                            charging,
                        },
                        Some((0x03F0, 0x098D)),
                    );
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Compx (ATK, VXE, Kysona, Pulsar, Redragon, G-Wolves) — Relatório 0x08 (17 B)
// ---------------------------------------------------------------------------

fn compx_query(dev: &HidDev, cmd: u8) -> Option<Vec<u8>> {
    let mut req = [0u8; 17];
    req[0] = 0x08;
    req[1] = cmd;
    req[16] = compx_checksum(&req);
    dev.flush();
    if !dev.write(&req, 200) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(350);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let ms = left.as_millis().min(200) as u32;
        if ms == 0 {
            break;
        }
        let r = dev.read(ms)?;
        if r.len() >= 17 && r[0] == 0x08 && r[1] == cmd && r[16] == compx_checksum(&r) {
            return Some(r);
        }
    }
    None
}

fn poll_compx(ctx: &mut PollCtx) {
    for vendor in [0x3554u16, 0x373B, 0x3770, 0x33E4, 0x3710, 0x35BB] {
        for (_, pid) in ctx.products(vendor) {
            // Headsets seriais ATK N9 Pro / Ultra: nunca mandar pacotes de mouse.
            if vendor == 0x373B && matches!(pid, 0x11CA | 0x11CB | 0x1203 | 0x1204) {
                continue;
            }
            let mut colls: Vec<HidColl> = ctx
                .colls
                .iter()
                .filter(|c| {
                    c.vid == vendor
                        && c.pid == pid
                        && ((c.out_len() == 17 && c.in_len() == 17)
                            || (c.page() >= 0xFF00 && c.out_len() >= 17 && c.in_len() >= 17))
                })
                .cloned()
                .collect();
            colls.sort_by_key(|c| !(c.page() == 0xFF02 && c.usage() == 0x0002));

            for c in colls {
                let Some(dev) = HidDev::open_rw(&c) else { continue };
                let id = format!("compx:{vendor:04x}:{pid:04x}");
                let online = compx_query(&dev, 0x03).map(|r| r[6] != 0);
                if online == Some(false) {
                    ctx.claim(vendor, pid);
                    break;
                }
                if let Some(bat) = compx_query(&dev, 0x04) {
                    if (1..=100).contains(&bat[6]) {
                        let name = compx_name(&c.name, vendor, pid);
                        let kind = ctx.kind_from_collections(vendor, pid).unwrap_or(Kind::Mouse);
                        ctx.add(
                            Peripheral { id, name, kind, percent: bat[6], charging: bat[7] != 0 },
                            Some((vendor, pid)),
                        );
                        break;
                    }
                } else if online == Some(true) {
                    ctx.add_asleep(id, vendor, pid);
                    break;
                }
            }
        }
    }
}

fn compx_name(raw: &str, vid: u16, pid: u16) -> String {
    if vid == 0x3554 && matches!(pid, 0xF55D | 0xF5D5 | 0xF55E) {
        return "Mouse Redragon".into();
    }
    if vid == 0x3554 && matches!(pid, 0xF58A | 0xF58C | 0xF58E | 0xF58F | 0xF505 | 0xF506) {
        return "VXE Dragonfly R1".into();
    }
    if vid == 0x3770 {
        return "Hitscan Hyperlight".into();
    }
    let clean = strip_receiver_suffix(raw);
    if clean.is_empty() || clean.eq_ignore_ascii_case("2.4g") {
        if vid == 0x373B {
            "Mouse ATK".into()
        } else {
            "Mouse sem fio".into()
        }
    } else {
        clean
    }
}

// ---------------------------------------------------------------------------
// 7. Rongyuan (Attack Shark, Ajazz, MSI) — Feature report 0xF7 (teclado + mouse)
// ---------------------------------------------------------------------------

const RONGYUAN_RECEIVERS: &[u32] = &[
    0x0DB0197E, 0x0DB08E9A, 0x145F0376, 0x145F038D, 0x14A5C045, 0x25AA2008, 0x25AA4008, 0x25AA4018, 0x31514011,
    0x31514014, 0x31514017, 0x31514021, 0x31514027, 0x3151402D, 0x31514038, 0x31515006, 0x31515007, 0x31515008,
    0x31515020, 0x31515026, 0x3151502B, 0x31515031, 0x31515037, 0x31515038, 0x3151503D, 0x3151503F, 0x31515040,
    0x31515042, 0x31515045, 0x3151504B, 0x3151504E, 0x31515050, 0x31515052, 0x31515055, 0x3151505A, 0x342DE3E0,
    0x342DE3E1, 0x342DE3E2, 0x342DE3E6, 0x342DE3E7, 0x342DE3E8, 0x342DE3F5, 0x342DE3F6, 0x342DE3F7, 0x342DE3FB,
    0x342DE3FC, 0x342DE3FD, 0x347AA302, 0x347AA305, 0x374AA205, 0x374AA217, 0x374AA220, 0x374AA223, 0x374AA226,
    0x374AA229, 0x374AA234, 0x374AA237, 0x374AA251, 0x374AA263, 0x374AA268, 0x374AA311, 0x374AA314, 0x374AA319,
    0x374AA322, 0x374AA329, 0x374AA355, 0x379A1804, 0x379A1807, 0x38A90002, 0x38EE0000, 0x38EE0002, 0x38EE0039,
    0x391D1A04, 0x391D1A08, 0x391D1A0E, 0x391D1A13, 0x391D1A25, 0x391D1A2B, 0x391D1A34, 0x391D2017, 0x391D2A05,
];

fn poll_rongyuan(ctx: &mut PollCtx) {
    let colls: Vec<HidColl> = ctx
        .colls
        .iter()
        .filter(|c| {
            RONGYUAN_RECEIVERS.contains(&c.tag())
                && !ctx.is_claimed(c.vid, c.pid)
                && c.iface == 2
                && c.page() == 0xFFFF
                && c.usage() == 0x0002
                && c.feat_len() >= 7
        })
        .cloned()
        .collect();

    for c in colls {
        if ctx.is_claimed(c.vid, c.pid) {
            continue;
        }
        let Some(dev) = HidDev::open_feature(&c) else { continue };
        let mut cmd = vec![0u8; c.feat_len()];
        cmd[1] = 0xF7;
        if !dev.set_feature(&cmd) {
            continue;
        }
        std::thread::sleep(Duration::from_millis(20));
        let Some(r) = dev.get_feature(0) else { continue };
        if r.len() < 7 || (r[2] | r[3] | r[4] | r[5] | r[6]) == 0 {
            continue;
        }
        let clean = strip_receiver_suffix(&c.name);
        let generic = clean.is_empty() || clean.contains("2.4G");
        for (is_kbd, pct_idx, off_idx) in [(true, 2usize, 4usize), (false, 3, 5)] {
            if r[off_idx] != 0 || !(1..=100).contains(&r[pct_idx]) {
                continue;
            }
            let name = if c.vid == 0x3151 && c.pid == 0x402D && !is_kbd {
                "Attack Shark R3".into()
            } else if generic {
                if is_kbd {
                    "Teclado sem fio".into()
                } else {
                    "Mouse sem fio".into()
                }
            } else {
                clean.clone()
            };
            ctx.add(
                Peripheral {
                    id: format!("rongyuan:{:04x}:{:04x}:{}", c.vid, c.pid, if is_kbd { "kb" } else { "ms" }),
                    name,
                    kind: if is_kbd { Kind::Keyboard } else { Kind::Mouse },
                    percent: r[pct_idx],
                    charging: false,
                },
                Some((c.vid, c.pid)),
            );
        }
        ctx.claim(c.vid, c.pid);
    }
}

// ---------------------------------------------------------------------------
// 8. SteelSeries (VID 0x1038) — Arctis / Nova + Aerox / Rival
// ---------------------------------------------------------------------------

fn poll_steelseries(ctx: &mut PollCtx) {
    const NOVA_PIDS: &[u16] = &[
        0x220E, 0x2212, 0x2216, 0x2236, 0x2269, 0x226D, 0x2232, 0x2253, 0x230A, 0x2202, 0x22A1, 0x227E, 0x2206, 0x2258,
        0x229E, 0x22AD, 0x223A, 0x22A9, 0x227A, 0x22A4, 0x22A5, 0x220A, 0x22A7,
    ];
    for (_, pid) in ctx.products(0x1038) {
        // Família Arctis Nova (FFC0:0001)
        if NOVA_PIDS.contains(&pid) {
            let colls: Vec<HidColl> = ctx
                .colls
                .iter()
                .filter(|c| c.vid == 0x1038 && c.pid == pid && c.page() == 0xFFC0 && c.usage() == 0x0001)
                .cloned()
                .collect();
            for c in colls {
                let Some(dev) = HidDev::open_rw(&c) else { continue };
                dev.flush();
                if !dev.write(&[0x00, 0xB0], 200) {
                    continue;
                }
                let Some(r) = dev.read(300) else { continue };
                if r.len() < 7 {
                    continue;
                }
                let (connected, pct, charging, name) = if matches!(pid, 0x220E | 0x2212 | 0x2216 | 0x2236) {
                    (r[1] != 0x01, (r[2].min(4) * 25), r[3] == 0x01, "SteelSeries Arctis 7+")
                } else if matches!(pid, 0x2269 | 0x226D | 0x2232 | 0x2253) {
                    let n = if matches!(pid, 0x2232 | 0x2253) {
                        "SteelSeries Arctis Nova 5"
                    } else {
                        "SteelSeries Arctis Nova 3P"
                    };
                    (r[1] != 0x02, r[3].min(100), r[4] == 0x01, n)
                } else {
                    let discrete = matches!(pid, 0x2202 | 0x2206 | 0x220A | 0x223A | 0x227A | 0x22A4);
                    let p = if discrete { r[2].min(4) * 25 } else { r[2].min(100) };
                    (r[3] != 0x00, p, matches!(r[3], 0x01 | 0x02), "SteelSeries Arctis Nova 7")
                };
                if connected && pct > 0 {
                    ctx.add(
                        Peripheral {
                            id: format!("ss:{pid:04x}"),
                            name: name.into(),
                            kind: Kind::Headset,
                            percent: pct,
                            charging,
                        },
                        Some((0x1038, pid)),
                    );
                } else {
                    ctx.claim(0x1038, pid);
                }
                break;
            }
            continue;
        }

        // Aerox 3/5/9 & Prime Wireless (iface 3)
        if matches!(pid, 0x1838 | 0x183A | 0x1852 | 0x1854 | 0x1858 | 0x185A | 0x1840 | 0x1842) {
            let wired = matches!(pid, 0x183A | 0x1854 | 0x185A | 0x1842);
            let colls: Vec<HidColl> = ctx
                .colls
                .iter()
                .filter(|c| c.vid == 0x1038 && c.pid == pid && c.iface == 3 && c.out_len() > 0)
                .cloned()
                .collect();
            for c in colls {
                let Some(dev) = HidDev::open_rw(&c) else { continue };
                dev.flush();
                if !dev.write(&[0x00, if wired { 0x92 } else { 0xD2 }], 200) {
                    continue;
                }
                let Some(r) = dev.read(250) else { continue };
                if r.len() >= 2 {
                    let raw = r[1] & 0x7F;
                    if (1..=21).contains(&raw) {
                        let name = if matches!(pid, 0x1838 | 0x183A) {
                            "SteelSeries Aerox 3"
                        } else if matches!(pid, 0x1852 | 0x1854) {
                            "SteelSeries Aerox 5"
                        } else if matches!(pid, 0x1858 | 0x185A) {
                            "SteelSeries Aerox 9"
                        } else {
                            "SteelSeries Prime"
                        };
                        ctx.add(
                            Peripheral {
                                id: format!("ss:{pid:04x}"),
                                name: name.into(),
                                kind: Kind::Mouse,
                                percent: (raw - 1) * 5,
                                charging: (r[1] & 0x80) != 0,
                            },
                            Some((0x1038, pid)),
                        );
                    } else {
                        ctx.claim(0x1038, pid);
                    }
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 9. Corsair Bragi / Slipstream (VID 0x1B1C)
// ---------------------------------------------------------------------------

fn bragi_get(dev: &HidDev, plain: bool, target: u8, prop: u8) -> Option<u32> {
    let req: &[u8] = if plain { &[0x00, target, 0x02, prop] } else { &[0x02, target, 0x02, prop] };
    dev.flush();
    if !dev.write(req, 200) {
        return None;
    }
    let source = if target == 0x08 { 0x00 } else { 0x01 };
    let off = if plain { 0 } else { 1 };
    let deadline = Instant::now() + Duration::from_millis(250);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let ms = left.as_millis().min(150) as u32;
        if ms == 0 {
            break;
        }
        let r = dev.read(ms)?;
        if r.len() < off + 7 || r[off + 1] != 0x02 || r[off] != source {
            continue;
        }
        if r[off + 2] != 0x00 {
            return None;
        }
        return Some(u32::from_le_bytes([r[off + 3], r[off + 4], r[off + 5], r[off + 6]]));
    }
    None
}

fn poll_corsair(ctx: &mut PollCtx) {
    for (_, pid) in ctx.products(0x1B1C) {
        let colls: Vec<HidColl> = ctx
            .colls
            .iter()
            .filter(|c| {
                c.vid == 0x1B1C
                    && c.pid == pid
                    && c.page() == 0xFF42
                    && matches!(c.out_len(), 64 | 65)
                    && matches!(c.in_len(), 64 | 65)
            })
            .cloned()
            .collect();
        for c in colls {
            let Some(dev) = HidDev::open_rw(&c) else { continue };
            let mut any = false;
            for target in [0x09u8, 0x0A, 0x0B, 0x08] {
                if target == 0x08 && any {
                    break;
                }
                let res = bragi_get(&dev, false, target, 0x0F)
                    .map(|v| (false, v))
                    .or_else(|| bragi_get(&dev, true, target, 0x0F).map(|v| (true, v)));
                let Some((plain, level)) = res.filter(|(_, l)| (1..=1000).contains(l)) else { continue };
                let pct = ((level + 5) / 10).min(100) as u8;
                let charging = bragi_get(&dev, plain, target, 0x10).is_some_and(|ch| ch == 1 || ch == 3);
                let mut name = strip_receiver_suffix(&c.name);
                if name.is_empty() {
                    name = "Corsair sem fio".into();
                }
                let kind = if (pid >> 8) == 0x0A {
                    Kind::Headset
                } else {
                    guess_kind_from_name(&name)
                        .or_else(|| ctx.kind_from_collections(0x1B1C, pid))
                        .unwrap_or(Kind::Mouse)
                };
                ctx.add(
                    Peripheral { id: format!("corsair:{pid:04x}:{target}"), name, kind, percent: pct, charging },
                    Some((0x1B1C, pid)),
                );
                any = true;
            }
            if any {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 10. Bluetooth do Windows (Classic HFP + BLE GATT)
// ---------------------------------------------------------------------------

fn poll_bluetooth(ctx: &mut PollCtx) {
    unsafe {
        let Ok(di) = SetupDiGetClassDevsW(None, None, None, DIGCF_ALLCLASSES | DIGCF_PRESENT) else {
            return;
        };
        let _guard = DevInfoGuard(di);

        struct BtFound {
            pct: u8,
            name: String,
            connected: bool,
        }

        let mut container_name: HashMap<String, String> = HashMap::new();
        let mut container_cod: HashMap<String, u32> = HashMap::new();
        let mut container_cat: HashMap<String, String> = HashMap::new();
        let mut container_conn: HashMap<String, bool> = HashMap::new();
        let mut container_guid: HashMap<String, GUID> = HashMap::new();
        let mut battery: HashMap<String, BtFound> = HashMap::new();

        let mut idx = 0u32;
        loop {
            let mut dd =
                SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(di, idx, &mut dd).is_err() {
                break;
            }
            idx += 1;

            let mut inst_buf = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(di, &dd, Some(&mut inst_buf), None).is_err() {
                continue;
            }
            let nul = inst_buf.iter().position(|&c| c == 0).unwrap_or(inst_buf.len());
            let inst = String::from_utf16_lossy(&inst_buf[..nul]);
            let upper = inst.to_uppercase();
            if !upper.starts_with("BTHENUM\\") && !upper.starts_with("BTHLE\\") && !upper.starts_with("BTHLEDEVICE\\") {
                continue;
            }

            let cid = match prop_guid(di, &dd, &DEVPKEY_Device_ContainerId) {
                Some(g) => {
                    let key = format!("{g:?}");
                    container_guid.insert(key.clone(), g);
                    key
                }
                None => upper.clone(),
            };

            let mut name = prop_string(di, &dd, &DEVPKEY_Device_FriendlyName);
            if name.is_empty() {
                name = prop_string(di, &dd, &DEVPKEY_Device_DeviceDesc);
            }

            if upper.starts_with("BTHENUM\\DEV_") || upper.starts_with("BTHLE\\DEV_") {
                if !name.is_empty() {
                    container_name.insert(cid.clone(), name.clone());
                }
                let mut ty = DEVPROPTYPE::default();
                let mut cod = [0u8; 4];
                if SetupDiGetDevicePropertyW(di, &dd, &PKEY_BT_COD, &mut ty, Some(&mut cod), None, 0).is_ok()
                    && ty == DEVPROP_TYPE_UINT32
                {
                    container_cod.insert(cid.clone(), u32::from_le_bytes(cod));
                }
                let cat = prop_string(di, &dd, &DEVPKEY_DeviceContainer_Category);
                if !cat.is_empty() {
                    container_cat.insert(cid.clone(), cat);
                }
                let mut conn = [0u8; 1];
                if SetupDiGetDevicePropertyW(di, &dd, &PKEY_IS_CONNECTED, &mut ty, Some(&mut conn), None, 0).is_ok()
                    && ty == DEVPROP_TYPE_BOOLEAN
                {
                    container_conn.insert(cid.clone(), DEVPROP_BOOLEAN(conn[0]).0 != 0);
                }
            }

            let mut ty = DEVPROPTYPE::default();
            let mut pct_buf = [0u8; 1];
            if SetupDiGetDevicePropertyW(di, &dd, &PKEY_BT_BATTERY, &mut ty, Some(&mut pct_buf), None, 0).is_err()
                || ty != DEVPROP_TYPE_BYTE
            {
                continue;
            }
            let pct = pct_buf[0];
            if pct > 100 {
                continue;
            }

            let mut status = CM_DEVNODE_STATUS_FLAGS::default();
            let mut prob = CM_PROB::default();
            if CM_Get_DevNode_Status(&mut status, &mut prob, dd.DevInst, 0) == CR_SUCCESS
                && (status.0 & DN_HAS_PROBLEM.0) != 0
            {
                continue;
            }

            let mut conn_buf = [1u8; 1];
            let connected = if SetupDiGetDevicePropertyW(
                di,
                &dd,
                &PKEY_IS_CONNECTED,
                &mut ty,
                Some(&mut conn_buf),
                None,
                0,
            )
            .is_ok()
                && ty == DEVPROP_TYPE_BOOLEAN
            {
                conn_buf[0] != 0
            } else {
                true
            };

            if debug_enabled() {
                crate::perf::line(format!(
                    "battery: bt inst={inst} cid={cid} pct={pct}% conn={connected} name={name:?}"
                ));
            }

            if battery.get(&cid).is_some_and(|prev| prev.connected && !connected) {
                continue;
            }
            battery.insert(cid, BtFound { pct, name: strip_bt_service_suffix(&name), connected });
        }

        // Contêineres já atendidos por um driver HID específico (ex.: Logitech HID++, DualSense).
        let mut claimed_containers: HashSet<GUID> = HashSet::new();
        let mut container_hid_ids: HashMap<GUID, (u16, u16)> = HashMap::new();
        for c in &ctx.colls {
            if let Some(g) = c.container {
                container_hid_ids.insert(g, (c.vid, c.pid));
                if ctx.is_claimed(c.vid, c.pid) {
                    claimed_containers.insert(g);
                }
            }
        }

        for (cid, found) in battery {
            let guid = container_guid.get(&cid).copied();
            if guid.is_some_and(|g| claimed_containers.contains(&g)) {
                continue;
            }
            let is_live = container_conn.get(&cid).copied().unwrap_or(found.connected);
            if !is_live {
                continue;
            }
            let name = container_name.get(&cid).filter(|s| !s.is_empty()).cloned().unwrap_or(found.name);
            let name = if name.is_empty() { "Dispositivo Bluetooth".into() } else { strip_bt_service_suffix(&name) };

            if ctx.out.readings.iter().any(|r| r.peripheral.name.eq_ignore_ascii_case(&name)) {
                continue;
            }

            let kind = container_cod
                .get(&cid)
                .and_then(|&cod| kind_from_bt_cod(cod))
                .or_else(|| container_cat.get(&cid).and_then(|cat| kind_from_category(cat)))
                .or_else(|| {
                    guid.and_then(|g| container_hid_ids.get(&g)).and_then(|&(v, p)| ctx.kind_from_collections(v, p))
                })
                .or_else(|| guess_kind_from_name(&name))
                .unwrap_or(Kind::Headset);

            if let Some(g) = guid {
                ctx.bt_containers.insert(g);
            }
            ctx.add(Peripheral { id: format!("bt:{cid}"), name, kind, percent: found.pct, charging: false }, None);
        }
    }
}

fn kind_from_bt_cod(cod: u32) -> Option<Kind> {
    let major = (cod >> 8) & 0x1F;
    let minor = (cod >> 2) & 0x3F;
    match major {
        0x04 => {
            if minor == 0x04 {
                Some(Kind::Microphone)
            } else {
                Some(Kind::Headset)
            }
        }
        0x05 => {
            if matches!(minor & 0x0F, 0x01 | 0x02) {
                Some(Kind::Gamepad)
            } else if (minor >> 4) == 0x01 {
                Some(Kind::Keyboard)
            } else if (minor >> 4) == 0x02 {
                Some(Kind::Mouse)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn kind_from_category(cat: &str) -> Option<Kind> {
    let low = cat.to_lowercase();
    if low.contains("mouse") {
        Some(Kind::Mouse)
    } else if low.contains("keyboard") {
        Some(Kind::Keyboard)
    } else if low.contains("gaming") || low.contains("gamepad") {
        Some(Kind::Gamepad)
    } else if low.contains("microphone") {
        Some(Kind::Microphone)
    } else if low.contains("headphone") || low.contains("headset") || low.contains("audio") {
        Some(Kind::Headset)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 11. HID Padrão (Battery Strength 0x06:0x20 / Absolute SoC 0x85:0x65)
// ---------------------------------------------------------------------------

fn poll_generic_hid(ctx: &mut PollCtx) {
    let colls = ctx.colls.clone();
    for c in &colls {
        // Nunca toca em mesas digitalizadoras (UsagePage 0x0D).
        if c.vid == 0 || c.page() == 0x0D || ctx.is_claimed(c.vid, c.pid) {
            continue;
        }
        if c.container.is_some_and(|g| ctx.bt_containers.contains(&g)) {
            continue;
        }

        let Some(pct) = (unsafe { read_standard_hid_battery(c) }) else { continue };
        let clean = strip_receiver_suffix(&c.name);
        let kind =
            guess_kind_from_name(&clean).or_else(|| ctx.kind_from_collections(c.vid, c.pid)).unwrap_or(Kind::Mouse);
        let name = if clean.is_empty() { kind.label_pt().to_string() } else { clean };
        ctx.add(
            Peripheral { id: format!("hid:{:04x}:{:04x}", c.vid, c.pid), name, kind, percent: pct, charging: false },
            Some((c.vid, c.pid)),
        );
    }
}

unsafe fn read_standard_hid_battery(c: &HidColl) -> Option<u8> {
    let h = CreateFileW(
        PCWSTR(c.path_wide.as_ptr()),
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_FLAGS_AND_ATTRIBUTES(0),
        None,
    )
    .ok()?;
    let mut pp = PHIDP_PREPARSED_DATA::default();
    if !HidD_GetPreparsedData(h, &mut pp) {
        let _ = CloseHandle(h);
        return None;
    }

    let mut found = None;
    for (rtype, count, len) in [
        (HidP_Feature, c.caps.NumberFeatureValueCaps, c.feat_len()),
        (HidP_Input, c.caps.NumberInputValueCaps, c.in_len()),
    ] {
        if count == 0 || len == 0 || found.is_some() {
            continue;
        }
        let mut n = count;
        let mut vcaps = vec![HIDP_VALUE_CAPS::default(); count as usize];
        if HidP_GetValueCaps(rtype, vcaps.as_mut_ptr(), &mut n, pp) != HIDP_STATUS_SUCCESS {
            continue;
        }
        for vc in vcaps.iter().take(n as usize) {
            let usage = if vc.IsRange { vc.Anonymous.Range.UsageMin } else { vc.Anonymous.NotRange.Usage };
            let is_bat =
                (vc.UsagePage == 0x06 && usage == 0x20) || (vc.UsagePage == 0x85 && matches!(usage, 0x65 | 0x66));
            if !is_bat {
                continue;
            }
            let mut buf = vec![0u8; len];
            buf[0] = vc.ReportID;
            let got = if rtype == HidP_Feature {
                HidD_GetFeature(h, buf.as_mut_ptr() as *mut c_void, len as u32)
            } else {
                HidD_GetInputReport(h, buf.as_mut_ptr() as *mut c_void, len as u32)
            };
            if !got {
                continue;
            }
            let mut val = 0u32;
            if HidP_GetUsageValue(rtype, vc.UsagePage, Some(vc.LinkCollection), usage, &mut val, pp, &buf)
                != HIDP_STATUS_SUCCESS
            {
                continue;
            }
            let pct = if c.vid == 0x05AC && val <= 100 {
                val as u8
            } else if vc.LogicalMax > vc.LogicalMin {
                let num = (val as i64 - vc.LogicalMin as i64).max(0) * 100;
                let den = (vc.LogicalMax as i64 - vc.LogicalMin as i64).max(1);
                (num / den).clamp(0, 100) as u8
            } else if val <= 100 {
                val as u8
            } else {
                continue;
            };
            if pct > 0 {
                found = Some(pct);
                break;
            }
        }
    }

    let _ = HidD_FreePreparsedData(pp);
    let _ = CloseHandle(h);
    found
}

// ---------------------------------------------------------------------------
// 12. XInput (Controles e Headsets sem fio de Xbox)
// ---------------------------------------------------------------------------

fn poll_xinput(ctx: &mut PollCtx) {
    // Se já encontramos um controle sem fio via Bluetooth/8BitDo/PlayStation,
    // evita duplicar caso algum tradutor XInput repita o mesmo controle.
    let existing_pads = ctx.out.readings.iter().filter(|r| !r.asleep && r.peripheral.kind == Kind::Gamepad).count();

    for slot in 0..4u32 {
        unsafe {
            let mut state = XINPUT_STATE::default();
            if XInputGetState(slot, &mut state) != 0 {
                continue;
            }
            for (dev_type, kind, base_name) in [
                (BATTERY_DEVTYPE_GAMEPAD, Kind::Gamepad, "Controle Xbox"),
                (BATTERY_DEVTYPE_HEADSET, Kind::Headset, "Headset Xbox"),
            ] {
                if kind == Kind::Gamepad && existing_pads > 0 && slot == 0 {
                    // Só pula se não for um controle Microsoft genuíno conectado via Xbox Wireless Adapter.
                    let has_xbox_wireless = ctx.colls.iter().any(|c| c.vid == 0x045E);
                    if !has_xbox_wireless {
                        continue;
                    }
                }
                let mut info = XINPUT_BATTERY_INFORMATION::default();
                if XInputGetBatteryInformation(slot, dev_type, &mut info) != 0 {
                    continue;
                }
                if debug_enabled() {
                    crate::perf::line(format!(
                        "battery: xinput slot={slot} kind={kind:?} type={} level={}",
                        info.BatteryType.0, info.BatteryLevel.0
                    ));
                }
                if matches!(info.BatteryType, BATTERY_TYPE_DISCONNECTED | BATTERY_TYPE_WIRED)
                    || info.BatteryType.0 == 0xFF
                {
                    continue;
                }
                let pct = match info.BatteryLevel {
                    BATTERY_LEVEL_EMPTY => 5,
                    BATTERY_LEVEL_LOW => 20,
                    BATTERY_LEVEL_MEDIUM => 55,
                    BATTERY_LEVEL_FULL => 100,
                    _ => continue,
                };
                let name = if slot == 0 { base_name.to_string() } else { format!("{base_name} {}", slot + 1) };
                ctx.add(
                    Peripheral {
                        id: format!("xinput:{slot}:{}", dev_type.0),
                        name,
                        kind,
                        percent: pct,
                        charging: false,
                    },
                    None,
                );
            }
        }
    }
}
