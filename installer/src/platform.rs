//! OS마다 값이 다른 "사실"을 한곳에 모은다 — 화면과 설치 로직이 `cfg`를 직접 들고
//! 있지 않게 하기 위한 것이다.
//!
//! **왜 모으는가**: 확장 브릿지를 한때 `cfg(windows)`로 감싸고 포장 스크립트도 갈라두었더니,
//! macOS·Linux 인스톨러가 확장 안내 화면 자체를 조용히 건너뛴 채 몇 달을 돌았다. 분기가
//! 부족해서가 아니라 **분기가 UI·포장까지 흩어져 있어서** 한쪽만 고치고 다른 쪽을 잊은 것이다.
//! 그래서 여기 모으는 기준은 "OS마다 다르고, **어긋나면 조용히 실패하는** 값"이다.

use std::path::{Path, PathBuf};

/// 설치 기본 위치. 그 OS에서 사용자 전용 프로그램이 들어가는 자리다.
pub fn default_install_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("LOCALAPPDATA").unwrap_or_default();
        PathBuf::from(base).join("inno-creed")
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/inno-creed")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join(".local/share/inno-creed")
    }
}

/// payload 안과 설치 위치에서 쓰는 본체 실행 파일 이름.
#[cfg(target_os = "windows")]
pub fn binary_name() -> &'static str {
    "inno-creed.exe"
}

/// payload 안과 설치 위치에서 쓰는 본체 실행 파일 이름.
#[cfg(not(target_os = "windows"))]
pub fn binary_name() -> &'static str {
    "inno-creed"
}

/// 파일을 설치 위치로 복사한 **직후** 그 OS에서 해줘야 하는 일.
///
/// - unix: 실행 권한. 압축 프로그램이 권한 비트를 떨어뜨리는 경우가 있다.
/// - macOS: `com.apple.quarantine` 제거. `std::fs::copy`는 macOS에서 확장속성까지 복사하므로,
///   브라우저로 내려받은 zip에 붙어 있던 격리 딱지가 **설치본에 그대로 따라간다**. 그대로 두면
///   설치 직후 인스톨러가 본체를 실행하는 순간(`--install-extension-host`)에도,
///   나중에 Claude Desktop이 그 본체를 MCP 서버로 띄울 때도 Gatekeeper가 막는다.
///   떼어내는 대상은 **사용자가 방금 실행을 허가한 인스톨러가 스스로 놓은 파일**뿐이다.
/// - Windows: 할 일 없음(SmartScreen은 파일 속성이 아니라 실행 시점 판단이다).
///
/// 실패해도 설치를 막지 않는다 — 권한이나 `xattr` 부재는 설치 자체를 무르게 할 이유가 아니고,
/// 막히면 그 다음 단계(`doctor`)가 사유를 보여준다.
pub fn post_copy(dest: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(dest) {
            let mut perm = meta.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(dest, perm);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(dest)
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = dest;
    }
}

/// Claude Desktop에 **정상 종료**를 요청한다.
///
/// 설정 파일을 쓰기 전에 앱이 꺼져 있어야 하는데, 이 앱은 창을 닫아도 트레이/메뉴바에
/// 남아서 사람들이 "껐다"고 생각한 채로 다음 단계로 온다. 그래서 인스톨러가 대신 눌러준다.
///
/// 강제 종료가 아니라 **종료 요청**이다(macOS는 quit 애플이벤트, Windows는 `WM_CLOSE`,
/// Linux는 `SIGTERM`) — 앱이 스스로 상태를 저장하고 닫을 기회를 준다. 이것으로 안 닫히는
/// 경우에만 `force_quit_claude_desktop`을 쓴다.
pub fn request_quit_claude_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // 다른 앱에 quit을 보내는 것은 **자동화(Apple 이벤트) 권한**이 필요한 일이다. 서명되지 않은
        // 이 프로그램은 첫 시도에서 허용 프롬프트를 받는데, 거기서 "허용 안 함"을 눌렀거나 회사
        // 정책(MDM)이 막아두면 osascript가 **-1743**으로 실패한다. 예전에는 그 실패를 통째로
        // 삼켜서, 사용자는 버튼을 눌러도 화면이 그대로인 이유를 알 수 없었다(실제 문의).
        match std::process::Command::new("/usr/bin/osascript")
            .args(["-e", r#"tell application id "com.anthropic.claudefordesktop" to quit"#])
            .output()
        {
            Ok(o) if o.status.success() => return Ok(()),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                // 애플이벤트가 막혔어도 **신호**는 보낼 수 있다(같은 사용자의 프로세스라 권한이 다르다).
                match signal_claude(&["-x", "Claude"]) {
                    Signaled::Sent => return Ok(()),
                    Signaled::Denied(d) => return Err(denied_hint(&d)),
                    Signaled::Failed(d) => return Err(d),
                    Signaled::NoMatch => {}
                }
                return Err(if err.contains("-1743") {
                    "macOS가 Apple 이벤트 전송을 막았습니다(-1743). [시스템 설정 → 개인정보 보호 및 보안 → 자동화]에서 \
                     이 설치 프로그램에 Claude 제어를 허용하거나, 아래 [강제 종료]를 쓰세요."
                        .to_string()
                } else if err.is_empty() {
                    "종료 요청이 받아들여지지 않았습니다. 아래 [강제 종료]를 쓰세요.".to_string()
                } else {
                    format!("종료 요청이 실패했습니다: {err}")
                });
            }
            Err(e) => return Err(format!("osascript를 실행할 수 없습니다: {e}")),
        }
    }
    #[cfg(target_os = "windows")]
    {
        // /F 없이 = 창에 닫기 요청. 트레이에 남는 구현이면 안 꺼질 수 있어 force가 뒤를 받는다.
        match std::process::Command::new("taskkill").args(["/IM", "Claude.exe"]).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!(
                "종료 요청이 실패했습니다: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Err(format!("taskkill을 실행할 수 없습니다: {e}")),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match signal_claude(&["-x", "Claude"]) {
            Signaled::Sent => Ok(()),
            Signaled::Denied(d) => Err(denied_hint(&d)),
            Signaled::Failed(d) => Err(d),
            Signaled::NoMatch => Err("종료 신호를 받을 프로세스를 찾지 못했습니다.".to_string()),
        }
    }
}

