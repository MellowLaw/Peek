use serde::Serialize;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, DragQueryFileW, IDropTargetHelper, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, IsWindowVisible, KillTimer, RegisterClassExW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, GUITHREADINFO, GUI_INMOVESIZE, HWND_TOPMOST, LWA_ALPHA, MSG, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{implement, w, Ref, Result as WinResult, PCWSTR};

use crate::island::Mode;

const CATCHER_CLASS: PCWSTR = w!("PeekDropCatcherWindow");
const TIMER_ID: usize = 1;
const POLL_INTERVAL_MS: u32 = 45;

static INTERNAL_DRAG_OUT: AtomicBool = AtomicBool::new(false);
static EXTERNAL_DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDragPayload {
    pub kind: &'static str,
    pub paths: Vec<String>,
}

/// RAII guard active while Peek itself is performing an outgoing OLE drag so the catcher ignores it.
pub struct InternalDragGuard;

impl Drop for InternalDragGuard {
    fn drop(&mut self) {
        INTERNAL_DRAG_OUT.store(false, Ordering::SeqCst);
    }
}

pub fn begin_internal_drag() -> InternalDragGuard {
    INTERNAL_DRAG_OUT.store(true, Ordering::SeqCst);
    InternalDragGuard
}

/// True while an external Explorer file drag is hovering Peek's drop catcher.
pub fn is_external_file_drag_active() -> bool {
    EXTERNAL_DRAG_ACTIVE.load(Ordering::SeqCst)
}

#[derive(Default)]
struct CatcherState {
    hwnd_raw: isize,
    in_file_drag: bool,
    rejected_this_press: bool,
    current_rect: (i32, i32, i32, i32),
}

/// Extract filesystem paths from a Windows `IDataObject` carrying `CF_HDROP`.
pub fn extract_hdrop_paths(data_obj: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe {
        if data_obj.QueryGetData(&fmt).is_err() {
            return Vec::new();
        }
        let Ok(mut medium) = data_obj.GetData(&fmt) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        let hdrop = HDROP(medium.u.hGlobal.0);
        if !hdrop.0.is_null() {
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            for idx in 0..count {
                let len = DragQueryFileW(hdrop, idx, None);
                if len == 0 {
                    continue;
                }
                let mut buf = vec![0u16; len as usize + 1];
                let copied = DragQueryFileW(hdrop, idx, Some(&mut buf));
                if copied > 0 {
                    let os = OsString::from_wide(&buf[..copied as usize]);
                    paths.push(PathBuf::from(os));
                }
            }
        }
        ReleaseStgMedium(&mut medium);
        paths
    }
}

fn expanded_drop_rect(app: &AppHandle) -> Option<(i32, i32, i32, i32)> {
    let (l, t, r, b, scale) = crate::island::monitor_notch_rect(app, Mode::Expanded)?;
    let pad = (12.0 * scale).round() as i32;
    Some((l - pad, t, r + pad, b + pad))
}

fn closed_approach_rect(app: &AppHandle) -> Option<(i32, i32, i32, i32)> {
    let (l, t, r, _b, scale) = crate::island::monitor_notch_rect(app, Mode::Idle)?;
    let cx = (l + r) / 2;
    let half_w = (150.0 * scale).round() as i32;
    let height = (70.0 * scale).round() as i32;
    Some((cx - half_w, t, cx + half_w, t + height))
}

