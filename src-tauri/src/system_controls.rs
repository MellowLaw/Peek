use serde::Serialize;
use std::{path::Path, sync::Mutex, time::Instant};
use tauri::{AppHandle, Manager, State};

use crate::windows::SnapLayout;

static BRIGHTNESS_LOCK: Mutex<()> = Mutex::new(());
static AUDIO_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterAudioState {
    pub volume: u32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAudioSession {
    pub pid: u32,
    pub name: String,
    pub volume: u32,
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSnapshot {
    pub master_available: bool,
    pub master_volume: u32,
    pub master_muted: bool,
    pub mic_available: bool,
    pub mic_volume: u32,
    pub mic_muted: bool,
    pub output_devices: Vec<AudioDeviceInfo>,
    pub app_sessions: Vec<AppAudioSession>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrightnessDisplay {
    pub id: String,
    pub name: String,
    pub supported: bool,
    pub level: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub ram_percent: u32,
    pub battery_present: bool,
    pub battery_percent: Option<u8>,
    pub battery_charging: bool,
    pub net_down_kbps: f64,
    pub net_up_kbps: f64,
    pub computer_name: String,
    pub uptime_minutes: u64,
}

#[derive(Default)]
struct MetricsSampler {
    prev_idle: u64,
    prev_kernel: u64,
    prev_user: u64,
    prev_in_octets: u64,
    prev_out_octets: u64,
    prev_at: Option<Instant>,
}

pub struct SystemMetricsState(Mutex<MetricsSampler>);

impl Default for SystemMetricsState {
    fn default() -> Self {
        Self(Mutex::new(MetricsSampler::default()))
    }
}

#[cfg(windows)]
fn with_com_mta<T, F: FnOnce() -> Result<T, String>>(f: F) -> Result<T, String> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let should_uninit = hr.is_ok();
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("COM init failed: {:?}", hr));
        }
        let res = f();
        if should_uninit {
            CoUninitialize();
        }
        res
    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
mod policy_config {
    use windows::Win32::Media::Audio::ERole;
    use windows_core::{interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR};

    pub const CLSID_POLICY_CONFIG_CLIENT: GUID =
        GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        pub fn GetMixFormat(
            &self,
            pszDeviceName: PCWSTR,
            ppFormat: *mut *mut core::ffi::c_void,
        ) -> HRESULT;
        pub fn GetDeviceFormat(
            &self,
            pszDeviceName: PCWSTR,
            bDefault: i32,
            ppFormat: *mut *mut core::ffi::c_void,
        ) -> HRESULT;
        pub fn ResetDeviceFormat(&self, pszDeviceName: PCWSTR) -> HRESULT;
        pub fn SetDeviceFormat(
            &self,
            pszDeviceName: PCWSTR,
            pEndpointFormat: *const core::ffi::c_void,
            pMixFormat: *const core::ffi::c_void,
        ) -> HRESULT;
        pub fn GetProcessingPeriod(
            &self,
            pszDeviceName: PCWSTR,
            bDefault: i32,
            pmftDefaultPeriod: *mut i64,
            pmftMinimumPeriod: *mut i64,
        ) -> HRESULT;
        pub fn SetProcessingPeriod(&self, pszDeviceName: PCWSTR, pmftPeriod: *mut i64) -> HRESULT;
        pub fn GetShareMode(&self, pszDeviceName: PCWSTR, pMode: *mut core::ffi::c_void)
            -> HRESULT;
        pub fn SetShareMode(
            &self,
            pszDeviceName: PCWSTR,
            mode: *const core::ffi::c_void,
        ) -> HRESULT;
        pub fn GetPropertyValue(
            &self,
            pszDeviceName: PCWSTR,
            bFxStore: i32,
            key: *const core::ffi::c_void,
            pv: *mut core::ffi::c_void,
        ) -> HRESULT;
        pub fn SetPropertyValue(
            &self,
            pszDeviceName: PCWSTR,
            bFxStore: i32,
            key: *const core::ffi::c_void,
            pv: *const core::ffi::c_void,
        ) -> HRESULT;
        pub fn SetDefaultEndpoint(&self, pszDeviceName: PCWSTR, role: ERole) -> HRESULT;
    }
}

#[cfg(windows)]
fn process_name_from_pid(pid: u32) -> String {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return "System Sounds".into();
    }
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("App ({pid})");
        };
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows_core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if ok && len > 0 {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            return Path::new(&full)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("App")
                .to_string();
        }
    }
    format!("App ({pid})")
}