/// `pkill`의 결과. **종료 코드만으로는 판단할 수 없다** — 대상을 찾아 신호를 보냈는데
/// 권한이 없어 거부된 경우에도 pkill은 "못 찾음"과 **똑같이 1**을 돌려주고, 진짜 사유는
/// stderr에만 적는다(실측: `pkill: signalling pid 535: Operation not permitted`).
/// 그래서 exit 코드와 stderr를 함께 본다 — 안 그러면 권한 거부를 "그런 프로세스 없음"으로
/// 잘못 안내하게 된다.
#[cfg(unix)]
enum Signaled {
    /// 하나 이상에 신호가 갔다.
    Sent,
    /// 맞는 프로세스가 없었다.
    NoMatch,
    /// 찾았지만 신호를 거부당했다(다른 사용자 계정 소유, 보호된 프로세스 등).
    Denied(String),
    /// 그 밖의 실패(pkill 자체를 못 돌렸다 등).
    Failed(String),
}

#[cfg(unix)]
fn signal_claude(args: &[&str]) -> Signaled {
    match std::process::Command::new("/usr/bin/pkill").args(args).output() {
        Ok(o) if o.status.success() => Signaled::Sent,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            classify_pkill_stderr(&err)
        }
        Err(e) => Signaled::Failed(format!("pkill을 실행할 수 없습니다: {e}")),
    }
}

/// stderr 한 줄로 실패 종류를 가른다(테스트 가능하도록 분리).
#[cfg(unix)]
fn classify_pkill_stderr(err: &str) -> Signaled {
    if err.is_empty() {
        Signaled::NoMatch
    } else if err.contains("Operation not permitted") || err.contains("not permitted") {
        Signaled::Denied(err.to_string())
    } else {
        Signaled::Failed(err.to_string())
    }
}

/// 신호 거부를 사람이 읽을 안내로. 같은 사용자의 앱은 보통 끌 수 있으므로, 거부됐다면
/// 대개 **다른 로그인 계정**에서 돌고 있는 것이다(사용자 빠른 전환).
#[cfg(unix)]
fn denied_hint(detail: &str) -> String {
    format!(
        "프로세스를 찾았지만 종료 신호가 거부됐습니다({detail}). 다른 사용자 계정에서 Claude Desktop이 \
         실행 중이거나(사용자 빠른 전환), 관리자 권한이 필요한 상태입니다. 그 계정에서 직접 종료해주세요."
    )
}