fn position_catcher_hwnd(hwnd: HWND, rect: (i32, i32, i32, i32)) {
    let (l, t, r, b) = rect;
    let w = (r - l).max(16);
    let h = (b - t).max(16);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            l,
            t,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn hide_catcher_hwnd(hwnd: HWND) {
    unsafe {
        if IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn emit_drag(app: &AppHandle, kind: &'static str, paths: Vec<String>) {
    let _ = app.emit("file-drag", FileDragPayload { kind, paths });
}

#[implement(IDropTarget)]
struct DropCatcherTarget {
    app: AppHandle,
    state: std::sync::Arc<Mutex<CatcherState>>,
    helper: Option<IDropTargetHelper>,
}

impl DropCatcherTarget {
    fn hwnd(&self) -> HWND {
        HWND(self.state.lock().unwrap().hwnd_raw as _)
    }
}

impl IDropTarget_Impl for DropCatcherTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let Some(data_obj) = pdataobj.as_ref() else {
            unsafe { *pdweffect = DROPEFFECT_NONE };
            return Ok(());
        };
        let hwnd = self.hwnd();
        let paths = extract_hdrop_paths(data_obj);
        if paths.is_empty() {
            {
                let mut st = self.state.lock().unwrap();
                st.in_file_drag = false;
                st.rejected_this_press = true;
            }
            EXTERNAL_DRAG_ACTIVE.store(false, Ordering::SeqCst);
            unsafe { *pdweffect = DROPEFFECT_NONE };
            hide_catcher_hwnd(hwnd);
            return Ok(());
        }

        let effect = DROPEFFECT_COPY;
        unsafe { *pdweffect = effect };
        EXTERNAL_DRAG_ACTIVE.store(true, Ordering::SeqCst);

        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragEnter(hwnd, data_obj, &POINT { x: pt.x, y: pt.y }, effect);
            }
        }

        let next_rect = expanded_drop_rect(&self.app);
        {
            let mut st = self.state.lock().unwrap();
            st.in_file_drag = true;
            if let Some(r) = next_rect {
                st.current_rect = r;
            }
        }
        if let Some(r) = next_rect {
            position_catcher_hwnd(hwnd, r);
        }

        let path_strs: Vec<String> = paths
            .iter()
            .filter_map(|p| p.to_str().map(str::to_string))
            .collect();
        emit_drag(&self.app, "enter", path_strs);
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let active = self.state.lock().unwrap().in_file_drag;
        let effect = if active {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
        unsafe { *pdweffect = effect };
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragOver(&POINT { x: pt.x, y: pt.y }, effect);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        let hwnd = self.hwnd();
        let was_active = {
            let mut st = self.state.lock().unwrap();
            let prev = st.in_file_drag;
            st.in_file_drag = false;
            prev
        };
        EXTERNAL_DRAG_ACTIVE.store(false, Ordering::SeqCst);
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragLeave();
            }
        }
        hide_catcher_hwnd(hwnd);
        if was_active {
            emit_drag(&self.app, "leave", Vec::new());
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let hwnd = self.hwnd();
        {
            let mut st = self.state.lock().unwrap();
            st.in_file_drag = false;
        }
        EXTERNAL_DRAG_ACTIVE.store(false, Ordering::SeqCst);
        hide_catcher_hwnd(hwnd);

        let paths = match pdataobj.as_ref() {
            Some(obj) => {
                if let Some(helper) = &self.helper {
                    unsafe {
                        let _ = helper.Drop(obj, &POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY);
                    }
                }
                extract_hdrop_paths(obj)
            }
            None => Vec::new(),
        };

        if paths.is_empty() {
            unsafe { *pdweffect = DROPEFFECT_NONE };
            emit_drag(&self.app, "leave", Vec::new());
        } else {
            unsafe { *pdweffect = DROPEFFECT_COPY };
            crate::shelf::ingest_dropped_paths(&self.app, &paths);
            let path_strs: Vec<String> = paths
                .iter()
                .filter_map(|p| p.to_str().map(str::to_string))
                .collect();
            emit_drag(&self.app, "drop", path_strs);
        }
        Ok(())
    }
}

fn ensure_catcher_class() -> bool {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(catcher_wnd_proc),
            hInstance: hinst.into(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: CATCHER_CLASS,
            ..Default::default()
        };
        let atom = RegisterClassExW(&wc);
        if atom != 0 {
            return true;
        }
        // Class may already be registered in the process.
        std::io::Error::last_os_error().raw_os_error() == Some(1410)
    }
}