#[cfg(windows)]
fn propvariant_to_string(
    pv: &mut windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
) -> String {
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Variant::{VARENUM, VT_BSTR};
    const VT_LPWSTR: VARENUM = VARENUM(31);
    unsafe {
        let vt = pv.Anonymous.Anonymous.vt;
        let s = if vt == VT_LPWSTR && !pv.Anonymous.Anonymous.Anonymous.pwszVal.is_null() {
            pv.Anonymous
                .Anonymous
                .Anonymous
                .pwszVal
                .to_string()
                .unwrap_or_default()
        } else if vt == VT_BSTR {
            pv.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
        } else {
            String::new()
        };
        let _ = PropVariantClear(pv);
        s
    }
}

#[cfg(windows)]
fn read_audio_snapshot_win() -> Result<AudioSnapshot, String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, IAudioSessionControl2,
        IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
    use windows_core::Interface;

    let pick_default = |enumerator: &IMMDeviceEnumerator, flow| -> Option<IMMDevice> {
        for role in [eConsole, eMultimedia, eCommunications] {
            if let Ok(dev) = unsafe { enumerator.GetDefaultAudioEndpoint(flow, role) } {
                return Some(dev);
            }
        }
        None
    };

    with_com_mta(|| unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;

        let mut default_id = String::new();
        let (master_available, master_volume, master_muted, default_render) =
            if let Some(dev) = pick_default(&enumerator, eRender) {
                if let Ok(pw) = dev.GetId() {
                    default_id = pw.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(pw.0 as _));
                }
                if let Ok(vol) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                    let lvl = vol.GetMasterVolumeLevelScalar().unwrap_or(0.0);
                    let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                    (true, (lvl * 100.0).round() as u32, muted, Some(dev))
                } else {
                    (false, 0, false, Some(dev))
                }
            } else {
                (false, 0, false, None)
            };

        let (mic_available, mic_volume, mic_muted) =
            if let Some(dev) = pick_default(&enumerator, eCapture) {
                if let Ok(vol) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                    let lvl = vol.GetMasterVolumeLevelScalar().unwrap_or(0.0);
                    let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                    (true, (lvl * 100.0).round() as u32, muted)
                } else {
                    (false, 0, false)
                }
            } else {
                (false, 0, false)
            };

        let mut output_devices = Vec::new();
        if let Ok(collection) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            let count = collection.GetCount().unwrap_or(0);
            for i in 0..count {
                if let Ok(dev) = collection.Item(i) {
                    let id = dev
                        .GetId()
                        .ok()
                        .map(|pw| {
                            let s = pw.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(pw.0 as _));
                            s
                        })
                        .unwrap_or_default();
                    let name = dev
                        .OpenPropertyStore(STGM_READ)
                        .ok()
                        .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName).ok())
                        .map(|mut pv| propvariant_to_string(&mut pv))
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| format!("Output {}", i + 1));
                    if !id.is_empty() {
                        output_devices.push(AudioDeviceInfo {
                            is_default: id == default_id,
                            id,
                            name,
                        });
                    }
                }
            }
        }

        let mut app_sessions: Vec<AppAudioSession> = Vec::new();
        if let Some(dev) = default_render {
            if let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                if let Ok(enum_sess) = mgr.GetSessionEnumerator() {
                    let count = enum_sess.GetCount().unwrap_or(0);
                    for i in 0..count {
                        let Ok(ctrl) = enum_sess.GetSession(i) else {
                            continue;
                        };
                        let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                            continue;
                        };
                        let Ok(pid) = ctrl2.GetProcessId() else {
                            continue;
                        };
                        let is_sys = ctrl2.IsSystemSoundsSession().0 == 0;
                        if pid == 0 && !is_sys {
                            continue;
                        }
                        if app_sessions.iter().any(|s| s.pid == pid) {
                            continue;
                        }
                        let Ok(simple) = ctrl.cast::<ISimpleAudioVolume>() else {
                            continue;
                        };
                        let vol = simple.GetMasterVolume().unwrap_or(1.0);
                        let muted = simple.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                        let name = if is_sys {
                            "System Sounds".into()
                        } else {
                            let disp = ctrl
                                .GetDisplayName()
                                .ok()
                                .map(|pw| {
                                    let s = pw.to_string().unwrap_or_default();
                                    CoTaskMemFree(Some(pw.0 as _));
                                    s
                                })
                                .unwrap_or_default();
                            if !disp.trim().is_empty() && !disp.starts_with('@') {
                                disp
                            } else {
                                process_name_from_pid(pid)
                            }
                        };
                        app_sessions.push(AppAudioSession {
                            pid,
                            name,
                            volume: (vol * 100.0).round() as u32,
                            muted,
                        });
                    }
                }
            }
        }

        Ok(AudioSnapshot {
            master_available,
            master_volume,
            master_muted,
            mic_available,
            mic_volume,
            mic_muted,
            output_devices,
            app_sessions,
        })
    })
}

