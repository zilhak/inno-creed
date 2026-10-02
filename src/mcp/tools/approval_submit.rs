//! 전자결재 — 상신·회수 도구.
//!
//! 라우터는 `approval_submit_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::approval_submit`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err_ctx, Amaranth};
use crate::mcp::args::approval::*;
use crate::modules;

#[tool_router(router = approval_submit_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "문서를 상신(제출)한다. ⚠️ 실제 결재요청·수신참조 통지가 나감(근태 양식은 양식필수 수신참조가 자동 병합돼 10여 명에게 보인다) — 시험 상신은 **본인 아닌 결재자를 최소 1명 포함한**(미리 양해를 구한 동료) 별도 결재라인으로 하고, 끝나면 `cancel_approval(doc_id, form_id, purge=true)`로 되돌릴 것(상신 직후 문서는 doc_sts=30이라 form_id 필요). ⛔ **기안자 단독 결재선으로는 상신하지 말 것 — 즉시 종결(doc_sts 90)돼 취소가 불가능해진다**(그래서 이 도구가 상신 전에 막는다). ⭐ **hp_application_json / bind_data_json 을 어떻게 채우는지는 `get_approval_submission_guide(양식명 또는 form_id)` 의 `draftHelp` 를 먼저 조회할 것** — 양식별 고정코드(atCd/linkAtCd 등)·의미별 채울 필드·복사용 실동작 예시(hpApplicationExample/bindDataExample)·권장 제목(defaultDocTitle)을 준다(CLI --help 격). 신원은 이 도구가 로그인 사용자 값으로 **자동 주입**한다 — 코드계(coCd/deptCd/empCd)·이름뿐 아니라 **문서에 렌더되는 표시문자열(부서명·직급·직책, `singleDeptNm`/`empNmDutyNm`/`employees` 등)까지** 조직도 값으로 덮어쓰므로 예시값을 그대로 둬도 됨. 결재라인은 `suggest_approval_line`으로 후보를 받아 **사용자 확인 후** save_approval_line으로 등록할 것. 흐름(근태): 0hr00011 → create(appSq 획득) → eap110A03(결재선 병합 + 양식별 form_d_tp 취득) → HP interlock 등록 3콜(GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup) → eap110A06 상신. **이 interlock 등록이 빠지면 2099(HP_HPD0110_000XX)** — 근태 상신 실패의 사실상 유일한 원인이었다(잔여 draft·날짜·payload 가설은 전부 반증됨). 성공 시 새 docId를 반환한다 — **docId 발급을 도구가 직접 판정하므로**(없으면 에러) 응답이 오면 상신된 것이다. 실증 범위: 근태 4양식(연차36/출장40/외근41/휴일43) 순수 API 상신·취소 e2e. HP 비연동(비근태) 양식은 hp_application_json 없이 호출하는 경로가 있으나 **미검증**. **수신참조를 더하려면** `cc_emp_seqs`(사람 empSeq — `find_person`/`person_group`)·`cc_dept_ids`(부서 deptId — `org_chart`)에 담는다. 양식필수 수신참조(서버가 자동으로 붙이는 인사총무팀 등) **뒤에 덧붙기만 한다**. ⛔ **양식필수 수신참조는 건드리지 않는다 — 제거 수단은 없고, 만들 계획도 없다**: 양식필수란 '이 양식의 문서는 그들이 봐야 한다'는 회사 규칙이 양식에 박힌 것이라 기안자가 뺄 대상이 아니다(서버도 a03 진입마다 재주입한다). 사용자가 양식필수를 빼달라고 하면 '그건 양식이 정한 필수 수신처라 뺄 수 없다'고 답할 것. 이미 양식필수에 들어 있는 대상은 건너뛰고 `cc.skipped`에 적는다. ℹ️ **수신참조는 알림을 보내지 않는다** — 그 사람의 **수신참조함 목록에 문서가 보일 뿐**이다(알림이 가는 것은 결재선에 든 사람뿐). 부서를 넣으면 그 부서원 전원의 수신참조함에 뜬다. 그래도 문서 내용이 그들에게 열람 가능해지는 것이므로 대상은 사용자에게 확인받고 넣을 것. 더한 것이 실제로 저장됐는지는 도구가 재조회해(eap110A03 `hidRefer`) `cc.verified`로 판정하고, 문서에 실린 수신참조 전체를 `cc.savedOnDoc`에 사람이 읽는 표기로 돌려준다 — `cc.verified:false`면 그 대상은 문서를 받지 못한다. 실증: 사람·부서 추가 모두 e2e 완료(연차36 상신 → 저장 확인 → 수신참조함 도달 확인 → 취소, 2026-10-02). **첨부**는 `attachments`에 로컬 파일 경로를 주면 상신 직전에 ECM에 올려 문서에 붙인다 — 첨부 2건(한글 파일명 포함) e2e 실증 완료(상신→read-back→바이트 대조→취소). ⚠️ 상신이 실패하면 올라간 파일이 문서에 안 붙은 채 ECM에 남는다. ⚠️ **`cancel_approval(purge:true)`로 문서를 지워도 첨부는 ECM에 남는다** — 시험 상신을 되돌릴 때 첨부는 따로 정리해야 한다."
    )]
    async fn submit_approval(
        &self,
        Parameters(a): Parameters<SubmitApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::submit_approval(
            &self.client,
            a.form_id,
            &a.doc_title,
            a.line_id,
            &a.hp_application_json,
            &a.bind_data_json,
            &a.doc_contents_html,
            &a.numbering_id,
            &a.attachments,
            &a.cc_emp_seqs,
            &a.cc_dept_ids,
        )
        .await
        .map_err(map_domain_err_ctx("상신 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "상신 문서를 취소한다. 문서 상태(doc_sts)에 따라 결재취소(eap110A54)→상신취소(eap110A18)→(purge시)임시보관삭제(eap110A19)를 순차 실행. ⚠️ doc_sts=30(결재 진행중) 문서는 결재취소가 선행돼야 하며 form_id 필요(list_approvals의 formId). 상신 직후(20)면 form_id 없이 상신취소만. purge=true면 임시보관 문서까지 완전 삭제. **검증은 도구가 한다** — 실행 후 문서 상태를 재조회해 `ok`/`verified_by_readback`(purge=false면 doc_sts 10 복귀, purge=true면 doc_sts 999=삭제)로 알려주므로 별도 확인 호출이 필요 없다. `ok:false`는 **반영이 확인되지 않았다**는 뜻이니(반영 실패이거나 확인 실패 — `postState`/`note`에 구분해 담긴다) 그대로 사용자에게 알릴 것. 없는 docId·남의 문서(기안자를 확인할 수 없는 경우 포함)·이미 삭제된 문서를 되돌리려는 요청은 **실행 없이 에러**로 끝난다. **취소가 실증된 상태는 10(임시보관)·20(상신)·30(결재 진행중)뿐**이라 종결(90)·반려(100) 등은 거동이 관측되지 않아 실행 없이 거부한다 — 그 문서는 아마란스 웹에서 처리할 것. 이미 삭제된 문서에 purge=true를 다시 걸면 `already:true`와 빈 `steps`로 '할 일이 없었다'를 알려준다."
    )]
    async fn cancel_approval(
        &self,
        Parameters(a): Parameters<CancelApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::cancel_and_verify(
            &self.client,
            a.doc_id.trim(),
            a.form_id.trim(),
            a.purge,
        )
        .await
        .map_err(map_domain_err_ctx("상신취소 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "임시보관 전자결재 문서를 삭제한다(eap107A25). doc_ids는 콤마구분 docId(list_approvals(box_name:\"draft\")에서 확인). ⚠️ 실제 삭제(복구 불가). 용도는 상신취소(purge=false)로 되돌아온 문서나 시험 잔여물 정리 — **상신 실패(2099)의 해결책이 아니다**(잔여 draft 원인설은 반증, 원인은 interlock 등록 누락). 삭제 후 draft 재조회로 검증."
    )]
    async fn delete_temp_approval(
        &self,
        Parameters(a): Parameters<DeleteTempApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::delete_temp_approval(&self.client, a.doc_ids.trim())
            .await
            .map_err(map_domain_err_ctx("임시보관 삭제 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
