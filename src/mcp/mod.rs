//! MCP 서버: 코어 모듈을 rmcp 도구로 노출.
//!
//! 이 파일에 남는 것은 **서버 골격**뿐이다 — 핸들러 상태(`Amaranth`), 세션 보장, 에러 매핑,
//! 라우터 합성, `ServerHandler`(도구 전체를 아우르는 instructions).
//! 도구 정의는 `tools/`(도메인 12개), 인자 스키마는 `args/`(도메인 9개)에 있다.

use std::sync::Arc;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo},
    tool_handler, ErrorData, ServerHandler,
};

use crate::client::GwClient;

pub mod args;
pub mod tools;

/// 모듈 에러 → MCP 에러 매핑. **호출자 잘못(`NotOwner`·`InvalidInput`)만 `invalid_params`**로
/// 분류한다 — 서버/네트워크 실패가 아니라 잘못된 대상·인자를 준 것이기 때문이다(리팩터 전 동작 보존).
/// 문자열 매칭이 아니라 타입(`downcast_ref`)으로 판별한다.
///
/// ⚠️ **도메인 모듈(`modules::*`)에서 올라온 `anyhow::Error`는 도메인 12개 라우터 전부가
/// 이 함수(또는 `map_domain_err_ctx`)를 지난다.** 새 도구를 추가할 때 `ErrorData::internal_error`를
/// 직접 부르면 그 자리만 조용히 분류를 놓친다 — 컴파일러도 스냅샷 테스트도 잡지 못한다.
/// (도구 층에서 자체 판단한 인자 오류 — JSON 파싱 실패·없는 doc_type 등 — 만 `invalid_params`를 직접 쓴다.)
pub(crate) fn map_domain_err(e: anyhow::Error) -> ErrorData {
    if is_caller_fault(&e) {
        ErrorData::invalid_params(e.to_string(), None)
    } else {
        ErrorData::internal_error(e.to_string(), None)
    }
}

/// `map_domain_err`의 접두사 붙는 변형 — 분류 규칙은 같고 메시지만 `"{prefix}: {e}"`가 된다.
/// `"메일 발송 실패: {e}"`처럼 어느 단계에서 깨졌는지 알려주던 자리를 그대로 두기 위한 것이다.
pub(crate) fn map_domain_err_ctx(prefix: &'static str) -> impl Fn(anyhow::Error) -> ErrorData {
    move |e| {
        let msg = format!("{prefix}: {e}");
        if is_caller_fault(&e) {
            ErrorData::invalid_params(msg, None)
        } else {
            ErrorData::internal_error(msg, None)
        }
    }
}

/// 서버/네트워크 실패가 아니라 **호출자가 잘못 준 것**인지 판별.
fn is_caller_fault(e: &anyhow::Error) -> bool {
    e.downcast_ref::<crate::error::NotOwner>().is_some()
        || e.downcast_ref::<crate::error::InvalidInput>().is_some()
}

/// MCP 서버 핸들러. 상태는 `client` 하나뿐이다.
/// ⚠️ **라우터를 필드로 들고 있지 않다.** `#[tool_handler]`의 기본 동작이 `call_tool`/`list_tools`
/// 본문에서 라우터 표현식을 **매번 평가**하는 것이라(rmcp-macros 2.2.0), 인스턴스 필드에 저장해도
/// 어떤 경로로도 읽히지 않는다. 대신 `#[tool_handler(router = Self::all_tools())]`로 경로를 명시한다.
#[derive(Clone)]
pub struct Amaranth {
    client: Arc<GwClient>,
}

impl Amaranth {
    pub fn new(client: GwClient) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    /// 도메인별 라우터 12개를 합성한 전체 도구 표면.
    /// 각 `*_router()`는 `tools/<도메인>.rs`의 `#[tool_router(router = …, vis = "pub(crate)")]`가
    /// 생성한다. `ToolRouter`의 `Add`가 rmcp가 의도한 합성 방식이다(`handler/server/router/tool.rs`).
    /// ⚠️ 도메인 파일을 추가하면 **여기에도 더해야** 도구가 노출된다 — 빠뜨려도 컴파일은 되고
    /// 도구만 조용히 사라지므로, 아래 스냅샷 테스트가 유일한 방어선이다.
    pub(crate) fn all_tools() -> ToolRouter<Self> {
        Self::resource_router()
            + Self::calendar_router()
            + Self::mail_router()
            + Self::board_router()
            + Self::approval_router()
            + Self::approval_line_router()
            + Self::approval_submit_router()
            + Self::approval_meta_router()
            + Self::org_router()
            + Self::person_group_router()
            + Self::attendance_router()
            + Self::search_router()
    }