#[tauri::command]
pub async fn get_audio_snapshot() -> Result<AudioSnapshot, String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(read_audio_snapshot_win)
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    Err("Windows only".into())
}

#[cfg(windows)]
fn apply_endpoint_volume<F>(
    flow: windows::Win32::Media::Audio::EDataFlow,
    mut f: F,
) -> Result<(), String>
where
    F: FnMut(&windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume) -> Result<(), String>,
{
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCommunications, eConsole, eMultimedia, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

    let _guard = AUDIO_LOCK.lock().unwrap();
    with_com_mta(|| unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let mut seen_ids = Vec::new();
        let mut last_err = None;
        for role in [eConsole, eMultimedia, eCommunications] {
            match enumerator.GetDefaultAudioEndpoint(flow, role) {
                Ok(dev) => {
                    let id = dev
                        .GetId()
                        .ok()
                        .map(|pw| {
                            let s = pw.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(pw.0 as _));
                            s
                        })
                        .unwrap_or_default();
                    if !id.is_empty() && seen_ids.contains(&id) {
                        continue;
                    }
                    if !id.is_empty() {
                        seen_ids.push(id);
                    }
                    match dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                        Ok(vol) => f(&vol)?,
                        Err(e) => last_err = Some(e.to_string()),
                    }
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        if seen_ids.is_empty() {
            Err(last_err.unwrap_or_else(|| "No default audio endpoint found".into()))
        } else {
            Ok(())
        }
    })
}

#[cfg(windows)]
fn read_master_state(
    vol: &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
) -> MasterAudioState {
    unsafe {
        let lvl = vol.GetMasterVolumeLevelScalar().unwrap_or(0.0);
        let muted = vol.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        MasterAudioState {
            volume: (lvl * 100.0).round().clamp(0.0, 100.0) as u32,
            muted,
        }
    }
}

#[cfg(windows)]
fn set_master_volume_win(level: u32) -> Result<MasterAudioState, String> {
    use windows::Win32::Media::Audio::eRender;
    let mut readback = None;
    let ctx = windows_core::GUID::zeroed();
    apply_endpoint_volume(eRender, |vol| unsafe {
        let clamped = level.min(100);
        let scalar = (clamped as f32) / 100.0;
        vol.SetMasterVolumeLevelScalar(scalar, &ctx)
            .map_err(|e| e.to_string())?;
        if clamped > 0 && vol.GetMute().is_ok_and(|b| b.as_bool()) {
            vol.SetMute(false, &ctx).map_err(|e| e.to_string())?;
        }
        if readback.is_none() {
            readback = Some(read_master_state(vol));
        }
        Ok(())
    })?;
    Ok(readback.unwrap_or(MasterAudioState {
        volume: level.min(100),
        muted: false,
    }))
}

#[cfg(windows)]
fn set_master_mute_win(muted: bool) -> Result<MasterAudioState, String> {
    use windows::Win32::Media::Audio::eRender;
    let mut readback = None;
    let ctx = windows_core::GUID::zeroed();
    apply_endpoint_volume(eRender, |vol| unsafe {
        vol.SetMute(muted, &ctx).map_err(|e| e.to_string())?;
        if readback.is_none() {
            readback = Some(read_master_state(vol));
        }
        Ok(())
    })?;
    Ok(readback.unwrap_or(MasterAudioState { volume: 0, muted }))
}

