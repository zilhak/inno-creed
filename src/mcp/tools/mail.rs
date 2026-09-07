//! 메일 도구.
//!
//! 라우터는 `mail_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::mail`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, map_domain_err_ctx, Amaranth};
use crate::client::GwClient;
use crate::mcp::args::mail::*;
use crate::modules;

/// 받는사람 미지정 시 본인 앞(표시형)으로. 발송·임시저장이 같은 규칙을 쓴다.
fn recipient_or_self(c: &GwClient, to: &Option<String>) -> String {
    to.clone().unwrap_or_else(|| {
        format!("{} <{}@{}>", c.emp_name(), c.email_addr(), c.email_domain())
    })
}

#[tool_router(router = mail_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(description = "메일함(폴더) 목록을 조회한다")]
    async fn list_mailboxes(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_mailboxes(&self.client)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "받은메일함 최근 20통을 조회한다. 메일함 번호는 계정마다 달라 이름(INBOX)으로 해석한다. ⚠️ 응답은 서버 원본 봉투 그대로다 — 메일 배열은 `Records`(다른 목록 도구처럼 정규화돼 있지 않음), 각 항목의 `muid`가 read_mail/delete_mail 키, `attach`(bool)가 첨부 유무.")]
    async fn list_mail_inbox(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_inbox(&self.client, 1, 20)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "임시보관함(DRAFTS) 최근 20통을 조회한다 — save_mail_draft로 저장한 초안을 발송 전에 사용자에게 확인받는 경로다. 확인되면 그 항목의 muid를 send_mail_from_draft(draft_muid)에 넘겨 초안 그대로 발송한다. ⚠️ 응답은 서버 원본 봉투 그대로다 — 메일 배열은 `Records`(list_mail_inbox와 동일), 각 항목의 `muid`가 read_mail/delete_mail의 키다. 메일함 번호는 계정마다 달라 이름(DRAFTS)으로 해석한다. ⚠️ 전자결재 임시보관함과는 무관하다(그쪽은 list_approvals(box_name=\"draft\")).")]
    async fn list_mail_drafts(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_drafts(&self.client, 1, 20)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일을 **새로 조립해 발송한다**(2단계: 작성폼 초기화→발송). 받는사람 미지정 시 본인에게. **여러 명에게 보내려면 to에 콤마로 잇는다**(`\"홍길동 <hong@innogrid.com>,kim@innogrid.com\"` — 표시형과 순수 주소를 섞어도 된다). 참조는 cc, 숨은참조는 bcc에 같은 형식으로. attachments에 로컬 파일 경로를 주면 첨부 발송. ⚠️ 발송은 되돌릴 수 없다(수신자에게 나가면 회수 불가) — 곧바로 보내지 말고, 먼저 save_mail_draft로 보낼 형상을 임시보관함에 만들고 list_mail_drafts로 사용자에게 확인을 요청한 뒤, 확인받고 나서는 **이 도구가 아니라 send_mail_from_draft(draft_muid)로 그 초안을 그대로 발송한다**(확인받은 형상과 실제 발송물이 어긋날 여지가 없고, 원본 초안 정리도 그 도구가 한다). 이 도구는 **초안을 거치지 않는 직접 발송용**이다 — 사용자가 즉시 발송을 명시적으로 지시했거나, 본인 앞 메모·자동화처럼 사람 확인이 필요 없는 발송에 쓴다. 아마란스에 등록해 둔 **서명이 기본으로 본문 끝에 붙는다**(웹에서 보낸 것과 같은 형상) — 붙지 않아야 하면 `signature:false`. 응답의 `signature_attached`가 실제로 붙었는지를 알려준다(서명 미등록 계정은 켜 두어도 false)."
    )]
    async fn send_mail(
        &self,
        Parameters(a): Parameters<SendMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let to = recipient_or_self(&self.client, &a.to);
        let cc = a.cc.as_deref().unwrap_or("");
        let bcc = a.bcc.as_deref().unwrap_or("");
        let data = modules::mail::send_mail(
            &self.client,
            &to,
            cc,
            bcc,
            &a.subject,
            &a.html,
            &a.attachments,
            a.signature,
        )
        .await
        .map_err(map_domain_err_ctx("메일 발송 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "to": to,
            "cc": cc,
            "bcc": bcc,
            "subject": a.subject,
            "attachments": a.attachments.len(),
            // 요청값(a.signature)이 아니라 **실제로 붙었는지**. 서명 미등록 계정에서는 켜 두어도 false다.
            "signature_attached": data.get("signature_attached"),
            "note": "발송 성공(result:true). 도착 확인은 list_mail_inbox/보낸메일함 재조회 권장"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "메일을 임시보관함(DRAFTS)에 저장한다 — **발송하지 않는다**(수신자에게 아무것도 가지 않는다). 발송 전 사람 확인을 받는 표준 경로라 send_mail보다 이 도구를 먼저 쓴다 — 초안을 만들고 list_mail_drafts로 사용자에게 확인받은 뒤, 확인되면 **send_mail_from_draft(draft_muid)로 그 초안을 그대로 발송**하거나 사용자가 아마란스 웹에서 직접 보낸다. 다만 사용자가 즉시 발송을 명시적으로 지시했다면 초안을 거치지 말고 곧바로 send_mail을 쓴다. 받는사람 미지정 시 본인. **여러 명이면 to에 콤마로 잇는다**; 참조는 cc, 숨은참조는 bcc에 같은 형식으로 — **여기 넣은 참조는 send_mail_from_draft가 그대로 승계해 발송한다.** attachments에 로컬 파일 경로를 주면 첨부까지 붙여 저장. 반환 draft_muid = 저장된 임시보관 메일의 muid. 아마란스에 등록해 둔 **서명이 기본으로 본문 끝에 붙어 저장된다**(웹에서 보낸 것과 같은 형상. send_mail_from_draft가 본문째로 승계하므로 두 번 붙지 않는다) — 붙지 않아야 하면 `signature:false`. 응답의 `signature_attached`가 실제로 붙었는지를 알려준다(서명 미등록 계정은 켜 두어도 false). ⚠️ 전자결재 임시보관함과는 무관하다(그쪽은 list_approvals(box_name=\"draft\"))."
    )]
    async fn save_mail_draft(
        &self,
        Parameters(a): Parameters<SaveMailDraftArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let to = recipient_or_self(&self.client, &a.to);
        let cc = a.cc.as_deref().unwrap_or("");
        let bcc = a.bcc.as_deref().unwrap_or("");
        let data = modules::mail::save_mail_draft(
            &self.client,
            &to,
            cc,
            bcc,
            &a.subject,
            &a.html,
            &a.attachments,
            a.signature,
        )
        .await
        .map_err(map_domain_err_ctx("메일 임시저장 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "to": to,
            "cc": cc,
            "bcc": bcc,
            "subject": a.subject,
            "attachments": a.attachments.len(),
            "draft_muid": data.get("draft_muid"),
            "mail_key": data.get("mail_key"),
            "sent": false,
            // 임시보관함을 재조회해 그 muid를 실제로 찾았는지. false여도 저장 자체가 실패한 것은
            // 아니지만(조회가 막혔을 수 있다), 그 경우 사람이 임시보관함을 눈으로 확인해야 한다.
            "verified_by_readback": data.get("verified_by_readback"),
            // 요청값(a.signature)이 아니라 **실제로 붙었는지**. 서명 미등록 계정에서는 켜 두어도 false다.
            "signature_attached": data.get("signature_attached"),
            "note": "임시보관함에 저장만 됨(발송 아님). 목록 확인은 list_mail_drafts"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "임시보관함(DRAFTS)에 저장된 초안을 **실제로 발송한다** — ⚠️ 되돌릴 수 없다. save_mail_draft로 만든 초안을 사람이 확인한 뒤 '이제 보내라'고 지시할 때 쓰는 도구다. 제목·본문·수신자·첨부는 **초안에 저장된 것을 그대로** 쓴다(to 인자를 주면 수신자만 덮어쓴다). 발송 성공 후 임시보관함 원본을 삭제한다 — ⚠️ 이 삭제는 휴지통을 거치지 않는 것으로 보인다(발송 직후 휴지통 건수 불변 관측). 삭제가 실패하면 발송은 성공으로 보고하되 `draft_deleted:false`가 실리니, 그때는 사람이 임시보관함에서 지워야 같은 메일을 또 보내지 않는다. 제약 — ① **초안을 못 찾으면 보내지 않는다**: 실재 확인이 임시보관함 **최근 20건**만 훑으므로 초안이 21건 이상 쌓인 계정에서는 오래된 초안을 이 도구로 못 보낸다(웹에서 발송하거나 초안을 정리할 것). ② 본문·제목·첨부목록 중 하나라도 초안 응답에서 읽어내지 못하면 **보내지 않는다**(내용이 빈 메일·첨부 누락 방지). ③ 첨부는 승계하지만 **파일명에 콤마가 있거나, 같은 이름의 첨부가 둘 이상이거나, 대용량 첨부(bigFile)면 거부**한다(그 경로는 미실측 — 웹에서 발송할 것). ④ **참조(cc)·숨은참조(bcc)는 초안에 저장된 것을 그대로 승계**해 발송한다(반환값의 cc/bcc로 무엇이 실렸는지 확인할 것). 다만 참조를 응답에서 **읽어내지 못하면 보내지 않는다** — 참조가 빠진 채 나가는 것을 막기 위해서다."
    )]
    async fn send_mail_from_draft(
        &self,
        Parameters(a): Parameters<SendMailFromDraftArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        // 여기서는 `recipient_or_self`를 쓰지 않는다 — 미지정의 뜻이 "본인에게"가 아니라
        // "초안에 저장된 수신자 그대로"이기 때문이다(모듈이 그 판단을 한다).
        let to = a.to.as_deref().unwrap_or("");
        let data = modules::mail::send_mail_from_draft(&self.client, &a.draft_muid, to)
            .await
            .map_err(map_domain_err_ctx("초안 발송 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "메일을 삭제한다(휴지통 이동). uids=콤마구분 muid. muid 출처는 list_mail_inbox, 또는 임시보관함 정리라면 list_mail_drafts(= save_mail_draft가 낸 draft_muid).")]
    async fn delete_mail(
        &self,
        Parameters(a): Parameters<DeleteMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        modules::mail::delete_mails(&self.client, &a.uids)
            .await
            .map_err(map_domain_err_ctx("메일 삭제 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "uids": a.uids,
            "deleted": true,
            "note": "휴지통 이동됨(muid 재부여 — 이후 추적은 재조회 필요)"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "⚠️ **읽음 처리된다** — 서버측 읽음 플래그가 세워진다(실증). 사용자가 아직 안 읽은 메일을 대신 열면 그 사람의 미읽음 표시가 사라진다. 되돌리려면 `mark_mail_unread` — ⚠️ **받은메일함 최근 200건 안의 메일만 되돌릴 수 있다**(그 밖이면 거절되므로 되돌림을 전제하고 열지 말 것). 메일 1건의 본문(평문)·헤더·첨부목록을 조회한다. 본문 HTML은 렌더링하지 않고 평문화(외부 이미지 자동로드 안 함, remoteResourceCount로 경고). 본문에 박힌 이미지 중 **이 서버가 가진 것**은 `inlineImages[]`로 나오고 `download_body_image`로 받아볼 수 있다(외부 호스트 이미지는 일부러 빼며, 그 개수가 remoteResourceCount다). 수신자는 to/cc/bcc로 낸다 — ⚠️ **받은 메일의 bcc는 대개 빈 값**이다(숨은참조는 수신자에게 보이지 않는 필드라 헤더에 남지 않는다). muid=list_mail_inbox의 muid."
    )]
    async fn read_mail(
        &self,
        Parameters(a): Parameters<ReadMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::mail::read_mail(&self.client, &a.muid)
            .await
            .map_err(map_domain_err_ctx("메일 조회 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "받은메일 1건을 **읽지 않음으로 되돌린다**(mail002A15). `read_mail`이 서버측 읽음 플래그를 세우므로, 대신 읽어준 메일을 사용자가 다시 '안 읽은 메일'로 만나게 하려면 이것을 쓴다. 반영 여부는 목록 재조회로 검증해 `verifiedByReadback`으로 보고한다 — 서버 응답 자체는 성패를 구분하지 못한다. 이미 미읽음이면 서버에 아무것도 보내지 않고 `already:true`. ⚠️ 받은메일함 **최근 200건** 안의 메일만 대상이다(그보다 오래되면 대상을 확인할 수 없어 거절)."
    )]
    async fn mark_mail_unread(
        &self,
        Parameters(a): Parameters<MarkMailUnreadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::mark_unread_and_verify(&self.client, &a.muid)
            .await
            .map_err(map_domain_err_ctx("읽지 않음 처리 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일함별 미읽음·전체 개수와 계정 전체 집계를 조회한다(mail000A03, 부작용 없음). `list_mailboxes`에 없는 집계를 함께 준다 — 응답 배열의 각 항목은 boxnameSeq/count(=미읽음)/totalCount이고 **마지막 항목이 계정 전체 집계**(unreadCount·toMeCount=나에게 온 메일·flaggedCount·attachCount·totalCount)다. 메일함 이름은 주지 않으므로 이름이 필요하면 `list_mailboxes`의 mboxSeq와 맞춰볼 것."
    )]
    async fn mailbox_counts(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::mailbox_counts(&self.client)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일 첨부파일을 다운로드해 out_path에 저장한다(실행하지 않고 저장만). **file_sn 은 순번이 아니라 read_mail 응답 `attachments[].fileSn` 의 긴 토큰 문자열을 그대로** 넣는다 — 숫자(0,1)를 넣으면 서버가 422로 거절한다."
    )]
    async fn download_mail_attachment(
        &self,
        Parameters(a): Parameters<DownloadMailAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data =
            modules::mail::download_attachment(&self.client, &a.muid, &a.file_sn, &a.out_path)
                .await
                .map_err(map_domain_err_ctx("첨부 다운로드 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
