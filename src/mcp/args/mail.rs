//! 메일 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct SendMailArgs {
    /// 받는사람 (표시형 "이름 <email>" 또는 email). **여러 명이면 콤마로 잇는다** — 예:
    /// `"홍길동 <hong@innogrid.com>,kim@innogrid.com"`. 미지정 시 본인에게 발송.
    #[serde(default)]
    pub to: Option<String>,
    /// 참조(cc, 선택). 형식은 to와 같다(콤마 구분). 비우면 참조 없음.
    #[serde(default)]
    pub cc: Option<String>,
    /// 숨은참조(bcc, 선택). 형식은 to와 같다(콤마 구분). 비우면 숨은참조 없음.
    /// ⚠️ 숨은참조는 다른 수신자에게 보이지 않지만 **발송 자체는 되돌릴 수 없다.**
    #[serde(default)]
    pub bcc: Option<String>,
    /// 제목
    pub subject: String,
    /// 간단한 본문: 일반 텍스트 또는 Markdown. body/html_file/html 중 정확히 하나를 지정한다.
    /// 서식 있는 메일은 html_file 또는 기존 웹 초안의 preview_mail_draft를 우선 사용한다.
    #[serde(default, deserialize_with = "deserialize_body")]
    pub body: Option<String>,
    /// 서식 있는 HTML 파일의 절대경로(UTF-8, MCP 서버가 실행되는 머신 기준).
    /// Agent가 HTML을 다시 입력하지 않고 파일 내용을 그대로 사용한다. 상대경로 이미지 파일은 자동 첨부하지 않는다.
    #[serde(default)]
    pub html_file: Option<String>,
    /// 기존 호출 호환용 HTML 원문. 가능하면 html_file 또는 웹 초안 ID를 사용한다.
    /// body/html_file과 함께 지정하면 오류이며, 빈 본문은 허용하지 않는다.
    #[serde(default)]
    pub html: Option<String>,
    /// 첨부할 로컬 파일 경로 목록(선택, 절대경로). 비우면 첨부 없음.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// 서명 자동 첨부 여부(선택, **기본 true**). 아마란스에 등록해 둔 서명을 본문 끝에 붙여
    /// **웹에서 보낼 때와 같은 형상**으로 만든다 — 사람이 보내는 메일이면 켜 두는 것이 맞다.
    /// 서명이 등록돼 있지 않으면 아무것도 붙지 않는다(에러 아님).
    /// `false`는 시스템 알림·자동화처럼 서명이 없어야 하는 발송에만 쓴다.
    #[serde(default = "super::yes")]
    pub signature: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct SaveMailDraftArgs {
    /// 받는사람 (표시형 "이름 <email>" 또는 email). **여러 명이면 콤마로 잇는다** — 예:
    /// `"홍길동 <hong@innogrid.com>,kim@innogrid.com"`. 미지정 시 본인.
    #[serde(default)]
    pub to: Option<String>,
    /// 참조(cc, 선택). 형식은 to와 같다(콤마 구분). 비우면 참조 없음.
    /// 여기 넣어두면 send_mail_from_draft가 **그대로 승계해** 발송한다.
    #[serde(default)]
    pub cc: Option<String>,
    /// 숨은참조(bcc, 선택). 형식은 to와 같다(콤마 구분). 비우면 숨은참조 없음.
    /// 여기 넣어두면 send_mail_from_draft가 **그대로 승계해** 발송한다.
    #[serde(default)]
    pub bcc: Option<String>,
    /// 제목. 비워두면 "(제목없음)"으로 저장된다.
    pub subject: String,
    /// 간단한 본문: 일반 텍스트 또는 Markdown. body/html_file/html 중 정확히 하나를 지정한다.
    /// 서식 있는 메일은 html_file 또는 기존 웹 초안의 preview_mail_draft를 우선 사용한다.
    #[serde(default, deserialize_with = "deserialize_body")]
    pub body: Option<String>,
    /// 서식 있는 HTML 파일의 절대경로(UTF-8, MCP 서버가 실행되는 머신 기준).
    /// Agent가 HTML을 다시 입력하지 않고 파일 내용을 그대로 사용한다. 상대경로 이미지 파일은 자동 첨부하지 않는다.
    #[serde(default)]
    pub html_file: Option<String>,
    /// 기존 호출 호환용 HTML 원문. 가능하면 html_file 또는 웹 초안 ID를 사용한다.
    /// body/html_file과 함께 지정하면 오류이며, 빈 본문은 허용하지 않는다.
    #[serde(default)]
    pub html: Option<String>,
    /// 첨부할 로컬 파일 경로 목록(선택, 절대경로). 비우면 첨부 없음.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// 서명 자동 첨부 여부(선택, **기본 true**). 아마란스에 등록해 둔 서명을 본문 끝에 붙여
    /// **웹에서 보낼 때와 같은 형상**으로 저장한다 — 초안은 사람이 확인하고 보내는 것이므로
    /// 켜 두는 것이 맞다. 여기 붙은 서명은 send_mail_from_draft가 **본문째로 승계**한다
    /// (그 도구는 서명을 다시 붙이지 않는다 — 두 번 붙는 일이 없다).
    /// 서명이 등록돼 있지 않으면 아무것도 붙지 않는다(에러 아님).
    #[serde(default = "super::yes")]
    pub signature: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct SendMailFromDraftArgs {
    /// 발송할 임시보관 메일의 muid. save_mail_draft의 `draft_muid` 또는 list_mail_drafts 결과의 muid.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub draft_muid: String,
    /// 받는사람(선택, 표시형 "이름 <email>" 또는 email). 미지정 시 **초안에 저장된 수신자**로 보낸다.
    /// 초안에 수신자가 없으면 에러가 나므로 그때 지정할 것.
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct PreviewMailDraftArgs {
    /// 미리 볼 임시보관 초안의 muid. 웹·구버전 초안도 가능하다. 실제 발송하지 않는다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub draft_muid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteMailArgs {
    /// 삭제할 메일 muid 목록(콤마 구분). list_mail_inbox의 muid 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub uids: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadMailArgs {
    /// 메일 muid. list_mail_inbox 결과의 muid 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub muid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MarkMailUnreadArgs {
    /// 읽지 않음으로 되돌릴 메일 muid. list_mail_inbox 결과의 muid 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub muid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadMailAttachmentArgs {
    /// 메일 muid. read_mail/list_mail_inbox의 muid.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub muid: String,
    /// read_mail 응답 `attachments[].fileSn` 의 토큰 문자열을 **그대로** 붙여넣는다.
    /// ⚠️ 순번(0,1,2)이 아니다 — 숫자를 주면 서버가 422로 거절한다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub file_sn: String,
    /// 저장 경로(절대경로 권장). 예: /tmp/attach.png
    pub out_path: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MoveMailArgs {
    /// 옮길 메일 muid 목록(콤마 구분). list_mail_inbox/search 결과의 muid.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub uids: String,
    /// 받을 메일함 **이름**. 시스템 메일함(INBOX·SENT·DRAFTS·TRASH·SPAM)과
    /// 사용자가 만든 메일함 둘 다 이름으로 지정한다 — seq는 계정마다 달라 쓰지 않는다.
    /// 이름은 list_mailboxes 결과의 name/fullname과 맞춘다.
    pub to_mailbox: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListMailboxMailsArgs {
    /// 조회할 메일함 **이름**(list_mailboxes의 name/fullname). 시스템 메일함
    /// (INBOX·SENT·DRAFTS·TRASH·SPAM)과 사용자가 만든 메일함 둘 다 된다.
    pub mailbox: String,
    /// 페이지 번호(1부터). 기본 1.
    #[serde(default)]
    pub page: Option<i64>,
    /// 한 페이지 건수. 기본 20.
    #[serde(default)]
    pub page_size: Option<i64>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateMailboxArgs {
    /// 만들 메일함 이름.
    pub name: String,
    /// 상위 메일함 이름(선택). 비우면 최상위. **1단계 하위까지만** 만들 수 있다.
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteMailboxArgs {
    /// 삭제할 메일함 이름. ⚠️ 안에 든 메일도 함께 사라지며 되돌릴 수 없다.
    pub name: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SaveMailFilterArgs {
    /// 조건을 검사할 필드. `subject`(제목) · `mailfrom`(보낸사람 주소) ·
    /// `rcptto`(받는사람 주소) · `mailfromdomain`(보낸사람 도메인) ·
    /// `rcpttodomain`(받는사람 도메인) 중 하나. 다른 값은 거부한다.
    pub field: String,
    /// 그 필드에 들어 있으면 걸리는 문자열(부분일치). 예: 제목에 `[Jira]`, 도메인에 `innogrid.com`.
    pub content: String,
    /// 걸린 메일을 보낼 메일함 **이름**. 먼저 create_mailbox로 만들어 둔다.
    pub to_mailbox: String,
    /// 고칠 규칙의 autoDivSeq(선택). 주면 그 규칙을 수정하고, 비우면 새로 만든다.
    /// 값은 list_mail_filters 결과의 `autoDivSeq`.
    #[serde(default)]
    pub filter_seq: Option<i64>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteMailFilterArgs {
    /// 지울 규칙의 autoDivSeq(list_mail_filters 결과). 여러 건이면 한 건씩 반복 호출한다.
    pub filter_seq: i64,
}

// 파싱 단계에서 거부하므로 세션 확보·첨부 업로드·발송 모두 실행되지 않는다.
fn deserialize_body<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let body = Option::<String>::deserialize(d)?;
    if let Some(body) = &body {
        crate::modules::mail::render_body(body).map_err(serde::de::Error::custom)?;
    }
    Ok(body)
}