unsafe extern "system" fn catcher_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn poll_catcher_tick(app: &AppHandle, state: &Mutex<CatcherState>) {
    let hwnd = HWND(state.lock().unwrap().hwnd_raw as _);
    if hwnd.0.is_null() {
        return;
    }

    let btn_down = crate::windows::left_mouse_down() || crate::windows::right_mouse_down();
    if !btn_down {
        let was_active = {
            let mut st = state.lock().unwrap();
            let prev = st.in_file_drag;
            st.in_file_drag = false;
            st.rejected_this_press = false;
            prev
        };
        EXTERNAL_DRAG_ACTIVE.store(false, Ordering::SeqCst);
        hide_catcher_hwnd(hwnd);
        if was_active {
            emit_drag(app, "leave", Vec::new());
        }
        return;
    }

    if INTERNAL_DRAG_OUT.load(Ordering::SeqCst) {
        hide_catcher_hwnd(hwnd);
        return;
    }

    {
        let st = state.lock().unwrap();
        if st.rejected_this_press {
            hide_catcher_hwnd(hwnd);
            return;
        }
        if st.in_file_drag {
            let rect = st.current_rect;
            drop(st);
            position_catcher_hwnd(hwnd, rect);
            return;
        }
    }

    let peek_hwnds = crate::windows::peek_hwnds(app);
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    if fg != 0 && peek_hwnds.contains(&fg) {
        hide_catcher_hwnd(hwnd);
        return;
    }

    let mut gti = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(0, &mut gti) }.is_ok() && (gti.flags.0 & GUI_INMOVESIZE.0) != 0 {
        hide_catcher_hwnd(hwnd);
        return;
    }

    let Some((cx, cy)) = crate::windows::cursor_position() else {
        return;
    };

    let mode = *app.state::<crate::island::CurrentMode>().0.lock().unwrap();
    let target_rect = if matches!(mode, Mode::Expanded | Mode::DragTarget) {
        expanded_drop_rect(app)
    } else {
        closed_approach_rect(app)
    };

    let Some((l, t, r, b)) = target_rect else {
        hide_catcher_hwnd(hwnd);
        return;
    };

    if cx >= l && cx <= r && cy >= t && cy <= b {
        state.lock().unwrap().current_rect = (l, t, r, b);
        position_catcher_hwnd(hwnd, (l, t, r, b));
    } else {
        hide_catcher_hwnd(hwnd);
    }
}

