<p align="center">
  <img src="assets/inno-creed-v2.jpg" width="400" alt="Inno Creed — 이노그리드 아마란스 MCP">
</p>

<h1 align="center">inno-creed</h1>

<p align="center">
  아마란스(<code>gw.innogrid.com</code>, 더존 WEHAGO/BIZCUBE 그룹웨어) 내부 API를 직접 호출하는 <b>Rust MCP 서버</b>
</p>

---

## 무엇인가

그룹웨어를 **브라우저 없이** 다루는 MCP 서버입니다. 회의실 예약, 일정, 메일, 게시판, 전자결재, 근태, 조직도를 Claude 같은 MCP 클라이언트에서 대화로 처리할 수 있습니다.

- **헤드리스** — Playwright 같은 브라우저 자동화 없이 순수 HTTP + HMAC 서명(`wehago-sign`)으로 호출합니다. 서버가 `Origin`/`Referer`를 검사하지 않고 쿠키도 필수가 아니라, 헤더 4종만 맞추면 인증이 통과합니다.
- **로그인 불필요** — 아이디/비밀번호를 요구하지 않습니다. 이미 브라우저에 로그인돼 있으면 그 쿠키를 복호화해 크레덴셜(`authToken` / `signKey`)만 가져옵니다.
- **안전 규약 내장** — 모든 쓰기 작업은 read-back으로 실제 반영을 검증하고, 남의 데이터 수정은 사전에 차단합니다(아래 [안전 규약](#안전-규약)).

## 할 수 있는 것

**통합검색**
| 도구 | 기능 |
|---|---|
| `search` | 메일·전자결재·게시판·일정·자원·파일을 한 번에 검색(기간 필터, 모듈별 범위 지정) |

> 결과에 후속 조회용 ID가 함께 옵니다 — 메일 `muid` → `read_mail`, 결재 `docId`+`formId` → `read_approval`, 게시판 `artSeqNo` → `read_notice`. "지난달 그 메일 찾아서 본문 보여줘" 같은 요청이 한 흐름으로 이어집니다.

**회의실(자원)**
| 도구 | 기능 |
|---|---|
| `find_free_rooms` | **빈 시간 탐색** — 날짜·필요시간·구간·건물을 주면 예약을 뺀 가용 구간만 반환. 점심시간 제외(`include_lunch:true`로 포함) |
| `list_resources` | 회의실(자원) 목록 |
| `list_reservations` | 기간·자원별 예약 현황(자원 미지정 시 전체). 기본 슬림 응답, 원본은 `verbose:true` |
| `my_reservations` | 본인 예약만 — 수정·취소에 필요한 `seqNum`/`resIdx`를 얻는 경로 |
| `reserve_resource` | 회의실 예약 — 점심시간이 걸치면 `lunchWarning` 동반 |
| `update_reservation` | 예약 수정(본인 소유만) |
| `cancel_reservation` | 예약 취소(본인 소유만) |

> `find_free_rooms`는 종일·다일 예약(예: 반년짜리 공용좌석)도 해당일 전체 점유로 처리합니다. `group="본사"|"구로"`로 건물을 나눠 볼 수 있습니다.
> **점심시간(13:00~14:00)은 예약과 동일하게 점유로 처리해 빈 구간에서 뺍니다.** 점심시간에도 찾아야 하면 `include_lunch:true`, 예약·수정 시 그 시간이 걸치면 응답에 `lunchWarning`이 실립니다(막지는 않습니다).
> **예약명(`reqText`) ≠ 화면 표시명(`displayTitle`)** — 아마란스 타임라인은 예약명이 아니라 `[예약자] 자원명`을 보여줍니다. 두 값을 함께 반환하니 "예약명이 다르게 보인다"는 오해는 이걸로 확인하세요.
> ⚠️ 회의실 **정원(수용인원) 정보는 아마란스에 존재하지 않습니다** — "10명 회의실" 류 조건은 지원할 수 없습니다.

**일정(캘린더)**
| 도구 | 기능 |
|---|---|
| `list_calendars` / `list_events` | 캘린더 목록 / 기간 일정 조회 |
| `create_calendar_event` | 일정 등록(참여자·화상회의·비밀메모 지정 가능) |
| `update_calendar_event` | 일정 제목·내용·시간 수정(본인 작성만) |
| `delete_calendar_event` | 일정 삭제(본인 작성만, 소프트 삭제) |

**메일**
| 도구 | 기능 |
|---|---|
| `list_mailboxes` / `list_mail_inbox` | 메일함 목록 / 받은메일 |
| `read_mail` | 메일 1건 본문(평문)·헤더·첨부목록 — 외부 이미지 자동로드 안 함 · ⚠️ **읽음 처리됨**(`mark_mail_unread`로 되돌림) |
| `send_mail` | 메일 발송(첨부 지원, 받는사람 미지정 시 본인에게) |
| `save_mail_draft` | 임시보관함에 저장만(발송 안 함, 첨부 지원) |
| `list_mail_drafts` | 임시보관함 조회 — 저장한 초안 확인, 항목의 `muid`가 후속 조회 키 |
| `send_mail_from_draft` | 임시보관 초안을 발송(첨부 승계 포함) — ⚠️ 실제 발송, 성공 시 원본 삭제. 실재 확인이 최근 20건만 훑는다 |
| `delete_mail` | 메일 삭제(휴지통 이동) |
| `download_mail_attachment` | 첨부파일 저장(실행 없이 저장만) |
| `mark_mail_unread` | **읽지 않음으로 되돌리기** — `read_mail`이 세운 읽음 플래그 해제(반영은 재조회로 검증, 이미 미읽음이면 `already`) |
| `mailbox_counts` | 메일함별 미읽음·전체 + 계정 집계(`unreadCount`·`toMeCount`·`flaggedCount`·`attachCount`) |

**게시판**
| 도구 | 기능 |
|---|---|
| `list_notices` | 공지/게시글 목록(본문 프리뷰, 검색어·기간 필터) |
| `read_notice` | 게시글 1건 본문(평문)·댓글 — 이미지는 `[이미지]`+`images[]` · ⚠️ 조회수 증가 |
| `list_notice_attachments` / `download_notice_attachment` | 게시글 첨부 목록 / 다운로드 |
| `download_body_image` | **본문 삽입 이미지** 다운로드(게시판·메일 공용) — 정식 첨부와 별개 경로 |

**전자결재**
| 도구 | 기능 |
|---|---|
| `pending_approvals` | **미결 요약** — 제목·기안자·대기일수(오래 기다린 순) |
| `list_approvals` | 함별 문서 목록(미결/기결/수신참조/시행/상신) |
| `read_approval` | 문서 1건 본문(평문)·헤더·결재선 (열람 부작용 없음) |
| `approval_counts` | 함별 미처리 건수(숫자만 — 내용까지 보려면 `pending_approvals`) |
| `submit_approval` | 문서 상신 — ⚠️ 실제 결재요청 통지 발송 |
| `cancel_approval` | 상신 취소 — 상태별 3단계(결재취소→상신취소→`purge` 시 삭제) |
| `delete_temp_approval` | 임시보관 문서 삭제 — 상신취소로 되돌아온 문서·시험 잔여물 정리용 |
| `list_approval_lines` / `read_approval_line` | 개인결재라인 목록 / 결재자 구성 조회 |
| `save_approval_line` / `delete_approval_line` | 개인결재라인 생성·수정 / 삭제 (상신 아님, 재사용 config) |
| `suggest_approval_line` | **결재선 후보 제안** — 본인 직책으로 구간 판정 + 직책→사람 해석 (⛔ 확정 아님, 사용자 확인 필수) |
| `get_approval_line_schema` / `list_approval_line_schemas` | 문서 종류별 결재라인 스키마(직책 기반) 원본 |
| `get_approval_submission_guide` / `list_approval_submission_guides` | 양식별 신청 가이드(필수항목·절차·주의) |

**근태 · 조직 · 나**
| 도구 | 기능 |
|---|---|
| `get_attendance_today` | 오늘 출퇴근 현황(부작용 없음) |
| `attendance_month` | **기간(월) 근태** — 일자별 출퇴근·근무시간·지각/연차 + 기간 합계 |
| `attendance_clock_in` / `attendance_clock_out` | 출근·퇴근 기록 — ⚠️ 실제 근태 punch, 기존 기록은 덮어쓰지 않음 |
| `find_person` | **사람 찾기** — 이름·ID·이메일 → `empSeq`/부서/직책/연락처 |
| `org_chart` | 부서 트리 / 부서별 사원·직책 |
| `person_group` · `save_person_group` · `delete_person_group` | 사람 그룹 — 아마란스에 없는 그룹메일을 대신한다. 메일 수신자·참조, 일정 참여자에 재사용 |
| `whoami` | 로그인한 본인 정보(`empSeq`·부서·이메일 + 근태용 `empCd` + 부서명·직책·직급) |

> 결재선 구성·회의 참석자·메일 수신자는 전부 `empSeq`를 요구합니다. `find_person`이 그 진입점이고, 본인 값은 `whoami`로 얻습니다.
> `find_person`의 첫 호출은 전사 명부를 조립하느라 1초 남짓 걸리고, 이후 30분간 캐시됩니다.

## 요구 사항

- **macOS · Linux · Windows** — 크레덴셜(`authToken`/`signKey`)을 가져오는 경로가 두 가지입니다: **Chrome/Edge 확장 프로그램**(권장) 또는 **브라우저 쿠키 직접 읽기**(폴백, OS마다 방식이 다름).
- **Chrome/Edge 또는 Firefox로 `https://gw.innogrid.com` 에 로그인된 상태** — 세션이 없으면 도구 호출 시 로그인 안내를 반환합니다.

### 권장 경로: Chrome/Edge 확장 프로그램

`gw.innogrid.com`의 로그인 쿠키(`BIZCUBE_AT`/`BIZCUBE_HK`)는 **세션 쿠키**라 브라우저가 켜져 있는 동안만 존재하고, Windows에서는 추가로 파일 잠금·`v20` app-bound 암호화까지 겹칩니다. 쿠키 DB 파일을 직접 읽는 방식은 이 조합을 다 통과해야 하는 데다, 아래([DBSC](#️-쿠키-db-직접-읽기는-점점-막히는-경로입니다-dbsc)) 이유로 점점 더 막히는 추세입니다. 확장 프로그램은 브라우저가 공식으로 열어준 `cookies` API로 평문 값을 바로 읽어 Native Messaging(로컬 프로세스 스폰 + stdio, 소켓 불필요)으로 inno-creed에 전달하므로 이 문제들을 전부 우회합니다.

```sh
inno-creed --install-extension-host   # native messaging host 등록(최초 1회)
```

이후 [릴리즈](https://github.com/zilhak/inno-creed/releases/latest)의 **`inno-creed-extension.zip`**을 받아 풀고, `chrome://extensions`(또는 `edge://extensions`) → **개발자 모드** 켜기 → **압축해제된 확장 프로그램 로드** → 푼 폴더 선택(저장소를 clone했다면 `extension/` 폴더를 그대로 써도 같습니다). 로드 시점에 이미 로그인돼 있으면 즉시, 이후로는 로그인·로그아웃할 때마다 자동으로 동기화됩니다. 자세한 절차는 [`docs/INSTALL.md`](docs/INSTALL.md) 참고.

### ⚠️ 쿠키 DB 직접 읽기는 점점 막히는 경로입니다 (DBSC)

Chrome은 **Device Bound Session Credentials(DBSC)**를 2026년 4월(Chrome 146, Windows) GA로 켜서 세션 쿠키를 기기에 암호학적으로 묶어, 브라우저 프로세스 밖에서 파일·COM으로 훔쳐 쓰는 걸 막고 있습니다(관리자 설정으로도 못 끔). Edge도 같은 Chromium 기반이라 뒤따를 걸로 보입니다(2025년 10월 Origin Trial 종료, GA는 아직 미발표). **지금은 운 좋게 되더라도, 쿠키 DB 직접 복호화 경로는 가까운 미래에 완전히 막힐 걸 전제로 쓰세요.** 확장 프로그램 경로는 DBSC와 무관합니다 — 브라우저 자신의 공식 `cookies` API를 그대로 쓰므로, DBSC가 막으려는 "브라우저 밖에서 훔쳐 쓰기"에 애초에 해당하지 않습니다.

**Firefox는 DBSC를 공식적으로 도입하지 않기로 했습니다** — Mozilla `standards-positions` 저장소에 `position: negative`로 명시돼 있고, 이유는 (1) 훔친 쿠키가 재인증 전까지는 여전히 쓸 수 있는 창이 남는다, (2) 애드혹 재인증 프로토콜이 기존 쿠키 관리 방식과 안 맞는다, (3) 향후 하드웨어 attestation 요구로 이어질 수 있다는 것입니다. 다만 이건 "DBSC로 안 막힌다"일 뿐, `gw.innogrid.com`이 되는 건 별개입니다 — `BIZCUBE_AT`/`HK`는 세션 쿠키라 **Firefox도 브라우저가 켜져 있는 동안은 `cookies.sqlite`에 아예 쓰지 않는다**는 걸 실측으로 확인했습니다(WAL 파일까지 포함해 라이브로 직접 읽어도 로그인 상태의 쿠키가 DB에 없음). 즉 Firefox는 DBSC와 무관한 이유로 이 사이트에서는 파일 기반 읽기가 원래 안 됩니다.

### 플랫폼별 크레덴셜 지원

| | macOS | Linux | Windows |
|---|---|---|---|
| **Chrome/Edge 확장 프로그램(권장)** | 미구현(`--install-extension-host` Windows 전용) | 〃 | ✅ |
| **Chrome 쿠키 직접 읽기** | 키체인 `Chrome Safe Storage` → AES-128-CBC | 키링(`v11`, `secret-tool`) / `"peanuts"`(`v10`) → AES-128-CBC | `v10` DPAPI만 됨. `v20` app-bound는 경로 검증 때문에 제3자 프로세스로는 **설계상 항상 거부** |
| **Edge 쿠키 직접 읽기** | — | — | Chrome과 동일(`v20`은 항상 거부) |
| **Firefox 쿠키 직접 읽기** | ✅ (쿠키 평문) | ✅ | **미지원** — 이 사이트의 세션 쿠키를 실행 중엔 디스크에 안 써서 시도 자체를 안 함 |

- **Windows에서 가장 확실한 경로는 Chrome/Edge 확장 프로그램**입니다. 세션쿠키·파일잠금·`v20` 암호화 문제를 전부 우회합니다.
- **macOS/Linux는 아직 확장 프로그램 자동 등록을 안 만들었습니다**(코드 자체는 크로스플랫폼이지만 `--install-extension-host`가 Windows 레지스트리만 건드림) — 그쪽은 브라우저가 이 정도로 강하게 잠그지 않아 지금은 쿠키 직접 읽기로도 잘 됩니다.
- **Windows Firefox는 지원하지 않기로 확정했습니다** — 파일 기반 읽기가 원천적으로 안 되고(위 DBSC 섹션), Firefox 확장 프로그램은 Mozilla AMO 서명 없이는 Chrome/Edge처럼 "압축해제 로드"로 못 깔아서 손쉬운 우회책도 없습니다. Windows는 Chrome/Edge 확장 프로그램을 쓰세요.
- **어떤 경로로도 못 가져오면** 값을 직접 지정할 수 있습니다(아래 [크레덴셜 직접 지정](#크레덴셜-직접-지정-수동)).

### 브라우저 경로 오버라이드 (snap/flatpak/커스텀 프로필)

브라우저가 표준 위치에 없으면(예: Ubuntu의 **snap Firefox** → `~/snap/firefox/common/.mozilla/firefox`) 환경변수로 직접 지정합니다:

| 환경변수 | 용도 |
|---|---|
| `INNO_CREED_EXTENSION_CACHE` | 확장 프로그램 캐시 파일 경로(직접) — 기본값은 OS별 표준 로컬 데이터 디렉토리 |
| `INNO_CREED_FIREFOX_COOKIES` | Firefox `cookies.sqlite` 파일 경로(직접) |
| `INNO_CREED_FIREFOX_DIR` | Firefox 프로필 **디렉토리**(스캔) |
| `INNO_CREED_CHROME_COOKIES` | Chrome `Cookies` DB 파일 경로(직접) |
| `INNO_CREED_CHROME_USER_DATA` | Chrome `User Data` 루트 |
| `INNO_CREED_EDGE_COOKIES` | Edge `Cookies` DB 파일 경로(직접, Windows 전용) |
| `INNO_CREED_EDGE_USER_DATA` | Edge `User Data` 루트(Windows 전용) |

크레덴셜 취득에 실패하면 에러 메시지에 **어느 소스가 왜 막혔는지**가 처방과 함께 표시됩니다. 한 화면으로 보려면:

```sh
inno-creed doctor
```

크레덴셜 소스별 결과, 익스텐션 브릿지·크레덴셜 파일·Claude Desktop 설정 파일의 실제 위치, 그리고 **실제로 인증이 되는지**(gw에 1회 요청)까지 확인합니다. 토큰 값은 출력하지 않습니다.

### 크레덴셜 직접 지정 (수동)

확장 프로그램도 브라우저 읽기도 안 되는 환경(**세션 쿠키**라 디스크에 안 남는 경우, **Windows Chrome/Edge app-bound**, 개발자 모드가 막혀 확장을 못 까는 경우)의 **최후 수단**입니다. DevTools → Application → Cookies → `gw.innogrid.com`에서 `BIZCUBE_AT`·`BIZCUBE_HK`를 복사해:

```sh
inno-creed auth set      # 두 값을 물어보고 ~/.config/inno-creed/creds.json에 저장(unix는 0600)
inno-creed auth clear    # 저장된 값 삭제
```

인자가 아니라 **stdin으로만** 받습니다 — 인자로 주면 셸 히스토리와 프로세스 목록에 세션 토큰이 남습니다.

환경변수로도 됩니다(둘 **모두** 설정해야 사용):

| 환경변수 | 값 |
|---|---|
| `INNO_CREED_AUTH_TOKEN` | `BIZCUBE_AT` 쿠키 값 (URL 인코딩된 `%7C`도 그대로 붙여넣기 가능) |
| `INNO_CREED_SIGN_KEY` | `BIZCUBE_HK` 쿠키 값 |

MCP 클라이언트로 실행할 땐 등록 설정의 `env` 블록에 넣으세요(셸 `export`는 전달되지 않음).

**취득 순서는 `환경변수` → `익스텐션 캐시` → `Chrome` → `Edge`(Windows) → `Firefox`(비-Windows) → `크레덴셜 파일`입니다.** 파일이 맨 아래인 것은 의도적입니다 — 위에 두면 만료된 파일 하나가 멀쩡한 브라우저 세션을 영영 가립니다. 반대로 `env`는 파일보다 위라, 둘을 같이 두면 `auth set`으로 새로 저장해도 안 먹습니다(`doctor`가 이 조합을 경고합니다).

## 설치

> 📖 **처음이거나 남에게 공유한다면 → [단계별 설치 가이드 `docs/INSTALL.md`](docs/INSTALL.md)** (OS별 절차 · Gatekeeper/SmartScreen 우회 · 문제 해결 포함). 아래는 요약입니다.

### GUI 인스톨러 (비개발자 권장)

Claude Desktop(채팅·Cowork·Code 탭)에서 쓸 거라면, JSON을 직접 안 만져도 되는 GUI 인스톨러를 받으세요.

| OS / arch | 파일 |
|---|---|
| macOS (Apple Silicon) | `inno-creed-installer-macos-arm64.zip` |
| Linux x86_64 | `inno-creed-installer-linux-x86_64.zip` |
| Linux aarch64 | `inno-creed-installer-linux-aarch64.zip` |
| Windows x86_64 | `inno-creed-installer-windows-x86_64.zip` |

압축을 풀면 나오는 `installer`(Windows는 `installer.exe`)를 실행하세요. **`installer`와 `payload/` 폴더를 같은 자리에 둔 채로 실행해야 합니다** — `installer`만 따로 옮기면 설치할 파일을 못 찾습니다. 자세한 화면별 안내는 [`docs/INSTALL.md`](docs/INSTALL.md) 참고. Claude Code CLI 전용으로만 쓸 거라면 아래 프리빌트 바이너리 방식이 더 간단합니다.

### 프리빌트 바이너리

[**릴리즈**](https://github.com/zilhak/inno-creed/releases/latest)에서 OS에 맞는 바이너리를 내려받으세요.

| OS / arch | 파일 |
|---|---|
| macOS (Apple Silicon) | `inno-creed-macos-arm64` |
| Linux x86_64 | `inno-creed-linux-x86_64` |
| Linux aarch64 | `inno-creed-linux-aarch64` |
| Windows x86_64 | `inno-creed-windows-x86_64.exe` |
| **(Windows 권장) 확장 프로그램** | `inno-creed-extension.zip` |

macOS·Linux는 내려받은 뒤 실행 권한을 부여하세요: `chmod +x inno-creed-*`. (macOS에서 Gatekeeper가 막으면 `xattr -d com.apple.quarantine <파일>`.)

### 소스 빌드

```sh
git clone https://github.com/zilhak/inno-creed && cd inno-creed
cargo build --release   # → target/release/inno-creed (Windows는 inno-creed.exe)
```

**Rust 1.96+** (edition 2024, 번들 `libsqlite3-sys`가 최신 toolchain 요구)와 **C 컴파일러**(rusqlite 번들 SQLite 컴파일용)가 필요합니다.

**커밋 전에는 다음 두 명령을 돌립니다.** CI가 없어 사람이 놓치면 그대로 쌓입니다. `crates/config-kit`·`installer`도 포함하려면 `--workspace`가 필요합니다.

```sh
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

현재 기준선은 **테스트 전건 통과**, **clippy 경고 9건**입니다(2026-08-29 실측). 내역은 `src/native_host.rs`의 dead-code 1건과 `installer/`의 8건(`copy_installer_self` dead-code 1 + `collapsible_if` 6 + `trim_split_whitespace` 1)이고, 전부 이 크레이트들이 들어올 때부터 있던 것입니다. 여기서 **늘어나면** 그 변경이 원인입니다.

> `-D warnings`를 붙였으니 이 상태에서는 clippy가 실패로 끝납니다. 경고를 새로 만들지 않았는지 보려면 개수를 위 기준선과 비교하세요.

### MCP 등록

Claude Code:

```sh
claude mcp add inno-creed -- /절대경로/inno-creed        # Windows는 ...\inno-creed.exe
```

다른 MCP 클라이언트는 stdio 전송으로 바이너리를 직접 실행하도록 설정하면 됩니다.

```json
{
  "mcpServers": {
    "inno-creed": {
      "command": "/절대경로/inno-creed"
    }
  }
}
```

등록 후 클라이언트를 재시작하면 도구가 노출됩니다. macOS에서 Chrome 크레덴셜을 쓸 경우 첫 실행 시 키체인(`Chrome Safe Storage`) 접근 허용 프롬프트가 한 번 뜹니다.

**막히면 `inno-creed doctor`를 먼저 실행하세요.** 어느 크레덴셜 소스에서 왜 막혔는지, 설정 파일이 실제로 어디 있는지, 그리고 **실제로 인증이 되는지**까지 한 화면에 보여줍니다(토큰 값은 출력하지 않습니다). 도구 목록이 뜨는 것과 인증 성공은 별개입니다 — 서버는 크레덴셜이 없어도 기동합니다.

> 🤖 **사용자를 대신해 이 설치를 진행하는 에이전트라면**: `claude mcp add`가 성공했다고 설치가 끝난 게 아닙니다. **`inno-creed doctor`를 반드시 실행해 인증(`✅ 인증 성공`)까지 확인**하세요. 실패했고 환경이 **Windows**라면, 이 서버는 브라우저 쿠키를 코드가 직접 읽을 수 없는 구조라 **거의 항상 [Chrome/Edge 확장 프로그램](docs/INSTALL.md#4-크레덴셜-연결--chromeedge-확장-프로그램-windows-권장) 설치가 필요합니다** — 이 단계는 브라우저 GUI 조작이라 에이전트가 대신 해줄 수 없으니, 사용자에게 그 절차를 안내하세요.

> **HTTP 전송은 정식 지원하지 않습니다.** 이 서버는 로그인을 받지 않고 **서버가 도는 머신의 브라우저 쿠키**로 동작하므로, 포트를 여는 순간 거기 닿는 누구나 당신 이름으로 결재를 상신하고(`submit_approval` — 결재선에 실제 알림이 갑니다) 메일을 보내고 근태를 찍을 수 있습니다. 그래서 배포 바이너리에 넣지 않았습니다.
> stdio를 쓸 수 없는 **로컬** 클라이언트 때문에 꼭 필요하다면, 직접 빌드하는 절차를 [`docs/HTTP.md`](docs/HTTP.md)에 적어두었습니다 — 기존 코드 수정 없이 의존성 2줄과 바이너리 1개면 됩니다. 원격 노출·공용 서버 상주는 하지 마세요.

## 동작 방식

```
inno-creed (Rust MCP 서버, 헤드리스)
 ├─ creds    환경변수 → 확장 프로그램 캐시(권장) → Chrome → Edge(Win) → Firefox(비-Win)
 │           → 크레덴셜 파일 → authToken / signKey. 소스별 실패 사유는 diagnose()가 한 곳에서 만든다
 ├─ doctor   `inno-creed doctor` — 위 진단 + 설정 파일 탐색 + 실제 인증 왕복 1회
 ├─ native_host  Chrome/Edge 확장 프로그램(`extension/`)의 Native Messaging 수신 — 1회성
 ├─ sign     wehago-sign(HMAC-SHA256) · transaction-id 생성
 ├─ util     도메인 무관 순수 함수(날짜 변환 · JSON 필드 추출)
 ├─ client   세션 lazy 취득(10분 TTL 캐시) · 헤더 주입 · POST · 응답 파싱
 ├─ modules  자원 · 일정 · 메일 · 게시판 · 전자결재 · 근태 · 조직
 │           API 래퍼 + 파생 조회 + 소유권 가드 · read-back 검증
 └─ mcp      rmcp stdio 서버 — tools/(도구 55개, 도메인별) · args/(인자 스키마) · 에러 변환
```

크레덴셜만 브라우저에서 빌려오고, 실행은 전부 순수 HTTP입니다. 서명·세션 규격은 [architecture.md](docs/architecture.md)에 정리돼 있습니다.

## 안전 규약

- **응답 성공 ≠ 실제 반영** — 서버는 권한 밖 대상에 대해 `successTf:true`를 주면서 실제로는 무시(silent no-op)합니다. 그래서 모든 mutation은 직후 **재조회(read-back)로 실제 상태를 확인**하고, 반영되지 않았으면 실패로 처리합니다.
- **소유권 가드** — 쓰기 도구는 대상의 소유자(예약은 `empSeq`, 일정은 `createSeq`)가 본인일 때만 실행하고, 아니면 명시적 에러를 냅니다. 서버도 남의 데이터 수정을 무시하지만, MCP에서 먼저 걸러 원인을 분명히 알려줍니다. 이 두 규약은 도구 층이 아니라 **각 도메인 모듈의 mutation 함수(`*_and_verify`) 안**에 있어 어떤 호출자도 우회할 수 없습니다.
- **부작용 있는 도구는 명시** — 근태 punch(`attendance_clock_in`/`attendance_clock_out`), 상신(`submit_approval`), 게시글 열람(`read_notice`, 조회수 증가), 메일 열람(`read_mail`, 읽음 처리 — `mark_mail_unread`로 되돌림)은 실제 기록이 남습니다. 사용자가 명시적으로 지시할 때만 호출하세요. 메일 발송은 되돌릴 수 없어 한 단계 더 두었습니다 — 지시받았더라도 `save_mail_draft`로 초안을 만들어 `list_mail_drafts`로 확인받은 뒤 `send_mail_from_draft`로 **그 초안을 그대로** 보냅니다(확인받은 형상과 발송물이 어긋나지 않고, 원본 초안 정리까지 그 도구가 합니다). 사용자가 즉시 발송을 지시하면 그때만 `send_mail`로 곧바로 보냅니다.
- **서버 자동 결재선 불신** — 서버가 채워주는 기본 결재선은 위임전결 규정과 일치하지 않습니다. `get_approval_line_schema`로 직책 기반 스키마를 받고, `org_chart`로 담당자를 해석한 뒤 사람이 확인하고 상신하세요.

## 문서

| 문서 | 내용 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 아키텍처, 크레덴셜 취득, 서명 규격, 공통 규약 |
| [docs/api-reference.md](docs/api-reference.md) | 모듈별 확정 API 스키마 |
| [docs/HTTP.md](docs/HTTP.md) | HTTP 전송을 정식 지원하지 않는 이유와, 그래도 필요할 때 직접 빌드하는 절차 |
| [tests/live/README.md](tests/live/README.md) | 라이브 스모크 테스트 — 실제 그룹웨어에 붙어 도구를 왕복시키는 하네스(승인 게이트·자동 정리) |

`docs/`에는 **실증으로 확정된 사실만** 담습니다 — 실제로 호출해 응답을 확인한 것만 적습니다. 거기 없는 것은 확정되지 않았다는 뜻입니다.
