# 아키텍처 · 인증 · 공통 규약

> 실증(브라우저 밖 순수 HTTP)으로 확정된 사실만 정리. gw.innogrid.com = 더존 WEHAGO/BIZCUBE 그룹웨어(React CSR SPA, 해시 라우팅).

## 1. 왜 브라우저가 필요 없는가 (확정)

서버는 요청을 **HTTP 헤더 4종(+Content-Type)** 만으로 인증한다. 실증 결과:

- `Origin` / `Referer` **검사 안 함** — 없거나 임의 값이어도 200.
- 쿠키 **필수 아님** — 헤더로 인증 정보를 다 넘기면 쿠키 없이 200.
- 즉 Python/Rust에서 헤더만 맞춰 POST하면 브라우저 없이 그룹웨어 조작이 가능하다. (playwright는 **크레덴셜 최초 취득/신규 API 캡처**에만 쓰고, 운영에는 불필요.)

## 2. 아키텍처

```
inno-creed (Rust MCP 서버, 헤드리스)
 ├─ creds       크레덴셜 취득: 익스텐션 캐시(권장) → Chrome → Edge(Win) → Firefox(비-Win) → authToken / signKey
 ├─ native_host Chrome/Edge 확장 프로그램(`extension/`)의 Native Messaging 수신 — 1회성
 ├─ sign     wehago-sign(HMAC-SHA256) · transaction-id 생성
 ├─ util     도메인 무관 순수 함수(날짜 days_to_ymd/fmt_ymd · digits_only · JSON 필드 추출 json_str/s)
 ├─ client   GwClient: ensure_session(gw050A02 lazy 취득+10분 TTL 캐시) · 캘린더 목록 캐시(10분 TTL) · 사원 명부 캐시(30분 TTL) · 본인 표시정보 캐시(30분 TTL) · signed()로 헤더 4종 주입 · 전송 · 응답봉투 파싱 · companyInfo 조립
 ├─ error    도메인 공통 에러 타입 — NotOwner(소유권 위반) · InvalidInput(호출자 인자 오류)
 ├─ doctor   `inno-creed doctor` — 위 진단 + 설정 파일 탐색 + 실제 인증 왕복 1회
 ├─ modules  resource(자원) · calendar(일정) · mail(메일) · board(게시판) · approval*(전자결재)
 │           org(조직) · person_group(사람 그룹) · attendance(근태) · search(통합검색) · submission_guide
 │           API 래퍼 + 파생 조회 + **소유권 가드 · read-back 검증**(`*_and_verify`)
 └─ mcp/     rmcp 서버(stdio)
    ├─ mod.rs   서버 골격: Amaranth · 라우터 합성(all_tools) · ensure_session · 에러 변환 · instructions
    ├─ tools/   도구 55개 — 도메인 12개(resource·calendar·mail·board·approval{,_line,_submit,_meta}·org·person_group·attendance·search)
    └─ args/    도구 인자 스키마 — 도메인 9개. ⚠️ doc comment가 그대로 LLM 프롬프트가 된다
```

이 저장소는 **워크스페이스**다. 위 트리는 루트 패키지(`inno-creed`) 얘기고, 옆에 둘이 더 있다:

| 크레이트 | 역할 |
|---|---|
| `crates/config-kit` | `claude_desktop_config.json` 탐지·백업·머지·원자적 쓰기. MCP 서버와 인스톨러가 같은 규칙을 쓰도록 뽑아낸 것 |
| `installer` | 비개발자용 GUI 인스톨러(eframe). `--uninstall` 모드 · Windows "프로그램 추가/제거" 등록. 배포 zip은 `scripts/package-installer.{sh,ps1}`가 만든다 |

`cargo build --release`는 **루트 패키지만** 만든다(워크스페이스 루트에 패키지가 있어서 기본 대상이 그것 하나다). 인스톨러는 `cargo build --release -p installer`로 따로 짓는다.

