//! 조용한 실패를 막는다.
//!
//! 이 인스톨러는 GUI 서브시스템(`windows_subsystem = "windows"`)으로 빌드돼 콘솔이
//! 아예 붙지 않는다. 그래서 패닉 메시지도, `main`이 반환한 `Err`도 찍힐 곳이 없다 —
//! 창 생성이나 그래픽 초기화가 실패하면 **창도 메시지도 없이 종료**되고, 사용자에게는
//! "더블클릭했는데 아무 일도 안 일어남"으로만 보인다. PowerShell에서 실행해도 GUI
//! 서브시스템이라 출력이 없어 원인 추적이 통째로 막힌다.
//!
//! 그래서 실패 경로를 화면에 뜨는 창 하나로 모은다. 콘솔 없이도 보이는 채널이라야
//! 하므로 Windows에서는 `MessageBoxW`를 직접 부른다(패닉 훅 안에서도 안전하도록
//! 할당·의존성을 최소로 둔다). 다른 OS에서는 stderr와 rfd 대화상자를 함께 쓴다.

use std::sync::atomic::{AtomicBool, Ordering};

/// 보고 중에 또 실패해 무한 재귀·이중 패닉으로 빠지는 것을 막는다.
/// 첫 번째 실패만 보여주면 충분하다.
static REPORTING: AtomicBool = AtomicBool::new(false);

/// 치명적 오류를 사용자에게 보여준다. 콘솔이 없어도 보이는 것이 유일한 요구사항이다.
pub fn report(body: &str) {
    if REPORTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let body = format!("{body}\n\n{}", environment_report());
    eprintln!("inno-creed 설치 프로그램 오류\n{body}");
    show_dialog("inno-creed 설치 프로그램 오류", &body);
}

/// 패닉도 같은 창으로 보낸다. `main`이 `Err`을 반환하는 경로만 막으면 아이콘 디코드
/// 실패나 폰트 파싱 실패처럼 패닉으로 죽는 경로가 그대로 조용히 남는다.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(알 수 없는 패닉)".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "(위치 불명)".to_owned());
        report(&format!(
            "설치 프로그램이 예기치 않게 중단됐습니다.\n\n\
             내용: {payload}\n\
             위치: {location}"
        ));
    }));
}

/// 문제를 보고받았을 때 되물어야 할 것들을 미리 담아 둔다 — 사용자가 창을 캡처해
/// 보내주면 그것만으로 판별이 되도록.
fn environment_report() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("(확인 실패: {e})"));
    let payload_dir = crate::payload::payload_dir();
    let payload_state = if crate::payload::payload_binary_path().exists() {
        "정상"
    } else {
        "없음 — 압축을 푼 폴더 전체에서 실행했는지 확인하세요"
    };
    let env_of = |k: &str| std::env::var(k).unwrap_or_else(|_| "(미설정)".to_owned());
    format!(
        "── 진단 정보 (이 창에서 Ctrl+C를 누르면 전체가 복사됩니다) ──\n\
         설치 프로그램: inno-creed {} 용 ({} {})\n\
         실행 파일: {exe}\n\
         payload: {} ({payload_state})\n\
         WGPU_BACKEND={} / WGPU_POWER_PREF={}",
        env!("INNO_CREED_RELEASE"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        payload_dir.display(),
        env_of("WGPU_BACKEND"),
        env_of("WGPU_POWER_PREF"),
    )
}

#[cfg(windows)]
fn show_dialog(title: &str, body: &str) {
    // MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST.
    // MessageBox는 자체 모달 루프를 돌기 때문에 이벤트 루프가 뜨기 전이나 패닉 훅
    // 안에서도 동작한다. 또 표시된 상태에서 Ctrl+C를 누르면 내용 전체가 클립보드로
    // 복사된다 — 사용자가 오류를 그대로 붙여넣어 보낼 수 있는 유일한 경로다.
    const FLAGS: u32 = 0x0000_0010 | 0x0001_0000 | 0x0004_0000;
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(body).as_ptr(),
            wide(title).as_ptr(),
            FLAGS,
        );
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, u_type: u32)
    -> i32;
}

#[cfg(not(windows))]
fn show_dialog(title: &str, body: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(body)
        .show();
}
