//! Claude Desktop 설정 파일(`claude_desktop_config.json`) 탐지·머지 공유 로직.
//!
//! inno-creed 본체(`doctor`)와, 별도로 배포되는 GUI 인스톨러가 이 크레이트를 함께 써서
//! 경로 탐지·JSON 머지 로직이 두 산출물에서 서로 어긋나지 않게 한다. 인스톨러는
//! inno-creed 실행 파일과 완전히 분리된 별개 바이너리이지만, 이 로직만은 하나를 공유한다.

use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};

/// Claude Desktop config 파일이 있을 만한 경로 후보를 OS별로 훑는다.
///
/// 경로를 하드코딩하지 않는 이유: MSIX(Microsoft Store)판은 패키지별 폴더 이름이
/// 환경마다 달라서(`Claude_<hash>`) 실제로 디렉터리를 뒤져야 하고, Anthropic이 과거
/// 앱 데이터 경로를 한 번 옮긴 전례가 있다 — 고정 경로를 문서/코드에 박아두면 언젠가 틀린다.
#[cfg(target_os = "macos")]
pub fn desktop_config_candidates() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    vec![PathBuf::from(home).join("Library/Application Support/Claude/claude_desktop_config.json")]
}

#[cfg(target_os = "linux")]
pub fn desktop_config_candidates() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    vec![PathBuf::from(home).join(".config/Claude/claude_desktop_config.json")]
}

#[cfg(target_os = "windows")]
pub fn desktop_config_candidates() -> Vec<PathBuf> {
    const LEAF: &str = "Claude\\claude_desktop_config.json";
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(appdata).join(LEAF));
    }
    // MSIX(Microsoft Store)판은 위 경로가 **존재하지 않는다**. 실제 경로는 패키지별
    // LocalCache 아래에 있고 패키지 폴더 이름이 환경마다 다르므로 훑어서 찾는다.
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let packages = PathBuf::from(local).join("Packages");
        if let Ok(entries) = std::fs::read_dir(&packages) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with("Claude_") {
                    out.push(e.path().join("LocalCache\\Roaming").join(LEAF));
                }
            }
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn desktop_config_candidates() -> Vec<PathBuf> {
    Vec::new()
}

/// 파일을 읽어 JSON으로 파싱한다. 파일이 없으면 빈 객체를 돌려준다(신규 등록 대비).
pub fn read_json(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

/// `desktop_config_candidates()`가 내놓은 경로 하나를 실제로 검사한 결과.
/// **파일이 그 자리에 있다는 것과, 그게 진짜 쓸 수 있는 Claude Desktop 설정이라는 것은
/// 다르다** — MSIX 패키지 폴더가 남아있는데 안이 깨져 있거나, 다른 프로그램이 우연히
/// 같은 이름의 파일을 만들어뒀을 수도 있다. 존재 여부만 보고 골랐다가 나중에 설치
/// 단계에서야 파싱 실패로 터지면 원인을 알기 어렵다 — 감지 시점에 미리 알려준다.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigCheck {
    /// 경로 자체가 없다.
    Missing,
    /// 파일은 있는데 JSON으로 못 읽는다(깨졌거나 다른 형식).
    ParseFailed,
    /// JSON은 읽히지만 `preferences`·`coworkUserFilesPath`·`mcpServers` 중 아무것도
    /// 없다 — Claude Desktop이 만든 파일이 아닐 가능성이 있다는 뜻(신뢰도 낮음).
    ParsedButUnfamiliar,
    /// 파싱도 되고 낯익은 키도 있다 — 실제 Claude Desktop 설정으로 볼 수 있다.
    LooksLikeClaudeDesktop,
}

pub fn inspect_config(path: &Path) -> ConfigCheck {
    if !path.exists() {
        return ConfigCheck::Missing;
    }
    match read_json(path) {
        Err(_) => ConfigCheck::ParseFailed,
        Ok(v) => {
            let familiar = ["preferences", "coworkUserFilesPath", "mcpServers"]
                .iter()
                .any(|k| v.get(k).is_some());
            if familiar {
                ConfigCheck::LooksLikeClaudeDesktop
            } else {
                ConfigCheck::ParsedButUnfamiliar
            }
        }
    }
}

