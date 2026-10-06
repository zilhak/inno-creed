//! 전자결재 상신/상신취소(쓰기) — `eap110A06`(상신) / `eap110A98`+`eap110A18`(상신취소).
//! 요청 형식·호출 순서는 실제 트래픽 캡처로 확정했다.
//! ⚠️ **실제 결재가 발생**한다(결재요청·수신참조 통지가 나감). 테스트는 반드시 테스트 결재라인으로.
//!
//! 상신 흐름(팝업이 하던 일을 재현):
//!  0) approkey = "ERP_<uuid>" 생성(클라 생성이 맞음 — 서버 발급 토큰 아님).
//!  1) 근태 양식: 0hr00011(검증/스테이징) → create(HP신청 커밋 → appSq/appDt 반환).
//!  2) eap110A03(appLineId=line_id)로 **완전 병합된 결재선(kyuljaeResult=양식필수 합의+개인라인 결재)
//!     + 수신참조(m_Refer) + 시행자(m_Oper) + form_info.form_d_tp(양식별 interlock 식별자)** 획득.
//!  3) 근태 양식: HP interlock 등록 3콜 — GetLinkKey(→linkKey) → saveAttendApplicationLinkKey(linkKey↔appSq 바인딩)
//!     → SetEnageGroup(approKey에 linkKey·콜백API 등록). ⭐ **이게 2099의 핵심**(아래).
//!  4) pTEAG_APPDOC_LINE = kyuljaeResult 무가공, pRefer = m_Refer(+org_div), pOper = m_Oper(+org_div).
//!     — 이 셋은 성공 브라우저 상신 payload와 바이트 동일(실측 대조).
//!  5) eap110A06 POST → resultData.result = 신규 docId.
//!
//! ⚠️ **2099(HP_HPD0110_000XX)의 원인은 interlock 등록 3콜 누락**이다(2026-08-05 무필터 전량 캡처로 확정).
//!    eap110A06의 eap→HP 서버간 연동은 approKey에 등록된 linkKey를 찾는데, 등록이 없으면 대상이 없어 HP가 500을 준다.
//!    (GetLinkKey/SetEnageGroup 누락 → "Internal Server Error", saveAttendApplicationLinkKey 누락 → "종결 처리 오류".)
//!    초기 캡처가 `/human/`·`/eap/`만 필터링해 `/system/apiUtilEap/*`·`/personal/hpd0110/*`를 통째로 놓친 게
//!    장기 오진의 원인이었다. **반증된 가설(재도입 금지)**: payload/pOper 누락, 잔여 임시 draft·대기신청 충돌,
//!    날짜, doc_sts(10/20), eap prep 콜, 쿠키·토큰·헤더·전송계층 지문, 포털로그인(gw050B01) 세션 — 전부 실측 반증.
//!    HP↔eap 링크도 "서버가 empCd+atDt로 매칭"이 아니라 **linkKey↔appSq 명시 바인딩**이다(실측으로 확정).

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::modules::approval_body;
use crate::modules::approval_line::LineShape;
use crate::util::days_to_ymd;

/// 삭제된 문서의 `doc_sts`. 실측(2026-08-06, docId 141826): `eap110A19`로 지운 뒤 `eap110A98`은
/// **에러가 아니라** `resultCode 0` + `doc_sts:"999"`(+ `doc_no_base:""`)를 준다. 삭제 직후와
/// 한참 뒤 재조회가 같은 값이었다. 같은 시점 `read_approval`은 2156("삭제된 문서는 열 수 없습니다"),
/// 상신함·임시보관함 양쪽에서 소멸.
///
/// 관측된 `doc_sts`: `10`(임시보관) `20`(상신) `30`(결재 진행중) `90`(종결) `100`(반려) `999`(삭제).
const DOC_STS_DELETED: &str = "999";

/// `eap110A98`이 알려주는 문서의 존재 여부. **판정에서 이 셋을 절대 뭉개지 않는다** —
/// "지워졌다"(사실)와 "못 찾겠다"(호출자 잘못)와 "못 읽었다"(장애)는 서로 다른 결말을 가져야 한다.
/// (판독 불가는 이 타입이 아니라 `Err`로 표현된다.)
#[derive(Debug)]
enum DocState {
    /// `resultData`가 객체 — 문서가 존재한다(삭제 상태 `999`도 여기 포함된다).
    /// `owner`는 기안자 empSeq(`user_id`). **필드가 없으면 빈 문자열**이고, 소유권 가드가
    /// 그것도 불일치로 보고 거부한다(`error::NotOwner`·`resource::check_owner`와 같은 규약).
    Found { doc_sts: String, owner: String },
    /// `resultData`가 `null` — 그 docId로 조회되는 문서가 없다.
    /// ⚠️ 실측상 **미발급 번호와 형식 오류가 이 응답을 공유**한다(둘 다 `resultData:null`).
    /// 접근 권한이 없는 문서도 여기로 오는지는 미관측이다.
    NotFound,
}

/// `eap110A98` 응답 봉투 → `DocState`. **분류도 판정**이라 네트워크에서 떼어내 순수 함수로 두고
/// 실측 표로 못박는다(판정의 나머지 절반은 `pre_verdict`/`cancel_verdict`).
///
/// ⚠️ 판별자는 **`resultData`의 유무 하나뿐**이다. 실측(입력 12종)에서 존재/미발급/형식오류가
/// 전부 `http 200` + `resultCode 0` + `resultMsg "성공하였습니다."`로 동일했다 — 코드·메시지로는
/// 아무것도 가를 수 없다. 그 외 형태(비200 응답, `resultData` 키 자체가 없음, 객체도 `null`도 아님,
/// 객체인데 `doc_sts`가 없음)는 **해석하지 않고 에러로 올린다**(모르면 멈춘다).
/// 특히 이것들을 `NotFound`나 `Found{999}`로 접으면 **존재한 적 없는 문서를 "이미 삭제됐음을
/// 조회로 확인했다"고 보고**하게 된다.
fn classify(http: u64, resp: &Value) -> Result<DocState> {
    if http != 200 {
        bail!("문서 상태 조회(eap110A98) 실패: http={http} 응답={resp}");
    }
    match resp.get("resultData") {
        Some(Value::Null) => Ok(DocState::NotFound),
        Some(Value::Object(rd)) => {
            let doc_sts = crate::util::json_str(rd.get("doc_sts"));
            if doc_sts.is_empty() {
                bail!("문서 상태 조회(eap110A98) 응답에 doc_sts가 없다: {resp}");
            }
            Ok(DocState::Found {
                doc_sts,
                // 기안자 empSeq. 소유권 가드에 쓴다(추가 호출 없이 여기서 얻는다).
                owner: crate::util::json_str(rd.get("user_id")),
            })
        }
        _ => bail!("문서 상태 조회(eap110A98) 응답 형태를 해석할 수 없다: {resp}"),
    }
}

/// 문서 상태 조회 — `eap110A98`. **성공판정 없이**(`call_raw`) 봉투째 받아 `classify`에 넘긴다:
/// 취소된 문서를 `c.call`로 읽으면 실패 코드가 `bail!`이 돼 "취소 성공"과 "장애"가 같은 모양이 되기 때문이다.
/// 이 함수에는 판단이 없다 — 호출과 배선뿐이다.
async fn doc_status(c: &GwClient, doc_id: &str) -> Result<DocState> {
    let raw = c
        .call_raw(
            "/eap/eap110A98",
            &json!({ "docId": doc_id, "pageCode": "UBAP002" }),
        )
        .await?;
    classify(
        raw.get("http").and_then(|v| v.as_u64()).unwrap_or(0),
        raw.get("response").unwrap_or(&Value::Null),
    )
}

/// 실행 3콜(A54/A18/A19)의 성공 신호. 실측상 성공하면 `resultData.returnValue:1`이다.
/// 값이 아예 없으면 **판단 보류**(`None`) — 그 경우 최종 판정은 read-back이 한다.
fn return_value(rd: &Value) -> Option<i64> {
    rd.get("returnValue").and_then(|v| v.as_i64())
}

/// 실행 콜 1건의 결과. `ok=false`면 서버가 성공 신호가 아닌 값을 준 것이다.
fn step_result(name: &str, api: &str, rd: &Value) -> Value {
    let rv = return_value(rd);
    json!({
        "step": name,
        "api": api,
        "returnValue": rv,
        // returnValue가 없는 응답(본문 없음)은 실패로 보지 않는다 — read-back이 최종 판정이다.
        "ok": rv.is_none_or(|v| v == 1)
    })
}

/// 사전조회 결과로 정해지는 것 — **실행 콜을 쏘기 전에** 결말이 갈린다.
enum PreVerdict {
    /// 3단계 실행으로 진행. 값은 현재 `doc_sts`.
    Proceed(String),
    /// 이미 삭제돼 있고 요청도 삭제(`purge=true`)였다 — 목표 상태에 이미 도달. 아무것도 실행하지 않는다.
    AlreadyDeleted,
    /// 실행하지 않고 실패로 끝낸다(호출자 잘못·소유권·불가능한 요청).
    Reject(anyhow::Error),
}

/// 취소 실행 **뒤** 재조회 결과. `Unreadable`은 "취소가 실패했다"가 아니라 **"확인하지 못했다"**다 —
/// 둘을 같은 칸에 넣지 않기 위해 별도 변형으로 둔다.
enum PostState {
    Found(String),
    NotFound,
    Unreadable(String),
}

/// 사전조회 → 결말. **실행 전 판단은 전부 여기 모은다**(네트워크 없이 결정되므로 순수 함수로 두고
/// 테스트로 고정한다 — 조립부가 무테스트면 "거부해야 할 것을 거부한다"가 아무 데서도 확인되지 않는다).
/// `me`는 로그인 사용자 empSeq, `form_id`는 결재취소(eap110A54)에 필요한 값.
fn pre_verdict(doc_id: &str, pre: &DocState, purge: bool, me: &str, form_id: &str) -> PreVerdict {
    let (doc_sts, owner) = match pre {
        // 그 docId로 조회되는 문서가 없다 = 호출자가 준 값이 잘못됐다. 실행 콜은 쏘지 않는다.
        // ⚠️ 접근 권한이 없는 문서도 여기로 오는지는 미관측이라 fail-closed로 끝낸다.
        DocState::NotFound => {
            return PreVerdict::Reject(
                crate::error::InvalidInput::new(format!(
                    "docId={doc_id} 로 조회되는 문서가 없습니다 — 번호를 확인하세요(list_approvals의 docId). 접근 권한이 없는 문서일 수도 있습니다."
                ))
                .into(),
            );
        }
        DocState::Found { doc_sts, owner } => (doc_sts.as_str(), owner.as_str()),
    };

    if doc_sts == DOC_STS_DELETED {
        return if purge {
            PreVerdict::AlreadyDeleted
        } else {
            PreVerdict::Reject(anyhow!(
                "docId={doc_id} 는 이미 삭제된 문서(doc_sts={DOC_STS_DELETED})라 임시보관으로 되돌릴 수 없습니다."
            ))
        };
    }

    // 소유권 가드 — 기안자가 본인일 때만 실행한다. 남의 문서에 취소 콜을 쏘면 서버가 어떻게
    // 반응하는지 **미관측**이라 시험 삼아 쏘지 않는다. `resource::check_owner`와 **같은 형태의
    // fail-closed**다: 기안자를 읽지 못했거나(응답에 `user_id` 없음) 본인 empSeq를 모르면
    // 빈 문자열끼리의 비교가 아닌 한 불일치가 돼 **거부**된다. "모른다"를 "내 것"으로 치는
    // 순간 가드가 아니고, 본인 empSeq가 빈 경우엔 **모든 문서**에 대해 가드가 꺼진다.
    // 막혀서 못 지운 문서는 웹에서 지우면 되지만, 남의 문서에 쏜 취소는 되돌릴 수 없다.
    if owner != me {
        return PreVerdict::Reject(
            crate::error::NotOwner {
                relation: "기안",
                kind: "문서",
                action: "취소",
                owner: owner.to_string(),
                me: me.to_string(),
            }
            .into(),
        );
    }

    // 취소가 실제로 어떻게 동작하는지 **관측된 상태는 10/20/30뿐**이다(임시보관·상신·결재 진행중).
    // 종결(90)·반려(100) 등에 A54/A18/A19를 쏘면 무슨 일이 벌어지는지 모르고, A19는 지울 대상이
    // 없어도 `returnValue:1`을 주므로 응답으로도 알 수 없다. 파괴적인데 결과를 모르면 쏘지 않는다.
    if !matches!(doc_sts, "10" | "20" | "30") {
        return PreVerdict::Reject(anyhow!(
            "docId={doc_id} 는 doc_sts={doc_sts} 라 취소하지 않습니다 — 이 상태의 취소 거동은 관측되지 않았습니다(취소가 확인된 상태는 10 임시보관·20 상신·30 결재 진행중뿐). 아마란스 웹에서 처리하세요."
        ));
    }

    // 진행중(30)은 결재취소(eap110A54)가 선행돼야 하고 그 콜은 formID를 요구한다.
    if doc_sts == "30" && form_id.trim().is_empty() {
        return PreVerdict::Reject(
            crate::error::InvalidInput::new(
                "doc_sts=30(결재 진행중) 문서는 결재취소(eap110A54)가 선행돼야 하며 form_id가 필요합니다 — list_approvals의 formId를 넘기세요.",
            )
            .into(),
        );
    }

    PreVerdict::Proceed(doc_sts.to_string())
}

