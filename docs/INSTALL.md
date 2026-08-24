# 설치 가이드

`inno-creed`는 아마란스(`gw.innogrid.com`) 그룹웨어를 다루는 **MCP 서버**입니다. 단독으로 실행하는 앱이 아니라, **Claude Code 같은 MCP 클라이언트에 등록해서** 대화로 사용합니다.

---

## 0. 전제 조건

- **MCP 클라이언트** — [Claude Code](https://claude.com/claude-code)(권장) 또는 stdio MCP를 지원하는 클라이언트. 이게 없으면 바이너리를 실행해도 아무 일도 안 합니다(입력을 기다리다 종료).
- **로그인된 브라우저** — Chrome 또는 Firefox로 `https://gw.innogrid.com` 에 로그인된 **데스크톱** 환경. (헤드리스 서버·외부인 사용 불가)
- **이노그리드 사내 계정.**

지원 바이너리: **macOS(Apple Silicon)**, **Linux x86_64 / aarch64**, **Windows x86_64**.
> Intel 맥용 바이너리는 제공하지 않습니다(필요하면 [소스 빌드](#부록-소스-빌드)).

---

## 1. 다운로드

[**릴리즈 최신본**](https://github.com/zilhak/inno-creed/releases/latest)에서 OS에 맞는 파일을 받습니다.

| OS / arch | 파일 |
|---|---|
| macOS (Apple Silicon) | `inno-creed-macos-arm64` |
| Linux x86_64 | `inno-creed-linux-x86_64` |
| Linux aarch64 | `inno-creed-linux-aarch64` |
| Windows x86_64 | `inno-creed-windows-x86_64.exe` |

---

## 2. OS별 설치

### macOS (Apple Silicon)

```sh
cd ~/Downloads
chmod +x inno-creed-macos-arm64
xattr -d com.apple.quarantine inno-creed-macos-arm64    # Gatekeeper 차단 해제(미서명 바이너리)
mkdir -p ~/bin && mv inno-creed-macos-arm64 ~/bin/inno-creed
```
> Gatekeeper 경고가 뜨면 위 `xattr` 명령으로 해제하거나, Finder에서 **우클릭 → 열기**를 한 번 해줍니다.

### Linux (x86_64 / aarch64)

```sh
cd ~/Downloads
chmod +x inno-creed-linux-*
mkdir -p ~/bin && mv inno-creed-linux-* ~/bin/inno-creed
```

### Windows (x86_64)

1. `inno-creed-windows-x86_64.exe`를 원하는 폴더로 옮깁니다(예: `C:\Tools\inno-creed.exe`).
2. 처음 실행 시 SmartScreen **"Windows가 PC를 보호했습니다"** 창이 뜨면 → **추가 정보 → 실행**.

---

## 3. MCP 클라이언트에 등록

**Claude Code:**

```sh
claude mcp add inno-creed --scope user -- /절대경로/inno-creed          # Windows: ...\inno-creed.exe
```

> ⚠️ **`--scope`를 꼭 지정하세요.** `claude mcp add`의 기본 스코프는 `local`이라, **등록한 그 디렉토리에서만** 도구가 보입니다. 다른 프로젝트 디렉토리에서 Claude Code를 열면 `inno-creed`가 목록에 없어 등록이 안 된 것처럼 보입니다(실제로는 다른 디렉토리 스코프에 걸려 있는 것). 여러 프로젝트에서 공통으로 쓰려면 위처럼 `--scope user`로 등록하세요. 특정 프로젝트에서만 쓰고 싶다면 그 프로젝트 디렉토리에서 `--scope project`를 쓰세요. 스코프는 `claude mcp list`로 확인할 수 있습니다.

또는 설정 JSON에 직접(`~/.claude.json`의 최상위 `mcpServers`에 넣으면 user 스코프와 동일):

```json
{
  "mcpServers": {
    "inno-creed": {
      "command": "/절대경로/inno-creed"
    }
  }
}
```

등록 후 **클라이언트를 재시작**하면 도구가 노출됩니다.

---

## 4. 로그인 & 확인

1. Chrome 또는 Firefox로 `https://gw.innogrid.com` 에 로그인해 둡니다.
2. (macOS + Chrome) 첫 실행 시 키체인 `Chrome Safe Storage` 접근 허용 프롬프트가 **1회** 뜹니다 → 허용.
3. 정상 기동이면 로그에 이렇게 찍힙니다:
   ```
   [inno-creed] 크레덴셜 취득 완료 (authToken NN자). MCP 서버 시작 (stdio)
   ```
   MCP 클라이언트에서 `list_resources` 같은 도구가 보이면 성공입니다.

---

## 5. 크레덴셜이 안 잡힐 때 (문제 해결)

실행 시 `⚠️ 크레덴셜 미취득`이 뜨면, 에러 메시지에 **Chrome/Firefox 각각 어떤 경로를 확인했는지**가 그대로 표시됩니다. 그걸 보고 아래처럼 처리하세요.

### 자주 걸리는 경우

- **Ubuntu 등에서 Firefox가 snap/flatpak** → 프로필 경로가 표준(`~/.mozilla/firefox`)과 달라 못 찾습니다. 환경변수로 지정:
  ```sh
  # snap Firefox
  export INNO_CREED_FIREFOX_DIR=~/snap/firefox/common/.mozilla/firefox
  # flatpak Firefox
  export INNO_CREED_FIREFOX_DIR=~/.var/app/org.mozilla.firefox/.mozilla/firefox
  ```
- **Windows에서 Chrome 쿠키를 못 읽음(`os error 32` / 파일 사용 중)** → 최신 Chrome은 실행 중 쿠키 파일을 **배타적으로 잠급니다**. **Chrome을 완전히 종료**(트레이·백그라운드 포함)한 뒤 다시 실행하세요. 그래도 불편하면 아래 **크레덴셜 직접 지정** 또는 Firefox를 쓰세요.
- **Chrome은 있는데 "복호화 실패"라고 나옴** →
  - **Linux 키링(gnome-keyring/kwallet, `v11`)**: v1.0.2+는 키링에서 키를 자동 조회하지만 **`secret-tool`이 필요**합니다. 없으면 설치 후 재시도: `sudo apt install libsecret-tools` (데스크톱 세션에서 키링이 잠금 해제돼 있어야 함).
  - **Windows Chrome app-bound(`v20`)**: v1.1.0+는 Chrome Elevator COM으로 복호화를 시도하지만(best-effort), Chrome 버전/보안설정에 따라 거부될 수 있습니다.
  - 그래도 안 되면 **Firefox로 로그인**하거나 아래 **크레덴셜 직접 지정**이 가장 확실합니다.

### 크레덴셜 직접 지정 (브라우저 읽기 우회)

브라우저에서 못 가져오는 환경이면 쿠키 값을 직접 넣습니다(모든 브라우저 읽기보다 우선). 브라우저 DevTools(F12) → **Application → Cookies → `https://gw.innogrid.com`** 에서 두 쿠키 값을 복사:

| 환경변수 | 값 |
|---|---|
| `INNO_CREED_AUTH_TOKEN` | `BIZCUBE_AT` 쿠키 값 (`%7C` 인코딩 그대로 가능) |
| `INNO_CREED_SIGN_KEY` | `BIZCUBE_HK` 쿠키 값 |

두 값 모두 있어야 사용됩니다. MCP로 실행 시 아래 `env` 블록에 넣으세요.

### 경로 오버라이드 환경변수

| 환경변수 | 용도 |
|---|---|
| `INNO_CREED_FIREFOX_COOKIES` | Firefox `cookies.sqlite` 파일 경로(직접) |
| `INNO_CREED_FIREFOX_DIR` | Firefox 프로필 **디렉토리**(스캔) |
| `INNO_CREED_CHROME_COOKIES` | Chrome `Cookies` DB 파일 경로(직접) |
| `INNO_CREED_CHROME_USER_DATA` | Chrome `User Data` 루트 |
| `INNO_CREED_AUTH_TOKEN` | `BIZCUBE_AT` 값 직접 지정(브라우저 우회) |
| `INNO_CREED_SIGN_KEY` | `BIZCUBE_HK` 값 직접 지정(브라우저 우회) |

### ⚠️ MCP 클라이언트로 실행할 땐 `env`에 넣어야 합니다

Claude Code가 서버를 띄우면 셸의 `export`가 전달되지 않습니다. 등록 설정의 `env` 블록에 넣으세요:

```json
{
  "mcpServers": {
    "inno-creed": {
      "command": "/절대경로/inno-creed",
      "env": {
        "INNO_CREED_FIREFOX_DIR": "/home/you/snap/firefox/common/.mozilla/firefox"
      }
    }
  }
}
```

---

## 부록: 소스 빌드

프리빌트가 없는 환경(예: Intel 맥)이나 직접 빌드하려면:

```sh
git clone https://github.com/zilhak/inno-creed && cd inno-creed
cargo build --release        # → target/release/inno-creed (Windows는 inno-creed.exe)
```

**Rust 1.96+**(edition 2024, 번들 `libsqlite3-sys`가 최신 toolchain 요구)와 **C 컴파일러**(rusqlite 번들 SQLite 컴파일용)가 필요합니다.

---

전체 기능·안전 규약·동작 방식은 [README](../README.md)를 참고하세요.