/// `mcpServers.inno-creed` 항목을 `{ "command": <설치 경로> }`로 **교체**한다.
///
/// 파일의 나머지는 손대지 않는다 — Claude Desktop이 같은 파일에 `preferences`(UI 상태, 6단계 이상
/// 중첩) 등을 저장하므로, 구조체로 역직렬화했다가 다시 쓰면 모르는 키가 전부 사라진다. 그래서
/// `Value`를 그대로 다룬다.
///
/// ⚠️ 다만 **`inno-creed` 항목 자체는 통째로 갈린다** — 그 항목에 손으로 넣어둔 `env`·`args`는
/// 재설치 때 사라진다. 지금은 이 서버가 그 둘을 쓰지 않아 문제가 없지만, 쓰게 되면 여기서
/// 병합으로 바꿔야 한다.
pub fn merge_inno_creed_entry(root: &mut Value, exe_path: &Path) {
    if !root.is_object() {
        *root = json!({});
    }
    let obj = root.as_object_mut().expect("방금 object로 만듦");
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().expect("방금 object로 만듦").insert(
        "inno-creed".to_string(),
        json!({ "command": exe_path.to_string_lossy() }),
    );
}

/// `mcpServers.inno-creed` 항목만 제거한다. 다른 서버 항목·나머지 키는 그대로 둔다.
pub fn remove_inno_creed_entry(root: &mut Value) {
    if let Some(servers) = root.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        servers.remove("inno-creed");
    }
}

/// 쓰기 전 백업한다. 이미 `.bak`이 있으면 타임스탬프를 붙여 이전 백업을 보존한다.
/// 원본 파일이 없으면(신규 등록) 백업할 것이 없으므로 `Ok(None)`.
pub fn backup(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut bak_name = path.as_os_str().to_os_string();
    bak_name.push(".bak");
    let mut bak = PathBuf::from(bak_name);
    if bak.exists() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut named = path.as_os_str().to_os_string();
        named.push(format!(".bak.{ts}"));
        bak = PathBuf::from(named);
    }
    std::fs::copy(path, &bak)?;
    Ok(Some(bak))
}

/// 임시 파일에 쓰고 rename — 쓰다 중단돼도 원본이 반쪽짜리로 깨지지 않는다.
/// BOM 없는 UTF-8로 기록한다(BOM이 붙으면 일부 JSON 파서가 파일 전체를 거부한다).
pub fn write_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let pretty = serde_json::to_string_pretty(value).expect("Value는 항상 직렬화 가능");
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let leaf = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("claude_desktop_config.json");
    let tmp = dir.join(format!(".{leaf}.tmp"));
    std::fs::write(&tmp, pretty.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Claude Desktop이 실행 중인지 본다.
///
/// **프로세스 "이름"만으로는 절대 판단하지 않는다.** Windows에서는 Claude Desktop과
/// Claude Code CLI 둘 다 실행 파일 이름이 똑같이 `claude.exe`다(개발자 컴퓨터에서 실측
/// 확인 — CLI는 `~\.local\bin\claude.exe`, Desktop MSIX는
/// `...\WindowsApps\Claude_<hash>\app\Claude....exe`). 이름만 보면 CLI를 쓰는 사람은
/// 100%에 가깝게 "Desktop이 켜져 있다"는 오탐을 만난다 — 실행 파일 **경로**로 구분한다.
pub fn is_claude_desktop_running() -> bool {
    !running_claude_desktop_processes().is_empty()
}

/// Claude Desktop으로 판정한 프로세스들. (pid, 실행 파일 경로)
///
/// **왜 목록을 밖으로 내주는가**: "다 껐는데도 켜져 있다고 한다"는 문의가 실제로 왔는데,
/// 화면이 판정 근거를 보여주지 않아 원격에서는 원인을 짚을 수 없었다(메뉴 막대에 남은 본체인지,
/// 죽지 않은 헬퍼인지, 자기를 띄운 부모인지). 판정한 쪽이 근거를 같이 내주면 그 화면 하나로 끝난다.
///
/// 프로세스 목록을 못 읽는 환경(샌드박스에서 `process-info` 차단 등)에서는 **빈 목록**이 되고,
/// 호출부는 "꺼져 있다"로 본다 — 못 본 것을 켜져 있다고 우기지 않는다(실측: `ps`조차 막히면
/// 열거 결과가 0건이 된다).
pub fn running_claude_desktop_processes() -> Vec<(u32, String)> {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    sys.processes()
        .values()
        .filter(|p| is_claude_desktop_process(p))
        .map(|p| {
            (
                p.pid().as_u32(),
                p.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| {
                    p.name().to_string_lossy().into_owned()
                }),
            )
        })
        .collect()
}

