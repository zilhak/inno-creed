//! 전자결재 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;
use super::{box_pending, one, thirty};


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PendingApprovalsArgs {
    /// 조회 건수(기본 20)
    #[serde(default)]
    pub page_size: Option<i64>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListApprovalsArgs {
    /// 함: pending(미결)/approved(기결)/approved_ongoing(기결진행)/approved_done(기결종결)/reference(수신참조)/enforcement(시행)/sent(상신)/draft(임시보관). 기본 pending.
    #[serde(default = "box_pending")]
    pub box_name: String,
    /// 페이지 번호(기본 1)
    #[serde(default = "one")]
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub page: i64,
    /// 페이지 크기(기본 30)
    #[serde(default = "thirty")]
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub page_size: i64,
    /// 기간 시작(선택, YYYY-MM-DD). 빈값이면 서버 기본 최근 3개월.
    #[serde(default)]
    pub from: String,
    /// 기간 종료(선택, YYYY-MM-DD)
    #[serde(default)]
    pub to: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadApprovalArgs {
    /// 문서 ID(docId). list_approvals 결과의 docId.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub doc_id: String,
    /// 양식 ID(formId). list_approvals 결과의 formId.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub form_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetApprovalSchemaArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "외근신청서", "41", "연차휴가신청", "출장신청", "휴일주말근무". list_approval_line_schemas로 목록 확인.
    pub doc_type: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SuggestApprovalLineArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "41", "연차휴가신청", "출장신청", "휴일주말근무".
    pub doc_type: String,
    /// 출장신청 전용 — "국내" 또는 "해외"(결재선이 갈린다). 다른 양식은 비워둘 것. 빈값이면 해당 양식의 국내·해외 branch를 모두 반환한다.
    #[serde(default)]
    pub trip: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetSubmissionGuideArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "외근", "41", "연차휴가신청", "출장신청", "휴일주말근무". list_approval_submission_guides로 목록 확인.
    pub doc_type: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadApprovalLineArgs {
    /// 라인 ID(lineId). list_approval_lines 결과의 lineId 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub line_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SaveApprovalLineArgs {
    /// 라인 ID. 0이면 신규 생성, 기존 lineId면 수정. 기본 0.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub line_id: i64,
    /// 라인 이름(예: "외근-표준").
    pub line_nm: String,
    /// 양식 ID(formId). 예: 41(외근)/36(연차). list_approval_line_schemas/list_approvals의 formId(단건 get_approval_line_schema는 `schema.form_id`).
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub form_id: i64,
    /// 결재자 **empSeq 목록**. ⚠️ **배열 순서 = 결재 순서** — 예: `["2083","2857"]`(조재봉 → 임병욱).
    /// empSeq는 `find_person`/`suggest_approval_line`이 준다. 서버 payload 필드(co_id·act_id·org_id·
    /// org_div·순서)는 **도구가 채운다** — 넘기지 않는다.
    /// ⛔ **결재자 0명·기안자 단독은 거부된다**(기안자 단독은 상신 즉시 종결돼 취소할 수 없다).
    /// 합의자는 담지 않는다 — 양식필수 합의자·수신참조·시행자는 상신 때 서버가 병합한다.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_vec")]
    #[schemars(schema_with = "super::flex_str_vec_schema")]
    pub approvers: Vec<String>,
    /// 프로세스 ID(기본 "1000" 기본프로세스).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub proc_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteApprovalLineArgs {
    /// 삭제할 라인의 `lineId`(list_approval_lines 결과의 lineId). 서버가 요구하는 행 객체는 도구가 조회해 채운다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub line_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListApprovalAttachmentsArgs {
    /// 문서 ID(docId). list_approvals 결과의 docId.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub doc_id: String,
    /// 양식 ID(formId). list_approvals 결과의 formId.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub form_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadApprovalAttachmentArgs {
    /// list_approval_attachments 결과 `files[].fileId`(32자 토큰)를 그대로.
    /// ⚠️ 1건만 — 콤마로 여러 개를 주면 서버가 zip으로 묶어 보내므로 도구가 거부한다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub file_id: String,
    /// 저장 경로(절대경로 권장). 예: /tmp/approval.pdf
    pub out_path: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitApprovalArgs {
    /// 양식 ID(formId). 41(외근)/36(연차) 등.
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub form_id: i64,
    /// 문서 제목. ⭐ 양식별 권장 형식은 `get_approval_submission_guide(form_id).guide.draftHelp.defaultDocTitle`/`titleHelp`(예 연차 `[휴가신청] 00/00 오후반차_홍길동(인사&총무팀)`). 사내 관례이므로 사용자 확인 후 확정할 것.
    pub doc_title: String,
    /// 사용할 개인결재라인 ID(save_approval_line으로 준비). 이 라인의 결재자는 eap110A03가 **양식필수 합의자·수신참조·시행자와 병합**해 돌려주고, 그 병합 결과가 그대로 결재선으로 실린다. 즉 라인에는 **결재(3000)만** 담으면 되고 양식필수 합의자를 또 넣으면 중복될 수 있음.
    #[serde(deserialize_with = "super::flex_i64")]
    #[schemars(schema_with = "super::flex_int_schema")]
    pub line_id: i64,
    /// HP 근태신청 저장 요청 body JSON(0hr00011 + create 두 콜에 쓰임). **근태 양식 전용** — 이걸 넘기면 상신 전에 HP 신청 레코드 생성 + interlock 등록(GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup)까지 수행한다. ⭐ **채우는 법·양식별 고정코드·복사용 예시는 `get_approval_submission_guide(form_id).guide.draftHelp.hpApplicationExample`**(예: 출장 linkAtCd"2010"/atCd"2101", 외근 종일 atCd"3101"/linkAtCd"3010"). 신원 필드(coCd/deptCd/empCd/empNm/korNm)는 **submit_approval이 로그인 사용자 값으로 자동 덮어씀** — 예시값 그대로 둬도 됨. 형식: `{"applicationList":[{...,linkAtCd,atCd,atDt,startDt,endDt,startTm,endTm,appDyFg,appDy,appTm,...}],"employeeList":[{...}]}`. 빈 문자열이면 이 단계 전체 생략(= 비근태 양식 경로, 아직 미검증).
    pub hp_application_json: String,
    /// 폼 본문 데이터 JSON 텍스트. `{"ITEMS":{...},"TABLE":{"dbTable1":{...},"dbTable2":{...}}}`. ⭐ **양식별 예시는 `get_approval_submission_guide(form_id).guide.draftHelp.bindDataExample`**. 실제 결재문서에 렌더되는 값이 이것(doc_contents_html이 아님). 서버엔 이중인코딩되어 전송됨.
    pub bind_data_json: String,
    /// 표시용 본문 HTML(raw). 내부에서 encodeURIComponent로 인코딩해 전송. 근태 양식은 본문이 bindData/HP연동으로 채워지므로 **한 줄 요약 HTML(예 `<div>2026-12-16 종일외근</div>`)로도 상신이 통과**한다(4양식 실증). 브라우저는 양식 표 전체를 조립해 보내므로, 문서 뷰 표시 품질까지 맞추려면 표 HTML이 필요(미검증). 빈 문자열 가능 여부는 미확인.
    pub doc_contents_html: String,
    /// 첨부할 로컬 파일 경로 목록(선택). 서버가 도는 머신 기준 절대경로. 비우면 첨부 없이 상신한다.
    /// 파일은 상신 직전에 ECM 에 올라가며, 상신이 실패하면 **문서에 안 붙은 채 ECM 에 남는다**(고아).
    #[serde(default)]
    pub attachments: Vec<String>,
    /// 채번 규칙 ID. 빈 문자열이면 "1001"(기본 채번)이 자동 적용된다 — 보통 그대로 두면 됨.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub numbering_id: String,
    /// **수신참조로 더할 사람**의 empSeq 목록(선택). `find_person`/`person_group`이 주는 값.
    /// 양식필수 수신참조(서버가 자동으로 붙이는 인사총무팀 등) **뒤에 덧붙는다** — 그것들을
    /// 없애거나 대체하지 않는다. 이미 양식필수에 들어 있는 사람은 건너뛰고 응답 `cc.skipped`에 적는다.
    /// ℹ️ **수신참조는 알림을 보내지 않는다** — 그 사람의 수신참조함 목록에 문서가 보일 뿐이다
    /// (알림은 결재선에 든 사람에게만 간다). 그래도 문서가 그들에게 열람 가능해지므로 대상은
    /// 사용자에게 확인받고 넣을 것. 상신 뒤 실제로 실렸는지는 도구가 재조회해 `cc.verified`로 알려준다.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_vec")]
    #[schemars(schema_with = "super::flex_str_vec_schema")]
    pub cc_emp_seqs: Vec<String>,
    /// **수신참조로 더할 부서**의 deptId 목록(선택). `org_chart`/`find_person`의 deptId.
    /// 부서를 넣으면 **그 부서원 전원**의 수신참조함에 문서가 뜬다 — 인원을 확인하고 쓸 것.
    /// 부서도 `cc.verified` 판정 대상이다(저장본 `hidRefer`에 전개 전 원본으로 남아 있다).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_vec")]
    #[schemars(schema_with = "super::flex_str_vec_schema")]
    pub cc_dept_ids: Vec<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelApprovalArgs {
    /// 취소할 문서의 docId(list_approvals/read_approval의 docId).
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub doc_id: String,
    /// form_id(list_approvals의 formId). doc_sts=30(결재 진행중) 문서의 결재취소(eap110A54)에 필요. 상신 직후(20) 문서만 취소할 땐 생략 가능.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub form_id: String,
    /// true면 결재취소→상신취소 후 임시보관 문서까지 완전 삭제(eap110A19). false(기본)면 임시보관에 남긴다.
    #[serde(default)]
    pub purge: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteTempApprovalArgs {
    /// 삭제할 임시보관 문서 docId. 여러 건은 콤마구분(예 "140764,140716"). list_approvals(box_name:"draft")의 docId.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub doc_ids: String,
}
