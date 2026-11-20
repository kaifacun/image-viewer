//! macOS FSEvents 감시 — 내용이 바뀐 **폴더** 경로를 채널로 보낸다 (파일 단위가 아니라 폴더 단위, 1초 모아 보냄).
//!
//! 서명·상수는 SDK `FSEvents.h` 그대로. 콜백은 경로를 걸러 채널에 넣기만 한다 (패닉 금지 — C 경계).

use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use core_foundation_sys::array::{CFArrayCreate, kCFTypeArrayCallBacks};
use core_foundation_sys::base::{CFIndex, CFRelease, kCFAllocatorDefault};
use core_foundation_sys::runloop::{CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRun, kCFRunLoopDefaultMode};
use core_foundation_sys::string::{CFStringCreateWithBytes, CFStringRef, kCFStringEncodingUTF8};

/// 바뀐 폴더 하나
#[derive(Debug, Clone)]
pub struct DirEvent {
    pub dir: PathBuf,
    /// 하위까지 다시 훑어야 함 (MustScanSubDirs · 이벤트 유실)
    pub deep: bool,
}

//#region FFI — FSEvents.h
type FSEventStreamRef = *mut c_void;
type FSEventStreamEventFlags = u32;
type FSEventStreamEventId = u64;
type FSEventStreamCallback = extern "C" fn(FSEventStreamRef, *mut c_void, usize, *mut c_void, *const FSEventStreamEventFlags, *const FSEventStreamEventId);

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

const K_SINCE_NOW: FSEventStreamEventId = 0xFFFF_FFFF_FFFF_FFFF;
const K_CREATE_FLAG_NONE: u32 = 0x0000_0000;
const K_FLAG_MUST_SCAN_SUBDIRS: u32 = 0x0000_0001;
const K_FLAG_USER_DROPPED: u32 = 0x0000_0002;
const K_FLAG_KERNEL_DROPPED: u32 = 0x0000_0004;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *mut FSEventStreamContext,
        paths_to_watch: *const c_void,
        since_when: FSEventStreamEventId,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamScheduleWithRunLoop(stream: FSEventStreamRef, run_loop: CFRunLoopRef, mode: CFStringRef);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
}
//#endregion

struct Ctx {
    tx: Sender<DirEvent>,
    /// 이 접두어 아래 변경은 버린다 (~/Library · 썸네일 캐시 — 잦고 사진 판정과 무관)
    ignore: Vec<Vec<u8>>,
    /// 버리는 접두어 안이라도 살리는 루트
    allow: Vec<Vec<u8>>,
}

extern "C" fn on_events(_s: FSEventStreamRef, info: *mut c_void, n: usize, paths: *mut c_void, flags: *const FSEventStreamEventFlags, _ids: *const FSEventStreamEventId) {
    if info.is_null() || paths.is_null() || flags.is_null() {
        return;
    }
    // SAFETY: info 는 watch() 가 새어 둔 Box<Ctx>, paths 는 char* 배열(UseCFTypes 아님), flags 는 n 개
    let (ctx, paths, flags) = unsafe { (&*(info as *const Ctx), std::slice::from_raw_parts(paths as *const *const i8, n), std::slice::from_raw_parts(flags, n)) };
    for (p, f) in paths.iter().zip(flags) {
        if p.is_null() {
            continue;
        }
        // SAFETY: FSEvents 가 넘긴 NUL 종료 문자열
        let bytes = unsafe { CStr::from_ptr(*p) }.to_bytes();
        let bytes = bytes.strip_suffix(b"/").unwrap_or(bytes);
        // 점 폴더 안 변경은 트리·격자에 안 보이므로 버린다
        // 단, 버리는 곳 안에 따로 감시하는 루트(예: ~/Library 안의 iCloud Drive)는 살린다
        let ignored = ctx.ignore.iter().any(|ig| bytes.starts_with(ig)) && !ctx.allow.iter().any(|a| bytes.starts_with(a));
        if bytes.windows(2).any(|w| w == b"/.") || ignored {
            continue;
        }
        let deep = f & (K_FLAG_MUST_SCAN_SUBDIRS | K_FLAG_USER_DROPPED | K_FLAG_KERNEL_DROPPED) != 0;
        use std::os::unix::ffi::OsStrExt;
        let dir = PathBuf::from(std::ffi::OsStr::from_bytes(bytes));
        let _ = ctx.tx.send(DirEvent { dir, deep });
    }
}

/// 감시 스레드를 띄운다 (앱이 끝날 때까지 산다). 실패하면 false.
pub fn watch(paths: Vec<PathBuf>, ignore: Vec<PathBuf>, tx: Sender<DirEvent>) -> bool {
    if paths.is_empty() {
        return false;
    }
    use std::os::unix::ffi::OsStrExt;
    let ignore: Vec<Vec<u8>> = ignore.iter().map(|p| p.as_os_str().as_bytes().to_vec()).collect();
    let allow: Vec<Vec<u8>> = paths.iter().map(|p| p.as_os_str().as_bytes().to_vec()).filter(|p| ignore.iter().any(|ig| p.starts_with(ig))).collect();
    std::thread::Builder::new()
        .name("iv-scan-watch".into())
        .spawn(move || {
            let ctx = Box::into_raw(Box::new(Ctx { tx, ignore, allow }));
            // SAFETY: CF 객체 생성·해제 짝을 맞추고, ctx 는 스트림 수명(=프로세스) 동안 유지한다
            unsafe {
                let strs: Vec<CFStringRef> = paths
                    .iter()
                    .map(|p| {
                        let b = p.as_os_str().as_bytes();
                        CFStringCreateWithBytes(kCFAllocatorDefault, b.as_ptr(), b.len() as CFIndex, kCFStringEncodingUTF8, 0)
                    })
                    .filter(|s| !s.is_null())
                    .collect();
                let arr = CFArrayCreate(kCFAllocatorDefault, strs.as_ptr() as *const *const c_void, strs.len() as CFIndex, &kCFTypeArrayCallBacks);
                for s in &strs {
                    CFRelease(*s as *const c_void);
                }
                let mut context = FSEventStreamContext { version: 0, info: ctx as *mut c_void, retain: std::ptr::null(), release: std::ptr::null(), copy_description: std::ptr::null() };
                let stream = FSEventStreamCreate(std::ptr::null(), on_events, &mut context, arr as *const c_void, K_SINCE_NOW, 1.0, K_CREATE_FLAG_NONE);
                CFRelease(arr as *const c_void);
                if stream.is_null() {
                    log::warn!("FSEvents 스트림 생성 실패");
                    return;
                }
                FSEventStreamScheduleWithRunLoop(stream, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
                if FSEventStreamStart(stream) == 0 {
                    log::warn!("FSEvents 시작 실패");
                    return;
                }
                CFRunLoopRun();
            }
        })
        .is_ok()
}