fn is_claude_desktop_process(p: &sysinfo::Process) -> bool {
    #[cfg(target_os = "linux")]
    {
        // 리눅스는 배포 위치가 제각각이라 경로 패턴을 하나로 못 잡는다. 대신 CLI는
        // 관례상 소문자 `claude`를 쓰므로, 정확히 대문자로 시작하는 `Claude` 프로세스
        // 이름으로 구분한다(파일시스템이 대소문자를 구분하므로 신뢰할 수 있다).
        return p.name().to_string_lossy() == "Claude";
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Some(exe) = p.exe() else { return false };
        exe_path_looks_like_claude_desktop(&exe.to_string_lossy().to_lowercase())
    }
}

/// 실행 파일 경로(소문자로 정규화됨) 하나만 보고 Claude Desktop인지 판단하는 순수 함수.
/// `sysinfo::Process` 없이도 테스트할 수 있도록 로직을 분리했다.
#[cfg(not(target_os = "linux"))]
fn exe_path_looks_like_claude_desktop(path_lower: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        // MSIX(Microsoft Store), 클래식 설치, 3P(엔터프라이즈) 배포본 각각의 실제 설치 경로.
        path_lower.contains(r"\windowsapps\claude_")
            || path_lower.contains(r"\programs\claude\")
            || path_lower.contains(r"\claude-3p\")
    }
    #[cfg(target_os = "macos")]
    {
        path_lower.contains("/claude.app/")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = path_lower;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("config-kit-inspect-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("claude_desktop_config.json")
    }

    #[test]
    fn inspect_missing_file() {
        let path = temp_file("missing");
        assert_eq!(inspect_config(&path), ConfigCheck::Missing);
    }

    #[test]
    fn inspect_broken_json() {
        let path = temp_file("broken");
        std::fs::write(&path, b"{ not valid json").unwrap();
        assert_eq!(inspect_config(&path), ConfigCheck::ParseFailed);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn inspect_valid_but_unfamiliar_json() {
        let path = temp_file("unfamiliar");
        std::fs::write(&path, b"{\"hello\": \"world\"}").unwrap();
        assert_eq!(inspect_config(&path), ConfigCheck::ParsedButUnfamiliar);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn inspect_real_claude_desktop_shape() {
        let path = temp_file("real");
        std::fs::write(&path, serde_json::to_vec(&epitaxy_fixture()).unwrap()).unwrap();
        assert_eq!(inspect_config(&path), ConfigCheck::LooksLikeClaudeDesktop);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    // 개발자 컴퓨터에서 실측한 실제 경로. Claude Code CLI와 Claude Desktop이 둘 다 켜져
    // 있는 상태에서 `Get-Process | Select ProcessName, Path`로 확인 — 이름은 둘 다
    // "claude"로 완전히 같고, 경로만 다르다. 이걸 구분 못 하면 CLI를 쓰는 사람은 거의
    // 항상 "Desktop이 켜져 있다"는 오탐을 만난다(설치 마법사가 못 넘어감).
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn cli_binary_path_is_not_desktop() {
        let cli = r"c:\users\zilha\.local\bin\claude.exe".to_lowercase();
        assert!(!exe_path_looks_like_claude_desktop(&cli));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn msix_desktop_path_is_detected() {
        let msix = r"c:\program files\windowsapps\claude_1.40609.0.0_x64__pzs8sxrjxfjjc\app\claude.exe"
            .to_lowercase();
        assert!(exe_path_looks_like_claude_desktop(&msix));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn classic_install_path_is_detected() {
        let classic = r"c:\users\zilha\appdata\local\programs\claude\claude.exe".to_lowercase();
        assert!(exe_path_looks_like_claude_desktop(&classic));
    }

    // INSTALLER_TODO.md에 남은 실제 관측값(등록 전 claude_desktop_config.json) — 이 구조가
    // 머지 후에도 바이트 단위로 무손상이어야 한다.
    fn epitaxy_fixture() -> Value {
        json!({
            "coworkUserFilesPath": "C:\\Users\\zilha\\Claude",
            "preferences": {
                "coworkBrowserToolsEnabled": true,
                "remoteToolsDeviceName": "jaehak",
                "epitaxyPrefs": {
                    "desktop-frame.paneStore.v1": {
                        "state": { "extraPanesByMode": {}, "rowSplit": 0.5 },
                        "version": 4
                    }
                }
            }
        })
    }

    #[test]
    fn merge_preserves_unrelated_deep_nesting() {
        let mut v = epitaxy_fixture();
        merge_inno_creed_entry(&mut v, Path::new("/opt/inno-creed"));
        assert_eq!(v["mcpServers"]["inno-creed"]["command"], "/opt/inno-creed");
        assert_eq!(v["preferences"]["epitaxyPrefs"], epitaxy_fixture()["preferences"]["epitaxyPrefs"]);
        assert_eq!(v["coworkUserFilesPath"], "C:\\Users\\zilha\\Claude");
    }

    #[test]
    fn merge_creates_mcp_servers_when_absent() {
        let mut v = json!({});
        merge_inno_creed_entry(&mut v, Path::new("/opt/inno-creed"));
        assert_eq!(v["mcpServers"]["inno-creed"]["command"], "/opt/inno-creed");
    }

    #[test]
    fn merge_preserves_other_server_entries() {
        let mut v = json!({ "mcpServers": { "other-tool": { "command": "/bin/other" } } });
        merge_inno_creed_entry(&mut v, Path::new("/opt/inno-creed"));
        assert_eq!(v["mcpServers"]["other-tool"]["command"], "/bin/other");
        assert_eq!(v["mcpServers"]["inno-creed"]["command"], "/opt/inno-creed");
    }

    #[test]
    fn unregister_removes_only_inno_creed() {
        let mut v = json!({
            "mcpServers": {
                "other-tool": { "command": "/bin/other" },
                "inno-creed": { "command": "/opt/inno-creed" }
            },
            "preferences": { "x": 1 }
        });
        remove_inno_creed_entry(&mut v);
        assert!(v["mcpServers"].get("inno-creed").is_none());
        assert_eq!(v["mcpServers"]["other-tool"]["command"], "/bin/other");
        assert_eq!(v["preferences"]["x"], 1);
    }

    #[test]
    fn unregister_on_missing_mcp_servers_is_noop() {
        let mut v = json!({ "preferences": { "x": 1 } });
        remove_inno_creed_entry(&mut v);
        assert_eq!(v["preferences"]["x"], 1);
    }

    #[test]
    fn backup_returns_none_when_file_absent() {
        let dir = std::env::temp_dir().join(format!("config-kit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude_desktop_config.json");
        assert!(backup(&path).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_atomic_then_read_json_roundtrip() {
        let dir = std::env::temp_dir().join(format!("config-kit-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude_desktop_config.json");
        let mut v = epitaxy_fixture();
        merge_inno_creed_entry(&mut v, Path::new("/opt/inno-creed"));
        write_atomic(&path, &v).unwrap();
        let back = read_json(&path).unwrap();
        assert_eq!(back, v);
        std::fs::remove_dir_all(&dir).ok();
    }
}