/// 마지막 수단. 정상 종료 요청이 통하지 않을 때만 쓴다 — 앱이 저장하지 못한 것이 있으면 잃는다.
/// macOS는 헬퍼 프로세스까지 함께 정리한다(메인만 죽이면 `is_claude_desktop_running`이
/// 헬퍼를 보고 계속 "켜져 있음"이라 답한다).
pub fn force_quit_claude_desktop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        match signal_claude(&["-9", "-f", "/Claude.app/Contents/"]) {
            Signaled::Sent => Ok(()),
            Signaled::Denied(d) => Err(denied_hint(&d)),
            Signaled::Failed(d) => Err(d),
            Signaled::NoMatch => Err(
                "강제 종료할 프로세스를 찾지 못했습니다. 터미널에서 `pkill -9 -f \"/Claude.app/Contents/\"`를 \
                 직접 실행하거나, 활성 상태 보기에서 Claude를 종료해주세요."
                    .to_string(),
            ),
        }
    }
    #[cfg(target_os = "windows")]
    {
        match std::process::Command::new("taskkill")
            .args(["/F", "/T", "/IM", "Claude.exe"])
            .output()
        {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!(
                "강제 종료가 실패했습니다: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Err(format!("taskkill을 실행할 수 없습니다: {e}")),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match signal_claude(&["-9", "-x", "Claude"]) {
            Signaled::Sent => Ok(()),
            Signaled::Denied(d) => Err(denied_hint(&d)),
            Signaled::Failed(d) => Err(d),
            Signaled::NoMatch => Err("강제 종료할 프로세스를 찾지 못했습니다.".to_string()),
        }
    }
}

/// 확장 관리 화면 주소는 브라우저마다 다른데, 설치 프로그램은 사용자가 어느 쪽을 쓰는지
/// 알 수 없다. 그래서 열어주는 대신 **어느 주소를 보여줄지**만 사용자가 고른다.
pub struct Browser {
    pub name: &'static str,
    pub url: &'static str,
}

const CHROME: Browser = Browser { name: "Chrome", url: "chrome://extensions" };
// macOS 목록에는 들어가지 않으므로 그 타깃에서는 쓰이지 않는다.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const EDGE: Browser = Browser { name: "Edge", url: "edge://extensions" };

/// 확장을 올릴 수 있는 브라우저 목록.
///
/// ⚠️ **본체의 `src/native_host.rs::manifest_targets()`와 같아야 한다.** native host 매니페스트를
/// 놓지 않는 브라우저를 여기서 권하면, 사용자는 확장을 올렸는데 브릿지는 **에러 없이 조용히**
/// 안 붙는다 — 증상만으로는 원인을 찾을 수 없는 종류의 실패다.
///
/// macOS가 Chrome만인 것은 의도된 것이다: 깔지도 않은 Edge의 지원 폴더가 root 소유로 남아
/// 있어 매니페스트 쓰기가 거부되는 사례를 실제로 만났다(`native_host.rs`의 그 주석 참고).
#[cfg(target_os = "macos")]
pub fn extension_browsers() -> &'static [Browser] {
    &[CHROME]
}

/// 확장을 올릴 수 있는 브라우저 목록. (Windows·Linux는 Chrome·Edge 둘 다 등록된다)
#[cfg(not(target_os = "macos"))]
pub fn extension_browsers() -> &'static [Browser] {
    &[CHROME, EDGE]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_is_absolute_and_named() {
        let dir = default_install_dir();
        assert!(dir.ends_with("inno-creed"), "설치 폴더 이름이 바뀌면 제거 경로가 어긋난다: {dir:?}");
    }

    /// 목록이 비면 확장 안내 화면이 주소를 하나도 못 보여준다.
    #[test]
    fn at_least_one_browser() {
        assert!(!extension_browsers().is_empty());
    }

    /// macOS에서 Edge를 권하면 native host가 없는 자리에 확장을 올리게 된다.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_lists_chrome_only() {
        let names: Vec<_> = extension_browsers().iter().map(|b| b.name).collect();
        assert_eq!(names, ["Chrome"]);
    }

    /// 실측한 pkill 동작: 권한 거부도 exit 1이라 stderr로만 갈린다.
    /// 이걸 놓치면 "프로세스를 찾지 못했다"고 잘못 안내하게 된다.
    #[cfg(unix)]
    #[test]
    fn pkill_permission_denied_is_not_reported_as_missing() {
        assert!(matches!(classify_pkill_stderr(""), Signaled::NoMatch));
        assert!(matches!(
            classify_pkill_stderr("pkill: signalling pid 535: Operation not permitted"),
            Signaled::Denied(_)
        ));
        assert!(matches!(classify_pkill_stderr("pkill: 뭔가 다른 오류"), Signaled::Failed(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn post_copy_strips_quarantine() {
        let dir = std::env::temp_dir().join(format!("platform-qtn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("inno-creed");
        std::fs::write(&f, b"bin").unwrap();
        std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine", "0081;00000000;Chrome;"])
            .arg(&f)
            .status()
            .unwrap();

        post_copy(&f);

        let out = std::process::Command::new("/usr/bin/xattr").arg(&f).output().unwrap();
        let listed = String::from_utf8_lossy(&out.stdout);
        assert!(!listed.contains("com.apple.quarantine"), "격리 딱지가 남아 있다: {listed}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