- 구조 근거: MCP는 **실행층**, 크레덴셜만 외부(브라우저)에서 취득. 그래서 헤드리스로 돌아간다.
- 서버 시작 순서: `creds::from_browser()`(크레덴셜 — env → 익스텐션 캐시 → Chrome → Edge(Win) → Firefox(비-Win) → 크레덴셜 파일) → stdio MCP 서브. [세션 정보](#4-authtoken-구조--세션-정보-lazy-취득--ttl-캐시)는 첫 도구 호출 시 `ensure_session()`이 lazy 취득(선취득 없음).
- 소스: `src/{creds,sign,util,client,error,doctor}.rs`, `src/modules/*.rs`, `src/mcp/{mod.rs,tools/,args/}`. 빌드 타깃은 `src/main.rs`(MCP 서버)와 `src/bin/probe.rs`(디버그 REPL — 임의 엔드포인트를 서명 호출) 둘.
- **보조 서브커맨드는 서버가 뜨기 전에 끝난다**(`main.rs`): `doctor`(진단), `auth set/clear`(크레덴셜 파일), `--install-extension-host`, `--version`, `--help`. MCP stdio는 stdout이 JSON-RPC 채널이라 **서버 기동 후에 무언가 찍으면 프로토콜이 깨진다** — 그래서 전부 앞에 둔다. 순서상 native-host 감지가 이들보다 **앞**이어야 한다(브라우저가 확장 origin·`--parent-window` 같은 인자를 붙여 스폰하므로, 뒤에 두면 "알 수 없는 인자"에 걸린다).
- **도구 라우터 합성**: 도메인마다 `#[tool_router(router = <도메인>_router, vis = "pub(crate)")]`로 라우터를 만들고 `Amaranth::all_tools()`가 `ToolRouter`의 `Add`로 합친다. `#[tool_handler(router = Self::all_tools())]`로 경로를 명시한다 — 핸들러는 라우터를 **필드로 갖지 않는다**(매크로가 호출 때마다 표현식을 평가하므로 필드에 담아도 읽히지 않는다).
- **모듈 함수 시그니처 규약**: 첫 인자는 `c: &GwClient`. 예외는 `org::roster`/`org::find_person` 둘뿐이며 `&Arc<GwClient>`를 받는다 — 부서를 `JoinSet`으로 병렬 순회하는데 `spawn`이 `'static`을 요구하고 `GwClient`는 `RwLock` 보유로 `Clone`이 아니기 때문이다. 대안(신규 의존성/역할 분담 붕괴/직렬화)이 전부 대가가 커서 **의도적으로 예외를 유지**한다. 새 함수는 `&GwClient`를 쓸 것(상세: `src/modules/org.rs` 헤더 주석).
- **파생 조회**: 일부 도구는 단일 API 래퍼가 아니라 여러 호출을 조합해 서버측에서 계산을 끝낸다 — `find_free_rooms`(자원 목록+예약을 인터벌 연산), `find_person`(부서 전수 순회 후 캐시), `my_reservations`·`pending_approvals`(필터+요약). LLM이 매 호출마다 같은 다단 조합을 반복하지 않게 하려는 것.

## 3. 크레덴셜 취득 (익스텐션 · Chrome/Edge · Firefox · macOS·Linux·Windows)

`gw.innogrid.com`이 발급하는 두 쿠키에서 값을 뽑는다(`src/creds.rs`):

| 쿠키 | 용도 | 후처리 |
|---|---|---|
| `BIZCUBE_AT` | `authToken` | URL 디코드(`%7C`→`|`) |
| `BIZCUBE_HK` | `signKey` | 그대로 |

`from_browser()`의 순서: 수동 지정(env) → **익스텐션 캐시**(권장) → Chrome → Edge(Windows만) →
Firefox(**Windows에서는 시도 안 함** — 아래 참고). 첫 성공에서 멈춘다.

### 익스텐션 캐시(Chrome/Edge, `extension/` + `src/native_host.rs`)

`BIZCUBE_AT`/`BIZCUBE_HK`는 **세션 쿠키**(만료시간 없음)라 브라우저가 켜져 있는 동안만 존재한다.
아래 쿠키 DB 직접 읽기 경로는 이 성질과 Windows의 파일잠금·`v20` 암호화가 겹쳐 사실상 항상
실패한다(실증됨). 확장 프로그램은 브라우저가 공식으로 제공하는 `chrome.cookies` API로 평문
값을 직접 받아 이 문제 자체를 우회한다.

- 흐름: 익스텐션이 `chrome.cookies.onChanged`로 두 쿠키를 감지 → `chrome.cookies.getAll({domain})`
  으로 읽음(`.get({url,name})`은 쿠키 `Path`가 `/`가 아니면 조용히 실패해서 안 씀) →
  Native Messaging(`connectNative`, 1회성 — `sendNativeMessage`는 MV3 서비스워커가 native
  host 스폰 중 유휴 종료되면 콜백이 유실되는 걸 실측 확인해서 안 씀)으로 `inno-creed
  --native-host`(브라우저가 직접 스폰, 인자는 `chrome-extension://<id>/`이지 우리가 정한
  플래그가 아님 — `main.rs`가 이 접두도 native-host 모드로 인식)를 호출 → 로컬 캐시 파일
  (`extension_cache_path()`, OS별 표준 로컬 데이터 디렉토리, `INNO_CREED_EXTENSION_CACHE`로
  오버라이드)에 씀 → `from_extension_cache()`가 매 취득마다 그 파일을 읽음.
- 등록: `inno-creed --install-extension-host [확장ID]`가 native messaging host 매니페스트를
  쓰고 Chrome/Edge 레지스트리 하이브 둘 다(`HKCU\Software\{Google\Chrome,Microsoft\Edge}\NativeMessagingHosts`)
  에 등록(Windows만 구현). 확장 ID는 `extension/manifest.json`의 `"key"`(고정 공개키)로
  결정되므로 unpacked로 재로드해도 안 바뀐다.
- 로그아웃(쿠키 삭제) 감지 시 캐시 파일도 지운다(익스텐션이 `{clear:true}` 메시지 전송) —
  안 지우면 만료된 값으로 계속 "성공"해서 나중에 API 401로 더 헷갈리는 실패가 난다.

### Chrome/Edge 쿠키 DB 직접 읽기 (폴백, best-effort)

쿠키 DB(SQLite, `WHERE host_key='gw.innogrid.com'`)의 `encrypted_value`를 OS별로 복호화:

| OS | 복호화 키 | 알고리즘 |
|---|---|---|
| macOS | 키체인 `security find-generic-password -s "Chrome Safe Storage"` → PBKDF2-HMAC-SHA1(1003, 16B) | AES-128-CBC(iv=0x20×16, Pkcs7) |
| Linux | 키링(`v11`): `secret-tool`로 `Chrome Safe Storage` 비밀 조회 → PBKDF2-HMAC-SHA1(1, 16B). 키링 미사용(`v10`): 고정 비번 `"peanuts"` | AES-128-CBC(iv=0x20×16, Pkcs7) |
| Windows | `v10`만: `os_crypt.encrypted_key`(base64, `DPAPI` 접두) → `CryptUnprotectData`로 32B 키. `v20`(app-bound)은 **시도하지 않는다** — 호출자 프로세스 경로를 검증해 제3자 프로세스는 설계상 항상 거부됨을 실증함(Edge COM 직접 호출로 재현, `hr=0x8004B016 last_error=5`) | AES-256-GCM(nonce 12B + tag 16B) |

- 공통: `encrypted_value` 앞 **3바이트 버전 프리픽스(`v10`/`v20`) 제거**. 최신 Chrome은 평문 앞에 **32B 도메인 SHA256**을 붙이므로 UTF-8 파싱 실패 시 앞 32B 제거.
- 쿠키 DB 경로: 신버전 `Default/Network/Cookies` → 구버전 `Default/Cookies` 폴백. User Data 루트는 OS별(mac `~/Library/…`, linux `~/.config/google-chrome`, win `%LOCALAPPDATA%\Google\Chrome\User Data`, Edge는 `%LOCALAPPDATA%\Microsoft\Edge\User Data`).
- Windows에서는 `BIZCUBE_AT`/`HK`가 전부 `v20`이라 이 경로로는 사실상 항상 실패한다 — 위 익스텐션 캐시가 실질적 경로.

### Firefox (macOS/Linux만 — Windows는 미지원)

`cookies.sqlite`(`moz_cookies`)가 **평문**이라 복호화 없이 읽는다. 프로필 루트만 OS별(mac
`~/Library/…/Firefox/Profiles`, linux `~/.mozilla/firefox`, win `%APPDATA%\Mozilla\Firefox\Profiles`)
로 분기, `*.default*` 프로필 우선.

**Windows에서는 `from_browser()`가 Firefox를 아예 호출하지 않는다** — `BIZCUBE_AT`/`HK`가
세션 쿠키라 Firefox가 브라우저 실행 중엔 `cookies.sqlite`에 아예 쓰지 않는 걸 실증함(WAL
사이드카까지 포함해 라이브로 직접 읽어도 해당 행이 없음, `mode=ro` URI로 복사 경합 가능성도
배제). Chrome/Edge의 파일잠금·`v20`과는 다른 메커니즘이지만 결과는 같다. `from_firefox()`
함수 자체는 남아 있어(macOS/Linux, 또는 명시적 직접 호출) 다른 OS에서는 계속 쓰인다.

### 취득 순서와 진단

- **순서**: `환경변수` → `익스텐션 캐시` → `Chrome` → `Edge`(Windows) → `Firefox`(비-Windows) →
  `크레덴셜 파일`(`~/.config/inno-creed/creds.json`, `inno-creed auth set`이 씀). 먼저 성공하는
  것을 쓰고 나머지는 시도하지 않는다. 목록의 정본은 `creds::sources()`이고,
  `source_names()`가 그것을 `doctor`·`auth set` 안내 문구에 공급한다 — **순서를 문서와 코드에
  두 번 적지 않으려는 것이다.**
  - **수동 우회(env)**: `INNO_CREED_AUTH_TOKEN`(=`BIZCUBE_AT`) + `INNO_CREED_SIGN_KEY`(=`BIZCUBE_HK`)를
    **둘 다** 지정하면 브라우저 읽기를 건너뛴다. 모든 OS·브라우저 우회.
  - **파일이 맨 아래인 이유**: 위에 두면 만료된 `creds.json` 하나가 멀쩡한 브라우저 세션을 영영
    가린다(env가 가진 병 그대로 — 재취득해도 같은 값이 돌아온다). 아래에 두면 나머지가 **실패할
    때만** 쓰이고, 파일을 쓰는 이유가 애초에 "다른 데서 못 가져온다"이므로 이 순서로 충분하다.
  - 단, **env는 파일보다 위**라 둘을 같이 두면 파일을 새로 저장해도 안 먹는다. `doctor`가 이
    조합을 경고한다.
- **진단은 한 곳에서만 만든다**: `creds::diagnose()`가 소스별 결과(`Outcome::{Ok,Absent,Failed}`)를
  돌려주고, **최종 에러 문구와 `inno-creed doctor`가 그것을 공유한다.** 따로 구현하면 "doctor는
  OK인데 서버는 실패"처럼 어긋난다. `Absent`(브라우저 미설치 등 처방 없는 것)는 최종 에러에서
  이름만 한 줄로 강등해, 손댈 곳 하나가 묻히지 않게 한다.
- **만료**: 401 감지 시 재취득(만료 주기 미관측 — 열린 질문). 익스텐션·브라우저·파일 경로는
  재취득이 같은 경로를 다시 읽으므로 클라이언트 재시작이 필요 없다. **env만 재시작이 필요하다** —
  재취득해도 같은 값이 돌아온다.
- 임시 파일(복사한 쿠키 DB)은 사용 후 삭제.

## 4. authToken 구조 & 세션 정보 (lazy 취득 + TTL 캐시)

```
authToken = "{groupSeq}|{empSeq}|{secret}"
          = "gcms<테넌트>|<본인 empSeq>|..."
```

- `split('|')`: `[0]`=groupSeq, `[1]`=empSeq(UC 본인 식별, 소유권 가드 기준).
- **나머지 세션 정보는 하드코딩하지 않고 `gw050A02`(SSO 세션정보 조회)로 취득** — 배포용(사용자마다 값이 다름). agent용 tool이 아니라 **값이 필요할 때 내부적으로 lazy 호출**하고 **인메모리 10분 TTL로 캐시**한다(`ensure_session()`).

  | 값 | 출처 |
  |---|---|
  | groupSeq, empSeq | authToken split |
  | compSeq, deptSeq, empName, emailAddr, emailDomain | `gw050A02` → `resultData.sessionInfo.ucUserInfo` (UC 계열) |
  | empCd, deptCd, coCd | 같은 `ucUserInfo`의 `erpEmpSeq`/`erpDeptSeq`/`erpCompSeq` (근태/ERP 계열 — UC seq와 별개 코드 체계) |

  - **`gw050A02` 호출**: `POST /gw/gw050A02`, `Content-Type: x-www-form-urlencoded`, body `a10Domain=https://gw.innogrid.com`. Bearer 인증 헤더만으로 "이미 로그인된 사용자"의 sessionInfo 반환(별도 CSRF 토큰 불필요). 브라우저는 SSO 진입 시 이 응답을 `sessionStorage.userInfo`에 캐시한다 — MCP는 sessionStorage 대신 동일 API를 직접 호출.
  - **lazy + TTL**: 첫 도구 호출 시 취득 → 10분간 캐시 재사용 → 만료 시 재조회. 시작 시 선취득하지 않음. 저장은 `RwLock<SessionCache>`(info + `Instant`), fetch 중 락 미보유(await 동안). 이전 방식(`mail000A01` + `sc111A02` 2회, 서버 시작 시 1회)을 대체 — `ucUserInfo` 하나로 UC + 근태 코드를 한 번에 확보.
- `companyInfo` 객체(compSeq/groupSeq/deptSeq/emailAddr/emailDomain)는 이 세션 정보로 조립해 요청 body에 공통 주입.

## 5. 요청 서명 (wehago-sign)

```
wehago-sign = Base64( HMAC_SHA256( authToken ‖ transactionId ‖ timestamp ‖ urlPathname , signKey ) )
```

- 4개 입력을 **구분자 없이** 순서대로 이어 HMAC(키=signKey). `src/sign.rs` 참조.
- `urlPathname` = 요청 경로(`/schres/rs121A06` 등, 쿼리 제외).
- `transaction-id` = 요청마다 새로 뽑는 32 hex(16바이트 랜덤).
- `timestamp` = unix epoch 초.

### 요청 헤더

| 헤더 | 값 |
|---|---|
| `Authorization` | `Bearer {authToken}` |
| `timestamp` | unix epoch 초 |
| `transaction-id` | 32 hex |
| `wehago-sign` | 위 서명 |
| `Content-Type` | `application/json`(대부분) / `multipart/form-data`(메일 발송) |

## 6. 응답 봉투

```json
{ "resultCode": 0, "resultMsg": "SUCCESS", "resultData": { ... } }
```

- 성공 판정: `resultCode ∈ {0, 200}`. (모듈별 혼용 주의)
- 도구는 `resultData`만 반환.

## 7. 필수 안전 규약

### 7.1 응답 성공 ≠ 실제 반영 → read-back 검증

서버는 **권한 밖 대상을 수정 요청받으면 `successTf:true`를 주면서 실제로는 무시(silent no-op)** 한다. 실증: 남이 만든 예약의 "내용"을 수정 요청 → 응답 성공 → **재조회하니 그대로**. 따라서:

> 모든 mutation(등록/수정/삭제)은 직후 **재조회(read-back)로 실제 상태를 확인**하고, 반영이 안 됐으면 실패로 처리한다.

**구현 위치**: 도구 층이 아니라 **각 도메인 모듈의 mutation 함수 안**이다(`resource::reserve/update/cancel_and_verify`, `calendar::create/update/delete_event_and_verify`, `attendance::punch_and_verify`, `approval_submit::cancel_and_verify`, `mail::save_mail_draft`). 검증 없는 raw 래퍼도 남아 있으나 새 호출부는 검증하는 쪽을 쓴다 — 규칙이 모듈에 있어야 MCP를 거치지 않는 호출자도 우회할 수 없다.

전자결재 취소(`approval_submit::cancel_and_verify`)는 **재조회 경로를 고르는 것 자체가 판정의 일부**인 사례다. 상세 조회(`eap111A04`)는 취소된 문서에 실패 코드(2385/2156)를 주는데 그것이 `c.call`의 `bail!`을 타서 **"취소 성공"과 "장애"가 같은 모양**이 된다. 그래서 상태 조회(`eap110A98`)를 **성공판정 없이**(`call_raw`) 불러 `doc_sts`로 판정한다 — 상신취소는 `10`(임시보관) 복귀, 삭제는 `999`. 실행 API가 주는 성공 신호(`returnValue:1`)는 **이미 삭제된 문서에도 그대로 오므로**(실측) 보조 신호로만 쓴다.

여기서 얻은 일반 교훈: **재조회 결과를 "반영됨 / 아님" 2분법으로 접으면 안 된다.** 이 조회는 세 가지를 말할 수 있고 셋의 결말이 전부 다르다.

| 재조회가 말하는 것 | 판별 | 결말 |
|---|---|---|
| 문서가 있다(상태값 포함) | 응답의 문서 정보 존재 | 상태값으로 반영 여부 판정 |
| 그 대상이 없다 | 문서 정보가 `null` | **호출자 잘못**(`InvalidInput` → `invalid_params`). 실행 콜을 쏘기 전이면 아예 쏘지 않는다 |
| 읽지 못했다 | 전송 실패·해석 불가 | **모르는 것**이다. 실행 전이면 멈추고(`Err`), 실행 후면 `ok:false` + "실행은 됐으나 확인 못 함" |

뒤의 둘을 "반영 안 됨"이나 "삭제 확인됨" 어느 쪽으로도 접지 않는 것이 핵심이다. 특히 **삭제처럼 양성 신호(`doc_sts 999`)가 있는 경우 "못 읽음"을 삭제의 증거로 쓰면 안 된다** — 실제로 그렇게 접었다가 사후 조회가 실패했을 때 `ok:true`가 나가는 결함이 있었다. 요청한 종착 상태에 **이미** 도달해 있으면(삭제 요청인데 이미 삭제됨) 실행 없이 `ok:true, already:true, steps:[]`로 끝낸다 — 아무것도 하지 않았음이 응답에 드러나므로 조용한 성공이 아니다(`attendance::punch_and_verify`의 `already`와 같은 규약).

상신(`submit_approval`)은 재조회 대신 응답의 docId 발급 여부로 판정한다(발급이 곧 접수).

이름 규칙은 `*_and_verify`가 기본이지만 `mail::save_mail_draft`는 예외다(대응하는 raw 래퍼가 없어 접미사로 구분할 이유가 없다). 이쪽은 재조회 결과를 **실패로 바꾸지 않고 `verified_by_readback`으로 보고만 한다** — 저장 자체는 성공했는데 조회가 막힌 경우와 정말 저장이 안 된 경우를 서버 응답만으로 가를 수 없기 때문이다. 반영 실패를 곧바로 에러로 올리는 위 셋과 다른 점이라 새 mutation을 만들 때 어느 쪽을 따를지 의식적으로 정할 것.

**`mail::send_mail_from_draft`는 세 번째 갈래 — 사전 확인(pre-check)이다.** 발송은 되돌릴 수 없어 사후 read-back으로는 늦다. 그래서 보내기 **전에** 임시보관함을 조회해 그 muid가 실재할 때만 진행하고, 조회 자체가 실패하면 "없다"가 아니라 "확인 못 했다"로 보고 발송을 중단한다. 되돌릴 수 없는 mutation은 이 방향을 따를 것.

### 7.2 소유권 가드

- 자원 예약의 쓰기는 서버가 **소유자(생성자) 본인일 때만** 실제 반영한다(IDOR 아님 — 정보 조회는 열려 있으나 쓰기는 막힘).
- 서버에 맡기지 않고, 쓰기 전에 대상의 소유자 == 본인 empSeq(authToken에서 추출)를 확인하고 아니면 **명시적 에러**를 반환한다. (조회는 제한 없음.)
- **소유자 필드는 도메인마다 다르다** — 자원 예약은 `empSeq`("소유자"), 일정은 `createSeq`("작성자"), 전자결재 문서는 상태 조회(`eap110A98`)가 주는 `user_id`("기안자"). 그래서 가드 함수는 도메인별로 각자 둔다(`resource::check_owner` / `calendar::check_author` / `approval_submit::pre_verdict`). 다만 **에러는 `error::NotOwner` 타입 하나를 공유**하고, `mcp::map_domain_err`가 `downcast_ref`로 판별해 `invalid_params`로 매핑한다 — 문자열 매칭이 아니라 타입으로 분류하는 것이 핵심이다.
- **가드는 fail-closed다** — 소유자 필드를 읽지 못하면(응답에 없음 → `""`) 그것도 **불일치로 보고 거부**한다. "모른다"를 "내 것"으로 치는 순간 가드가 아니다. 세 곳이 같은 규약이다: `error::NotOwner`의 `owner` 필드 주석, `resource::check_owner`(`unwrap_or("")` 후 `owner != me`), `approval_submit::pre_verdict`. 비용/편익이 비대칭이라 그렇다 — 막혀서 못 고친 것은 웹에서 하면 되지만, 남의 것에 쓰기를 쏜 것은 되돌릴 수 없다.
- 전자결재 취소의 가드는 **추가 호출 비용이 0**이다 — 어차피 상태를 알려고 부르는 사전조회가 기안자까지 함께 준다. 남의 문서에 취소 콜을 보내면 서버가 어떻게 반응하는지는 **미관측**이라(시험 삼아 쏘지 않았다) 실행 전에 막는다.
- 같은 이유로 **관측되지 않은 상태에는 파괴 콜을 쏘지 않는다.** 전자결재 취소가 실증된 상태는 `10`(임시보관)·`20`(상신)·`30`(결재 진행중)뿐이라, 종결(`90`)·반려(`100`) 등은 실행 전에 거부하고 **거부 사유에 "이 상태의 취소 거동은 관측되지 않았다"를 밝힌다**. 실행 API(`eap110A19`)가 지울 대상이 없어도 `returnValue:1`을 주므로 **쏜 뒤에는 응답으로 아무것도 알 수 없다** — 판단은 쏘기 전에 끝나야 한다.