    /// 세션 정보(gw050A02, 10분 TTL 캐시)를 lazy 보장. 도구 핸들러가 진입 시 호출한다.
    /// ⚠️ **모든 도구가 부르는 것은 아니다** — 게시판·결재 읽기·조직도처럼 헤더 인증만으로
    /// 완결되는 도구는 의도적으로 생략한다(불필요한 API 호출 방지). 도구별 주석 참조.
    pub(crate) async fn ensure_session(&self) -> Result<(), ErrorData> {
        self.client
            .ensure_session()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
}

#[tool_handler(router = Self::all_tools())]
impl ServerHandler for Amaranth {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "이노그리드 그룹웨어 **아마란스**(gw.innogrid.com) 도구. \
             회의실·일정·메일·게시판·전자결재·근태·조직도를 다룬다. (Dooray 등 다른 그룹웨어가 아님)\n\
             \n\
             먼저 잡을 도구:\n\
             - 무언가를 '찾아야' 하면 `search` — 메일·결재·게시판·일정·자원·파일을 한 번에 훑고, \
               결과의 ID로 `read_mail`(muid)/`read_approval`(docId+formId)/`read_notice`(artSeqNo)에 바로 이어진다. \
               모듈별 전용 검색 API는 존재하지 않으므로 이것이 유일한 검색 경로다.\n\
             - '언제 회의실이 비나'는 `find_free_rooms`(빈 구간 계산 완료본). 예약 목록을 직접 훑어 계산하지 말 것.\n\
             - 사람의 empSeq가 필요하면 `find_person`, 본인 값은 `whoami`. \
               결재선·참석자·수신자가 전부 empSeq를 요구한다.\n\
             - 사용자가 **여러 사람을 이름 붙여 묶어 부르면**(팀·주간보고 수신자 등) `person_group`. \
               아마란스에 그룹메일이 없어 이 서버가 대신 갖는다 — `save_person_group`으로 만들고(이름/empSeq를 그대로 주면 된다), \
               쓸 때 `person_group(name)`이 `empSeqs`(→ 일정 참여자)와 `emails`(→ 콤마로 이어 메일 수신자·참조)를 준다.\n\
             - 내 예약을 고치거나 취소하려면 `my_reservations`로 seqNum/resIdx를 먼저 얻는다.\n\
             \n\
             주의:\n\
             - 부작용 있는 도구 — `attendance_clock_in`/`attendance_clock_out`(실제 근태 기록), `submit_approval`(결재요청 발송), \
               `send_mail`, `read_notice`(조회수 증가), `read_mail`(읽음 처리 — 받은메일함 최근 200건 이내면 `mark_mail_unread`로 되돌릴 수 있다). 사용자가 명시적으로 지시할 때만 호출한다.\n\
             - **메일 발송은 되돌릴 수 없다** — 지시받았더라도 곧바로 `send_mail` 하지 말고, \
               `save_mail_draft`로 초안을 만들어 `list_mail_drafts`로 사용자 확인을 받은 뒤 \
               `send_mail_from_draft`로 **그 초안을 그대로** 보낸다(원본 초안 정리까지 그 도구가 한다). \
               사용자가 '확인 없이 바로 보내'라고 명시하면 그때는 `send_mail`로 곧바로 발송한다.\n\
             - 회의실 **정원(수용인원) 데이터는 아마란스에 없다**. 'N명 회의실' 조건은 답할 수 없다.\n\
             - 날짜는 YYYYMMDD, 시각은 YYYYMMDDHHmm(입력). 조회 결과의 시각은 ISO로 정규화해 반환한다."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// 도구 표면(이름·개수) 스냅샷.
///
/// 도메인 라우터 12개의 합성 결과(`Amaranth::all_tools()`)를 검사한다 — 도메인 파일을
/// `all_tools()`에 더하는 것을 빠뜨리면 그 도메인 도구가 통째로 사라지는데 컴파일러는 못 잡는다.
/// 목적은 커버리지가 아니라 **회귀 기준선**이다 — 파일 분해(단일 `mcp.rs` → `args/`+`tools/`)처럼
/// 코드를 옮기는 작업에서 도구가 사라지거나 이름이 바뀌는 것을 컴파일러가 못 잡기 때문이다.
#[cfg(test)]
mod tests {
    use super::*;

    /// 도구 목록 스냅샷. 의도적으로 도구를 추가/삭제했다면 이 목록을 함께 고치면 된다
    /// (그때 README·docs 도구표도 같이 갱신할 것).
    const EXPECTED_TOOLS: &[&str] = &[
        "approval_counts", "attendance_clock_in", "attendance_clock_out", "attendance_month",
        "cancel_approval", "cancel_reservation", "create_calendar_event", "delete_approval_line",
        "delete_calendar_event", "delete_mail", "delete_person_group", "delete_temp_approval",
        "download_body_image", "download_mail_attachment", "download_notice_attachment",
        "find_free_rooms", "find_person", "get_approval_line_schema",
        "get_approval_submission_guide", "get_attendance_today", "list_approval_line_schemas",
        "list_approval_lines", "list_approval_submission_guides", "list_approvals",
        "list_calendars", "list_events", "list_mail_drafts", "list_mail_inbox", "list_mailboxes",
        "list_notice_attachments", "list_notices", "list_reservations", "list_resources",
        "mailbox_counts", "mark_mail_unread",
        "my_reservations", "org_chart", "pending_approvals", "person_group", "read_approval",
        "read_approval_line", "read_mail", "read_notice", "reserve_resource", "save_approval_line", "save_mail_draft",
        "save_person_group", "search", "send_mail", "send_mail_from_draft", "submit_approval",
        "suggest_approval_line",
        "update_calendar_event", "update_reservation", "whoami",
    ];

    #[test]
    fn 도구_표면이_스냅샷과_일치한다() {
        let mut names: Vec<String> = Amaranth::all_tools()
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        let expected: Vec<String> = EXPECTED_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(names, expected, "MCP 도구 표면이 변했다");
    }

    /// 라우터 생성이 네트워크·크레덴셜 없이 되는지(=핸들러 구성이 순수한지) 확인.
    /// `GwClient::new(None)` 은 필드 초기화만 한다.
    #[test]
    fn 핸들러는_크레덴셜_없이_만들어진다() {
        let a = Amaranth::new(GwClient::new(None));
        drop(a);
        assert_eq!(Amaranth::all_tools().list_all().len(), EXPECTED_TOOLS.len());
    }
}
