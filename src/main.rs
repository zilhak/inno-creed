//! inno-creed MCP 서버 (stdio).
//! 크레덴셜(Chrome 쿠키 복호화) 취득 → gw API 도구를 rmcp로 노출.

use anyhow::Result;
use inno_creed::{client::GwClient, creds, mcp::Amaranth};
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<()> {
    // --version/-V: 인자 파싱기가 따로 없어 설치본 버전을 확인할 방법이 없었다.
    // 크레덴셜 취득(브라우저 쿠키 읽기) 전에 먼저 처리해 부작용 없이 즉시 종료한다.
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("inno-creed {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 크리덴셜 취득 실패해도 서버는 뜬다(비치명적). 실패 시 도구 호출 시점에 로그인 안내를
    // tool 응답으로 반환한다(사용자가 채팅에서 볼 수 있게). 성공하면 캐시를 seed.
    let initial = match creds::from_browser() {
        Ok(c) => {
            eprintln!(
                "[inno-creed] 크레덴셜 취득 완료 (authToken {}자). MCP 서버 시작 (stdio)",
                c.auth_token.len()
            );
            Some(c)
        }
        Err(e) => {
            eprintln!(
                "[inno-creed] ⚠️ 크레덴셜 미취득 — 서버는 시작하되 도구 호출 시 로그인 안내를 반환합니다.\n{e}"
            );
            None
        }
    };

    // 세션 정보(compSeq/deptSeq/근태 empCd 등)는 첫 도구 호출 시 gw050A02로 lazy 취득 후
    // 10분 TTL 캐시된다(ensure_session). 시작 시 선취득하지 않는다.
    let client = GwClient::new(initial);

    let service = Amaranth::new(client).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