/// 실행 결과 + 사후 상태 → 최종 응답. 여기서만 `ok`가 정해진다(네트워크 무관 → 테스트 가능).
fn cancel_verdict(
    doc_id: &str,
    pre_sts: &str,
    purge: bool,
    steps: Vec<Value>,
    post: &PostState,
) -> Value {
    let steps_ok = steps.iter().all(|s| s["ok"] == json!(true));
    // ⚠️ 문서가 사라졌다는 **양성 신호(doc_sts 999)** 가 있으므로, "못 찾음"·"못 읽음"을
    // 소멸의 증거로 세지 않는다. 확인하지 못한 것은 확인하지 못한 것으로 보고한다.
    let verified = match post {
        PostState::Found(sts) => verify_cancel(purge, sts),
        PostState::NotFound | PostState::Unreadable(_) => false,
    };
    let ok = steps_ok && verified;
    let note = cancel_note(ok, steps_ok, steps.is_empty(), purge, post);
    json!({
        "kind": "approvalCancelled",
        "ok": ok,
        "verified_by_readback": verified,
        "docId": doc_id,
        "preDocSts": pre_sts,
        "postState": match post {
            PostState::Found(_) => "found",
            PostState::NotFound => "notFound",
            PostState::Unreadable(_) => "unreadable",
        },
        "postDocSts": match post {
            PostState::Found(sts) => Value::String(sts.clone()),
            _ => Value::Null,
        },
        "steps": steps,
        "purged": purge,
        "note": note
    })
}

/// 취소 read-back 판정. `post_sts`는 취소 실행 뒤 `eap110A98`이 준 `doc_sts`.
/// - purge=true  → 삭제 상태(`999`)인가. **판독 불가·문서 없음은 여기 오지 않는다**(호출부가 이미 걸렀다).
/// - purge=false → 임시보관(`10`)으로 내려왔는가. `999`는 성공이 아니다(지우려던 게 아니다).
fn verify_cancel(purge: bool, post_sts: &str) -> bool {
    if purge {
        post_sts == DOC_STS_DELETED
    } else {
        post_sts == "10"
    }
}

fn cancel_note(ok: bool, steps_ok: bool, steps_empty: bool, purge: bool, post: &PostState) -> String {
    if ok {
        let target = if purge {
            format!("삭제(doc_sts {DOC_STS_DELETED})")
        } else {
            "임시보관(doc_sts 10)".to_string()
        };
        // 문서가 이미 목표 상태였으면 실행한 콜이 하나도 없다 — 한 것처럼 말하지 않는다.
        return if steps_empty {
            format!("실행한 단계 없음 — 문서가 이미 {target} 상태였고 재조회로 그대로임을 확인했다.")
        } else {
            format!("{target} 도달을 재조회로 확인했다.")
        };
    }
    let mut why = Vec::new();
    if !steps_ok {
        why.push("실행 콜이 성공 신호(returnValue:1)를 주지 않았다".to_string());
    }
    why.push(match post {
        PostState::Found(sts) if purge => {
            format!("재조회에서 문서가 아직 남아 있다(doc_sts={sts})")
        }
        PostState::Found(sts) => format!("재조회 doc_sts가 10(임시보관)이 아니다(doc_sts={sts})"),
        PostState::NotFound => format!(
            "취소는 실행됐으나 **확인하지 못했다** — 재조회가 문서를 찾지 못했다(삭제됐다면 doc_sts {DOC_STS_DELETED}가 나와야 한다)"
        ),
        PostState::Unreadable(e) => {
            format!("취소는 실행됐으나 **확인하지 못했다** — 상태 재조회 실패: {e}")
        }
    });
    format!(
        "⚠️ 취소가 반영됐다고 볼 수 없다 — {}. 아마란스 웹에서 직접 확인할 것.",
        why.join(" / ")
    )
}

/// 상신 문서 취소 + **read-back 검증**.
/// 상태 전이(실측): 30(결재 진행중) --eap110A54 결재취소--> 20(상신) --eap110A18 상신취소--> 10(임시보관) --eap110A19 삭제--> `doc_sts 999`.
/// ⚠️ 필드명: 사전조회 eap110A98은 `docId`(소문자), 실행 콜들은 `docID`(대문자) — 실측 확정.
/// doc_sts 30이면 결재취소가 선행돼야 하고 그 콜(eap110A54)은 `form_id`를 요구한다(eap110A98 응답엔 없음 → caller가 list_approvals의 formId 전달).
/// purge=true면 임시보관(10)까지 되돌린 뒤 eap110A19로 완전 삭제. false면 임시보관에 남는다.
///
/// **판정**(전부 모듈 안에서):
/// - 사전조회 → `pre_verdict`: 문서 없음=호출자 오류 / 이미 삭제+purge=무실행 성공 / 남의 문서=거부 / 그 외 진행
/// - 실행 콜 `resultData.returnValue`(성공=1) — 단독 판정 불가(이미 삭제된 문서에도 1이 온다)
/// - 재조회 → `cancel_verdict`: purge면 `999`, 아니면 `10`. **못 찾음·못 읽음은 성공이 아니다.**
///
/// ⚠️ read-back에 `read_approval`(eap111A04)을 쓰지 않는다 — 취소된 문서에 2385/2156을 주고
/// `c.call`이 그걸 `bail!`로 올려 **"취소 성공"과 "장애"가 구분 불가**가 된다.
pub async fn cancel_and_verify(
    c: &GwClient,
    doc_id: &str,
    form_id: &str,
    purge: bool,
) -> Result<Value> {
    // ① 사전조회(상태·기안자 확인). 판독 불가는 여기서 Err — 상태를 모르면 실행하지 않는다.
    let pre = doc_status(c, doc_id).await?;
    let doc_sts = match pre_verdict(doc_id, &pre, purge, &c.emp_seq(), form_id) {
        PreVerdict::Reject(e) => return Err(e),
        PreVerdict::AlreadyDeleted => return Ok(already_deleted(doc_id)),
        PreVerdict::Proceed(sts) => sts,
    };
    let mut steps: Vec<Value> = Vec::new();

    // ② 결재취소(진행중 30 → 상신 20). eap110A54는 formID 필요(위에서 확인됨).
    if doc_sts == "30" {
        let rd = c
            .call(
                "/eap/eap110A54",
                &json!({ "docID": doc_id, "formID": form_id, "actID": "", "pageCode": "UBAP002" }),
            )
            .await
            .map_err(|e| anyhow!("결재취소(eap110A54) 실패: {e}"))?;
        steps.push(step_result("결재취소", "eap110A54", &rd));
    }

    // ③ 상신취소(상신 20 → 임시보관 10).
    if doc_sts == "30" || doc_sts == "20" {
        let rd = c
            .call(
                "/eap/eap110A18",
                &json!({ "docID": doc_id, "pageCode": "UBAP002" }),
            )
            .await
            .map_err(|e| anyhow!("상신취소(eap110A18) 실패: {e}"))?;
        steps.push(step_result("상신취소", "eap110A18", &rd));
    }

    // ④ (옵션) 임시보관 삭제(10 → 소멸).
    if purge {
        let rd = c
            .call(
                "/eap/eap110A19",
                &json!({ "docID": doc_id, "pageCode": "UBAP001" }),
            )
            .await
            .map_err(|e| anyhow!("임시보관 삭제(eap110A19) 실패: {e}"))?;
        steps.push(step_result("임시보관삭제", "eap110A19", &rd));
    }

    // ⑤ read-back — 응답만으로 성공을 선언하지 않는다. 여기서의 조회 실패는 **에러가 아니라
    //    "확인 못 함"**이다(취소 콜은 이미 나갔다) → ok:false로 그 사실을 드러낸다.
    let post = match doc_status(c, doc_id).await {
        Ok(DocState::Found { doc_sts, .. }) => PostState::Found(doc_sts),
        Ok(DocState::NotFound) => PostState::NotFound,
        Err(e) => PostState::Unreadable(e.to_string()),
    };
    Ok(cancel_verdict(doc_id, &doc_sts, purge, steps, &post))
}

/// 이미 삭제된 문서에 `purge=true`가 온 경우 — 요청한 종착 상태에 이미 도달해 있다.
/// **실행 콜을 하나도 쏘지 않았음**이 빈 `steps`와 `already`로 드러난다(조용한 성공 아님).
///
/// ⚠️ `postState`/`postDocSts`를 싣지 않는다 — 그 필드들은 **"취소 실행 뒤 재조회 결과"**를 뜻하는데
/// 이 경로는 실행도 사후조회도 하지 않았다. 논리적으로 pre==post라 해도 하지 않은 조회를 한 것처럼
/// 적으면 응답이 거짓말이 된다. 근거는 사전조회 하나뿐이고 그건 `preDocSts`에 있다.
fn already_deleted(doc_id: &str) -> Value {
    json!({
        "kind": "approvalCancelled",
        "ok": true,
        "already": true,
        "verified_by_readback": true,
        "docId": doc_id,
        "preDocSts": DOC_STS_DELETED,
        "steps": [],
        "purged": true,
        "note": "이미 삭제된 문서다(사전조회에서 doc_sts 999 확인) — 실행한 단계 없음. 실행한 것이 없어 사후 재조회도 하지 않았다."
    })
}

/// 페이로드의 신원 필드를 로그인 사용자 값으로 덮어쓴다(존재하는 키만 교체 — 새 키 추가 안 함).
/// draftHelp 예시 템플릿에 박힌 타인 신원(empCd/deptCd/coCd/이름)이 그대로 상신되는 것을 방지.
/// 빈 문자열은 덮어쓰지 않는다 — 조직도 조회 실패 시 예시값을 지워버리는 것보다 남기는 편이 낫다.
fn overwrite_if_present(v: &mut Value, key: &str, val: &str) {
    if val.is_empty() {
        return;
    }
    if let Some(obj) = v.as_object_mut()
        && obj.contains_key(key)
    {
        obj.insert(key.to_string(), Value::String(val.to_string()));
    }
}

/// 로그인 사용자의 신원·표시정보. 코드계(co/dept/emp)는 세션에서, 표시문자열(부서명/직책/직급)은
/// `org::my_profile`(조직도 1콜, 30분 캐시)에서 온다. 조직도가 안 잡히면 표시문자열만 빈 값이 되고
/// 그 필드는 예시값이 유지된다(상신은 그대로 진행).
struct Identity {
    co: String,
    dept: String,
    emp: String,
    name: String,
    dept_nm: String,
    duty: String,
    position: String,
    co_nm: String,
}

/// 신원 코드 + **문서에 렌더되는 표시문자열**까지 주입한다.
/// ⚠️ 표시필드를 안 채우면 draftHelp 예시에 박힌 **타인의 이름·부서·직급이 결재문서 본문에 그대로
/// 찍힌다**(예시 작성자 기준값). cosmetic이 아니라 실제 출력값이라 반드시 덮어쓴다.
/// 대상 필드(존재할 때만): 코드계 `coCd/deptCd/empCd`, 이름 `empNm/empName/korNm`,
/// 부서명 `deptNm/deptName/singleDeptNm`, 회사명 `divNm`, 직급 `singlePositionNm`,
/// 직책 `singleDutyNm`, 조합문자열 `empNmDutyNm`("이름 직책")·`employees`("이름 직급").
/// `employees`는 신청 대상자 목록이지만 MCP 상신은 항상 **본인 1인** 기준이라 단일값으로 채운다.
fn inject_identity(item: &mut Value, id: &Identity) {
    // `groupByKey`("<empCd><날짜>" — 예 "1109720260803")처럼 **empCd를 접두사로 품은 조합 문자열**이
    // 있다. empCd만 갈아끼우면 이쪽은 예시 작성자의 사번을 그대로 달고 나간다.
    // 덮어쓰기 전에 옛 empCd를 붙잡아 둔다.
    let old_emp = item.get("empCd").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let emp_duty = if id.duty.is_empty() {
        String::new()
    } else {
        format!("{} {}", id.name, id.duty)
    };
    let emp_position = if id.position.is_empty() {
        String::new()
    } else {
        format!("{} {}", id.name, id.position)
    };
    for (k, val) in [
        ("coCd", id.co.as_str()), ("deptCd", id.dept.as_str()), ("empCd", id.emp.as_str()),
        ("empNm", id.name.as_str()), ("empName", id.name.as_str()), ("korNm", id.name.as_str()),
        ("deptNm", id.dept_nm.as_str()), ("deptName", id.dept_nm.as_str()),
        ("singleDeptNm", id.dept_nm.as_str()), ("divNm", id.co_nm.as_str()),
        ("singlePositionNm", id.position.as_str()), ("singleDutyNm", id.duty.as_str()),
        // `single*` 없는 짧은 이름도 온다 — 본문 표(TABLE.dbTable1)가 이쪽을 쓴다.
        ("positionNm", id.position.as_str()), ("dutyNm", id.duty.as_str()),
        ("empNmDutyNm", emp_duty.as_str()), ("employees", emp_position.as_str()),
    ] {
        overwrite_if_present(item, k, val);
    }

    // 조합 키의 empCd 접두사 교체. 옛 사번으로 시작할 때만 손댄다 — 형식이 다르면 두는 편이 낫다.
    if !old_emp.is_empty() && old_emp != id.emp && !id.emp.is_empty() {
        let swapped = item
            .get("groupByKey")
            .and_then(|v| v.as_str())
            .and_then(|v| v.strip_prefix(old_emp.as_str()))
            .map(|rest| format!("{}{rest}", id.emp));
        if let Some(new) = swapped
            && let Some(obj) = item.as_object_mut()
        {
            obj.insert("groupByKey".into(), Value::String(new));
        }
    }
}

