# HTTP 전송으로 쓰기 (비공식 · 직접 빌드)

inno-creed는 **stdio 전송만** 정식 지원합니다. Streamable HTTP 전송도 기술적으로는 문제없이 동작하지만(아래 절차로 실제 확인함), **배포 바이너리에 넣지 않습니다.**

넣지 않는 이유부터 읽으세요. 그 이유를 감수할 수 있는 경우에만 아래 절차를 따르세요.

## 왜 정식 경로로 만들지 않는가

한 줄 요약: **HTTP로 열면 그 포트가 곧 당신의 신원이 되고, 이 서버에는 되돌릴 수 없는 도구가 있습니다.**

### 1. 크레덴셜은 "서버를 띄운 사람" 것이다

inno-creed는 로그인을 받지 않습니다. **서버가 도는 머신의 브라우저 쿠키를 복호화**해 `authToken`/`signKey`를 얻습니다([architecture.md §3](architecture.md#3-크레덴셜-취득-chrome--firefox--macoslinuxwindows)). MCP 프로토콜에는 요청자를 구분할 개념이 없으므로, **포트에 접속한 모두가 서버를 띄운 사람 본인으로 동작**합니다.

그래서 이 서버에는 "다중 사용자"가 존재할 수 없습니다. 원격에 띄워 여러 명이 붙는 구성은 권한 분리가 되는 게 아니라, **한 사람의 계정을 여러 명이 공유**하는 것입니다.

### 2. 도구가 읽기 전용이 아니다

포트에 닿을 수 있는 누구든(또는 무엇이든) 당신 이름으로 다음을 실행할 수 있습니다.

| 도구 | 결과 |
|---|---|
| `submit_approval` | 실제 결재요청 상신 — 결재선 사람들에게 **알림이 발송**됩니다. 문서를 지워도 알림은 회수되지 않습니다 |
| `send_mail` | 메일 발송 — **되돌릴 수 없습니다** |
| `attendance_clock_in` / `attendance_clock_out` | 실제 근태 기록 |
| `delete_mail` · `cancel_reservation` · `delete_calendar_event` | 데이터 삭제·취소 |

### 3. 파일 경로가 "서버 머신" 기준이다

`download_mail_attachment` · `download_notice_attachment` · `download_body_image`는 **서버가 도는 머신의 로컬 경로**에 파일을 씁니다. `send_mail`의 첨부도 서버 머신의 로컬 경로에서 읽습니다. 클라이언트가 원격이면 이 경로들의 의미가 클라이언트 쪽 기대와 어긋납니다.

### 결론: 같은 머신 안에서만

HTTP 전송이 정당한 용도는 **"stdio를 못 쓰는 로컬 클라이언트에 붙이기"** 하나입니다(컨테이너 안의 클라이언트, HTTP 전송만 지원하는 툴 등). 그 경우에도 `127.0.0.1` 바인드 + 토큰 인증을 유지하세요.

> ⚠️ **`0.0.0.0` 바인드나 리버스 프록시로 외부 노출, 공용 서버 상주 배포는 하지 마세요.** 그렇게 쓰다 생긴 문제는 이 프로젝트가 상정한 사용 범위 밖입니다.

## 만드는 법

전송층은 이미 분리돼 있습니다 — `mcp::Amaranth`는 전송과 무관한 `ServerHandler`이고, stdio 결합은 `src/main.rs`의 `.serve(stdio())` 한 줄뿐입니다. 그래서 **기존 코드는 한 줄도 고칠 필요가 없고**, 의존성 두 줄과 새 바이너리 하나만 추가하면 됩니다.

### 1) `Cargo.toml` — rmcp feature 추가 + axum

```diff
-rmcp = { version = "3.0.1", features = ["server", "transport-io"] }
+rmcp = { version = "3.0.1", features = ["server", "transport-io", "transport-streamable-http-server"] }
+axum = "0.8"
```

### 2) `src/bin/http.rs` — 새로 만들기

`src/bin/` 아래 파일은 자동으로 바이너리가 되므로 `[[bin]]` 선언은 필요 없습니다.

```rust
//! inno-creed — Streamable HTTP 전송 (비공식). 반드시 localhost 에서만 쓸 것.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::Response,
};
use inno_creed::{client::GwClient, creds, mcp::Amaranth};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

const MCP_PATH: &str = "/mcp";

#[tokio::main]
async fn main() -> Result<()> {
    // 토큰은 선택이 아니라 필수다 — 무인증으로 뜨면 포트에 닿는 누구나
    // 내 이름으로 결재를 상신하고 메일을 보낼 수 있다.
    let token = std::env::var("INNO_CREED_HTTP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("INNO_CREED_HTTP_TOKEN 이 필요합니다."))?;
    let addr =
        std::env::var("INNO_CREED_HTTP_ADDR").unwrap_or_else(|_| "127.0.0.1:8899".to_string());

    // stdio 판(src/main.rs)과 동일: 크레덴셜 실패해도 서버는 뜨고,
    // 도구 호출 시점에 로그인 안내를 반환한다.
    let client = GwClient::new(creds::from_browser().ok());
    let handler = Amaranth::new(client);

    // ⚠️ factory 는 세션마다 호출된다. `Amaranth` 를 매번 새로 만들면 GwClient 의
    //    캐시(세션정보 10분·사원명부 30분·캘린더 10분)가 세션끼리 공유되지 않아
    //    gw050A02 와 부서 전수 순회를 반복하게 된다. Clone 은 Arc<GwClient> 를
    //    공유하므로 아래처럼 clone 해서 캐시를 살린다.
    let service: StreamableHttpService<Amaranth, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = axum::Router::new()
        .route_service(MCP_PATH, service)
        .layer(from_fn_with_state(Arc::new(token), auth));

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[inno-creed] HTTP 리스닝: http://{addr}{MCP_PATH}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// `Authorization: Bearer <token>` 검증.
async fn auth(
    State(expected): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t == expected.as_str())
        .unwrap_or(false);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

### 3) 빌드 · 실행

```sh
cargo build --release --bin http

INNO_CREED_HTTP_TOKEN=$(openssl rand -hex 32) \
INNO_CREED_HTTP_ADDR=127.0.0.1:8899 \
  ./target/release/http
```

| 환경변수 | 기본값 | 비고 |
|---|---|---|
| `INNO_CREED_HTTP_TOKEN` | 없음 (**필수**) | 미설정이면 시작하지 않음 |
| `INNO_CREED_HTTP_ADDR` | `127.0.0.1:8899` | 바꾸더라도 루프백을 벗어나지 말 것 |

크레덴셜 관련 환경변수(`INNO_CREED_AUTH_TOKEN` / `INNO_CREED_SIGN_KEY`)는 stdio 판과 동일하게 동작합니다.

### 4) MCP 클라이언트 등록

```json
{
  "mcpServers": {
    "inno-creed": {
      "type": "http",
      "url": "http://127.0.0.1:8899/mcp",
      "headers": { "Authorization": "Bearer <위에서 만든 토큰>" }
    }
  }
}
```

## 확인된 동작

위 절차를 그대로 적용해 실제로 검증한 결과입니다 (2026-08-10, rmcp 3.0.1 / axum 0.8.9, macOS arm64).

- 빌드 통과 — 기존 소스 수정 없이 의존성 2줄 + 바이너리 1개 추가만으로.
- 토큰 없는 요청 → `401`.
- 토큰 있는 `initialize` → `200`, `mcp-session-id` 발급, `instructions` 정상 전달.
- `tools/list` → **당시의 도구 49개 전부 노출** (stdio 판과 동일한 표면). 49는 2026-08-10 시점의 수이고 지금은 55개다 — 요점은 개수가 아니라 **두 전송의 도구 표면이 같다**는 것이다.

검증 후 변경분은 되돌렸습니다. 저장소에는 이 문서만 있고 HTTP 바이너리 코드는 없습니다.

## 유지보수상 유의

이 문서의 코드는 **저장소에 없으므로 컴파일러도 테스트도 지켜주지 않습니다.** `mcp::Amaranth`의 생성 방식이나 rmcp 버전이 바뀌면 조용히 낡습니다. 적용했는데 빌드가 깨지면 `src/main.rs`의 현재 조립 순서를 먼저 보세요 — 달라진 부분이 거기 있습니다.