#[cfg(windows)]
fn set_mic_mute_win(muted: bool) -> Result<(), String> {
    use windows::Win32::Media::Audio::eCapture;
    let ctx = windows_core::GUID::zeroed();
    apply_endpoint_volume(eCapture, |vol| unsafe {
        vol.SetMute(muted, &ctx).map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub async fn set_master_volume(level: u32) -> Result<MasterAudioState, String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || set_master_volume_win(level))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    Ok(MasterAudioState {
        volume: level.min(100),
        muted: false,
    })
}

#[tauri::command]
pub async fn set_master_mute(muted: bool) -> Result<MasterAudioState, String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || set_master_mute_win(muted))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    Ok(MasterAudioState { volume: 100, muted })
}

#[tauri::command]
pub async fn set_mic_mute(muted: bool) -> Result<(), String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || set_mic_mute_win(muted))
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    {
        let _ = muted;
        Ok(())
    }
}

#[tauri::command]
pub async fn set_default_output_device(device_id: String) -> Result<(), String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

        with_com_mta(|| unsafe {
            let policy: policy_config::IPolicyConfig =
                CoCreateInstance(&policy_config::CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                    .map_err(|e| e.to_string())?;
            let wide = HSTRING::from(device_id);
            for role in [eConsole, eMultimedia, eCommunications] {
                let hr = policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role);
                if hr.is_err() {
                    return Err(format!("Could not switch output device: {:?}", hr));
                }
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    Ok(())
}

#[tauri::command]
pub async fn set_app_audio(
    pid: u32,
    volume: Option<u32>,
    muted: Option<bool>,
) -> Result<(), String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || {
        use windows::Win32::Media::Audio::{
            eConsole, eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2,
            IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
        use windows_core::Interface;

        let _guard = AUDIO_LOCK.lock().unwrap();
        let ctx = windows_core::GUID::zeroed();
        with_com_mta(|| unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| e.to_string())?;
            let mut seen_ids = Vec::new();
            for role in [eConsole, eMultimedia] {
                let Ok(dev) = enumerator.GetDefaultAudioEndpoint(eRender, role) else {
                    continue;
                };
                let id = dev
                    .GetId()
                    .ok()
                    .map(|pw| {
                        let s = pw.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(pw.0 as _));
                        s
                    })
                    .unwrap_or_default();
                if !id.is_empty() && seen_ids.contains(&id) {
                    continue;
                }
                if !id.is_empty() {
                    seen_ids.push(id);
                }
                let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    continue;
                };
                let Ok(enum_sess) = mgr.GetSessionEnumerator() else {
                    continue;
                };
                let count = enum_sess.GetCount().unwrap_or(0);
                for i in 0..count {
                    if let Ok(ctrl) = enum_sess.GetSession(i) {
                        if let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() {
                            let is_sys = ctrl2.IsSystemSoundsSession().0 == 0;
                            let matches = if pid == 0 {
                                is_sys
                            } else {
                                ctrl2.GetProcessId().ok() == Some(pid)
                            };
                            if matches {
                                if let Ok(simple) = ctrl.cast::<ISimpleAudioVolume>() {
                                    if let Some(lvl) = volume {
                                        simple
                                            .SetMasterVolume((lvl.min(100) as f32) / 100.0, &ctx)
                                            .map_err(|e| e.to_string())?;
                                    }
                                    if let Some(m) = muted {
                                        simple.SetMute(m, &ctx).map_err(|e| e.to_string())?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    {
        let _ = (pid, volume, muted);
        Ok(())
    }
}

#[cfg(windows)]
unsafe fn set_wmi_proxy_blanket<P: windows_core::Interface>(proxy: &P) {
    use windows::Win32::System::Com::{
        CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    if let Ok(unk) = proxy.cast::<windows_core::IUnknown>() {
        let _ = CoSetProxyBlanket(
            &unk,
            10,
            0,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        );
    }
}

#[cfg(windows)]
fn read_wmi_brightness() -> Result<Vec<BrightnessDisplay>, String> {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::System::Variant::{
        VariantClear, VARIANT, VT_BSTR, VT_I2, VT_I4, VT_UI1, VT_UI2, VT_UI4,
    };
    use windows::Win32::System::Wmi::{
        IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
        WBEM_INFINITE,
    };

    with_com_mta(|| unsafe {
        let mut out = Vec::new();
        let loc: IWbemLocator =
            CoCreateInstance(&WbemLocator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let svc = loc
            .ConnectServer(
                &BSTR::from("ROOT\\WMI"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
            .map_err(|e| e.to_string())?;
        set_wmi_proxy_blanket(&svc);
        let enumerator = svc
            .ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from("SELECT InstanceName, CurrentBrightness FROM WmiMonitorBrightness"),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
            .map_err(|e| e.to_string())?;
        set_wmi_proxy_blanket(&enumerator);

        loop {
            let mut objs = [None];
            let mut returned = 0u32;
            let hr = enumerator.Next(WBEM_INFINITE, &mut objs, &mut returned);
            if hr.is_err() || returned == 0 {
                break;
            }
            if let Some(obj) = objs[0].take() {
                let mut v_name = VARIANT::default();
                let mut v_val = VARIANT::default();
                let _ = obj.Get(windows_core::w!("InstanceName"), 0, &mut v_name, None, None);
                let _ = obj.Get(
                    windows_core::w!("CurrentBrightness"),
                    0,
                    &mut v_val,
                    None,
                    None,
                );

                let inst = if v_name.Anonymous.Anonymous.vt == VT_BSTR {
                    v_name.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
                } else {
                    String::new()
                };
                let lvl = match v_val.Anonymous.Anonymous.vt {
                    x if x == VT_UI1 => v_val.Anonymous.Anonymous.Anonymous.bVal as u32,
                    x if x == VT_I2 => v_val.Anonymous.Anonymous.Anonymous.iVal.max(0) as u32,
                    x if x == VT_UI2 => v_val.Anonymous.Anonymous.Anonymous.uiVal as u32,
                    x if x == VT_I4 => v_val.Anonymous.Anonymous.Anonymous.lVal.max(0) as u32,
                    x if x == VT_UI4 => v_val.Anonymous.Anonymous.Anonymous.ulVal,
                    _ => 50,
                };
                let _ = VariantClear(&mut v_name);
                let _ = VariantClear(&mut v_val);
                if !inst.is_empty() {
                    out.push(BrightnessDisplay {
                        id: format!("wmi:{inst}"),
                        name: if out.is_empty() {
                            "Built-in display".into()
                        } else {
                            format!("Built-in display {}", out.len() + 1)
                        },
                        supported: true,
                        level: lvl.min(100),
                    });
                }
            }
        }
        Ok(out)
    })
}

#[cfg(windows)]
fn set_wmi_brightness(instance_name: &str, level: u32) -> Result<(), String> {
    use std::mem::ManuallyDrop;
    use windows::core::{w, BSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::System::Variant::{
        VariantClear, VARIANT, VARIANT_0_0, VARIANT_0_0_0, VT_BSTR, VT_I4, VT_UI1,
    };
    use windows::Win32::System::Wmi::{
        IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
        WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE,
    };

    let _guard = BRIGHTNESS_LOCK.lock().unwrap();
    let inst = instance_name.trim().to_string();
    let clamped = level.min(100) as u8;
    with_com_mta(|| unsafe {
        let loc: IWbemLocator =
            CoCreateInstance(&WbemLocator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let svc = loc
            .ConnectServer(
                &BSTR::from("ROOT\\WMI"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
            .map_err(|e| e.to_string())?;
        set_wmi_proxy_blanket(&svc);

        let mut target_relpath = String::new();
        if let Ok(enum_methods) = svc.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT __RELPATH, InstanceName FROM WmiMonitorBrightnessMethods"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        ) {
            set_wmi_proxy_blanket(&enum_methods);
            let mut first_relpath = String::new();
            loop {
                let mut objs = [None];
                let mut returned = 0u32;
                let hr = enum_methods.Next(WBEM_INFINITE, &mut objs, &mut returned);
                if hr.is_err() || returned == 0 {
                    break;
                }
                if let Some(obj) = objs[0].take() {
                    let mut v_rel = VARIANT::default();
                    let mut v_name = VARIANT::default();
                    let _ = obj.Get(w!("__RELPATH"), 0, &mut v_rel, None, None);
                    let _ = obj.Get(w!("InstanceName"), 0, &mut v_name, None, None);
                    let rel = if v_rel.Anonymous.Anonymous.vt == VT_BSTR {
                        v_rel.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
                    } else {
                        String::new()
                    };
                    let name = if v_name.Anonymous.Anonymous.vt == VT_BSTR {
                        v_name.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
                    } else {
                        String::new()
                    };
                    let _ = VariantClear(&mut v_rel);
                    let _ = VariantClear(&mut v_name);
                    if first_relpath.is_empty() && !rel.is_empty() {
                        first_relpath = rel.clone();
                    }
                    if name.trim().eq_ignore_ascii_case(&inst) && !rel.is_empty() {
                        target_relpath = rel;
                        break;
                    }
                }
            }
            if target_relpath.is_empty() {
                target_relpath = first_relpath;
            }
        }
        if target_relpath.is_empty() {
            let escaped = inst.replace('\\', "\\\\").replace('\'', "\\'");
            target_relpath = format!("WmiMonitorBrightnessMethods.InstanceName='{escaped}'");
        }

        let mut class_obj = None;
        svc.GetObject(
            &BSTR::from("WmiMonitorBrightnessMethods"),
            Default::default(),
            None,
            Some(&mut class_obj),
            None,
        )
        .map_err(|e| e.to_string())?;
        let class_obj =
            class_obj.ok_or_else(|| "WMI brightness methods unavailable".to_string())?;

        let mut in_sig = None;
        class_obj
            .GetMethod(w!("WmiSetBrightness"), 0, &mut in_sig, std::ptr::null_mut())
            .map_err(|e| e.to_string())?;
        let in_sig = in_sig.ok_or_else(|| "WmiSetBrightness signature missing".to_string())?;
        let in_params = in_sig.SpawnInstance(0).map_err(|e| e.to_string())?;

        let mut v_timeout = VARIANT::default();
        v_timeout.Anonymous.Anonymous = ManuallyDrop::new(VARIANT_0_0 {
            vt: VT_I4,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: VARIANT_0_0_0 { lVal: 1 },
        });
        let mut v_brightness = VARIANT::default();
        v_brightness.Anonymous.Anonymous = ManuallyDrop::new(VARIANT_0_0 {
            vt: VT_UI1,
            wReserved1: 0,
            wReserved2: 0,
            wReserved3: 0,
            Anonymous: VARIANT_0_0_0 { bVal: clamped },
        });

        in_params
            .Put(w!("Timeout"), 0, &v_timeout, 0)
            .map_err(|e| e.to_string())?;
        in_params
            .Put(w!("Brightness"), 0, &v_brightness, 0)
            .map_err(|e| e.to_string())?;
        let _ = VariantClear(&mut v_timeout);
        let _ = VariantClear(&mut v_brightness);

        // Execute synchronously (flags = 0) so driver commits brightness and returns real HRESULT errors.
        svc.ExecMethod(
            &BSTR::from(target_relpath),
            &BSTR::from("WmiSetBrightness"),
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            &in_params,
            None,
            None,
        )
        .map_err(|e| format!("Failed to set WMI brightness: {e}"))
    })
}

#[cfg(windows)]
fn read_ddcci_monitors() -> Vec<BrightnessDisplay> {
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR,
    };
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    unsafe extern "system" fn enum_proc(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows_core::BOOL {
        let list = &mut *(data.0 as *mut Vec<HMONITOR>);
        list.push(hmon);
        true.into()
    }

    let mut hmons: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut hmons as *mut _ as isize),
        );
    }

    let mut out = Vec::new();
    for (idx, hmon) in hmons.into_iter().enumerate() {
        unsafe {
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count).is_err() || count == 0 {
                continue;
            }
            let mut pms = vec![PHYSICAL_MONITOR::default(); count as usize];
            if GetPhysicalMonitorsFromHMONITOR(hmon, &mut pms).is_err() {
                continue;
            }
            for (sub, pm) in pms.iter().enumerate() {
                let mut min = 0u32;
                let mut cur = 0u32;
                let mut max = 0u32;
                let raw_desc = std::ptr::addr_of!(pm.szPhysicalMonitorDescription).read_unaligned();
                let desc =
                    String::from_utf16_lossy(raw_desc.split(|&c| c == 0).next().unwrap_or(&[]));
                let name = if desc.trim().is_empty() {
                    format!("Monitor {}", idx + 1)
                } else {
                    desc
                };
                if GetMonitorBrightness(pm.hPhysicalMonitor, &mut min, &mut cur, &mut max) != 0
                    && max > min
                {
                    let pct = ((cur.saturating_sub(min)) * 100) / (max - min);
                    out.push(BrightnessDisplay {
                        id: format!("ddc:{idx}:{sub}"),
                        name,
                        supported: true,
                        level: pct.min(100),
                    });
                } else {
                    out.push(BrightnessDisplay {
                        id: format!("ddc:{idx}:{sub}"),
                        name,
                        supported: false,
                        level: 0,
                    });
                }
            }
            let _ = DestroyPhysicalMonitors(&pms);
        }
    }
    out
}

#[cfg(windows)]
fn set_ddcci_brightness(target_idx: usize, target_sub: usize, level: u32) -> Result<(), String> {
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
    };
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    unsafe extern "system" fn enum_proc(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows_core::BOOL {
        let list = &mut *(data.0 as *mut Vec<HMONITOR>);
        list.push(hmon);
        true.into()
    }

    let mut hmons: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut hmons as *mut _ as isize),
        );
    }
    let hmon = *hmons
        .get(target_idx)
        .ok_or_else(|| "Display not found".to_string())?;
    unsafe {
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count).map_err(|e| e.to_string())?;
        let mut pms = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(hmon, &mut pms).map_err(|e| e.to_string())?;
        let res = if let Some(pm) = pms.get(target_sub) {
            let mut min = 0u32;
            let mut cur = 0u32;
            let mut max = 0u32;
            if GetMonitorBrightness(pm.hPhysicalMonitor, &mut min, &mut cur, &mut max) != 0
                && max > min
            {
                let target_val = min + ((max - min) * level.min(100)) / 100;
                if SetMonitorBrightness(pm.hPhysicalMonitor, target_val) != 0 {
                    Ok(())
                } else {
                    Err("Monitor rejected brightness change".into())
                }
            } else {
                Err("Monitor does not support DDC/CI brightness".into())
            }
        } else {
            Err("Monitor sub-display missing".into())
        };
        let _ = DestroyPhysicalMonitors(&pms);
        res
    }
}

#[tauri::command]
pub async fn get_brightness_displays() -> Result<Vec<BrightnessDisplay>, String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(|| {
        let wmi_res = read_wmi_brightness();
        let ddc = read_ddcci_monitors();
        match wmi_res {
            Ok(wmi) if !wmi.is_empty() => {
                // If WMI internal panel succeeded, keep only supported external DDC/CI displays.
                let mut combined = wmi;
                for d in ddc {
                    if d.supported {
                        combined.push(d);
                    }
                }
                Ok(combined)
            }
            Ok(_) => Ok(ddc),
            Err(err) => {
                if ddc.iter().any(|d| d.supported) {
                    Ok(ddc)
                } else {
                    Err(err)
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    Ok(Vec::new())
}

#[tauri::command]
pub async fn set_display_brightness(id: String, level: u32) -> Result<(), String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(move || {
        if let Some(inst) = id.strip_prefix("wmi:") {
            return set_wmi_brightness(inst, level);
        }
        if let Some(rest) = id.strip_prefix("ddc:") {
            let _guard = BRIGHTNESS_LOCK.lock().unwrap();
            let mut parts = rest.split(':');
            let idx: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let sub: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            return set_ddcci_brightness(idx, sub, level);
        }
        Err("Unknown display identifier".into())
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    {
        let _ = (id, level);
        Ok(())
    }
}

#[cfg(windows)]
fn filetime_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

#[tauri::command]
pub fn get_system_metrics(state: State<'_, SystemMetricsState>) -> Result<SystemMetrics, String> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfTable2, MIB_IF_TABLE2,
        };
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        use windows::Win32::System::SystemInformation::{
            GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
        };
        use windows::Win32::System::Threading::GetSystemTimes;

        let mut sampler = state.0.lock().unwrap();

        let (mut idle_ft, mut kernel_ft, mut user_ft) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let _ = GetSystemTimes(Some(&mut idle_ft), Some(&mut kernel_ft), Some(&mut user_ft));
        let idle = filetime_u64(idle_ft);
        let kernel = filetime_u64(kernel_ft);
        let user = filetime_u64(user_ft);
        let cpu_percent = if sampler.prev_kernel + sampler.prev_user > 0 {
            let di = idle.saturating_sub(sampler.prev_idle);
            let dk = kernel.saturating_sub(sampler.prev_kernel);
            let du = user.saturating_sub(sampler.prev_user);
            let total = dk + du;
            if total > 0 {
                (((total.saturating_sub(di)) as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
                    as f32
            } else {
                0.0
            }
        } else {
            0.0
        };
        sampler.prev_idle = idle;
        sampler.prev_kernel = kernel;
        sampler.prev_user = user;

        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let _ = GlobalMemoryStatusEx(&mut mem);
        let total_gb = mem.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
        let avail_gb = mem.ullAvailPhys as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_gb = (total_gb - avail_gb).max(0.0);

        let mut pwr = SYSTEM_POWER_STATUS::default();
        let _ = GetSystemPowerStatus(&mut pwr);
        let battery_present = (pwr.BatteryFlag & 128) == 0 && pwr.BatteryLifePercent <= 100;
        let battery_percent = battery_present.then_some(pwr.BatteryLifePercent);
        let battery_charging = pwr.ACLineStatus == 1;

        let mut total_in = 0u64;
        let mut total_out = 0u64;
        let mut table_ptr: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table_ptr).0 == 0 && !table_ptr.is_null() {
            let table = &*table_ptr;
            let rows = std::slice::from_raw_parts(table.Table.as_ptr(), table.NumEntries as usize);
            for row in rows {
                // Skip loopback (IfType == 24)
                if row.Type == 24 {
                    continue;
                }
                total_in = total_in.saturating_add(row.InOctets);
                total_out = total_out.saturating_add(row.OutOctets);
            }
            FreeMibTable(table_ptr as _);
        }

        let now = Instant::now();
        let (net_down_kbps, net_up_kbps) = if let Some(prev_at) = sampler.prev_at {
            let dt = now.duration_since(prev_at).as_secs_f64().max(0.1);
            let din = total_in.saturating_sub(sampler.prev_in_octets) as f64;
            let dout = total_out.saturating_sub(sampler.prev_out_octets) as f64;
            (din / 1024.0 / dt, dout / 1024.0 / dt)
        } else {
            (0.0, 0.0)
        };
        sampler.prev_in_octets = total_in;
        sampler.prev_out_octets = total_out;
        sampler.prev_at = Some(now);

        let computer_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into());
        let uptime_minutes = GetTickCount64() / 60_000;

        Ok(SystemMetrics {
            cpu_percent,
            ram_used_gb: (used_gb * 10.0).round() / 10.0,
            ram_total_gb: (total_gb * 10.0).round() / 10.0,
            ram_percent: mem.dwMemoryLoad,
            battery_present,
            battery_percent,
            battery_charging,
            net_down_kbps: (net_down_kbps * 10.0).round() / 10.0,
            net_up_kbps: (net_up_kbps * 10.0).round() / 10.0,
            computer_name,
            uptime_minutes,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Err("Windows only".into())
    }
}

#[tauri::command]
pub fn snap_active_window(app: AppHandle, layout: SnapLayout) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnds = crate::windows::peek_hwnds(&app);
        let target = app
            .state::<crate::windows::ForegroundTracker>()
            .best_external(&hwnds)
            .ok_or_else(|| "No external window found to organize".to_string())?;
        crate::windows::snap_window(target, layout)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, layout);
        Ok(())
    }
}

#[tauri::command]
pub fn open_windows_setting(key: String) -> Result<(), String> {
    #[cfg(windows)]
    return crate::windows::open_ms_settings(&key);
    #[cfg(not(windows))]
    {
        let _ = key;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn reads_and_reapplies_system_audio_and_brightness_roundtrip() {
        let snap = read_audio_snapshot_win().expect("audio snapshot");
        assert!(snap.master_volume <= 100);
        if snap.master_available {
            let orig_vol = snap.master_volume;
            let orig_muted = snap.master_muted;
            let probe_vol = if orig_vol >= 4 {
                orig_vol - 2
            } else {
                orig_vol + 2
            };

            let applied = set_master_volume_win(probe_vol).expect("set probe master volume");
            assert_eq!(applied.volume, probe_vol);
            let after_probe = read_audio_snapshot_win().expect("audio snapshot after probe");
            assert_eq!(after_probe.master_volume, probe_vol);

            let restored = set_master_volume_win(orig_vol).expect("restore original master volume");
            assert_eq!(restored.volume, orig_vol);
            set_master_mute_win(orig_muted).expect("restore original master mute");
        }

        if let Ok(displays) = read_wmi_brightness() {
            if let Some(disp) = displays.first() {
                let inst = disp.id.strip_prefix("wmi:").expect("wmi prefix");
                set_wmi_brightness(inst, disp.level).expect("reapply current wmi brightness");
            }
        }
    }
}