/// `inject_identity`를 **JSON 트리 전체**에 적용한다.
///
/// ⚠️ 방문 대상을 손으로 열거하던 예전 방식이 실제로 샜다. 주입이 `bindData.ITEMS` 최상위와
/// `applicationList[]`/`employeeList[]` **직속 필드**만 돌아서, 아래 세 갈래가 예시 작성자의
/// 신원을 그대로 달고 나갔다(2026-08-20 전수 대조로 발견 — 4개 양식 중 3개, 20곳):
///  - `bindData.TABLE.dbTable1.group[].items` — **문서 본문 표에 렌더되는** 사번·이름·부서·직급
///  - `applicationList[].employeeList[]` — 한 겹 더 중첩돼 최상위 열거에서 빠짐(휴일주말근무)
///  - `weeklyOvertimeWorkInfo` 등 중첩 오브젝트의 `empCd`
///
/// 메일함 seq 사고와 달리 **이쪽은 실패하지 않고 성공한다** — 문서는 정상 접수되고 근태 레코드만
/// 남의 사번으로 남는다. 그래서 열거 대신 트리를 통째로 훑는다. **새 양식이 추가돼도 자동으로 걸린다.**
///
/// ⚠️ 전제: 이 페이로드의 신원은 **전부 기안자 본인**이다(근태 4양식은 본인 신청뿐). 타인의 신원을
/// 정당하게 싣는 양식이 생기면 이 함수가 그것까지 덮어쓰므로, 그때는 예외 경로가 필요하다.
fn inject_identity_deep(node: &mut Value, id: &Identity) {
    match node {
        Value::Object(_) => {
            inject_identity(node, id);
            if let Some(obj) = node.as_object_mut() {
                for v in obj.values_mut() {
                    inject_identity_deep(v, id);
                }
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                inject_identity_deep(v, id);
            }
        }
        _ => {}
    }
}

/// 상신 전 개인결재라인 점검 — 되돌릴 수 없는 mutation 앞의 사전확인(§7.1의 세 번째 갈래).
///
/// 막는 두 가지:
/// - **결재자 0명** — 그 라인으로는 상신이 성립하지 않는다. 그런 라인이 실제로 만들어졌고
///   증상이 "4분 무응답"으로 나타난 사례가 있다(2026-09-15).
/// - **기안자 단독** — 상신 즉시 `종결`(doc_sts 90)이 되고 `cancel_approval`은 90 취소를
///   거부하므로 되돌릴 수 없는 문서가 남는다(실측 docId 148978).
///
/// 조회 실패는 "통과"로 치지 않는다 — fail-closed(§7.2와 같은 이유: 못 쏜 상신은 다시 쏘면
/// 되지만 잘못 나간 상신은 되돌릴 수 없다).
async fn check_line_before_submit(c: &GwClient, line_id: i64) -> Result<()> {
    let back = crate::modules::approval_line::read_line(c, &line_id.to_string())
        .await
        .map_err(|e| {
            anyhow!("결재라인 {line_id} 을 조회할 수 없어 상신하지 않는다(결재선을 확인하지 못한 채 상신하지 않는다): {e}")
        })?;
    let seqs: Vec<String> = back
        .get("members")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(|m| crate::util::s(m, "user_id")).collect())
        .unwrap_or_default();
    let me = c.emp_seq();
    // 저장 시점과 **같은 판정 함수**를 쓴다 — 두 가드가 어긋나지 않게.
    match crate::modules::approval_line::line_shape(&seqs, &me) {
        LineShape::Usable => Ok(()),
        LineShape::Empty => bail!(
            "결재라인 {line_id} 에 결재자가 없어 상신하지 않는다 — save_approval_line으로 결재자를 \
             다시 등록할 것(결재선 규칙은 suggest_approval_line으로 확인)."
        ),
        LineShape::SoleDrafter => bail!(
            "결재라인 {line_id} 이 기안자 단독({me})이라 상신하지 않는다 — 상신 즉시 종결(doc_sts 90)되어 \
             cancel_approval로 취소할 수 없다. 본인 아닌 결재자를 최소 1명 포함한 라인을 쓸 것."
        ),
    }
}