/// Start Peek's dedicated pure-Win32 `IDropTarget` catcher thread so Explorer drags onto both
/// the closed notch and open Files panel bypass WebView2's `Intermediate D3D Window`.
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("peek-drop-catcher".into())
        .spawn(move || unsafe {
            let ole_ok = OleInitialize(None).is_ok();
            if !ensure_catcher_class() {
                if ole_ok {
                    OleUninitialize();
                }
                return;
            }
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                CATCHER_CLASS,
                w!("PeekDropCatcher"),
                WS_POPUP,
                0,
                0,
                16,
                16,
                None,
                None,
                Some(hinst.into()),
                None,
            ) else {
                if ole_ok {
                    OleUninitialize();
                }
                return;
            };
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);
            let _ = ShowWindow(hwnd, SW_HIDE);

            let state = std::sync::Arc::new(Mutex::new(CatcherState {
                hwnd_raw: hwnd.0 as isize,
                ..Default::default()
            }));
            let helper: Option<IDropTargetHelper> =
                CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok();
            let drop_target: IDropTarget = DropCatcherTarget {
                app: app.clone(),
                state: state.clone(),
                helper,
            }
            .into();

            if RegisterDragDrop(hwnd, &drop_target).is_err() {
                let _ = DestroyWindow(hwnd);
                if ole_ok {
                    OleUninitialize();
                }
                return;
            }

            let _ = SetTimer(Some(hwnd), TIMER_ID, POLL_INTERVAL_MS, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if msg.message == WM_TIMER && msg.hwnd == hwnd && msg.wParam.0 == TIMER_ID {
                    poll_catcher_tick(&app, &state);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = KillTimer(Some(hwnd), TIMER_ID);
            let _ = RevokeDragDrop(hwnd);
            let _ = DestroyWindow(hwnd);
            if ole_ok {
                OleUninitialize();
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{
        GlobalFree, DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_NOTIMPL, HGLOBAL, S_FALSE,
    };
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET, STGMEDIUM,
        STGMEDIUM_0,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Shell::DROPFILES;
    use windows_core::BOOL;

    #[implement(IDataObject)]
    struct TestHdropDataObject(Vec<String>);

    impl IDataObject_Impl for TestHdropDataObject_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> WinResult<STGMEDIUM> {
            if pformatetcin.is_null() {
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            }
            let fmt = unsafe { &*pformatetcin };
            if fmt.cfFormat != CF_HDROP.0 || (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) == 0 {
                return Err(windows_core::Error::from(DV_E_FORMATETC));
            }
            let mut wide = Vec::<u16>::new();
            for p in &self.0 {
                wide.extend(p.encode_utf16());
                wide.push(0);
            }
            wide.push(0);
            let hdr_size = std::mem::size_of::<DROPFILES>();
            let total = hdr_size + wide.len() * 2;
            unsafe {
                let hmem = GlobalAlloc(GMEM_MOVEABLE, total)?;
                let ptr = GlobalLock(hmem) as *mut u8;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(hmem));
                    return Err(windows_core::Error::from(DV_E_FORMATETC));
                }
                let hdr = DROPFILES {
                    pFiles: hdr_size as u32,
                    pt: POINT { x: 0, y: 0 },
                    fNC: BOOL(0),
                    fWide: BOOL(1),
                };
                std::ptr::copy_nonoverlapping(&hdr as *const _ as *const u8, ptr, hdr_size);
                std::ptr::copy_nonoverlapping(
                    wide.as_ptr() as *const u8,
                    ptr.add(hdr_size),
                    wide.len() * 2,
                );
                let _ = GlobalUnlock(hmem);
                Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 {
                        hGlobal: HGLOBAL(hmem.0),
                    },
                    pUnkForRelease: core::mem::ManuallyDrop::new(None),
                })
            }
        }

        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> WinResult<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn QueryGetData(&self, pformatetc: *const FORMATETC) -> windows_core::HRESULT {
            if pformatetc.is_null() {
                return DV_E_FORMATETC;
            }
            let fmt = unsafe { &*pformatetc };
            if fmt.cfFormat == CF_HDROP.0 && (fmt.tymed & (TYMED_HGLOBAL.0 as u32)) != 0 {
                windows::Win32::Foundation::S_OK
            } else {
                DV_E_FORMATETC
            }
        }

        fn GetCanonicalFormatEtc(
            &self,
            _: *const FORMATETC,
            out: *mut FORMATETC,
        ) -> windows_core::HRESULT {
            if !out.is_null() {
                unsafe { (*out).ptd = std::ptr::null_mut() };
            }
            DATA_S_SAMEFORMATETC
        }

        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> WinResult<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn EnumFormatEtc(&self, dir: u32) -> WinResult<IEnumFORMATETC> {
            let _ = (dir, DATADIR_GET, S_FALSE);
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> WinResult<u32> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn DUnadvise(&self, _: u32) -> WinResult<()> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }

        fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
            Err(windows_core::Error::from(E_NOTIMPL))
        }
    }

    #[implement(IDropTarget)]
    struct DummyDropTarget;

    impl IDropTarget_Impl for DummyDropTarget_Impl {
        fn DragEnter(
            &self,
            _: Ref<IDataObject>,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            eff: *mut DROPEFFECT,
        ) -> WinResult<()> {
            unsafe { *eff = DROPEFFECT_COPY };
            Ok(())
        }
        fn DragOver(
            &self,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            eff: *mut DROPEFFECT,
        ) -> WinResult<()> {
            unsafe { *eff = DROPEFFECT_COPY };
            Ok(())
        }
        fn DragLeave(&self) -> WinResult<()> {
            Ok(())
        }
        fn Drop(
            &self,
            _: Ref<IDataObject>,
            _: MODIFIERKEYS_FLAGS,
            _: &POINTL,
            eff: *mut DROPEFFECT,
        ) -> WinResult<()> {
            unsafe { *eff = DROPEFFECT_COPY };
            Ok(())
        }
    }

    #[test]
    fn extracts_unicode_hdrop_paths_and_registers_drop_target() {
        let obj: IDataObject = TestHdropDataObject(vec![
            r"C:\Temp\demo-one.png".into(),
            r"C:\Temp\folder\two words.txt".into(),
        ])
        .into();
        let extracted = extract_hdrop_paths(&obj);
        assert_eq!(
            extracted,
            vec![
                PathBuf::from(r"C:\Temp\demo-one.png"),
                PathBuf::from(r"C:\Temp\folder\two words.txt"),
            ]
        );

        unsafe {
            let ole_ok = OleInitialize(None).is_ok();
            assert!(ensure_catcher_class());
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                CATCHER_CLASS,
                w!("PeekDropCatcherTest"),
                WS_POPUP,
                0,
                0,
                16,
                16,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("create catcher test hwnd");
            let dt: IDropTarget = DummyDropTarget.into();
            RegisterDragDrop(hwnd, &dt).expect("RegisterDragDrop on pure catcher HWND");
            RevokeDragDrop(hwnd).expect("RevokeDragDrop");
            let _ = DestroyWindow(hwnd);
            if ole_ok {
                OleUninitialize();
            }
        }
    }
}