/// 문서 상신 — eap110A06. 근태 계열 양식(외근/연차 등) 대상.
/// ⚠️ 진입 즉시 `check_line_before_submit`로 결재선을 점검한다(0명·기안자 단독 차단).
/// - `form_id`: 양식 ID(41 외근/36 연차 …).
/// - `doc_title`: 문서 제목.
/// - `line_id`: 사용할 개인결재라인 ID. a03에 appLineId로 넘겨 완전 병합된 결재선을 받는다. save_approval_line으로 준비.
/// - `bind_data_json`: KISS 폼 본문 데이터 JSON 텍스트(외근=`{"ITEMS":{...},"TABLE":{...}}`). 서버엔 이중인코딩되어 전송.
/// - `doc_contents_html`: 표시용 본문 HTML(raw). 내부에서 encodeURIComponent로 인코딩해 전송.
///   **근태 5양식에서는 무시된다** — `approval_body`가 bindData로 양식 표를 조립해 싣는다.
/// - `numbering_id`: 채번 규칙(기본 "1001").
///
/// - `cc_emp_seqs`/`cc_dept_ids`: **기안자가 직접 더하는 수신참조**(사람 empSeq / 부서 deptId).
///   양식필수 수신참조 뒤에 덧붙는다. 비우면 종래대로 양식필수만 실린다.
///
/// 양식필수 합의자/수신참조는 eap110A03에서 서버가 해석한 것을 자동 병합한다.
#[allow(clippy::too_many_arguments)]
pub async fn submit_approval(
    c: &Arc<GwClient>,
    form_id: i64,
    doc_title: &str,
    line_id: i64,
    hp_application_json: &str,
    bind_data_json: &str,
    doc_contents_html: &str,
    numbering_id: &str,
    attachments: &[String],
    cc_emp_seqs: &[String],
    cc_dept_ids: &[String],
) -> Result<Value> {
    // ── 결재선 사전 점검 — HP 근태 레코드를 만들기 **전에** 막는다 ──────────────
    // a03 응답(`kyuljaeResult`)에 기대지 않는 이유: 근태 양식은 양식필수 수신참조·시행자가
    // 자동 병합되므로 개인라인이 0명이어도 non-empty가 될 수 있다(추측이지만, 0명 라인으로
    // 실제 상신해 확인하는 것은 잘못된 결재선의 실문서가 나가는 위험이 있어 시험하지 않았다).
    // 여기서 막으면 그 질문 자체가 닫힌다. 비용은 조회 1콜 — 되돌릴 수 없는 상신 앞의 1콜이다.
    check_line_before_submit(c, line_id).await?;

    let co_id = c.comp_seq();
    let dept_id = c.dept_seq();
    let user_id = c.emp_seq();
    let user_nm = c.emp_name();

    // 신원 자동 주입값. 코드계(ERP — seq와 별개)는 세션에서, 표시문자열(부서명/직책/직급)은
    // 조직도 1콜(30분 캐시)에서. hp/bind 페이로드의 해당 필드를 이 값으로 덮어쓴다.
    let prof = crate::modules::org::my_profile(c).await;
    let ps = |k: &str| prof.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id = Identity {
        co: c.co_cd().to_string(),
        dept: c.dept_cd().to_string(),
        emp: c.emp_cd().to_string(),
        name: c.emp_name().to_string(),
        dept_nm: ps("deptName"),
        duty: ps("duty"),
        position: ps("position"),
        co_nm: ps("coName"),
    };

    // bindData 검증(유효 JSON이어야 함)
    let mut bind_obj: Value = serde_json::from_str(bind_data_json)
        .map_err(|e| anyhow!("bind_data_json이 유효한 JSON이 아님: {e}"))?;
    // 신원 자동 주입: 트리 전체(ITEMS뿐 아니라 **문서 본문 표인 TABLE.dbTable1.group[].items까지**).
    inject_identity_deep(&mut bind_obj, &id);
    // 이중 인코딩: 최종 wire 값 = JSON.stringify(JSON.stringify(bindObj)).
    let s1 = serde_json::to_string(&bind_obj)?; // {"ITEMS":...}
    let bind_data_field = Value::String(serde_json::to_string(&s1)?); // "{\"ITEMS\":...}"

    let numbering_id = if numbering_id.trim().is_empty() {
        "1001"
    } else {
        numbering_id.trim()
    };
    let approkey = gen_approkey();

    // create가 반환하는 HP 신청 식별자(appSq/appDt) — interlock linkKey 바인딩에 필요.
    let mut app_sq: Option<i64> = None;
    let mut app_dt = String::new();

    // ── 0) HP 근태 신청 저장 (2-phase의 1단계, eap110A06가 참조할 근태 레코드 생성) ──
    // 근태 양식(HPD0110)은 상신(eap110A06) 전에 신청완료가 이 콜로 HP draft를 먼저 만든다.
    // 이 단계를 건너뛰면 eap110A06 연동이 resultCode 2099(HP_HPD0110)로 실패한다.
    if !hp_application_json.trim().is_empty() {
        let mut hp_body: Value = serde_json::from_str(hp_application_json)
            .map_err(|e| anyhow!("hp_application_json이 유효한 JSON이 아님: {e}"))?;
        // 신원 자동 주입: 트리 전체. 최상위 applicationList/employeeList뿐 아니라 **중첩된
        // applicationList[].employeeList[]와 weeklyOvertimeWorkInfo 같은 하위 오브젝트까지** 닿는다.
        inject_identity_deep(&mut hp_body, &id);
        let create_body = json!({
            "coCd": "", "appDt": "", "appEmpCd": id.emp, "deptCd": "",
            "titleDc": doc_title, "approLineId": line_id.to_string(),
            "calLinkKey": "", "linkKey": "", "approState": "", "fileGroup": 0, "version": "v2",
            "employeeList": hp_body.get("employeeList").cloned().unwrap_or(json!([])),
            "applicationList": hp_body.get("applicationList").cloned().unwrap_or(json!([])),
        });
        // 1단계: 0hr00011 (검증/스테이징). 응답은 빈 SUCCESS.
        c.call("/human/attendapplication/0hr00011", &hp_body)
            .await
            .map_err(|e| anyhow!("HP 근태신청 저장(0hr00011) 실패: {e}"))?;
        // 2단계: create (HP신청 커밋 — approLineId에 묶인 대기 HP신청 등록, appSq 반환).
        let create_res = c.call("/human/attendapplication/create", &create_body)
            .await
            .map_err(|e| anyhow!("HP 근태신청 커밋(create) 실패: {e}"))?;
        app_sq = create_res.get("appSq").and_then(|v| v.as_i64());
        app_dt = create_res.get("appDt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    }

    // ── 1) eap110A03: 결재선 해석 + form_d_tp(양식별 interlock 식별자) 취득 ─────
    // interlock 등록(SetEnageGroup)이 form_d_tp를 요구하므로 a03를 먼저 호출해 얻는다.
    let a03 = c
        .call(
            "/eap/eap110A03",
            &json!({
                "docID": 0, "formID": form_id.to_string(), "approkey": approkey,
                "appLineId": line_id.to_string(), "draftTp": "", "reDraft": "", "docType": "",
                "doc_auth": 0, "pageCode": "UBAP001"
            }),
        )
        .await?;
    let result_map = a03.get("resultMap").cloned().unwrap_or(Value::Null);
    // form_d_tp = 양식별 HP interlock 식별자(연차36 _00011 / 출장40 _00021 / 외근41 _00031 / 휴일43 _00051).
    // 하드코딩 금지 — 양식마다 다르다(틀리면 eap110A06가 HP_HPD0110_000XX로 2099). a03가 formID 기준으로 반환.
    let form_d_tp = result_map
        .get("form_info")
        .and_then(|f| f.get("form_d_tp"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("HP_HPD0110_00011")
        .to_string();

    // ── 2) 근태 interlock 등록 (eap110A06 성공의 핵심) ──────────────────────
    // eap110A06의 eap→HP 서버간 연동은 이 3콜 등록을 요구한다. 누락 시:
    //   · GetLinkKey/SetEnageGroup 없으면 → HP_HPD0110_000XX "Internal Server Error"(연동 대상 없음)
    //   · saveAttendApplicationLinkKey(linkKey↔appSq 바인딩) 없으면 → "근태신청서 종결 처리 오류"
    // 브라우저는 이 콜들을 치지만 /system//personal/ 경로라 초기 캡처(/human//eap/만)가 놓쳤던 조각.
    // menuCode(HPD0110)는 근태 공통 상수지만 formDTp는 양식별(위 form_d_tp). 콜백 API는 eap가 상신 시 서버간 호출하는 HP 엔드포인트.
    // 본문(doc_contents) 조립에도 쓰려고 블록 밖에 둔다 — HP가 이 키로 bindData를 돌려준다.
    let mut link_key = String::new();
    if !hp_application_json.trim().is_empty() {
        let glk = c
            .call(
                "/system/apiUtilEap/GetLinkKey",
                &json!({"menuCode":"HPD0110","approKey":approkey,"vPCoCd":id.co,"coCd":id.co}),
            )
            .await
            .map_err(|e| anyhow!("GetLinkKey 실패: {e}"))?;
        link_key = glk.get("linkKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
        // linkKey ↔ 실제 HP 신청(appSq) 바인딩. 없으면 finalize가 대상 신청을 못 찾아 '종결 처리 오류'.
        c.call(
            "/personal/hpd0110/saveAttendApplicationLinkKey",
            &json!({"linkKey": link_key, "appSq": app_sq, "coCd": id.co, "appDt": app_dt}),
        )
        .await
        .map_err(|e| anyhow!("saveAttendApplicationLinkKey 실패: {e}"))?;
        c.call(
            "/system/apiUtilEap/SetEnageGroup",
            &json!({
                "approKey": approkey, "formDTp": form_d_tp, "formId": form_id.to_string(),
                "linkKey": link_key, "formNm": doc_title, "docTitle": doc_title, "contents": "",
                "contentsApi": "/human/attendapplication/interlock/getInterlockFormContents",
                "statusApi": "/human/attendapplication/interlock/setInterlockSync",
                "dummy1": "", "link": "", "vPCoCd": id.co, "coCd": id.co
            }),
        )
        .await
        .map_err(|e| anyhow!("SetEnageGroup 실패: {e}"))?;
    }

    // ── 3) 결재선(양식필수 합의자/수신참조/시행자) 해석 — 위 a03의 result_map 재사용 ──
    let kyuljae = result_map
        .get("kyuljaeResult")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let m_refer = result_map
        .get("m_Refer")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // 필수 시행자(m_Oper) — 브라우저 성공 payload와 동일하게 pOper로 그대로 실어 보낸다(정합성).
    // ※ "pOper 누락이 2099의 원인"이라는 이전 가설은 반증됨 — 원인은 interlock 등록 누락.
    let m_oper = result_map
        .get("m_Oper")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ── 2) pTEAG_APPDOC_LINE = kyuljaeResult 원본 그대로 ──────────────────────
    // a03에 appLineId를 주면 kyuljaeResult가 [양식필수 합의 + 개인결재라인]까지 완전히 병합된
    // 결재선으로 온다. 브라우저는 이걸 무가공으로 pTEAG_APPDOC_LINE에 실어 보낸다(실측 확인).
    // 직접 재구성(act_id 강제/read_line 병합)하면 브라우저와 어긋나므로 그대로 패스스루한다.
    if kyuljae.is_empty() {
        return Err(anyhow!(
            "a03가 결재선(kyuljaeResult)을 반환하지 않음 — 결재라인 {line_id} 확인 필요"
        ));
    }
    let line_nodes: Vec<Value> = kyuljae.clone();

    // ── 3) pRefer = 수신참조, pOper = 시행자 — a03 원본 패스스루(+org_div) ──────
    let mut refer_nodes: Vec<Value> = m_refer.iter().map(norm_participant).collect();
    let oper_nodes: Vec<Value> = m_oper.iter().map(norm_participant).collect();

    // 기안자가 직접 더한 수신참조를 양식필수 뒤에 잇는다(순번은 이어서 매긴다).
    // 이미 양식필수로 들어 있는 대상은 건너뛴다 — 같은 사람이 두 번 실리면 서버 거동이 미지수다.
    let cc_added = build_cc_nodes(c, cc_emp_seqs, cc_dept_ids, &refer_nodes).await?;
    refer_nodes.extend(cc_added.added.iter().cloned());

    // ── 5) modifyDocInfo compact 뷰 ──────────────────────────────────────────
    let line_compact: Vec<Value> = line_nodes
        .iter()
        .map(|n| {
            json!({
                "doc_line_m_seq": n.get("doc_line_m_seq").cloned().unwrap_or(json!(0)),
                "doc_line_s_seq": 1,
                "act_id": n.get("act_id").cloned().unwrap_or(json!(3000)),
                "co_id": n.get("co_id").cloned().unwrap_or(json!(co_id)),
                "dept_id": n.get("dept_id").cloned().unwrap_or(Value::Null),
                "user_id": n.get("user_id").cloned().unwrap_or(Value::Null),
                "doc_line_gb": "1"
            })
        })
        .collect();
    // appdocReceiveList: 시행자(40) + 수신참조(10). org_div/org_id는 각 노드 원본에서. (실측)
    let recv_of = |n: &Value, div: &str| {
        json!({
            "receive_div": div,
            "org_div": n.get("org_div").cloned().unwrap_or(Value::Null),
            "org_id": n.get("org_id").cloned().unwrap_or(Value::Null)
        })
    };
    let mut receive_list: Vec<Value> = Vec::new();
    for n in oper_nodes.iter() {
        receive_list.push(recv_of(n, "40"));
    }
    for n in refer_nodes.iter() {
        receive_list.push(recv_of(n, "10"));
    }

    // ── 5) 본문(doc_contents) 조립 ───────────────────────────────────────────
    // 근태 양식은 서버가 본문 HTML을 만들어주지 않는다(브라우저가 표를 조립해 보낸다 —
    // `approval_body` 머리 주석의 실측 참조). 그래서 한 줄 HTML을 그대로 실으면 문서가
    // 그 한 줄만 담긴 채 상신된다. 템플릿이 있는 양식이면 우리가 같은 표를 만들어 싣는다.
    //
    // 값은 **서버가 계산한 것**을 우선한다 — 연차 잔여/사용/차감 같은 집계는 HP가 정답을
    // 쥐고 있어서, 호출자가 넘긴 bindData보다 interlock 조회분이 정확하다. 조회가 안 되면
    // (linkKey 없음·비근태 경로) 호출자 bindData로 렌더한다.
    let doc_contents_html = if approval_body::supports(form_id) {
        let bind = fetch_interlock_bind(c, &link_key, &id.co)
            .await
            .unwrap_or(None)
            .unwrap_or_else(|| bind_obj.clone());
        approval_body::render(form_id, &bind)
            .map_err(|e| anyhow!("본문 HTML 조립 실패(form_id={form_id}): {e}"))?
    } else {
        doc_contents_html.to_string()
    };
    let doc_contents = encode_uri_component(&doc_contents_html);
    let rep_dt = now_kst_datetime();

    // 첨부: 로컬 파일을 ECM 에 올리고 pVCM_ATTACHFILEINFO 항목으로 만든다(07 §11.5·§11.5.3).
    // 신규 문서라 doc_id 는 0 — authKeyMap.docId 도 그 값을 따른다.
    let (attach_items, modify_file_list) =
        upload_attachments(c, attachments, &co_id, &user_id, "0").await?;

    // ── 6) eap110A06 상신 ────────────────────────────────────────────────────
    let param_item = json!({
        "bindData": bind_data_field,
        "interDivId": "divInterJson", "interDocTp": "json",
        "doc_id": 0, "form_id": form_id.to_string(), "numbering_id": numbering_id,
        "rep_dt": rep_dt, "repdt_mod_yn": "0",
        "co_id": co_id, "dept_id": dept_id, "biz_id": co_id, "user_id": user_id,
        // dept_nm: 브라우저는 기안부서명을 싣는다(캡처 diff의 유일한 차이였음) — 조직도 값으로 채운다.
        "co_nm": "(주)이노그리드", "dept_nm": id.dept_nm, "user_nm": user_nm,
        "doc_title": doc_title, "doc_sts": "20", "inservice_time": "0",
        "doc_level": "001", "emergency_level": "1", "doc_security": "0", "use_yn": "1",
        "approkey": approkey, "contents_tp": "10", "doc_contents": doc_contents,
        "pTEAG_APPDOC_LINE": line_nodes,
        "pVKD_TKDDITEM": [], "pVCM_ATTACHFILEINFO": attach_items,
        "pRefer": refer_nodes, "pReceive": [], "pOper": oper_nodes, "pTEAG_APPDOC_REF": [],
        "pTEAG_TOC_FOLDER": "", "pDraftTp": "", "seal_use_yn": "", "receipient": "",
        "receipt": "", "iframeHtml": "", "re_draft": "",
        "modifyAppLineYn": "Y", "modifyReceive10": "Y", "modifyReceive20": "Y",
        "modifyReceive30": "Y", "modifyReceive40": "Y", "modifyTitle": "Y",
        "modifyContent": "Y", "modifyRef": "Y", "modifyAttach": "Y", "modifyAddItem": "Y",
        "modifyInservice": "Y", "modifyDoclevel": "Y", "modifyEmergency": "Y",
        "modifySeal": "Y", "modifyEabox": "Y", "modifyFileList": modify_file_list,
        "delFileSnList": [], "auditorYn": "0",
        "modifyDocInfo": {
            "docId": 0,
            "appdoc": {
                "inservice_time": "0", "doc_level": "001", "doc_security": "0",
                "emergency_level": "1", "doc_title": doc_title
            },
            "appdocReceiveList": receive_list,
            "appdocLineList": line_compact,
            "appdocFileList": [], "appdocFolderList": [{ "menu_id": "" }], "appdocRefList": []
        },
        "modifyItemList": Value::Null, "isLatestVerContentsFile": true,
        "versionCheck": Value::Null, "formLang": "kr", "aiVerifyHistories": [],
        "aiVerifyAutoOnSubmit": false, "aiVerifyUseYn": "0"
    });
    let d = c
        .call("/eap/eap110A06", &json!({ "paramItem": param_item, "pageCode": "UBAP001" }))
        .await?;

    let new_doc_id = submitted_doc_id(&d).ok_or_else(|| {
        anyhow!("상신(eap110A06)이 docId를 주지 않았다 — resultData.result 없음/빈값. 서버 응답: {d}")
    })?;

    // 더한 수신참조가 **실제로 실렸는지 되읽어 확인한다.** 아마란스는 받아들이지 않은 항목을
    // 조용히 버리므로(모르는 파라미터 무시) 상신 성공만으로는 반영을 단정할 수 없다.
    let cc_report = cc_readback(c, &new_doc_id, form_id, &cc_added).await;
    Ok(json!({
        "kind": "approvalSubmitted",
        "ok": true,
        "docId": new_doc_id,
        "formId": form_id,
        "title": doc_title,
        "lineCount": line_nodes.len(),
        "referCount": refer_nodes.len(),
        "cc": cc_report,
        "note": "상신 성공(docId 발급 확인). 취소는 cancel_approval(docId, formId) — 상신 직후는 doc_sts=30이라 formId가 필요하다. 근태 양식은 create→GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup(HP interlock 등록) 후 eap110A06으로 상신한다. 등록 누락 시 2099(HP_HPD0110)."
    }))
}

/// 더한 수신참조가 문서에 **실제로 저장됐는지** 되읽어 확인한다.
///
/// 소스는 `eap110A03`에 **저장된 문서의 docID를 주고** 받는 `resultMap.hidRefer` — 그 문서에
/// 실린 수신참조 목록 그 자체다(`valRefer`는 같은 것의 사람이 읽는 표기). `docID:0`으로 부를 때와
/// 달리 양식 정의가 아니라 저장본이 오고, 부서는 전개되기 전 원본(`div:"d"`, `org_id`=deptId)으로
/// 남아 있어 **사람과 부서를 모두 판정할 수 있다**. 열람 부작용은 없다(상신 팝업을 여는 것과 같다).
///
/// ⚠️ `read_approval`의 `receiveDiv:"10"`을 쓰지 않는 이유 — 그 목록에는 수신참조가 아닌
/// 사람도 섞여 나오는 것을 실측했다(결재자가 10으로 보였다). 판정에 쓰면 위양성이 난다.
///
/// 확인에 실패해도 에러로 올리지 않는다 — 상신은 이미 끝났고 되돌릴 수 없으므로, 여기서 할 일은
/// "확인됐는지"를 정직하게 전하는 것뿐이다(`verified:false`는 반영 실패일 수도, 확인 실패일 수도 있다).
async fn cc_readback(c: &GwClient, doc_id: &Value, form_id: i64, plan: &CcPlan) -> Value {
    if plan.persons.is_empty() && plan.depts.is_empty() && plan.skipped.is_empty() {
        return Value::Null;
    }
    let mut out = json!({
        "requestedPersons": plan.persons,
        "requestedDepts": plan.depts,
        "skipped": plan.skipped,
    });
    let put = |o: &mut Value, k: &str, v: Value| {
        if let Some(m) = o.as_object_mut() {
            m.insert(k.to_string(), v);
        }
    };
    if plan.persons.is_empty() && plan.depts.is_empty() {
        put(&mut out, "verified", Value::Null);
        put(&mut out, "note", json!("요청한 수신참조가 모두 양식필수와 겹쳐 더한 것이 없다."));
        return out;
    }

    let fail = |mut out: Value, why: String| {
        if let Some(m) = out.as_object_mut() {
            m.insert("verified".into(), json!(false));
            m.insert("note".into(), json!(why));
        }
        out
    };
    let a03 = match c
        .call(
            "/eap/eap110A03",
            &json!({
                "docID": doc_id, "formID": form_id.to_string(), "approkey": gen_approkey(),
                "appLineId": "", "draftTp": "", "reDraft": "", "docType": "",
                "doc_auth": 0, "pageCode": "UBAP001"
            }),
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return fail(out, format!(
                "상신은 됐으나 수신참조 저장을 확인하지 못했다(문서 재조회 실패: {e}).                  아마란스 웹에서 문서의 수신참조를 직접 확인할 것."
            ))
        }
    };
    let Some(saved_nodes) = a03
        .get("resultMap")
        .and_then(|m| m.get("hidRefer"))
        .and_then(|v| v.as_array())
    else {
        return fail(out, "상신은 됐으나 수신참조 저장을 확인하지 못했다(a03 응답에 hidRefer 없음). 아마란스 웹에서 직접 확인할 것.".into());
    };
    let saved: std::collections::HashSet<(String, String)> = saved_nodes
        .iter()
        .map(|n| (jstr(n.get("div")), jstr(n.get("org_id"))))
        .collect();

    let missing_p: Vec<Value> = plan
        .persons
        .iter()
        .filter(|p| !saved.contains(&("m".to_string(), jstr(p.get("empSeq")))))
        .cloned()
        .collect();
    let missing_d: Vec<Value> = plan
        .depts
        .iter()
        .filter(|d| !saved.contains(&("d".to_string(), jstr(d.get("deptId")))))
        .cloned()
        .collect();

    let ok = missing_p.is_empty() && missing_d.is_empty();
    put(&mut out, "verified", json!(ok));
    put(&mut out, "missingPersons", json!(missing_p));
    put(&mut out, "missingDepts", json!(missing_d));
    // 사람이 읽는 확인용 — 문서에 실린 수신참조 전체(양식필수 포함)를 서버 표기 그대로.
    put(&mut out, "savedOnDoc", a03.get("resultMap").and_then(|m| m.get("valRefer")).cloned().unwrap_or(Value::Null));
    put(&mut out, "note", json!(if ok {
        "더한 수신참조가 문서에 저장된 것을 재조회로 확인했다(eap110A03 hidRefer). savedOnDoc이 문서에 실린 수신참조 전체다."
    } else {
        "⚠️ 더한 수신참조 중 일부가 문서에 저장되지 않았다 — 그 대상은 이 문서를 받지 못한다. savedOnDoc에서 실제로 실린 것을 확인하고, 필요하면 아마란스 웹에서 직접 추가할 것."
    }));
    out
}

/// 상신 응답(`eap110A06`의 resultData)에서 새 docId를 꺼낸다. **성공 판정이 곧 이것이다** —
/// docId가 없으면 상신은 이뤄지지 않은 것이다(실측상 성공 응답은 `resultData.result`에 docId를 싣는다).
/// 서버가 number/string 어느 쪽으로 주든 받되, `null`·빈 문자열·`0`은 발급 실패로 본다.
pub(crate) fn submitted_doc_id(rd: &Value) -> Option<Value> {
    let v = rd.get("result")?;
    match v {
        Value::Number(n) => (n.as_i64() != Some(0)).then(|| v.clone()),
        Value::String(s) => {
            let t = s.trim();
            (!t.is_empty() && t != "0").then(|| v.clone())
        }
        _ => None,
    }
}

/// 임시보관 전자결재 문서 삭제 — `GET /eap/sse/eap107A25?docIdList=<csv>`(SSE 스트림).
/// 콤마구분 docId를 한 콜로 일괄삭제. 상신취소(purge=false)로 되돌아온 문서나 시험 잔여물 정리용.
/// ⚠️ 상신 실패(2099)와는 무관 — 잔여 draft 원인설은 실측으로 반증됐다.
/// 응답 이벤트별 resultCode + resultData.failCnt로 성공 판정.
pub async fn delete_temp_approval(c: &GwClient, doc_ids: &str) -> Result<Value> {
    let ids = doc_ids.trim();
    if ids.is_empty() {
        anyhow::bail!("doc_ids(콤마구분 docId)가 비어있음");
    }
    let path = format!("/eap/sse/eap107A25?docIdList={ids}");
    let events = c.call_get_sse("/eap/sse/eap107A25", &path).await?;

    let mut deleted: Vec<String> = Vec::new();
    let mut fail: i64 = 0;
    for e in &events {
        let code = e.get("resultCode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if !(code == 0 || code == 200) {
            fail += 1;
            continue;
        }
        if let Some(rd) = e.get("resultData") {
            fail += rd.get("failCnt").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(id) = rd.get("docId").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                deleted.push(id.to_string());
            }
        }
    }
    Ok(json!({
        "kind": "tempApprovalDeleted",
        "requested": ids,
        "deletedDocIds": deleted,
        "failCount": fail,
        "note": "임시보관 문서 삭제(eap107A25). list_approvals(box_name:\"draft\")로 사라졌는지 재확인 권장."
    }))
}

/// a03의 m_Oper/m_Refer 원본 노드를 pOper/pRefer 상신 노드로 패스스루.
/// 실측(브라우저 캡처) 확인: a03 노드는 org_id/dept_line/seq/doc_line_* 가 이미 정확하고,
/// 브라우저는 딱 하나 `org_div = div` 만 추가해 그대로 보낸다. 그 외 재구성은 하지 않는다.
/// (개인 시행자/참조자를 부서노드로 강제 변환하던 이전 로직은 브라우저와 어긋나 폐기 — 2099와는 무관했음.)
pub(crate) fn norm_participant(src: &Value) -> Value {
    let mut n = src.clone();
    if let Some(o) = n.as_object_mut() {
        let div = o.get("div").and_then(|v| v.as_str()).unwrap_or("m").to_string();
        o.insert("org_div".into(), json!(div));
    }
    n
}

/// 기안자가 직접 더하는 수신참조의 조립 결과.
#[derive(Default)]
pub(crate) struct CcPlan {
    /// `pRefer`에 덧붙일 노드.
    pub added: Vec<Value>,
    /// 요청한 사람 `{empSeq,name}` — 상신 뒤 read-back 판정 대상.
    pub persons: Vec<Value>,
    /// 요청한 부서 `{deptId,name}`.
    pub depts: Vec<Value>,
    /// 이미 양식필수로 들어 있어 건너뛴 대상 `{kind,id}`.
    pub skipped: Vec<Value>,
}

/// 기안자가 **직접 더하는** 수신참조(`pRefer`) 노드를 조직도로 조립한다.
///
/// 양식필수 수신참조는 서버(a03 `m_Refer`)가 주는 대로 싣고, 이 함수가 만드는 것은 그 뒤에
/// **덧붙는** 임의 참조자다.
///
/// ⛔ **양식필수 수신참조를 빼는 기능은 두지 않는다(설계 방침, 2026-10-02).** 양식필수란 "이 양식의
/// 문서는 그들이 봐야 한다"는 회사 규칙이 양식에 박힌 것이라 기안자가 임의로 뺄 대상이 아니다.
/// 서버도 그렇게 동작한다 — `a03`가 진입할 때마다 주입하므로 payload에서 지워도 되살아난다.
/// 즉 **막혀서 못 하는 것이 아니라 하지 않는 것이 맞다**. "미해결 과제"로 되살리지 말 것. 노드 모양은 브라우저 상신 캡처(`captures/annual-half-am_eap110A06.json`
/// 의 `pRefer`)와 같은 필드 구성으로 맞추되, 양식이 강제한 것이 아니므로 `must_yn:"0"`으로 둔다.
///
/// - `emp_seqs`: 사람 — `div:"m"`, `user_id`=`org_id`=empSeq, `dept_line:false`
/// - `dept_ids`: 부서 — `div:"d"`, `user_id:"0"`, `org_id`=deptId, `deptline_yn:"1"`
///   (등록되면 서버가 그 부서원 전원을 수신참조로 전개한다)
/// - `existing`: 이미 실린 양식필수 노드. 순번(`doc_line_m_seq`)을 그 다음부터 매기고,
///   같은 대상이 거기 있으면 건너뛴다(같은 사람이 두 번 실릴 때의 거동은 미지수다).
///
/// ⛔ **해석 못 한 대상은 조용히 빠뜨리지 않고 에러로 올린다** — 참조자가 말없이 누락되면
/// 기안자는 보냈다고 믿는데 상대는 못 받고, 그게 되돌릴 수 없는 상신 뒤에 드러난다.
pub(crate) async fn build_cc_nodes(
    c: &Arc<GwClient>,
    emp_seqs: &[String],
    dept_ids: &[String],
    existing: &[Value],
) -> Result<CcPlan> {
    let mut plan = CcPlan::default();
    let persons = dedup_trimmed(emp_seqs);
    let depts = dedup_trimmed(dept_ids);
    if persons.is_empty() && depts.is_empty() {
        return Ok(plan);
    }

    // 이미 실린 대상 — (div, org_id). 부서는 org_id=deptId, 사람은 org_id=empSeq다.
    let taken: std::collections::HashSet<(String, String)> = existing
        .iter()
        .map(|n| (jstr(n.get("div")), jstr(n.get("org_id"))))
        .collect();
    let mut seq = existing.len();

    // ── 사람 ───────────────────────────────────────────────────────────────
    if !persons.is_empty() {
        // 명부(30분 캐시)로 empSeq → 소속 부서를 찾고, 그 부서의 **원본** 사원 레코드를 쓴다.
        // 명부 요약본에는 노드가 요구하는 positionCode/workStatus/compSeq가 없다.
        let roster = crate::modules::org::roster(c).await?;
        let mut raw_cache: std::collections::HashMap<String, Vec<Value>> = Default::default();

        for emp in &persons {
            let found = roster.iter().find(|m| jstr(m.get("empSeq")) == *emp);
            let Some(m) = found else {
                bail!(
                    "수신참조로 지정한 empSeq '{emp}' 를 조직도 명부에서 찾지 못했다 — \
                     find_person으로 정확한 empSeq를 확인할 것(명부는 부서 단위로 조립되어 \
                     전사 인원보다 적을 수 있다)."
                );
            };
            let dept_id = jstr(m.get("deptId"));
            let name = jstr(m.get("name"));
            if taken.contains(&("m".to_string(), emp.clone())) {
                plan.skipped.push(json!({ "kind": "person", "id": emp, "name": name,
                                          "reason": "이미 양식필수 수신참조에 포함됨" }));
                continue;
            }

            if !raw_cache.contains_key(&dept_id) {
                let raw = crate::modules::org::dept_members_raw(c, &dept_id).await?;
                raw_cache.insert(dept_id.clone(), raw);
            }
            let raw = raw_cache.get(&dept_id).expect("방금 넣었다");
            let Some(r) = raw.iter().find(|x| jstr(x.get("empSeq")) == *emp) else {
                bail!(
                    "수신참조 대상 '{name}'(empSeq {emp})의 조직도 원본 레코드를 부서 \
                     {dept_id} 에서 찾지 못했다 — 부서 이동 직후일 수 있으니 find_person으로 재확인할 것."
                );
            };

            seq += 1;
            plan.added.push(person_refer_node(r, seq));
            plan.persons.push(json!({ "empSeq": emp, "name": name }));
        }
    }

    // ── 부서 ───────────────────────────────────────────────────────────────
    if !depts.is_empty() {
        let tree = crate::modules::org::dept_tree(c).await?;
        let all = tree.get("depts").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        // deptId → 이름. 회사와 사업장이 deptId "1000"을 공유하므로 먼저 나온 쪽을 쓴다.
        let mut name_of: std::collections::HashMap<String, String> = Default::default();
        for d in &all {
            name_of.entry(jstr(d.get("deptId"))).or_insert_with(|| jstr(d.get("name")));
        }

        for dept in &depts {
            let Some(d) = all.iter().find(|x| jstr(x.get("deptId")) == *dept) else {
                bail!(
                    "수신참조로 지정한 부서 deptId '{dept}' 를 조직도 트리에서 찾지 못했다 — \
                     org_chart로 부서 id를 확인할 것."
                );
            };
            let name = jstr(d.get("name"));
            if taken.contains(&("d".to_string(), dept.clone())) {
                plan.skipped.push(json!({ "kind": "dept", "id": dept, "name": name,
                                          "reason": "이미 양식필수 수신참조에 포함됨" }));
                continue;
            }
            // 이름 경로: dept_tree의 path("1000|1000|2986|…")를 부서명으로 바꿔 잇는다.
            let path_name = jstr(d.get("path"))
                .split('|')
                .filter(|x| !x.is_empty())
                .map(|id| name_of.get(id).cloned().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("|");

            seq += 1;
            plan.added.push(dept_refer_node(dept, &name, &path_name, &c.comp_seq(), seq));
            plan.depts.push(json!({ "deptId": dept, "name": name }));
        }
    }

    Ok(plan)
}

/// 공백을 털고 빈 값·중복을 걷어낸 목록(입력 순서 유지).
fn dedup_trimmed(src: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    src.iter()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .filter(|x| seen.insert(x.clone()))
        .collect()
}

/// `Value`를 문자열로. 숫자도 문자열로 받는다(서버가 id를 양쪽으로 준다).
fn jstr(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 코드 문자열("600")을 정렬용 숫자로. 숫자가 아니면 `null`.
fn order_of(code: &str) -> Value {
    code.parse::<i64>().map(Value::from).unwrap_or(Value::Null)
}

/// 개인 수신참조 노드. `raw`는 `org::dept_members_raw`의 사원 레코드(gw102A02 원본).
fn person_refer_node(raw: &Value, seq: usize) -> Value {
    let g = |k: &str| jstr(raw.get(k));
    let emp = g("empSeq");
    let duty_cd = g("dutyCode");
    let grade_cd = g("positionCode");
    json!({
        "doc_line_gb": "1", "div": "m", "org_div": "m",
        "act_id": 5000, "act_order": 5000, "act_nm": "수신참조", "act_type": "40",
        "user_id": emp, "org_id": emp, "user_nm": g("empName"),
        "dept_id": g("deptSeq"), "dept_nm": g("deptName"), "dept_nm_disp": g("deptName"),
        "dp_nm_disp": Value::Null,
        "co_id": g("compSeq"), "co_nm": g("compName"),
        "biz_id": g("bizSeq"), "biz_nm": g("bizName"),
        "dept_line": false, "deptline_yn": "0",
        "doc_line_m_seq": seq, "doc_line_mseq": seq, "seq": seq,
        "doc_line_s_seq": 1, "doc_line_sseq": 1,
        "duty_cd": duty_cd, "duty_nm": g("dutyName"), "duty_order": order_of(&duty_cd),
        "grade_cd": grade_cd, "grade_nm": g("positionName"), "grade_order": order_of(&grade_cd),
        "login_id": g("loginId"),
        // 서버 노드의 path_name은 '|' 구분인데 조직도는 '>'로 준다(실측).
        "path_name": g("pathName").replace('>', "|"),
        "work_status": g("workStatus"), "work_order": 1,
        // 양식이 강제한 참조자가 아니라 기안자가 더한 것이므로 must_yn=0.
        "must_yn": "0", "app_yn": "0", "arbitary_yn": "0"
    })
}

/// 부서 수신참조 노드. 부서는 사람과 달리 `user_id:"0"`이고 `org_id`가 deptId다.
fn dept_refer_node(dept_id: &str, name: &str, path_name: &str, co_id: &str, seq: usize) -> Value {
    json!({
        "doc_line_gb": "1", "div": "d", "org_div": "d",
        "act_id": 5000, "act_order": 5000, "act_nm": "수신참조", "act_type": "40",
        "user_id": "0", "org_id": dept_id, "user_nm": name,
        "dept_id": dept_id, "dept_nm": name, "dept_nm_disp": name,
        "dp_nm_disp": Value::Null,
        "co_id": co_id, "co_nm": "(주)이노그리드",
        "biz_id": co_id, "biz_nm": "(주)이노그리드",
        "dept_line": true, "deptline_yn": "1",
        "doc_line_m_seq": seq, "doc_line_mseq": seq, "seq": seq,
        "doc_line_s_seq": 1, "doc_line_sseq": 1,
        "duty_cd": "", "duty_nm": "", "duty_order": Value::Null,
        "grade_cd": "", "grade_nm": "", "grade_order": Value::Null,
        "login_id": "", "path_name": path_name,
        "work_status": "", "work_order": 1,
        "must_yn": "0", "app_yn": "0", "arbitary_yn": "0"
    })
}

/// 로컬 파일들을 ECM(`ecm001A01`)에 올리고 상신 payload 의 `pVCM_ATTACHFILEINFO` 항목으로 만든다.
/// 반환: (항목 배열, `modifyFileList` HTML).
///
/// ⚠️ **신규 첨부는 기존 첨부와 항목 구성이 다르다**(07 §11.5.3 브라우저 캡처). 갓 올린 파일은
/// **`fileId` 하나면 되고 `fileSn`을 보내지 않는다**(서버가 저장할 때 부여한다). ECM 메타
/// (`createdAt`/`hashValue`/`linkedFilePath`/`type`/`originalFileName` …)도 싣지 않는다 —
/// 이미 저장된 첨부를 다시 실을 때만 그것들이 붙는다.
///
/// ⚠️ 업로드의 `moduleGbn`은 **`EAP`** 다. 파일의 `type`이 그 값 그대로 새겨지고(§11.5),
/// 항목의 `moduleGbn`도 `EAP`다. (⚠️ **다운로드만 `BOARD`** — 방향에 따라 다르다. §11.2)
async fn upload_attachments(
    c: &GwClient,
    paths: &[String],
    co_id: &str,
    user_id: &str,
    doc_id: &str,
) -> Result<(Vec<Value>, String)> {
    if paths.is_empty() {
        return Ok((vec![], String::new()));
    }
    let mut items = Vec::with_capacity(paths.len());
    let mut changes = String::new();

    for (idx, p) in paths.iter().enumerate() {
        let bytes = std::fs::read(p).map_err(|e| anyhow!("첨부 읽기 실패 {p}: {e}"))?;
        let full_name = std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("첨부 경로에서 파일명을 얻지 못했습니다: {p}"))?;
        let size = bytes.len();

        // 폼은 401 재시도 때 다시 조립될 수 있어 디스크 I/O 를 밖에서 끝내둔다(mail 쪽과 같은 이유).
        let name_for_form = full_name.clone();
        let form = move || {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(name_for_form.clone())
                .mime_str("application/octet-stream")
                .expect("고정 MIME 문자열");
            reqwest::multipart::Form::new()
                .part("file[]", part)
                .text("moduleGbn", "EAP")
        };
        let up = c.call_multipart("/ecm/ecm001A01", form).await?;
        let file_id = up
            .pointer("/resultData/list/0/fileId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("ecm001A01 업로드 응답에 fileId 없음({full_name}): {up}"))?
            .to_string();

        // 서버는 이름과 확장자를 나눠 갖는다 — fileName 은 확장자 없는 이름, filePath 가 원본 파일명.
        let (stem, ext) = match full_name.rsplit_once('.') {
            Some((a, b)) if !a.is_empty() => (a.to_string(), b.to_string()),
            _ => (full_name.clone(), String::new()),
        };
        let size_text = human_size(size);
        items.push(json!({
            "fileId": file_id,
            "fileName": stem,
            "fileExtsn": ext,
            "filePath": full_name,
            "fileSize": size_text,
            "noConvertFileSize": size,
            "title": format!("{stem}{size_text}"),
            "fileClass": icon_class(&ext),
            "fileThumUrl": "", "fileUrl": "", "filePublicYn": "N",
            "modifyLocalAttach": "N", "link": "N", "fileDeleteYN": "Y",
            "id": idx,
            "moduleGbn": "EAP",
            "authKeyMap": {
                "compSeq": co_id, "empSeq": user_id, "docId": doc_id, "migYn": "0"
            }
        }));
        changes.push_str(&format!("<p class=\"le\">[추가]{full_name}</p>"));
    }
    Ok((items, changes))
}

/// 서버 표기와 같은 크기 문자열. 실측: 42 → "42 Bytes", 56849 → "55.52 KB".
fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} Bytes")
    } else {
        format!("{:.2} KB", n as f64 / 1024.0)
    }
}

/// 첨부 목록의 아이콘 클래스. 표시용이고 실측으로 확인된 것은 `txt`(`icon_txt`)와
/// 그 밖(`icon_etc`, md 가 그랬다)뿐이라 그 둘만 구분한다.
fn icon_class(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "txt" => "icon_txt",
        _ => "icon_etc",
    }
}

/// HP interlock이 쥐고 있는 그 신청의 `bindData`(ITEMS/TABLE)를 받아온다.
///
/// 응답은 `{contents:"<bindData JSON 문자열>", title}` 뿐이다 — **HTML은 안 준다**(실측).
/// `resultCode`가 없는 응답이라 `call`의 성공판정에 걸리므로 `call_raw`로 받는다.
/// 본문에 박히는 연차 잔여·사용·차감 같은 집계의 정답이 여기 있다.
async fn fetch_interlock_bind(c: &Arc<GwClient>, link_key: &str, co_cd: &str) -> Result<Option<Value>> {
    if link_key.trim().is_empty() {
        return Ok(None);
    }
    let r = c
        .call_raw(
            "/human/attendapplication/interlock/getInterlockFormContents",
            &json!({"linkKey": link_key, "coCd": co_cd, "vPCoCd": co_cd}),
        )
        .await?;
    let s = r
        .pointer("/response/contents")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if s.is_empty() {
        return Ok(None);
    }
    Ok(serde_json::from_str(&s).ok())
}

/// approkey = "ERP_<uuid4-ish>" — 16 랜덤바이트를 uuid 포맷으로.
/// (첨부 목록 조회도 `eap110A03`를 부르므로 `approval` 모듈이 같이 쓴다 — 07 §11.1.)
pub(crate) fn gen_approkey() -> String {
    let b: [u8; 16] = rand::random();
    format!(
        "ERP_{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// JS encodeURIComponent 동등 — A-Za-z0-9 와 `-_.!~*'()` 만 남기고 UTF-8 바이트를 %XX 로.
pub(crate) fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for &byte in s.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// 현재 KST(UTC+9) "YYYY-MM-DD HH:MM:SS".
pub(crate) fn now_kst_datetime() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 9 * 3600;
    let days = secs / 86400;
    let tod = secs % 86400;
    let (y, m, d) = days_to_ymd(days);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    /// 크기 문자열은 **서버 표기와 같아야** 한다 — 브라우저 캡처 실측값으로 고정한다(07 §11.5.3/§11.1).
    #[test]
    fn human_size는_서버표기를_따른다() {
        assert_eq!(human_size(42), "42 Bytes");     // 캡처: newattach.txt
        assert_eq!(human_size(68), "68 Bytes");     // 캡처: test 복사본.md
        assert_eq!(human_size(56849), "55.52 KB");  // 캡처: 20260803_17_57_10.png
        assert_eq!(human_size(1023), "1023 Bytes"); // 경계: 1KB 미만은 Bytes
        assert_eq!(human_size(1024), "1.00 KB");
    }

    #[test]
    fn icon_class는_확인된_것만_구분한다() {
        assert_eq!(icon_class("txt"), "icon_txt");
        assert_eq!(icon_class("TXT"), "icon_txt", "확장자는 대소문자를 가리지 않는다");
        assert_eq!(icon_class("md"), "icon_etc", "캡처에서 md 는 icon_etc 였다");
        assert_eq!(icon_class(""), "icon_etc");
    }

    /// 파일명은 **이름과 확장자를 나눠** 싣는다. `filePath` 만 원본 그대로다(캡처 §11.5.3).
    #[test]
    fn 파일명은_이름과_확장자로_나뉜다() {
        let split = |n: &str| match n.rsplit_once('.') {
            Some((a, b)) if !a.is_empty() => (a.to_string(), b.to_string()),
            _ => (n.to_string(), String::new()),
        };
        assert_eq!(split("newattach.txt"), ("newattach".into(), "txt".into()));
        assert_eq!(split("test 복사본.md"), ("test 복사본".into(), "md".into()));
        // 점이 여럿이면 마지막 것이 확장자
        assert_eq!(split("a.b.zip"), ("a.b".into(), "zip".into()));
        // 확장자가 없으면 이름만 남고 확장자는 빈 문자열
        assert_eq!(split("README"), ("README".into(), "".into()));
        // 숨김파일(.gitignore)은 이름이 비지 않도록 통째로 이름 취급
        assert_eq!(split(".gitignore"), (".gitignore".into(), "".into()));
    }

    /// 첨부가 없으면 **빈 배열과 빈 문자열** — 기존 payload 와 완전히 같은 모양이어야 한다
    /// (첨부 기능 추가가 첨부 없는 상신을 바꾸면 안 된다).
    #[tokio::test]
    async fn 첨부가_없으면_네트워크를_타지_않는다() {
        let c = GwClient::new(None);
        let (items, mfl) = upload_attachments(&c, &[], "1000", "3166", "0")
            .await
            .expect("빈 입력은 네트워크 없이 성공해야 한다");
        assert!(items.is_empty());
        assert_eq!(mfl, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity {
            co: "1000".into(), dept: "BB999".into(), emp: "22222".into(), name: "김철수".into(),
            dept_nm: "인프라팀".into(), duty: "팀장".into(), position: "부장".into(),
            co_nm: "(주)이노그리드".into(),
        }
    }

    /// draftHelp 예시(출장 ITEMS)에 박힌 타인 표시값이 전부 로그인 사용자 값으로 바뀌어야 한다.
    /// 이게 안 되면 남의 이름·부서·직급이 결재문서 본문에 그대로 찍힌다.
    #[test]
    fn 표시필드까지_전부_주입된다() {
        let mut v = json!({
            "empNm": "이재학", "employees": "이재학 책임연구원", "empNmDutyNm": "이재학 팀원",
            "singleDeptNm": "네이티브 플랫폼팀", "singlePositionNm": "책임연구원", "singleDutyNm": "팀원",
            "deptNm": "네이티브 플랫폼팀", "divNm": "(주)타사", "empCd": "11097", "taskDc": "업무내용"
        });
        inject_identity(&mut v, &me());
        assert_eq!(v["empNm"], "김철수");
        assert_eq!(v["employees"], "김철수 부장");
        assert_eq!(v["empNmDutyNm"], "김철수 팀장");
        assert_eq!(v["singleDeptNm"], "인프라팀");
        assert_eq!(v["singlePositionNm"], "부장");
        assert_eq!(v["singleDutyNm"], "팀장");
        assert_eq!(v["deptNm"], "인프라팀");
        assert_eq!(v["divNm"], "(주)이노그리드");
        assert_eq!(v["empCd"], "22222");
        // 신원과 무관한 필드는 건드리지 않는다.
        assert_eq!(v["taskDc"], "업무내용");
    }

    /// 없는 키를 새로 만들지 않는다(양식마다 필드 구성이 달라 임의 추가는 위험).
    #[test]
    fn 없는_키는_추가하지_않는다() {
        let mut v = json!({ "empNm": "이재학" });
        inject_identity(&mut v, &me());
        assert!(v.get("singleDeptNm").is_none());
        assert!(v.get("employees").is_none());
    }

    /// 조직도 조회 실패(표시정보 빈 값) 시엔 예시값을 지우지 말고 그대로 둔다 —
    /// 빈 문자열로 덮으면 문서에 부서·직급이 통째로 사라진다.
    #[test]
    fn 표시정보를_모르면_예시값을_유지한다() {
        let mut v = json!({ "empNm": "이재학", "singleDeptNm": "네이티브 플랫폼팀", "employees": "이재학 책임연구원" });
        let unknown = Identity {
            co: "1000".into(), dept: "BB999".into(), emp: "22222".into(), name: "김철수".into(),
            dept_nm: String::new(), duty: String::new(), position: String::new(), co_nm: String::new(),
        };
        inject_identity(&mut v, &unknown);
        assert_eq!(v["empNm"], "김철수");                       // 아는 값은 바꾸고
        assert_eq!(v["singleDeptNm"], "네이티브 플랫폼팀");      // 모르는 값은 유지
        assert_eq!(v["employees"], "이재학 책임연구원");
    }

    /// JS `encodeURIComponent` 동등성. 틀리면 상신 본문이 깨지는데 증상이 서버 2099로만 드러나
    /// 원인 추적이 매우 어렵다. 기대값은 JS 규격(비이스케이프 집합 `A-Za-z0-9-_.!~*'()`)에서 도출.
    /// ⚠️ `client::form_urlencode`(공백→`+`)와 **규칙이 다르다** — 여기선 공백이 `%20`.
    #[test]
    #[allow(non_snake_case)] // 이름 속 `JS` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn encode_uri_component는_JS와_같은_규칙이다() {
        assert_eq!(encode_uri_component("a b"), "a%20b");
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(encode_uri_component("azAZ09"), "azAZ09");
        assert_eq!(encode_uri_component("<div>"), "%3Cdiv%3E");
        assert_eq!(encode_uri_component("가"), "%EA%B0%80");
        assert_eq!(encode_uri_component("&=?#/"), "%26%3D%3F%23%2F");
        assert_eq!(encode_uri_component(""), "");
    }

    /// a03가 준 참가자 노드는 원본 그대로 두고 `org_div`(=div)만 덧붙인다 —
    /// 브라우저 성공 payload와의 유일한 차이라서 재구성하면 어긋난다.
    #[test]
    fn norm_participant는_org_div만_덧붙인다() {
        let src = json!({ "org_id": "3052", "div": "d", "act_id": 5000, "dept_line": "x" });
        let out = norm_participant(&src);
        assert_eq!(out["org_div"], "d");
        assert_eq!(out["org_id"], "3052");
        assert_eq!(out["act_id"], 5000);
        assert_eq!(out["dept_line"], "x");
        assert_eq!(norm_participant(&json!({ "org_id": "1" }))["org_div"], "m");
    }

    #[test]
    fn 상신은_doc_id가_없으면_성공이_아니다() {
        assert_eq!(submitted_doc_id(&json!({ "result": 140689 })), Some(json!(140689)));
        assert_eq!(submitted_doc_id(&json!({ "result": "140689" })), Some(json!("140689")));
        for bad in [
            json!({ "result": Value::Null }),
            json!({}),
            json!({ "result": "" }),
            json!({ "result": "  " }),
            json!({ "result": 0 }),
            json!({ "result": "0" }),
            json!({ "result": { "docId": 1 } }),
        ] {
            assert_eq!(submitted_doc_id(&bad), None, "{bad} 를 성공으로 봤다");
        }
    }

    #[test]
    fn 실행콜은_return_value_1만_성공신호로_본다() {
        let ok = step_result("상신취소", "eap110A18", &json!({ "returnValue": 1 }));
        assert_eq!(ok["ok"], json!(true));
        assert_eq!(ok["returnValue"], json!(1));
        assert_eq!(ok["api"], json!("eap110A18"));
        assert_eq!(
            step_result("x", "y", &json!({ "returnValue": 0 }))["ok"],
            json!(false)
        );
        // 본문이 없는 응답은 그 자체로 실패가 아니다 — 최종 판정은 read-back이 한다.
        assert_eq!(step_result("x", "y", &json!({}))["ok"], json!(true));
        assert_eq!(step_result("x", "y", &Value::Null)["returnValue"], json!(null));
    }

    #[test]
    fn 취소_판정은_purge_여부로_갈린다() {
        // purge=false: 임시보관(10)으로 내려왔을 때만 성공
        assert!(verify_cancel(false, "10"));
        assert!(!verify_cancel(false, "20"));
        assert!(!verify_cancel(false, "30"));
        assert!(
            !verify_cancel(false, DOC_STS_DELETED),
            "삭제는 임시보관 복귀가 아니다"
        );

        // purge=true: 삭제 상태(999)만 성공. 실측상 삭제는 999라는 **양성 신호**를 남기므로
        // 빈 값(=상태를 못 읽음)을 소멸의 증거로 세지 않는다.
        assert!(verify_cancel(true, DOC_STS_DELETED));
        assert!(!verify_cancel(true, ""), "판독 불가는 삭제 확인이 아니다");
        assert!(!verify_cancel(true, "10"), "아직 임시보관에 남아 있다");
        assert!(!verify_cancel(true, "30"));
    }

    fn ok_step() -> Value {
        step_result("임시보관삭제", "eap110A19", &json!({ "returnValue": 1 }))
    }

    /// 실측 응답 봉투 — **존재하는 문서**. 케이스별로 다른 것은 `doc_sts`와 기안자뿐이다.
    fn a98_found(doc_sts: &str, user_id: &str) -> Value {
        json!({ "resultCode": 0, "resultMsg": "성공하였습니다.", "resultData": {
            "biz_id": "1000", "co_id": "1000", "cooperate_doc_id": 0, "dept_id": "<deptSeq>",
            "doc_no_base": "<부서명>-2607-", "doc_sts": doc_sts, "receive_type": "",
            "repdt_mod_yn": "0", "req_api_sts": "0", "user_id": user_id }})
    }

    /// 실측 응답 봉투 — **조회되지 않는 docId**. 미발급·형식오류 여섯 경우가 바이트 단위로 동일했다.
    fn a98_null() -> Value {
        json!({ "resultCode": 0, "resultData": null, "resultMsg": "성공하였습니다." })
    }

    /// 실측 12입력의 판별표. 핵심은 **`resultCode`·`resultMsg`·http로는 아무것도 못 가른다**는 것 —
    /// 열두 케이스가 전부 `200 / 0 / "성공하였습니다."`이고 오직 `resultData` 유무만이 판별자다.
    #[test]
    fn 판별자는_result_data_유무_하나뿐이다() {
        // (설명, 응답, 기대 doc_sts — None이면 NotFound)
        let cases: [(&str, Value, Option<&str>); 12] = [
            ("#1 137650 내 문서(종결)", a98_found("90", "<me>"), Some("90")),
            ("#2 137548 내 문서(반려)", a98_found("100", "<me>"), Some("100")),
            ("#3 141826 삭제한 문서", a98_found("999", "<me>"), Some("999")),
            ("#4 140668 남의 문서(수신참조)", a98_found("90", "<other>"), Some("90")),
            ("#5 139174 남의 문서(수신참조)", a98_found("90", "<other>"), Some("90")),
            ("#6 999999 미발급 6자리", a98_null(), None),
            ("#7 99999999 미발급 8자리", a98_null(), None),
            ("#8 \"\" 빈 문자열", a98_null(), None),
            ("#9 abc 숫자 아님", a98_null(), None),
            ("#10 -1 음수", a98_null(), None),
            ("#11 141826.5 소수점", a98_null(), None),
            ("#12 \" 141826 \" 공백(서버가 트림)", a98_found("999", "<me>"), Some("999")),
        ];
        for (label, resp, want) in cases {
            // 전부 같은 코드·메시지였다는 사실 자체를 표에 못박는다.
            assert_eq!(resp["resultCode"], json!(0), "{label}");
            assert_eq!(resp["resultMsg"], json!("성공하였습니다."), "{label}");
            match (classify(200, &resp).expect(label), want) {
                (DocState::Found { doc_sts, .. }, Some(w)) => assert_eq!(doc_sts, w, "{label}"),
                // ⛔ 회귀 방지: `resultData:null`을 `Found{999}`로 접으면 **존재한 적 없는 번호**에
                //    "이미 삭제됐음을 조회로 확인했다"고 답하게 된다.
                (DocState::NotFound, None) => {}
                (got, _) => panic!("{label}: {got:?} 로 분류됐다"),
            }
        }
    }

    #[test]
    fn 기안자는_user_id에서_온다() {
        let DocState::Found { owner, .. } = classify(200, &a98_found("30", "2792")).unwrap() else {
            panic!("객체 resultData는 Found다");
        };
        assert_eq!(owner, "2792", "소유권 가드가 이 값을 쓴다");
        // user_id가 없으면 빈 문자열 — 가드가 fail-closed로 거부한다(가드 테스트에서 확인).
        let DocState::Found { owner, .. } =
            classify(200, &json!({ "resultData": { "doc_sts": "30" }})).unwrap()
        else {
            panic!("doc_sts만 있어도 Found다");
        };
        assert_eq!(owner, "");
    }

    /// 해석할 수 없는 응답을 **상태로 접지 않는다.** 접는 순간 "확인했다"는 거짓 주장이 붙는다.
    #[test]
    fn 해석할_수_없는_응답은_에러다() {
        // http≠200 — 봉투가 멀쩡해 보여도 신뢰하지 않는다.
        assert!(classify(500, &a98_found("30", "<me>")).is_err(), "비200은 판독 불가다");
        assert!(classify(0, &a98_null()).is_err(), "응답 없음(http=0)도 판독 불가다");
        // resultData 키 자체가 없음
        assert!(classify(200, &json!({ "resultCode": 0, "resultMsg": "성공하였습니다." })).is_err());
        // resultData가 객체인데 doc_sts가 없음
        assert!(classify(200, &json!({ "resultData": { "user_id": "<me>" }})).is_err());
        // resultData가 객체도 null도 아님
        assert!(classify(200, &json!({ "resultData": "999" })).is_err());
        assert!(classify(200, &json!({ "resultData": [] })).is_err());
    }

    #[test]
    fn 삭제_확인은_999로만_선다() {
        // 정상: 실행 성공 + 사후 999
        let v = cancel_verdict("1", "10", true, vec![ok_step()], &PostState::Found("999".into()));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["verified_by_readback"], json!(true));
        assert_eq!(v["postState"], json!("found"));
        assert_eq!(v["postDocSts"], json!("999"));

        // ⛔ 회귀 방지: 재조회가 문서를 못 찾거나(NotFound) 아예 못 읽어도(Unreadable)
        //    **삭제 확인으로 세지 않는다**. 예전 판정은 이 둘을 성공으로 흘려보냈다.
        for post in [
            PostState::NotFound,
            PostState::Unreadable("http=500".into()),
        ] {
            let v = cancel_verdict("1", "10", true, vec![ok_step()], &post);
            assert_eq!(v["ok"], json!(false), "{}", v["note"]);
            assert_eq!(v["verified_by_readback"], json!(false));
            assert_eq!(v["postDocSts"], json!(null));
            let note = v["note"].as_str().unwrap();
            assert!(note.contains("확인하지 못했다"), "{note}");
        }
        assert_eq!(
            cancel_verdict("1", "10", true, vec![ok_step()], &PostState::NotFound)["postState"],
            json!("notFound")
        );
        assert_eq!(
            cancel_verdict("1", "10", true, vec![ok_step()], &PostState::Unreadable("x".into()))
                ["postState"],
            json!("unreadable")
        );

        // 아직 남아 있으면 실패
        let v = cancel_verdict("1", "10", true, vec![ok_step()], &PostState::Found("10".into()));
        assert_eq!(v["ok"], json!(false));
        assert!(v["note"].as_str().unwrap().contains("아직 남아 있다"));
    }

    #[test]
    fn 실행_신호가_나빠도_ok가_서면_안_된다() {
        // returnValue가 1이 아니면 사후 상태가 목표여도 ok:false — steps 판정이 살아 있어야 한다.
        let bad = step_result("상신취소", "eap110A18", &json!({ "returnValue": 0 }));
        let v = cancel_verdict("1", "30", false, vec![bad], &PostState::Found("10".into()));
        assert_eq!(v["ok"], json!(false), "{}", v["note"]);
        assert_eq!(
            v["verified_by_readback"],
            json!(true),
            "재조회는 목표 상태였다 — 실패 사유는 실행 신호다"
        );
        assert!(v["note"].as_str().unwrap().contains("returnValue"));
    }

    #[test]
    fn 아무것도_실행하지_않았으면_note가_그렇게_말한다() {
        // purge=false인데 이미 doc_sts 10 → 실행 콜 0건. "복귀를 확인했다"고 말하면 거짓이다.
        let v = cancel_verdict("1", "10", false, vec![], &PostState::Found("10".into()));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["steps"], json!([]));
        let note = v["note"].as_str().unwrap();
        assert!(note.contains("실행한 단계 없음"), "{note}");
    }

    #[test]
    fn 사전조회가_실행_전에_결말을_가른다() {
        let me = "3166";
        let mine = |sts: &str| DocState::Found {
            doc_sts: sts.to_string(),
            owner: me.to_string(),
        };

        // ① 살아 있는 내 문서 → 진행
        assert!(matches!(
            pre_verdict("1", &mine("30"), true, me, "41"),
            PreVerdict::Proceed(s) if s == "30"
        ));

        // ② 문서 없음 → 호출자 잘못(invalid_params로 매핑되도록 타입으로 전달)
        let PreVerdict::Reject(e) = pre_verdict("999999", &DocState::NotFound, true, me, "41") else {
            panic!("NotFound는 실행 없이 거부돼야 한다");
        };
        assert!(e.downcast_ref::<crate::error::InvalidInput>().is_some());
        assert!(e.to_string().contains("999999"));

        // ③ 이미 삭제 + purge → 실행 없이 성공(멱등). 취소 콜을 다시 쏘지 않는다.
        assert!(matches!(
            pre_verdict("1", &mine(DOC_STS_DELETED), true, me, "41"),
            PreVerdict::AlreadyDeleted
        ));

        // ④ 이미 삭제 + purge=false → 되돌릴 수 없으니 실패
        let PreVerdict::Reject(e) = pre_verdict("1", &mine(DOC_STS_DELETED), false, me, "41") else {
            panic!("삭제된 문서를 임시보관으로 되돌릴 수는 없다");
        };
        assert!(e.downcast_ref::<crate::error::InvalidInput>().is_none());
        assert!(e.to_string().contains("이미 삭제된 문서"));

        // ⑤ 남의 문서 → NotOwner(실행 콜을 쏘지 않는다. 서버 거동이 미관측이라 fail-closed)
        let others = DocState::Found {
            doc_sts: "30".into(),
            owner: "2792".into(),
        };
        let PreVerdict::Reject(e) = pre_verdict("1", &others, true, me, "41") else {
            panic!("남의 문서는 거부돼야 한다");
        };
        assert!(e.downcast_ref::<crate::error::NotOwner>().is_some());

        // ⑥ 진행중(30) 문서인데 form_id가 없으면 결재취소를 쏠 수 없다 → 호출자 잘못
        let PreVerdict::Reject(e) = pre_verdict("1", &mine("30"), true, me, "  ") else {
            panic!("form_id 없이 doc_sts=30을 취소할 수는 없다");
        };
        assert!(e.downcast_ref::<crate::error::InvalidInput>().is_some());
        assert!(e.to_string().contains("form_id"));
        // 20(상신)은 form_id 없이도 진행할 수 있다(eap110A18만 쏜다)
        assert!(matches!(
            pre_verdict("1", &mine("20"), false, me, ""),
            PreVerdict::Proceed(_)
        ));

        // ⑦ 기안자를 알 수 없으면(응답에 user_id 없음) **거부**한다 — fail-closed.
        //    `resource::check_owner`가 empSeq 부재를 ""와 비교해 거부하는 것과 같은 규약이다.
        let unknown = DocState::Found {
            doc_sts: "20".into(),
            owner: String::new(),
        };
        let PreVerdict::Reject(e) = pre_verdict("1", &unknown, false, me, "41") else {
            panic!("기안자를 모르면 거부한다 — '모른다'는 '내 것'이라는 근거가 아니다");
        };
        assert!(e.downcast_ref::<crate::error::NotOwner>().is_some());

        // ⑧ 본인 empSeq를 모를 때도 마찬가지. 여기서 열어주면 **모든 문서**에 가드가 꺼진다.
        let PreVerdict::Reject(e) = pre_verdict("1", &mine("20"), false, "", "41") else {
            panic!("본인 empSeq를 모르면 거부한다");
        };
        assert!(e.downcast_ref::<crate::error::NotOwner>().is_some());
    }

    /// 취소 거동이 **관측된 상태(10/20/30)** 밖으로는 파괴 콜을 쏘지 않는다.
    /// A19는 지울 대상이 없어도 `returnValue:1`을 주므로, 쏜 뒤에 응답을 봐서는 알 수 없다.
    #[test]
    fn 관측되지_않은_상태에는_취소를_쏘지_않는다() {
        let me = "3166";
        let mine = |sts: &str| DocState::Found {
            doc_sts: sts.to_string(),
            owner: me.to_string(),
        };
        for sts in ["90", "100", "50"] {
            for purge in [true, false] {
                let PreVerdict::Reject(e) = pre_verdict("1", &mine(sts), purge, me, "41") else {
                    panic!("doc_sts={sts}(purge={purge})는 거부돼야 한다");
                };
                let msg = e.to_string();
                assert!(msg.contains("관측되지 않았습니다"), "거부 이유를 밝혀야 한다: {msg}");
                assert!(msg.contains(sts), "{msg}");
            }
        }
        // 관측된 셋은 그대로 진행한다(10=임시보관은 purge=true에서 A19만 쏜다).
        for sts in ["10", "20", "30"] {
            assert!(
                matches!(pre_verdict("1", &mine(sts), true, me, "41"), PreVerdict::Proceed(s) if s == sts),
                "doc_sts={sts}"
            );
        }
    }

    #[test]
    fn 이미_삭제된_문서는_실행_없이_멱등_성공이다() {
        let v = already_deleted("141826");
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["already"], json!(true));
        assert_eq!(v["steps"], json!([]), "실행 콜을 쏘지 않았음이 드러나야 한다");
        assert_eq!(v["preDocSts"], json!(DOC_STS_DELETED));
        // 사후조회를 하지 않았으므로 "취소 실행 뒤 재조회 결과" 필드를 채우지 않는다.
        assert_eq!(v.get("postDocSts"), None, "하지 않은 조회를 한 것처럼 적으면 안 된다");
        assert_eq!(v.get("postState"), None);
        assert!(v["note"].as_str().unwrap().contains("실행한 단계 없음"));
    }

    /// 회귀: 신원 주입이 방문할 곳을 손으로 열거하던 시절, `bindData.TABLE...group[].items`·
    /// 중첩 `applicationList[].employeeList[]`·`weeklyOvertimeWorkInfo`가 빠져 **예시 작성자의
    /// 사번·이름·부서가 그대로 상신**됐다(4개 양식 중 3개, 20곳). 이쪽은 실패하지 않고 성공해서
    /// 아무도 못 본다 — **번들된 실제 가이드 payload**로 훑어 흔적이 하나도 없음을 못박는다.
    #[test]
    fn 실제_가이드_예시에_예시작성자_신원이_남지_않는다() {
        let guides: Value = serde_json::from_str(crate::modules::submission_guide::BUNDLED)
            .expect("번들 가이드가 유효한 JSON이어야 한다");
        let forms = guides["forms"].as_object().expect("forms 오브젝트");
        let id = me();

        // 예시 작성자를 가리키는 값들. 하나라도 남으면 남의 신원이 문서에 찍힌다.
        const AUTHOR: [&str; 5] = ["11097", "이재학", "AA121", "네이티브 플랫폼팀", "책임연구원"];

        /// 트리에서 예시 작성자 흔적이 있는 경로를 모은다.
        fn traces(node: &Value, path: &str, out: &mut Vec<String>) {
            match node {
                Value::Object(map) => {
                    for (k, v) in map {
                        traces(v, &format!("{path}/{k}"), out);
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        traces(v, &format!("{path}[{i}]"), out);
                    }
                }
                Value::String(s) if AUTHOR.iter().any(|a| s.contains(a)) => {
                    out.push(format!("{path} = {s}"))
                }
                _ => {}
            }
        }

        let mut checked = 0;
        for (form, g) in forms {
            for key in ["hpApplicationExample", "bindDataExample"] {
                // 예시는 JSON 문자열이 아니라 오브젝트로 들어 있다(양쪽 다 받아둔다).
                let Some(raw) = g["draftHelp"].get(key) else { continue };
                let mut payload: Value = match raw.as_str() {
                    Some(text) => serde_json::from_str(text)
                        .unwrap_or_else(|e| panic!("{form}/{key} 예시가 유효한 JSON이 아니다: {e}")),
                    None => raw.clone(),
                };

                // 주입 전에는 흔적이 있어야 한다 — 없으면 이 테스트가 아무것도 지키지 못한다.
                let mut before = Vec::new();
                traces(&payload, "", &mut before);
                assert!(!before.is_empty(), "{form}/{key}: 예시에 작성자 신원이 없어 회귀를 못 잡는다");

                inject_identity_deep(&mut payload, &id);

                let mut after = Vec::new();
                traces(&payload, "", &mut after);
                assert!(
                    after.is_empty(),
                    "{form}/{key}: 주입 후에도 예시 작성자 신원이 {}곳 남았다 →\n  {}",
                    after.len(),
                    after.join("\n  ")
                );
                checked += 1;
            }
        }
        assert!(checked >= 4, "양식 예시를 {checked}개만 훑었다 — 대상이 사라졌는지 확인할 것");
    }

    /// `groupByKey`("<empCd><날짜>")는 empCd를 접두사로 품은 조합 문자열이라
    /// empCd만 갈아끼우면 옛 사번이 그대로 남는다.
    #[test]
    fn group_by_key의_사번_접두사도_교체된다() {
        let mut v = json!({ "empCd": "11097", "groupByKey": "1109720260803" });
        inject_identity(&mut v, &me());
        assert_eq!(v["empCd"], "22222");
        assert_eq!(v["groupByKey"], "2222220260803", "접두사 사번이 바뀌어야 한다");

        // 형식이 다르면(접두사가 옛 사번이 아니면) 건드리지 않는다 — 함부로 자르지 않는다.
        let mut other = json!({ "empCd": "11097", "groupByKey": "XX-20260803" });
        inject_identity(&mut other, &me());
        assert_eq!(other["groupByKey"], "XX-20260803");
    }

    #[test]
    fn now_kst_datetime은_상신일시_형식이다() {
        let t = now_kst_datetime();
        assert_eq!(t.len(), 19, "YYYY-MM-DD HH:MM:SS");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
        assert_eq!(&t[13..14], ":");
        assert!(t.starts_with("20"));
    }

    /// 조립한 개인 수신참조 노드가 **브라우저 상신 캡처의 양식필수 노드와 같은 필드 구성**인지.
    /// (`.claude-workspace/approval-analysis/captures/annual-half-am_eap110A06.json` 의 `pRefer[0]`
    /// = 정영준. 서버가 모르는 모양을 조용히 버리므로 필드가 빠지면 반영이 말없이 실패한다.)
    #[test]
    fn 개인_수신참조_노드는_캡처와_같은_필드를_갖는다() {
        let raw = json!({
            "empSeq": "2246", "empName": "정영준", "loginId": "jyj",
            "deptSeq": "3052", "deptName": "인사총무팀",
            "dutyCode": "600", "dutyName": "팀원",
            "positionCode": "900", "positionName": "사원",
            "compSeq": "1000", "compName": "(주)이노그리드",
            "bizSeq": "1000", "bizName": "(주)이노그리드",
            "workStatus": "999",
            "pathName": "(주)이노그리드>(주)이노그리드>경영지원부문>경영지원본부>인사지원실>인사총무팀"
        });
        let n = person_refer_node(&raw, 1);

        // 서버가 수신참조로 알아보는 핵심 4종.
        assert_eq!(n["act_id"], 5000, "수신참조 act_id");
        assert_eq!(n["act_type"], "40");
        assert_eq!(n["div"], "m");
        assert_eq!(n["org_div"], "m");
        // 사람은 user_id == org_id == empSeq (부서와 갈리는 지점).
        assert_eq!(n["user_id"], "2246");
        assert_eq!(n["org_id"], "2246");
        assert_eq!(n["dept_line"], false);
        assert_eq!(n["deptline_yn"], "0");
        // 양식이 강제한 참조자가 아니다.
        assert_eq!(n["must_yn"], "0");
        // 정렬 코드는 숫자로.
        assert_eq!(n["duty_order"], 600);
        assert_eq!(n["grade_order"], 900);
        // path_name 구분자는 '|' — 조직도는 '>'로 준다.
        assert_eq!(
            n["path_name"],
            "(주)이노그리드|(주)이노그리드|경영지원부문|경영지원본부|인사지원실|인사총무팀"
        );
        // 캡처 노드의 키를 하나도 빠뜨리지 않았는지.
        for k in [
            "doc_line_gb", "org_div", "dept_nm_disp", "dept_line", "act_id", "deptline_yn",
            "user_nm", "arbitary_yn", "div", "biz_nm", "dept_nm", "co_nm", "app_yn",
            "doc_line_m_seq", "duty_order", "duty_cd", "work_status", "seq", "work_order",
            "path_name", "grade_cd", "doc_line_mseq", "login_id", "act_order", "act_nm",
            "co_id", "dp_nm_disp", "doc_line_sseq", "doc_line_s_seq", "grade_nm", "must_yn",
            "user_id", "org_id", "act_type", "grade_order", "duty_nm", "biz_id", "dept_id",
        ] {
            assert!(n.get(k).is_some(), "캡처에 있는 필드 '{k}' 가 빠졌다");
        }
    }

    /// 부서 노드는 사람과 달리 `user_id:"0"` 이고 `org_id` 가 deptId다(캡처 `pRefer[1]` = 인사총무팀).
    #[test]
    fn 부서_수신참조_노드는_user_id가_0이다() {
        let n = dept_refer_node("3052", "인사총무팀", "(주)이노그리드|인사총무팀", "1000", 2);
        assert_eq!(n["div"], "d");
        assert_eq!(n["org_div"], "d");
        assert_eq!(n["user_id"], "0", "부서 노드의 user_id는 0");
        assert_eq!(n["org_id"], "3052", "부서 노드의 org_id는 deptId");
        assert_eq!(n["dept_line"], true);
        assert_eq!(n["deptline_yn"], "1");
        assert_eq!(n["act_id"], 5000);
        assert_eq!(n["must_yn"], "0");
        assert_eq!(n["doc_line_m_seq"], 2, "순번은 양식필수 다음부터");
    }

    #[test]
    fn 수신참조_대상은_공백과_중복을_걷어낸다() {
        let got = dedup_trimmed(&[
            " 3166 ".into(), "3166".into(), "".into(), "   ".into(), "2246".into(),
        ]);
        assert_eq!(got, vec!["3166".to_string(), "2246".to_string()]);
    }
}

