//! 메일 모듈 — `/mail/mail0*`

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::error::InvalidInput;
use crate::modules::board::{collapse_ws, html_to_text, json_str};

/// 메일함 **이름**. seq는 계정마다 다르므로 상수로 박지 않고 이름으로 해석한다 —
/// 자세한 이유는 `mbox_seq` 참조.
pub const INBOX: &str = "INBOX";
pub const DRAFTS: &str = "DRAFTS";

/// 메일함(폴더) 목록 — `mail000A01`
pub async fn list_mailboxes(c: &GwClient) -> Result<Value> {
    c.call("/mail/mail000A01", &json!({})).await
}

/// 메일 목록 조회 — `mail003A01`. `page_size`를 크게 주면 전량 조회(상한 없음).
///
/// ⚠️ **`boxName`은 서버가 무시한다**(실측: `mboxSeq`=SENT에 `boxName="INBOX"`를 실어도 보낸메일이
/// 나온다). 판단 기준은 `mboxSeq` 하나뿐이라 임시보관함 조회도 seq만 바꿔 이 함수를 그대로 쓴다.
/// 값을 고치지 않고 두는 이유는 받은메일함 조회가 이 값으로 이미 실증돼 있기 때문이다.
pub async fn list_mails(c: &GwClient, mbox_seq: i64, page: i64, page_size: i64) -> Result<Value> {
    c.call(
        "/mail/mail003A01",
        &json!({
            "boxName": "INBOX",
            "mainApiCode": "mail003A01",
            "mboxSeq": mbox_seq,
            "page": page,
            "pageSize": page_size,
            "sort": "rfc822date",
            "sortType": "desc",
            "listType": "",
            "showType": "",
            "seen": false
        }),
    )
    .await
}

/// `list_mailboxes` 응답에서 이름이 `want`인 메일함의 `mboxSeq`를 찾는다.
///
/// 응답이 중첩 구조로 올 수 있어(계정·서버 버전차) 키 존재로 노드를 판별하며 트리를 훑는다.
/// `fullname`("DRAFTS")과 `name` 둘 다 본다 — 어느 쪽에 들어오는지가 함마다 다르다.
/// `mboxSeq`가 숫자/문자열 양쪽으로 오는 것은 `json_str`로 흡수한다(이 API 계열의 고질).
fn find_mbox_seq(node: &Value, want: &str) -> Option<i64> {
    match node {
        Value::Object(map) => {
            if map.contains_key("mboxSeq")
                && [map.get("fullname"), map.get("name")]
                    .iter()
                    .any(|v| json_str(*v).eq_ignore_ascii_case(want))
                && let Ok(seq) = json_str(map.get("mboxSeq")).parse::<i64>()
            {
                return Some(seq);
            }
            map.values().find_map(|v| find_mbox_seq(v, want))
        }
        Value::Array(items) => items.iter().find_map(|v| find_mbox_seq(v, want)),
        _ => None,
    }
}

/// 메일함 이름 → `mboxSeq`. 못 찾으면 어느 이름을 찾았는지 밝힌 에러.
///
/// ⚠️ **`mboxSeq`를 상수로 박지 말 것.** 계정마다 값이 다르다. 예전에 `INBOX = 26986`
/// (개발자 계정의 값)을 박아둔 탓에 **다른 계정 전원이 받은메일함 조회에 실패**했다
/// (`resultCode=-1 msg=not Box : <메일주소> -> boxSeq(26986)`, 2026-08-20 제보로 발견).
/// 단일 계정 테스트로는 구조적으로 못 잡는 종류라, 새 메일함을 다룰 때도 반드시 이 함수를 거친다.
pub async fn mbox_seq(c: &GwClient, name: &str) -> Result<i64> {
    let boxes = list_mailboxes(c).await?;
    find_mbox_seq(&boxes, name).ok_or_else(|| {
        anyhow!("메일함 '{name}' 을 찾지 못했다 — list_mailboxes 응답에 그 이름의 mboxSeq가 없다")
    })
}

/// 받은메일함(INBOX) 목록 — 이름으로 seq를 해석한 뒤 `list_mails`.
pub async fn list_inbox(c: &GwClient, page: i64, page_size: i64) -> Result<Value> {
    let seq = mbox_seq(c, INBOX).await?;
    list_mails(c, seq, page, page_size).await
}

/// 임시보관함(DRAFTS) 목록 — 이름으로 seq를 해석한 뒤 `list_mails`.
/// 응답은 받은메일함과 동일한 서버 원본 봉투(메일 배열 키는 `Records`).
pub async fn list_drafts(c: &GwClient, page: i64, page_size: i64) -> Result<Value> {
    let seq = mbox_seq(c, DRAFTS).await?;
    list_mails(c, seq, page, page_size).await
}

/// 목록 응답(`Records`)에 그 `muid`가 있는지. read-back 검증용.
fn has_muid(list: &Value, muid: &str) -> bool {
    list.get("Records")
        .and_then(|r| r.as_array())
        .is_some_and(|rows| rows.iter().any(|m| json_str(m.get("muid")) == muid))
}

/// 해석 불가를 읽음으로 접으면 성공한 쓰기를 실패로 보고하므로 별도 상태로 둔다.
#[derive(Debug, PartialEq, Eq)]
enum SeenState {
    Unseen,
    Seen,
    Unknown,
}

/// 대상의 읽음 상태. 목록에서 찾지 못하면 `None`, 대상의 `seen`만 해석 불가면 `Unknown`.
/// API가 숫자·문자열·불리언을 혼용하므로 `json_str`로 흡수한다.
fn seen_flag(list: &Value, muid: &str) -> Option<SeenState> {
    list.get("Records")?
        .as_array()?
        .iter()
        .find(|m| json_str(m.get("muid")) == muid)
        .map(|m| match json_str(m.get("seen")).as_str() {
            "0" | "false" => SeenState::Unseen,
            "1" | "true" => SeenState::Seen,
            _ => SeenState::Unknown,
        })
}

/// `mark_unread_and_verify`가 대상을 확인하는 목록 창(최근 N건).
/// 도구 기본값 20보다 넉넉히 잡되 전량 조회는 하지 않는다 — 받은메일함이 만 단위인 계정이 있다.
const UNREAD_WINDOW: i64 = 200;

/// 메일함별 미읽음·전체 카운트 — `mail000A03`(body `{}`). **조회 전용, 부작용 없음**(실증 2026-08-31).
///
/// `list_mailboxes`(`mail000A01`)와 겹치지만 그쪽에 없는 **계정 전체 집계**가 배열 마지막 항목으로
/// 온다: `unreadCount`·`toMeCount`(나에게 온 것)·`flaggedCount`·`attachCount`·`totalCount`.
pub async fn mailbox_counts(c: &GwClient) -> Result<Value> {
    c.call("/mail/mail000A03", &json!({})).await
}

/// 받은메일 1건을 **읽지 않음으로 되돌린다** — `mail002A15` + read-back 검증.
///
/// **왜 필요한가**: `read_mail`(`mail002A01`)은 서버측 `seen` 플래그를 세운다(실증). 에이전트가
/// 대신 읽어버리면 사람이 "안 읽은 메일"로 다시 만날 방법이 없어진다. 그 되돌림이 이 함수다.
///
/// ⚠️ **`type`은 `"unseen"`으로 고정한다.** `mail002A15`는 범용 플래그 변경 API다(`uids` 복수형 +
/// `type` 판별자). `"seen"`·`"flagged"` 같은 다른 값은 **관측된 적이 없어** 무엇을 하는지 모른다 —
/// 관측되지 않은 상태에 콜을 쏘지 않는다(`docs/architecture.md` §7.2).
///
/// ⚠️ **응답으로는 아무것도 알 수 없다** — `{"code":"0","msg":"SUCCESS"}`만 오고 uid별 결과가 없다.
/// 그래서 판정은 전부 목록 재조회가 한다(§7.1 3분법):
///
/// | 재조회 | 결말 |
/// |---|---|
/// | 있고 `seen=0` | 반영됨 → `ok:true` |
/// | 있고 `seen=1` | 반영 안 됨 → 실패(`Err`) |
/// | 창 안에 없거나 `seen` 해석 불가 | **모르는 것** → `ok:false` + "실행은 됐으나 확인 못 함" |
///
/// 이미 미읽음이면 **실행하지 않고** `already:true`로 끝낸다(`attendance::punch_and_verify`와 같은 규약).
pub async fn mark_unread_and_verify(c: &GwClient, muid: &str) -> Result<Value> {
    if muid.trim().is_empty() {
        return Err(InvalidInput("muid가 비어 있다".into()).into());
    }
    let seq = mbox_seq(c, INBOX).await?;

    // 사전 확인 — 창 안에 있는지, 이미 미읽음인지. 여기서 끝나면 콜을 쏘지 않는다.
    let before = list_mails(c, seq, 1, UNREAD_WINDOW).await?;
    match seen_flag(&before, muid) {
        Some(SeenState::Unseen) => {
            return Ok(json!({
                "ok": true, "already": true, "muid": muid, "verifiedByReadback": true,
                "note": "이미 읽지 않음 상태 — 서버에 아무것도 보내지 않았다"
            }));
        }
        // 대상이 있으면 상태 미상이어도 실행한다 — 이미 미읽음으로 오판해 건너뛰지 않는다.
        Some(SeenState::Seen | SeenState::Unknown) => {}
        // 창 밖일 수도, 정말 없을 수도 있다. 어느 쪽인지 모르는 채로 쏘지 않는다.
        None => {
            return Err(InvalidInput(format!(
                "받은메일함 최근 {UNREAD_WINDOW}건에서 muid={muid} 를 찾지 못했다 — 그보다 오래된 메일이거나 존재하지 않는다. list_mail_inbox로 확인할 것."
            ))
            .into());
        }
    }

    c.call(
        "/mail/mail002A15",
        &json!({ "mbox": INBOX, "uids": muid, "type": "unseen" }),
    )
    .await?;

    let after = list_mails(c, seq, 1, UNREAD_WINDOW).await?;
    match seen_flag(&after, muid) {
        Some(SeenState::Unseen) => Ok(json!({
            "ok": true, "already": false, "muid": muid, "verifiedByReadback": true
        })),
        Some(SeenState::Seen) => bail!("읽지 않음 처리가 반영되지 않았다(muid={muid}) — 재조회 결과가 여전히 읽음이다"),
        Some(SeenState::Unknown) => Ok(json!({
            "ok": false, "muid": muid, "verifiedByReadback": false,
            "note": "실행은 됐으나 재조회의 seen 값을 해석할 수 없어 반영을 확인하지 못했다"
        })),
        None => Ok(json!({
            "ok": false, "muid": muid, "verifiedByReadback": false,
            "note": "실행은 됐으나 재조회에서 대상을 찾지 못해 반영을 확인하지 못했다"
        })),
    }
}

/// 작성폼 초기화 — `mail014A01`. 발송(A04)·임시저장(A14)에 필요한 `sessionKey`/`fileDir`을
/// 확보하기 위해 선행 호출. 응답에는 재저장 때 되돌려줘야 하는 `mailkey`도 들어 있다.
/// ⚠️ `mailKind`는 **`"plain"`**(= 브라우저의 `메일쓰기`)이다.
/// 예전에는 `"me"`를 보냈는데, 실측 결과 `"me"`는 **`내게쓰기`** 모드였다 — 받는사람 칸에 본인을
/// 미리 채워주는 화면이다(`analyze/15` §부수 관측). 실제 배달은 폼의 `to`가 정하므로 `"me"`로도
/// 동작해 왔지만, 참조(cc/bcc)를 싣기 시작하면 서버가 모드별로 폼을 다르게 처리할 여지가 있어
/// 브라우저와 같은 경로로 맞췄다.
pub async fn compose_init(c: &GwClient) -> Result<Value> {
    c.call(
        "/mail/mail014A01",
        &json!({ "mainApiCode": "mail014A01", "mailKind": "plain" }),
    )
    .await
}

/// 문서 래퍼 태그(`<html>`/`<head>`/`<body>` 여닫이) **만** 걷어낸다. 나머지 태그·텍스트·속성은
/// 손대지 않는다. 다른 HTML의 **안쪽에 끼워 넣을** 조각을 만들 때 쓴다.
fn strip_document_wrapper(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                // 닫히지 않은 채 다시 '<'가 나오면 앞의 것은 태그가 아니었다 — 원문대로 흘린다.
                if in_tag {
                    out.push_str(&tag);
                }
                in_tag = true;
                tag.clear();
                tag.push(ch);
            }
            '>' if in_tag => {
                in_tag = false;
                tag.push(ch);
                let name = tag
                    .trim_start_matches('<')
                    .trim_start_matches('/')
                    .trim_end_matches('>')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(name.as_str(), "html" | "head" | "body") {
                    out.push_str(&tag);
                }
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    if in_tag {
        out.push_str(&tag);
    }
    out
}

/// A01(`compose_init`) 응답에서 **아마란스에 등록된 서명 HTML**을 꺼낸다. 웹 편집기가 작성 화면을
/// 열 때 본문에 끼워 넣는 바로 그 값이라, 이어 붙이면 웹에서 보낸 것과 같은 형상이 된다.
///
/// 값은 `resultData.signature`에 **삽입용 래퍼(`<div class="dze_signature" …>`)까지 완성된 상태**로
/// 온다 — 현재 선택된 서명 하나다(`configSignature.signatureIdx`가 가리키는 것). 등록 목록 전체는
/// `configSignature.signatureList[]`(최대 3개)에 따로 있으나, 웹이 기본으로 넣는 것은 이 한 값이므로
/// 그것만 쓴다. 서명을 등록하지 않은 계정은 이 필드가 빈 문자열이다.
///
/// ⛔ **`<html>`·`<head>`·`<body>`를 걷어내야 한다.** 서버가 주는 값은 래퍼 div **안에** 완전한 html
/// 문서가 중첩된 모양(`<div …><html><head></head><body><table>…</body></html></div>`)이다. 그대로
/// 이어 붙이면 메일 하나에 `<html>`이 둘이 되어 클라이언트에 따라 렌더가 깨진다. 실제 웹 발송분에는
/// 그 세 겹이 없다 — 편집기가 벗겨낸다.
/// (실측 대조 2026-09-01: 걷어낸 값과 웹 발송분(muid 14086132)의 서명 블록이 공백 정규화 후
/// 앞 4,103자 완전 일치. 차이는 편집기가 덧붙인 뒤쪽 빈 줄 `<p><br></p>`뿐이었다.)
fn signature_html(init: &Value) -> String {
    let raw = init.get("signature").and_then(|v| v.as_str()).unwrap_or("");
    strip_document_wrapper(raw).trim().to_string()
}

/// 본문 끝에 서명을 이어 붙인다. `on`이 false거나 등록된 서명이 없으면 본문 그대로.
///
/// 서명 테이블 자체가 `margin:60px 0px 0px`를 갖고 있어 본문과의 간격은 서명이 스스로 만든다 —
/// 여기서 빈 줄을 더 끼우지 않는다.
///
/// 이미 서명이 들어 있는 본문에는 붙이지 않는다. 기본값이 `on=true`라 호출자가 서명이 붙는 줄
/// 모르고 본문에 직접 서명을 써 넣을 수 있는데, 그때 두 개가 되면 사람 눈에만 보이고 도구는
/// 성공을 보고한다. 판정 기준은 더존 편집기가 서명에 늘 붙이는 마커 클래스다.
///
/// 반환의 두 번째 값 = **실제로 붙였는지**. 요청값(`on`)을 그대로 돌려주지 않는 이유는, 서명을
/// 등록하지 않은 계정에서 이 함수가 조용히 아무것도 안 하기 때문이다 — 호출자가 `on:true`만 보고
/// "서명이 들어갔다"고 보고하면 거짓이 된다. 도구 응답에 이 값을 실어 사람이 구분할 수 있게 한다.
fn with_signature(html: &str, init: &Value, on: bool) -> (String, bool) {
    if !on || html.contains("dze_signature") {
        return (html.to_string(), false);
    }
    let sig = signature_html(init);
    if sig.is_empty() {
        return (html.to_string(), false);
    }
    (format!("{html}{sig}"), true)
}

/// 발송(`mail014A04`)과 임시저장(`mail014A14`)이 공유하는 작성 폼.
///
/// 담기는 값은 **`mail014A01` 응답 스냅샷 + 호출 인자**뿐이다. 크레덴셜 파생값은 담지 않는다
/// (`fields`가 조립할 때마다 새로 읽는다 — 이유는 그쪽 주석).
struct ComposeForm {
    email: String,
    filedir: String,
    big_file_day: String,
    big_file_cnt: String,
    uid_auth_list: String,
    session_key: String,
    external_send_limit: String,
    inside_domain: String,
    neobiz_addr: String,
    neobiz_inted_addr: String,
    neobiz_org: String,
    /// 폼의 `to`. **여러 명이면 콤마로 이어 붙인 하나의 문자열**이다(실측 `analyze/15`).
    /// 표시형(`이름 <주소>`)과 순수 주소를 섞어도 된다.
    to: String,
    /// 폼의 `cc`(참조). 형식은 `to`와 같다. 비우면 참조 없음.
    cc: String,
    /// 폼의 `bcc`(숨은참조). 형식은 `to`와 같다. 비우면 숨은참조 없음.
    bcc: String,
    subject: String,
    html: String,
    /// 폼의 `fwFile`. 신규 발송·임시저장은 항상 빈 값이고, **초안 발송이 첨부를 승계할 때만**
    /// 서버에 이미 있는 첨부의 `originalFileName`을 콤마로 이어 싣는다(실측).
    fw_file: String,
    /// 폼의 `muid`. 신규 발송·임시저장은 `"0"`, **초안 발송만** 그 초안의 muid를 싣는다.
    muid: String,
    /// 폼의 `mail_kind`. 신규는 `"plain"`(브라우저 `메일쓰기`와 동일), 초안 발송은 `"draft"`.
    mail_kind: String,
    /// 폼의 `mimeHeader`. 신규는 빈 값, 초안 발송은 그 초안의 mime 헤더 JSON.
    mime_header: String,
}

impl ComposeForm {
    /// `init`은 `compose_init`(mail014A01) 응답. 여기서 오는 값들 — sessionKey/filedir/email/
    /// bigFileDay/externalSendLimit/insideDomainArray/groupMailOption — 은 크레덴셜이 아니라
    /// **작성폼 세션**에서 온 것이라 401 재취득으로 바뀌지 않는다. 다만 전송 직전에 토큰이 만료되면
    /// 이 스냅샷도 함께 낡을 수 있는데, 폼 조립은 동기 클로저라 거기서 A01을 다시 부를 수 없다.
    /// 그 경우 재시도가 실패하고 로그인 안내로 끝난다(다음 호출의 `compose_init`이 새 세션을 받으므로
    /// 사용자가 다시 시도하면 정상 동작한다).
    fn new(
        init: &Value,
        to: &str,
        subject: &str,
        html: &str,
        uid_auth_list: String,
        big_file_cnt: String,
    ) -> Self {
        let g = |k: &str| init.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let gmo = init.get("groupMailOption");
        let gm = |k: &str| {
            gmo.and_then(|o| o.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        Self {
            email: g("email"), // 발신 이메일(순수 형식) — from/email 필드에 사용
            filedir: g("filedir"),
            big_file_day: g("bigFileDay"),
            big_file_cnt,
            uid_auth_list,
            session_key: g("sessionKey"),
            external_send_limit: g("externalSendLimit"),
            inside_domain: match init.get("insideDomainArray") {
                Some(v) if !v.is_null() => v.to_string(),
                _ => "[]".to_string(),
            },
            neobiz_addr: gm("groupMailAddr"),
            neobiz_inted_addr: gm("groupMailIntedAddr"),
            neobiz_org: gm("groupMailOrg"),
            to: to.to_string(),
            // 참조는 기본이 "없음"이다 — 실을 때만 `with_carbon_copy`로 덮어쓴다.
            cc: String::new(),
            bcc: String::new(),
            subject: subject.to_string(),
            html: html.to_string(),
            // 아래 4개의 기본값 = 신규 발송/임시저장의 실측값. 초안 발송만 덮어쓴다.
            fw_file: String::new(),
            muid: "0".to_string(),
            mail_kind: "plain".to_string(),
            mime_header: String::new(),
        }
    }

    /// 참조(`cc`)·숨은참조(`bcc`)를 싣는다. 여러 명이면 **콤마로 이어 붙인 하나의 문자열**이다.
    ///
    /// 값을 해석하지 않고 그대로 싣는다 — 브라우저도 칩 목록을 콤마로 이어 붙일 뿐이다(실측).
    /// 그래서 표시형(`이름 <주소>`)과 순수 주소가 섞여 있어도 안전하다.
    fn with_carbon_copy(mut self, cc: &str, bcc: &str) -> Self {
        self.cc = cc.trim().to_string();
        self.bcc = bcc.trim().to_string();
        self
    }

    /// 초안 발송 좌표를 싣는다 — 브라우저가 임시보관 메일을 보낼 때 신규 발송과 달라지는 값이다.
    /// `fileDir`/`sessionKey`는 draft 모드 `mail014A01` 응답을 `new`에 넘기면 자동으로 반영되므로
    /// 여기서 따로 다루지 않는다(그래서 실측 차이 5개 중 3개만 여기 있다).
    fn with_draft(mut self, muid: &str, mime_header: String) -> Self {
        self.muid = muid.to_string();
        self.mail_kind = "draft".to_string();
        self.mime_header = mime_header;
        self
    }

    /// 승계한 첨부의 파일명 목록(`fwFile`)을 싣는다. 첨부를 승계할 때만 부른다.
    fn with_fw_file(mut self, fw_file: String) -> Self {
        self.fw_file = fw_file;
        self
    }

    /// 실측 FormData 전 필드.
    ///
    /// ⚠️ **크레덴셜에서 파생되는 값(`authToken`)은 호출할 때마다 여기서 새로 읽는다.**
    /// 밖에서 스냅샷하면 401 재취득 후 폼을 재조립해도 옛 토큰이 그대로 실려 재시도가 형식만 남는다.
    fn fields(&self, c: &GwClient) -> Vec<(&'static str, String)> {
        // body용 authToken: 헤더용(groupSeq|empSeq|secret) 앞에 loginId를 덧붙인 형식.
        let body_auth = format!("{}|{}", c.email_addr(), c.auth_token());
        vec![
            ("from", self.email.clone()),
            ("fromName", c.emp_name()),
            ("to", self.to.clone()),
            ("cc", self.cc.clone()),
            ("bcc", self.bcc.clone()),
            ("htmlContents", self.html.clone()),
            ("email", self.email.clone()),
            ("fileDir", self.filedir.clone()),
            ("bigFile", String::new()),
            ("bigFileDay", self.big_file_day.clone()),
            ("bigFileCnt", self.big_file_cnt.clone()),
            ("bigFilePeriod", String::new()),
            ("mail_kind", self.mail_kind.clone()),
            ("uidAuthList", self.uid_auth_list.clone()),
            ("fwFile", self.fw_file.clone()),
            ("urlList", String::new()),
            ("fileNameList", String::new()),
            ("receipt_notific", String::new()),
            ("securitymailuse", String::new()),
            ("securitymailpass_enc_web", String::new()),
            ("immediately", "false".into()),
            ("toBeDeleted", "false".into()),
            ("expirationDate", "Invalid date".into()),
            ("importantmailuse", String::new()),
            ("eachTrans", String::new()),
            ("neobizaddr", self.neobiz_addr.clone()),
            ("neobizIntedAddr", self.neobiz_inted_addr.clone()),
            ("neobizOrg", self.neobiz_org.clone()),
            ("muid", self.muid.clone()),
            ("domainSeq", String::new()),
            ("mimeHeader", self.mime_header.clone()),
            ("sessionKey", self.session_key.clone()),
            ("externalSendLimit", self.external_send_limit.clone()),
            ("insideDomainArray", self.inside_domain.clone()),
            ("aiResultJSON", String::new()),
            ("subject", self.subject.clone()),
            ("authToken", body_auth),
        ]
    }

    fn build(&self, c: &GwClient) -> reqwest::multipart::Form {
        self.fields(c)
            .into_iter()
            .fold(reqwest::multipart::Form::new(), |f, (k, v)| f.text(k, v))
    }
}

/// 첨부를 `mail014A06`에 올려 폼의 `uidAuthList`/`bigFileCnt` 짝을 만든다. 없으면 빈 값.
async fn attachment_fields(c: &GwClient, attachments: &[String]) -> Result<(String, String)> {
    if attachments.is_empty() {
        return Ok((String::new(), "0".to_string()));
    }
    let uploaded = upload_files(c, attachments).await?;
    Ok((uploaded, attachments.len().to_string()))
}

/// 메일 발송 — `mail014A01`(작성폼 초기화) → `mail014A04`(multipart) 2단계.
/// A01 응답에서 sessionKey/filedir/email/externalSendLimit/bigFileDay/groupMailOption을 동적 취득.
/// ⚠️ 발송은 헤더 서명 + **body 내 authToken**(형식 `loginId|groupSeq|empSeq|secret`)을 함께 요구.
/// `to`/`cc`/`bcc`는 표시형("이름 <email>") 또는 이메일이고, **여러 명이면 콤마로 잇는다**
/// (실측 `analyze/15`). 실측 FormData 전 필드를 그대로 재현.
/// `signature`가 true면 A01이 준 등록 서명을 본문 끝에 붙인다(웹 발송과 같은 형상).
pub async fn send_mail(
    c: &GwClient,
    to: &str,
    cc: &str,
    bcc: &str,
    subject: &str,
    html: &str,
    attachments: &[String],
    signature: bool,
) -> Result<Value> {
    let init = compose_init(c).await?;
    // 첨부: 로컬 파일을 mail014A06(multipart `file[]`)로 업로드 → uidAuthList 조립.
    let (uid_auth_list, big_file_cnt) = attachment_fields(c, attachments).await?;
    let (html, signature_attached) = with_signature(html, &init, signature);
    let cf = ComposeForm::new(&init, to, subject, &html, uid_auth_list, big_file_cnt)
        .with_carbon_copy(cc, bcc);

    // 폼을 "만드는 방법"으로 넘긴다 — 401 재취득 재시도 때 클라이언트가 재조립해야 하기 때문
    // (`multipart::Form`은 Clone이 아니고 전송이 소비한다). 호출당 최대 2회 평가된다.
    let v = c.call_multipart("/mail/mail014A04", || cf.build(c)).await?;
    // 발송 응답은 표준 봉투로 감싸짐: {"resultCode":0,"resultData":{"result":true,"muid":..,"resultMessage":"SUCCESS"}}
    let rd = v.get("resultData").unwrap_or(&v);
    let ok = rd.get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    if !ok {
        bail!("메일 발송 실패: {v}");
    }
    // 서버 응답에 우리 관측값 하나를 얹는다(응답이 객체가 아닌 형태로 오면 그냥 흘린다).
    let mut out = rd.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert("signature_attached".into(), json!(signature_attached));
    }
    Ok(out)
}

/// 임시저장(A14)이 발송 폼 위에 덧붙이는 전용 필드. 값은 **신규 저장** 기준이다
/// (재저장은 `autoMUID`/`beforeMUID`/`mailKey`에 직전 저장분 좌표가 들어가는데, 이 도구는 신규만 한다).
///
/// ⚠️ `isFirst`는 첫 저장이 **"0"**이다 — 프론트가 `isFirst === true ? "0" : "1"`로 보낸다.
/// 뒤집힌 것처럼 보이지만 서버가 그 값을 기대하므로 그대로 재현한다.
const DRAFT_FIELDS: &[(&str, &str)] = &[
    ("autoMUID", ""),
    // compose_init이 연 폼의 종류(그래서 `mailKind`와 같이 움직인다).
    // ⚠️ 브라우저는 여기에 **문자열 `"undefined"`**를 싣는다(실측 `analyze/15` — 프론트의
    // 미초기화 변수가 그대로 새어나온 값이다). 남의 버그를 재현하는 대신 실제로 연 모드를 적는다.
    // 예전 값 `"me"`로 임시저장이 성공해 온 것은 확인돼 있으나, 서버가 이 필드를 보기는 하는지
    // 자체가 미확인이다 — `"plain"`은 회귀 점검의 save_mail_draft로 검증한다.
    ("beforeMailType", "plain"),
    ("beforeMUID", ""),
    ("mailKey", ""),
    ("isFirst", "0"),
    ("draftType", "true"),      // 수동 임시저장(파일 첨부 여부와 무관하게 true)
    ("autoDraftType", "false"), // 에디터 자동저장(A13)이 아님
];

/// 저장 직후 read-back이 훑는 임시보관함 통수. 목록은 최신순(`rfc822date desc`)이라 방금 저장한
/// 건이 맨 앞에 오므로 넉넉하다 — 크게 잡을 이유가 없다(조회 비용만 는다).
const DRAFT_READBACK_PAGE: i64 = 20;

/// 메일 임시저장 — `mail014A01`(작성폼 초기화) → `mail014A14`(multipart) 2단계.
/// **발송하지 않는다** — 임시보관함(DRAFTS)에 저장만 한다.
/// 폼은 발송(A04)과 동일하고 `DRAFT_FIELDS` 7개만 더 붙는다. 첨부 경로도 발송과 같다.
/// 반환: `draft_muid`(= 응답 `resultData.autoMUID`, 후속 조회·삭제 키),
/// `mail_key`(= A01의 `mailkey`, 재저장 때 `mailKey`로 되돌려줄 값),
/// `verified_by_readback`(임시보관함 재조회로 그 muid를 실제로 찾았는지 — 프로젝트 규약 §7).
/// `signature`가 true면 A01이 준 등록 서명을 본문 끝에 붙여 **저장**한다 — 초안 본문에 들어가므로
/// `send_mail_from_draft`가 본문째로 승계한다(그쪽에서 다시 붙이지 않는다).
pub async fn save_mail_draft(
    c: &GwClient,
    to: &str,
    cc: &str,
    bcc: &str,
    subject: &str,
    html: &str,
    attachments: &[String],
    signature: bool,
) -> Result<Value> {
    let init = compose_init(c).await?;
    let (uid_auth_list, big_file_cnt) = attachment_fields(c, attachments).await?;
    // 프론트는 제목이 비면 "(제목없음)"으로 채워 저장한다.
    let subject = if subject.trim().is_empty() {
        "(제목없음)"
    } else {
        subject
    };
    let (html, signature_attached) = with_signature(html, &init, signature);
    let cf = ComposeForm::new(&init, to, subject, &html, uid_auth_list, big_file_cnt)
        .with_carbon_copy(cc, bcc);
    let form = || {
        DRAFT_FIELDS
            .iter()
            .fold(cf.build(c), |f, (k, v)| f.text(*k, *v))
    };

    let v = c.call_multipart("/mail/mail014A14", form).await?;
    // 999 = "로그인 사용자가 변경되어 임시저장을 할 수 없습니다"(프론트 문구). 전송 실패와 구분한다.
    if v.get("resultCode").and_then(|x| x.as_i64()) == Some(999) {
        bail!("메일 임시저장 실패: 로그인 사용자가 변경돼 임시저장할 수 없다(resultCode 999) — 브라우저에서 다시 로그인한 뒤 재시도할 것");
    }
    let rd = v.get("resultData").unwrap_or(&v);
    let draft_muid = json_str(rd.get("autoMUID"));
    if draft_muid.is_empty() {
        bail!("메일 임시저장 실패(autoMUID 없음): {v}");
    }
    // read-back — 성공 응답을 반영으로 단정하지 않는다(프로젝트 규약). 임시보관함을 재조회해
    // **그 muid가 목록에 실제로 있는지** 본다.
    // ⚠️ 통수(+1) 비교가 아니라 muid 대조인 이유: 저장 전후 사이에 메일이 들어오거나 다른 세션이
    // 초안을 지우면 통수는 조용히 오탐이 난다. muid는 우리가 만든 그 한 건만 가리킨다.
    // 조회 실패(권한·네트워크)는 "확인 못 함"이지 "저장 실패"가 아니므로 에러로 올리지 않는다.
    // 그 경우 `verified_by_readback: false`로 보고한다 — **저장이 안 됐다는 뜻이 아니라
    // 확인하지 못했다는 뜻**이고, 목록에 없어서 false인 경우와 응답에서 구분되지 않는다.
    // 어느 쪽이든 사람이 임시보관함을 눈으로 확인해야 한다.
    let verified = list_drafts(c, 1, DRAFT_READBACK_PAGE)
        .await
        .map(|list| has_muid(&list, &draft_muid))
        .unwrap_or(false);

    Ok(json!({
        "draft_muid": draft_muid,
        "mail_key": json_str(init.get("mailkey")),
        "verified_by_readback": verified,
        "signature_attached": signature_attached
    }))
}

/// 초안을 **여는** 작성폼 초기화 — 신규 작성과 **같은 `mail014A01`**에 draft 좌표를 실어 부른다.
///
/// 이 한 콜이 초안 발송에 필요한 것을 전부 준다(2026-08-06 브라우저 캡처):
/// `mailkey`(발송 뒤 원본 삭제용) · 새 `sessionKey`/`filedir` · `mailInfo.mime.header`(폼의 `mimeHeader`) ·
/// `mailInfo.mime.body.html`(본문) · `mailInfo.decodeMime.subject`(제목) ·
/// `mailInfo.decodeMime.to`(수신자) · `mailInfo.mime.fileList`(첨부).
/// 덕분에 MCP가 호출마다 독립이어도 `mailkey`를 인자로 들고 다닐 필요가 없다.
///
/// ⚠️ **경로를 줄여 적지 말 것.** 쓸모 있는 값은 전부 `mailInfo` 아래에 있다 —
/// 본문·첨부·헤더는 `mailInfo.mime` 밑, 디코드된 제목·수신자는 `mailInfo.decodeMime` 밑이다.
/// **최상위에는 `decodeMime`이 없다**(있다고 보고 `/decodeMime/subject`를 읽던 코드가 라이브에서
/// 제목을 못 찾아 발송이 막혔다). `save_mail_draft`가 만든 초안과 브라우저가 만든 초안 **양쪽을
/// 실측해 같은 모양임을 확인**했다.
///
/// ⚠️ 최상위 `paramTo`는 **우리가 보낸 요청 파라미터의 메아리**다. `draft_init_body`가 `mailTo`를
/// 늘 `"(Unknown)"`으로 보내므로 이 자리도 늘 `"(Unknown)"`이다 — 수신자 출처로 쓸 수 없다.
///
/// `mailTo`/`viewFlag`/`fromFlag`/`readType`/`domainSeq`는 브라우저가 보내는 값을 그대로 재현한 것이다
/// — 무엇이 필수인지는 실측하지 않았으므로 빼지 말 것.
pub async fn compose_init_draft(c: &GwClient, draft_muid: &str) -> Result<Value> {
    c.call("/mail/mail014A01", &draft_init_body(draft_muid)).await
}

/// `compose_init_draft`가 보내는 body. 실측 캡처 그대로다.
fn draft_init_body(draft_muid: &str) -> Value {
    json!({
        "mailKind": "draft",
        "mailTo": "(Unknown)",
        "uid": draft_muid,
        "mbox": DRAFTS,
        "viewFlag": "noRead",
        "fromFlag": true,
        "readType": "",
        "domainSeq": ""
    })
}

/// 초안 발송 뒤 임시보관함 원본을 지운다 — `mail002A07`(JSON).
///
/// 프론트가 발송 성공 직후 반드시 치는 콜이다. **서버는 자동으로 정리하지 않는다**(실측).
/// 빼먹으면 보낸 메일이 임시보관함에 남아 다음에 또 보내는 사고가 난다.
async fn delete_draft_original(c: &GwClient, mail_key: &str, draft_muid: &str) -> Result<Value> {
    let before: i64 = draft_muid
        .parse()
        .map_err(|_| anyhow!("draft muid가 숫자가 아니다: {draft_muid}"))?;
    c.call(
        "/mail/mail002A07",
        &json!({ "mailKey": mail_key, "beforeMUID": before }),
    )
    .await
}

/// 임시보관 메일 발송 — `mail014A01`(draft 모드) → (첨부 있으면 `mail014A08`) → `mail014A04`
/// → `mail002A07`.
///
/// **전용 발송 API는 없다.** 신규 발송과 같은 `mail014A04`를 쓰고, 실측 차이는 값 5개뿐이다
/// (`muid`=초안 muid / `mail_kind`="draft" / `mimeHeader`=초안 mime 헤더 / `fileDir`·`sessionKey`=draft 모드
/// A01 응답 / 첨부를 승계하면 `uidAuthList`·`bigFileCnt`·`fwFile`).
/// 나머지 필드는 발송 폼 그대로다 — 브라우저가 보내는 것은 전부 보낸다.
/// (브라우저는 `bigFilePeriod`에 기간 문자열을 싣지만 우리는 빈 값이다 — `bigFileCnt`가 0이라
/// 쓸 데가 없고 기존 `send_mail`이 빈 값으로 성공해 왔다.)
///
/// 안전장치:
/// 1. **muid 실재 확인** — 발송 전에 임시보관함을 조회해 그 muid가 실제로 있는지 본다.
///    없으면 발송하지 않는다(이미 보냈거나 지워진 초안을 다시 보내는 사고 방지).
/// 2. **판정 불가는 전부 중단** — 첨부 목록·본문·제목 중 하나라도 응답에서 읽어내지 못하면
///    보내지 않는다. 조용히 빈 값으로 나가면 첨부가 빠지거나 **내용이 텅 빈 메일**이 수신자에게
///    가는데, 어느 쪽도 회수할 수 없다.
/// 3. **첨부는 승계하되 미실측 경로는 거부** — `mail014A08`로 발송용 `fileId`를 받아 승계한다.
///    파일명에 콤마가 있거나 동명 파일이 둘 이상이거나 대용량 첨부(`bigFile`)면 중단한다
///    (자세한 이유는 `inherit_draft_attachments`).
/// 4. **참조(cc/bcc)는 승계한다** — 초안에 저장된 값을 되읽어 그대로 싣는다(`draft_carbon_copy`).
///    읽어내지 못하면 "참조 없음"이 아니라 중단이다.
/// 5. **발송 성공 ≠ 정리 성공** — 원본 삭제가 실패해도 발송은 성공으로 보고하되
///    `draft_deleted:false`와 안내를 함께 실어 중복 발송을 사람이 막을 수 있게 한다.
///
/// `to_override`가 비어 있으면 초안에 저장된 수신자(`draft_recipient`)로 보낸다.
///
/// ⛔ **여기서 서명을 붙이지 않는다.** 이 도구는 초안 본문을 **그대로** 보내는 것이 계약이고,
/// 서명은 초안을 만든 쪽(`save_mail_draft`, 또는 아마란스 웹 편집기)이 이미 본문에 넣어 두었다.
/// 여기서 한 번 더 붙이면 사람이 확인한 형상과 실제 발송물이 어긋나고 서명이 두 개가 된다.
pub async fn send_mail_from_draft(
    c: &GwClient,
    draft_muid: &str,
    to_override: &str,
) -> Result<Value> {
    let draft_muid = draft_muid.trim();
    if draft_muid.is_empty() {
        return Err(InvalidInput("draft_muid가 비어 있다".into()).into());
    }

    // ① 임시보관함 조회. 조회 자체가 실패하면 "없다"가 아니라 "확인 못 했다"이므로 발송하지 않는다
    //    — 발송은 되돌릴 수 없어서, 모르는 채로 진행하는 쪽이 더 나쁘다.
    let drafts = list_drafts(c, 1, DRAFT_READBACK_PAGE)
        .await
        .map_err(|e| anyhow!("임시보관함 조회 실패로 draft_muid를 확인하지 못해 발송을 중단한다: {e}"))?;
    // muid 실재 확인. 반환값(`DraftExists`)이 ③의 입력이라 **이 확인을 건너뛰면 컴파일되지 않는다.**
    let found = ensure_draft_exists(&drafts, draft_muid)?;

    // ② 초안 열기. 발송에 필요한 값이 전부 이 응답에서 나온다.
    let init = compose_init_draft(c, draft_muid).await?;

    // ③ **보낼 것을 확정한다.** 판정은 전부 여기(순수 함수)에 있다 — 거부 사유도 여기서 나온다.
    let plan = plan_draft_send(found, &init, draft_muid, to_override)?;

    // ④ 첨부 승계. 보낼 내용이 다 확인된 뒤에 부른다 — 어차피 거부될 발송에 콜을 더 쓰지 않는다.
    let (uid_auth_list, big_file_cnt, fw_file) = if plan.files.is_empty() {
        (String::new(), "0".to_string(), String::new())
    } else {
        inherit_draft_attachments(c, &init, draft_muid, &plan.files).await?
    };

    let cf = ComposeForm::new(
        &init,
        &plan.to,
        &plan.subject,
        &plan.html,
        uid_auth_list,
        big_file_cnt,
    )
    .with_carbon_copy(&plan.cc, &plan.bcc)
    .with_draft(draft_muid, plan.mime_header.clone())
    .with_fw_file(fw_file);
    let v = c.call_multipart("/mail/mail014A04", || cf.build(c)).await?;
    let rd = v.get("resultData").unwrap_or(&v);
    if !rd.get("result").and_then(|r| r.as_bool()).unwrap_or(false) {
        bail!("초안 발송 실패: {v}");
    }

    // ⑤ 원본 정리. 실패해도 발송은 이미 나갔으므로 에러로 올리지 않는다 — 대신 사실을 실어 보고한다.
    let mail_key = json_str(init.get("mailkey"));
    let (draft_deleted, delete_note) = match delete_draft_original(c, &mail_key, draft_muid).await {
        Ok(_) => (true, String::new()),
        Err(e) => (
            false,
            format!("메일은 발송됐으나 임시보관함 원본을 지우지 못했다({e}) — 그대로 두면 같은 메일을 또 보낼 수 있으니 임시보관함에서 직접 삭제할 것"),
        ),
    };

    Ok(json!({
        "sent": true,
        "draft_muid": draft_muid,
        "to": plan.to,
        "cc": plan.cc,
        "bcc": plan.bcc,
        "subject": plan.subject,
        "attachments": plan.files.len(),
        "draft_deleted": draft_deleted,
        "note": if draft_deleted {
            "발송 후 임시보관함 원본을 삭제했다(mail002A07). 이 삭제는 휴지통을 거치지 않는 것으로 보인다".to_string()
        } else {
            delete_note
        }
    }))
}

/// "임시보관함에 그 초안이 실재한다"는 증거. `plan_draft_send`가 이것을 **인자로 요구한다** —
/// 발송은 되돌릴 수 없어서, 실재 확인이 코드에서 조용히 사라지는 것을 타입으로 막는다.
/// (보증 범위: `ensure_draft_exists` **호출을 지우면 컴파일되지 않는다.** 같은 모듈 안에서 이 값을
/// 손으로 만들어 우회하는 것까지는 막지 못한다 — 그건 검증을 건너뛰겠다는 명시적 행위다.)
struct DraftExists;

/// 임시보관함 목록에 그 muid가 실제로 있는지. 없으면 발송하지 않는다
/// (이미 보냈거나 지워진 초안을 다시 보내는 사고 방지).
fn ensure_draft_exists(drafts: &Value, draft_muid: &str) -> Result<DraftExists> {
    if !has_muid(drafts, draft_muid) {
        return Err(InvalidInput(format!(
            "임시보관함 최근 {DRAFT_READBACK_PAGE}건에 draft_muid={draft_muid}가 없다 — 이미 발송했거나 삭제된 초안일 수 있다. list_mail_drafts로 확인할 것"
        ))
        .into());
    }
    Ok(DraftExists)
}

/// 초안 발송에 실을 값. `plan_draft_send`가 낸다.
struct DraftSendPlan {
    to: String,
    /// 초안에서 승계한 참조. 없으면 빈 문자열.
    cc: String,
    /// 초안에서 승계한 숨은참조. 없으면 빈 문자열.
    bcc: String,
    subject: String,
    html: String,
    mime_header: String,
    /// 초안이 들고 있는 첨부(`mailInfo.mime.fileList`) 원본. 빈 배열이면 첨부 없음.
    files: Vec<Value>,
}

/// **초안 발송의 판정 전부**를 한자리에 모은 순수 함수 — 무엇을 보낼지, 아니면 왜 보내지 않을지.
///
/// I/O(A01·A08·A04)와 갈라둔 이유는 회귀 때문이다. 이 판단들은 전부 "틀리면 잘못된 메일이
/// 나가고 회수할 수 없는" 종류인데, 호출부에 인라인으로 두면 **가드를 지워도 테스트가 통과한다.**
/// 여기 있으면 캡처한 A01 응답 JSON만으로 전부 검증된다.
///
/// 막는 것(전부 "조용히 잘못 나가는" 경우다):
/// - **첨부 목록을 못 읽음** → 첨부 없음이 아니라 판정 불가
/// - **본문·제목이 빔** → 내용이 텅 빈 메일 발송
/// - **참조(cc/bcc)를 읽지 못함** → 참조 수신자 누락(`draft_carbon_copy`)
/// - **수신자가 없거나 주소로 보이지 않음** → 엉뚱한 곳으로 나가거나 실패
fn plan_draft_send(
    _found: DraftExists,
    init: &Value,
    draft_muid: &str,
    to_override: &str,
) -> Result<DraftSendPlan> {
    let mail_info = init.get("mailInfo").cloned().unwrap_or(Value::Null);

    // 첨부 목록의 정본은 `mailInfo.mime.fileList` **하나**다(API 초안·브라우저 초안 양쪽 실측:
    // 첨부 없으면 `[]`). 대안 자리를 덧대지 않는다 — 어느 쪽이 정본인지 흐려지면 다음 사람이
    // 엉뚱한 자리를 승격시킨다.
    // **키가 아예 없으면 "첨부 없음"이 아니라 "판정 불가"로 본다** — 응답 모양이 예상과 다른 채로
    // 진행하면 첨부를 조용히 빠뜨린 메일이 나가는데, 그건 되돌릴 수 없다.
    let Some(files) = mail_info.pointer("/mime/fileList").and_then(|v| v.as_array()) else {
        bail!(
            "초안(draft_muid={draft_muid})의 첨부 목록(mailInfo.mime.fileList)을 응답에서 찾지 못해 \
             첨부 유무를 판정할 수 없다 — 첨부를 빠뜨린 채 보낼 위험이 있어 발송하지 않는다"
        );
    };

    // 제목은 서버가 디코드해준 값(`mailInfo.decodeMime.subject`)을 쓴다 — mime 헤더 쪽 subject는
    // `=?UTF-8?B?...?=` 인코딩 상태라 그대로 쓰면 안 된다.
    // ⚠️ **`mailInfo` 아래다.** 최상위 `decodeMime`은 존재하지 않는다(실측) — 거기서 읽던 코드가
    // 라이브에서 제목을 못 찾아 발송이 막혔다.
    let subject = json_str(init.pointer("/mailInfo/decodeMime/subject"));
    // 본문 정본도 하나다 — `mailInfo.mime.body.html`(양쪽 초안 실측).
    let html = json_str(mail_info.pointer("/mime/body/html"));
    // ⚠️ **본문·제목도 첨부와 같은 기준이다 — 판정 불가는 중단이다.**
    // 두 경로 어디에도 본문이 없으면 `html`이 빈 문자열인 채로 A04가 나가고, 내용이 텅 빈 메일이
    // 수신자에게 도착한다(회수 불가). 재저장(2회차 이상) 초안의 응답 모양은 실측하지 않았으므로
    // "빈 값이 곧 빈 본문"이라고 단정할 수 없다.
    if html.trim().is_empty() {
        bail!(
            "초안(draft_muid={draft_muid})의 본문(mailInfo.mime.body.html)이 비어 있거나 응답에서 \
             찾지 못했다 — 내용이 텅 빈 메일이 나가는 것을 막기 위해 발송하지 않는다. \
             임시보관함에서 그 초안을 확인할 것"
        );
    }
    if subject.trim().is_empty() {
        bail!(
            "초안(draft_muid={draft_muid})의 제목(mailInfo.decodeMime.subject)이 비어 있거나 응답에서 \
             찾지 못했다 — 제목 없는 메일이 나가는 것을 막기 위해 발송하지 않는다. \
             임시보관함에서 그 초안을 확인할 것"
        );
    }

    // 폼의 `mimeHeader`는 초안의 헤더 객체를 JSON 문자열로 직렬화한 것이다(브라우저와 동일).
    let header = mail_info.pointer("/mime/header").cloned().unwrap_or(Value::Null);
    let mime_header = if header.is_null() {
        String::new()
    } else {
        header.to_string()
    };

    // 참조(cc/bcc)는 초안에서 승계한다. 읽어내지 못하면 "참조 없음"이 아니라 중단이다.
    let (cc, bcc) = draft_carbon_copy(init, draft_muid)?;

    // 수신자: 인자 우선, 없으면 초안에 저장된 값.
    let to = if to_override.trim().is_empty() {
        draft_recipient(init, draft_muid)?
    } else {
        to_override.trim().to_string()
    };
    if to.trim().is_empty() {
        return Err(InvalidInput(format!(
            "초안(draft_muid={draft_muid})에 수신자가 없다 — to 인자로 수신자를 지정해 발송할 것"
        ))
        .into());
    }

    Ok(DraftSendPlan {
        to,
        cc,
        bcc,
        subject,
        html,
        mime_header,
        files: files.clone(),
    })
}

/// 헤더 값(주소·표시명)의 **HTML 엔티티만** 되돌린다.
///
/// ⛔ **`html_to_text`를 주소에 쓰지 말 것.** 그쪽은 ① 태그 제거 → ② 엔티티 디코드 순서라,
/// 입력이 이미 언이스케이프된 `홍길동 <a@b.c>`면 `<a@b.c>`를 태그로 보고 **통째로 버린다**
/// (남는 값 `"홍길동 "`은 공백이 있어 빈 값 검사도 통과한다). 로컬파트가 `p`/`br`/`div` 같은
/// 블록태그로 시작하면(`<p.kim@x.co>`) 개행 하나로 치환되기까지 한다.
/// 주소에 필요한 것은 엔티티 언이스케이프뿐이다.
fn unescape_entities(s: &str) -> String {
    // `&amp;`를 마지막에 푸는 순서는 `html_to_text`와 같다 — `&amp;lt;`가 `<`로 접히지 않게.
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// 초안에 저장된 수신자를 폼에 실을 형태로 꺼낸다. 없으면 빈 문자열(호출부가 거절한다).
///
/// **정본은 `mailInfo.decodeMime.to` 하나다**(API 초안·브라우저 초안 양쪽 실측). 수신자가 없는
/// 초안에는 이 키가 아예 없고, 있으면 서버가 디코드해준 표시형이 들어 있다.
///
/// ⚠️ **`mime.header.to`를 쓰지 않는다.** 같은 초안에서 그 값은 표시명이 `=?UTF-8?B?...?=`로
/// MIME 인코딩된 데다 `<`/`>`가 `&lt;`/`&gt;`로 HTML 이스케이프돼 온다 — 그대로 실으면 표시명이
/// 깨지고 주소 파싱도 깨질 수 있다. 브라우저가 A04에 싣는 값은 디코드된 표시형이다.
///
/// ⚠️ **최상위 `paramTo`를 대안으로 두지 않는다.** 그 자리는 우리가 보낸 `mailTo` 요청 파라미터를
/// 되돌려줄 뿐이고, `draft_init_body`가 그것을 늘 `"(Unknown)"`으로 보내므로 **어떤 초안에서도
/// 주소가 나올 수 없다**(실측: 수신자가 채워진 초안에서도 `"(Unknown)"`). 대안 자리를 쌓으면
/// 어느 것이 정본인지 흐려질 뿐이다.
///
/// **수신자가 여럿이어도 안전하다** — 이 함수도 브라우저도 그 **문자열을 통째로** 싣는다.
/// 우리가 주소를 쪼개거나 다시 잇지 않으므로 구분자가 무엇이든 브라우저가 보내는 것과 같은 값이
/// 나간다(구분자 자체는 여전히 미실측이지만 그 값을 해석할 일이 없다).
///
/// ⚠️ **변환이 주소를 먹었는지 검사한다** — 값이 있는데 `@`가 없으면 조용히 통과시키지 않고 거부한다.
fn draft_recipient(init: &Value, draft_muid: &str) -> Result<String> {
    let decoded = unescape_entities(&json_str(init.pointer("/mailInfo/decodeMime/to")));
    if decoded.trim().is_empty() {
        return Ok(String::new()); // 수신자 없는 초안 — 호출부가 `to` 인자를 요구한다
    }
    if !decoded.contains('@') {
        bail!(
            "초안(draft_muid={draft_muid})의 수신자(mailInfo.decodeMime.to)가 주소로 보이지 않는다('{decoded}') \
             — 엉뚱한 곳으로 보내지 않도록 발송하지 않는다. to 인자로 수신자를 지정할 것"
        );
    }
    Ok(decoded)
}

/// 초안에 걸린 **참조(cc)·숨은참조(bcc)를 발송 폼에 실을 형태로 꺼낸다.** 없으면 빈 문자열.
///
/// 예전에는 참조가 걸린 초안을 **거부**했다. 폼이 `cc`/`bcc`를 늘 빈 값으로 보내던 시절에는
/// 그대로 발송하면 참조 수신자가 조용히 빠졌기 때문이다. 실측(`analyze/15`)으로 **초안이 참조를
/// 저장하고 되읽기 응답이 그것을 돌려준다**는 것이 확인돼 승계로 바꿨다.
///
/// **정본은 `mailInfo.decodeMime`**다 — `mime.header` 쪽은 RFC 2822 줄접기(`,\r\n `)가 살아 있고
/// 표시명이 `=?UTF-8?B?...?=`로 MIME 인코딩돼 있어 그대로 실으면 깨진다(`draft_recipient`와 같은 이유).
/// 값에는 HTML 엔티티(`&lt;`)가 남아 있어 `unescape_entities`를 태운다.
///
/// **판정 불가는 중단이다** — 다만 그 판정의 기준은 `cc` 하나다.
///
/// ⚠️ **`cc`와 `bcc`의 "키 없음"은 뜻이 다르다**(둘 다 실측):
/// - `cc`는 **참조가 없어도 `mime.header.cc`가 빈 문자열로 실려 온다.** 그래서 이 키가 아예
///   없다는 것은 "참조가 없다"가 아니라 **응답 모양이 예상과 다르다**는 뜻이라 중단한다.
/// - `bcc`는 **숨은참조가 없으면 키 자체가 오지 않는다.** 반대로 숨은참조가 걸린 초안은
///   `header`·`decodeMime` **양쪽에 실려 온다**(`analyze/15` §③). 그래서 `cc`로 응답 모양이
///   이미 확인된 뒤라면 `bcc` 키의 부재는 "숨은참조 없음"으로 읽어도 된다.
///
/// 이 비대칭을 `cc`에 맞춰 일반화하면(둘 다 키를 요구하면) **참조 없는 정상 초안이 전부 거부되고**,
/// `bcc`에 맞춰 일반화하면(둘 다 부재를 허용하면) 응답 모양이 바뀌었을 때 참조를 조용히 빠뜨린다.
///
/// 최상위 `paramCc`/`paramBcc`는 **응답에 존재하지 않아**(실측) 보지 않는다.
///
/// ⚠️ **구분자가 왕복하며 달라진다** — 보낼 때는 `,`(공백 없음), 되읽으면 `, `(공백 포함, 실측).
/// 서버가 양쪽을 받으므로 되읽은 값을 그대로 되싣는다. 값을 쪼개거나 다시 잇지 않는다.
fn draft_carbon_copy(init: &Value, draft_muid: &str) -> Result<(String, String)> {
    // ① 응답 모양 확인 — `cc` 자리는 참조가 없어도 존재해야 한다.
    if init.pointer("/mailInfo/decodeMime/cc").is_none()
        && init.pointer("/mailInfo/mime/header/cc").is_none()
    {
        bail!(
            "초안(draft_muid={draft_muid})의 참조(cc)를 응답 어디에서도 읽지 못해 참조 유무를 \
             판정할 수 없다 — 참조를 빠뜨린 채 보낼 위험이 있어 발송하지 않는다"
        );
    }

    // ② 값 회수. 정본은 decodeMime 이고, 거기 없는데 header 에만 값이 있으면 승계 경로가
    //    없는 형태(미실측)이므로 조용히 빠뜨리는 대신 중단한다.
    let pick = |kind: &str, decoded_path: &str, header_path: &str| -> Result<String> {
        let decoded = init.pointer(decoded_path);
        if let Some(v) = decoded
            && !is_blank(v)
        {
            return Ok(unescape_entities(&json_str(Some(v))));
        }
        if init.pointer(header_path).is_some_and(|v| !is_blank(v)) {
            bail!(
                "초안(draft_muid={draft_muid})의 참조({kind})가 원본 헤더에는 있는데 \
                 디코드된 자리(mailInfo.decodeMime.{kind})에는 없다 — 그대로 실으면 주소가 \
                 깨질 수 있어 발송하지 않는다. 아마란스 웹에서 그 초안을 열어 발송할 것"
            );
        }
        Ok(String::new())
    };

    let cc = pick("cc", "/mailInfo/decodeMime/cc", "/mailInfo/mime/header/cc")?;
    let bcc = pick("bcc", "/mailInfo/decodeMime/bcc", "/mailInfo/mime/header/bcc")?;
    Ok((cc, bcc))
}

/// JSON 어딘가에 그 키가 **비어 있지 않은 값**으로 들어 있는지(정확한 키 이름 일치).
/// 초안이 우리가 재현하지 않는 값을 들고 있는지 감지하는 데 쓴다.
fn has_nonempty_key(v: &Value, key: &str) -> bool {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(key)
                && !is_blank(x)
            {
                return true;
            }
            m.values().any(|x| has_nonempty_key(x, key))
        }
        Value::Array(a) => a.iter().any(|x| has_nonempty_key(x, key)),
        _ => false,
    }
}

/// "값이 없는 것과 같다"의 판정 — 빈 문자열·빈 배열·빈 객체·null.
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

/// 초안이 **이미 서버에 들고 있는** 첨부를 발송 폼으로 승계한다 — `mail014A08`(초안 `fileSn` →
/// 발송용 `fileId`). 반환은 폼에 실을 `(uidAuthList, bigFileCnt, fwFile)`.
///
/// ⛔ **신규 업로드(`upload_files`)와 공유하지 않는다.** 브라우저가 만드는 `uidAuthList` 항목의
/// 키 구성이 서로 다르다(실측) — 이쪽은 A08 응답 객체를 **그대로 두고** 7개를 덧붙이고,
/// `serverFile`/`useDownView`/`offset`/`encoding`처럼 신규 업로드에는 없는 키가 함께 간다.
/// 하나로 합치면 어느 한쪽이 조용히 깨진다.
///
/// **막는 것**(전부 "조용히 잘못 나가는" 경우라 명시적으로 중단한다):
/// - **파일명에 콤마** — 폼의 `fwFile`이 콤마로 파일을 구분한다. 서버가 그 콤마를 어떻게 읽는지
///   실측하지 않았고, 이름이 쪼개져 엉뚱한 파일 목록이 될 수 있다.
/// - **동명 파일 둘 이상** — A08 응답 순서가 요청 순서와 같은지는 실측하지 않아 **이름으로**
///   짝짓는데, 이름이 겹치면 짝을 확정할 수 없다.
/// - **대용량 첨부(`bigFile`/`bigFilePeriod`)** — 우리 폼은 이 두 값을 빈 값으로 보낸다.
///   초안이 값을 들고 있으면 그 부분이 빠진 채 나가므로 보내지 않는다.
///   ⚠️ 대용량 첨부가 `fileList`에 아예 실리지 않는 형태라면 이 검사에 걸리지 않는다(미실측).
/// - **개수 불일치** — A08이 요청한 수만큼 돌려주지 않으면 일부를 빠뜨린 채 보내게 되므로 중단.
async fn inherit_draft_attachments(
    c: &GwClient,
    init: &Value,
    draft_muid: &str,
    files: &[Value],
) -> Result<(String, String, String)> {
    // ① 승계 가능한 모양인지 먼저 판정한다(A08을 부르기 전에 — 부작용 없는 순서).
    let want = draft_attachment_plan(init, draft_muid, files)?;

    // ② A08 — `fileSn`은 **콤마로 이어붙여 한 번에** 보낸다(실측).
    // `authKeyMap`의 email/empSeq는 **A01 응답값**을 쓴다(실측 사양) — 클라이언트 세션값과
    // 갈릴 여지를 두지 않는다. 없으면 판정 불가이므로 보내지 않는다.
    let email = json_str(init.get("email"));
    let emp_seq = json_str(init.get("empSeq"));
    if email.is_empty() || emp_seq.is_empty() {
        bail!(
            "초안 열기 응답에 email/empSeq가 없어 첨부를 승계할 수 없다(draft_muid={draft_muid}) \
             — 첨부를 빠뜨린 채 보낼 위험이 있어 발송하지 않는다"
        );
    }
    let auth = json!({ "email": email, "empSeq": emp_seq, "muid": draft_muid }).to_string();
    let sns = want
        .0
        .iter()
        .map(|(_, _, s)| s.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let a08 = c
        .call_form(
            "/mail/mail014A08",
            &[
                ("moduleGbn", "MAIL"),
                ("authKeyMap", &auth),
                ("fileSn", &sns),
                ("condition", "99"),
            ],
        )
        .await?;

    // ③ 응답 → 폼 값. 개수 검사와 조립은 순수 함수에 있다(회귀를 테스트로 잡기 위해).
    let list = a08.get("list").and_then(|v| v.as_array()).ok_or_else(|| {
        anyhow!("mail014A08 응답에 list가 없어 첨부를 승계할 수 없다 — 발송하지 않는다: {a08}")
    })?;
    build_inherited_fields(&want, list)
}

/// A08 응답 → 폼에 실을 `(uidAuthList, bigFileCnt, fwFile)`.
///
/// **개수가 어긋나면 보내지 않는다** — 응답이 요청보다 적으면 일부를 빠뜨린 채 발송하게 되고,
/// 많으면 우리가 요청하지 않은 파일이 섞여 온 것이라 어느 쪽도 그대로 보낼 수 없다.
fn build_inherited_fields(
    plan: &AttachmentPlan,
    list: &[Value],
) -> Result<(String, String, String)> {
    let want = &plan.0;
    if list.len() != want.len() {
        bail!(
            "mail014A08이 첨부 {}개 중 {}개만 돌려줬다 — 일부를 빠뜨린 채 보낼 수 없어 발송하지 않는다",
            want.len(),
            list.len()
        );
    }
    let items: Vec<Value> = want
        .iter()
        .enumerate()
        .map(|(i, (orig, ext, sn))| build_inherited_item(list, i, orig, ext, sn))
        .collect::<Result<_>>()?;
    let fw_file = want
        .iter()
        .map(|(o, _, _)| o.as_str())
        .collect::<Vec<_>>()
        .join(",");
    Ok((
        Value::Array(items).to_string(),
        want.len().to_string(),
        fw_file,
    ))
}

/// 승계할 첨부 `(originalFileName, fileExtsn, fileSn)`의 **검증된** 목록.
/// `draft_attachment_plan`이 내고 `build_inherited_fields`가 요구한다 — 검증 **호출만 지우면
/// 컴파일되지 않는다**(타입이 안 맞는다). `DraftExists`와 같은 보증 범위다: 같은 모듈에서 이
/// 값을 손으로 조립해 우회하는 것까지는 막지 못한다.
struct AttachmentPlan(Vec<(String, String, String)>);

/// 초안 첨부 목록 → 승계 계획 `(originalFileName, fileExtsn, fileSn)`. **A08을 부르기 전에**
/// 승계할 수 없는 모양을 전부 걸러낸다(부작용 없는 순서). 막는 이유는 `inherit_draft_attachments`.
///
/// ⚠️ **대용량 첨부 감지의 미확인 지점** — 우리 폼은 `bigFile`/`bigFilePeriod`를 빈 값으로
/// 보내므로 초안이 그 값을 들고 있으면 거부한다. 그런데 **참조한 A01 캡처는 발췌라, 정상 초안의
/// 응답에도 `bigFilePeriod`가 (값을 가진 채) 늘 실려 오는지는 확인하지 못했다.** 실려 온다면
/// 첨부 있는 초안이 전부 거부된다 — fail-closed 방향이라 위험은 아니지만, **첫 실사용에서
/// 확인할 지점**이다(거부가 잦으면 이 키 목록부터 볼 것).
fn draft_attachment_plan(
    init: &Value,
    draft_muid: &str,
    files: &[Value],
) -> Result<AttachmentPlan> {
    for key in ["bigFile", "bigFilePeriod"] {
        if has_nonempty_key(init, key) {
            return Err(InvalidInput(format!(
                "초안(draft_muid={draft_muid})이 대용량 첨부({key})를 들고 있어 발송할 수 없다 \
                 — 그 경로는 실측하지 않아 값을 빠뜨린 채 보내게 된다. 아마란스 웹에서 그 초안을 열어 발송할 것"
            ))
            .into());
        }
    }
    let mut want: Vec<(String, String, String)> = Vec::with_capacity(files.len());
    for f in files {
        let orig = json_str(f.get("originalFileName"));
        let ext = json_str(f.get("fileExtsn"));
        let sn = json_str(f.get("fileSn"));
        if orig.is_empty() || sn.is_empty() {
            return Err(InvalidInput(format!(
                "초안(draft_muid={draft_muid}) 첨부의 originalFileName/fileSn이 비어 있어 승계할 수 없다 \
                 — 아마란스 웹에서 그 초안을 열어 발송할 것"
            ))
            .into());
        }
        if orig.contains(',') || ext.contains(',') {
            return Err(InvalidInput(format!(
                "첨부 파일명에 콤마가 있어 발송할 수 없다('{orig}') — 발송 폼의 fwFile이 콤마로 파일을 \
                 구분해서 이름이 쪼개진다. 파일명을 바꿔 다시 첨부하거나 아마란스 웹에서 발송할 것"
            ))
            .into());
        }
        if want.iter().any(|(o, e, _)| o == &orig && e == &ext) {
            return Err(InvalidInput(format!(
                "같은 이름의 첨부가 둘 이상이라 발송할 수 없다('{orig}') — 첨부 재조회 응답을 이름으로 \
                 짝짓는데(응답 순서 보장이 미실측이다) 이름이 겹치면 어느 것이 어느 것인지 확정할 수 없다. \
                 아마란스 웹에서 그 초안을 열어 발송할 것"
            ))
            .into());
        }
        want.push((orig, ext, sn));
    }
    Ok(AttachmentPlan(want))
}

/// A08 응답 한 항목 → 발송 폼 `uidAuthList` 원소. 브라우저가 덧붙이는 것을 그대로 재현한다.
fn build_inherited_item(
    list: &[Value],
    idx: usize,
    orig: &str,
    ext: &str,
    sn: &str,
) -> Result<Value> {
    // ⚠️ **인덱스로 짝짓지 않는다** — A08 응답 순서가 요청 순서와 같은지는 실측하지 않았다.
    let got = list
        .iter()
        .find(|x| json_str(x.get("originalFileName")) == orig && json_str(x.get("fileExtsn")) == ext)
        .ok_or_else(|| {
            anyhow!("mail014A08 응답에서 첨부 '{orig}'를 찾지 못했다 — 발송하지 않는다")
        })?;
    let mut o = got
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("mail014A08 응답 항목이 객체가 아니다: {got}"))?;
    if json_str(got.get("fileId")).is_empty() {
        bail!("mail014A08이 첨부 '{orig}'의 fileId를 주지 않았다 — 발송하지 않는다");
    }
    let size: i64 = json_str(got.get("fileSize")).parse().unwrap_or(0);
    o.insert("fileSize".into(), json!(format!("{size} Bytes")));
    // `authKeyMap`은 객체가 아니라 **JSON 문자열로 한 번 더 감싸** 보낸다(브라우저 실측).
    let ak = got.get("authKeyMap").cloned().unwrap_or(Value::Null);
    o.insert(
        "authKeyMap".into(),
        json!(if ak.is_null() {
            String::new()
        } else {
            ak.to_string()
        }),
    );
    // A08 응답의 `fileSn`은 빈 문자열로 온다 — 요청에 쓴 초안 토큰을 되살린다(브라우저 실측).
    o.insert("fileSn".into(), json!(sn));
    for (k, v) in [
        ("link", "N"),
        ("fileDeleteYN", "Y"),
        ("serverFile", "Y"),
        ("useDownView", "N"),
    ] {
        o.insert(k.into(), json!(v));
    }
    o.insert("fileClass".into(), json!(format!("icon_{ext}")));
    o.insert("noConvertFileSize".into(), json!(size));
    o.insert("id".into(), json!(idx));
    Ok(Value::Object(o))
}

/// 로컬 파일들을 `mail014A06`(multipart `file[]`)로 업로드하고, 발송의 `uidAuthList`
/// (JSON 문자열)을 조립한다. 업로드 응답 `resultData.list[]`의 `fileId`를 그대로 사용.
/// (필드명 `file[]`·응답구조는 프론트 번들 실측)
async fn upload_files(c: &GwClient, paths: &[String]) -> Result<String> {
    // 파일 내용을 먼저 읽어둔다 — 폼 조립은 401 재시도 때 한 번 더 일어날 수 있으므로
    // 디스크 I/O(와 그 실패 처리)를 조립 밖으로 뺀다.
    let mut files = Vec::new();
    for p in paths {
        let bytes = std::fs::read(p).map_err(|e| anyhow!("첨부 읽기 실패 {p}: {e}"))?;
        let fname = std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        files.push((fname, bytes));
    }
    // 이 폼은 파일 내용뿐이라 크레덴셜 파생 값이 없다(인증은 서명 헤더가 전담 — 재시도 때
    // `signed()`가 새 토큰으로 다시 계산한다). 발송 폼의 body authToken 같은 함정이 없다.
    let form = || {
        files.iter().fold(reqwest::multipart::Form::new(), |f, (name, bytes)| {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(name.clone())
                .mime_str("application/octet-stream")
                .expect("고정 MIME 문자열 — 파싱 실패할 수 없다");
            f.part("file[]", part)
        })
    };
    let up = c.call_multipart("/mail/mail014A06", form).await?;
    let list = up
        .pointer("/resultData/list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("mail014A06 업로드 응답에 resultData.list 없음: {up}"))?;

    let items: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let orig = json_str(f.get("originalFileName"));
            let ext = json_str(f.get("fileExtsn"));
            let size: i64 = json_str(f.get("fileSize")).parse().unwrap_or(0);
            let size_str = format!("{size} Bytes");
            let path = if ext.is_empty() {
                orig.clone()
            } else {
                format!("{orig}.{ext}")
            };
            json!({
                "fileName": orig,
                "fileSize": size_str,
                "fileExtsn": ext,
                "title": format!("{orig}{size_str}"),
                "fileClass": format!("icon_{ext}"),
                "fileId": json_str(f.get("fileId")),
                "filePath": path,
                "fileThumUrl": "", "fileUrl": "", "filePublicYn": "N",
                "noConvertFileSize": size,
                "modifyLocalAttach": "N", "link": "N", "fileDeleteYN": "Y",
                "id": i, "moduleGbn": "MAIL"
            })
        })
        .collect();
    Ok(Value::Array(items).to_string())
}

/// 메일 상세 읽기 — `mail002A01`(body `{uid: muid}`). 본문(HTML→평문)·헤더·첨부목록 반환.
/// ⚠️ 보안: 본문 HTML은 **렌더링하지 않고 서버측에서 평문화**한다 → 외부 이미지(추적 픽셀)를
/// 자동 fetch하지 않는다. 외부 리소스가 있으면 `remoteResourceCount`로 알리기만 한다.
/// 첨부는 메타데이터만 반환(다운로드는 별도 `download_attachment` — 실행 아님, 저장만).
pub async fn read_mail(c: &GwClient, muid: &str) -> Result<Value> {
    let d = c.call("/mail/mail002A01", &json!({ "uid": muid })).await?;
    let mime = d.get("mime").cloned().unwrap_or(json!({}));
    let dm = d.get("decodeMime").cloned().unwrap_or(json!({}));

    let html = mime.pointer("/body/html").and_then(|v| v.as_str()).unwrap_or("");
    let plain = mime.pointer("/body/plain").and_then(|v| v.as_str()).unwrap_or("");
    let body = if !plain.trim().is_empty() {
        collapse_ws(plain)
    } else {
        collapse_ws(&html_to_text(html))
    };
    let remote = count_remote_resources(html);

    let attachments: Vec<Value> = mime
        .get("fileList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, f)| {
                    let ext = json_str(f.get("fileExtsn"));
                    let name = json_str(f.get("originalFileName"));
                    json!({
                        "index": i,
                        "fileName": if ext.is_empty() { name.clone() } else { format!("{name}.{ext}") },
                        "fileExt": ext,
                        "fileSizeApprox": approx_decoded_size(&json_str(f.get("fileSize"))),
                        "fileSn": json_str(f.get("fileSn")),
                        "isImage": matches!(json_str(f.get("fileExtsn")).to_ascii_lowercase().as_str(),
                            "png"|"jpg"|"jpeg"|"gif"|"bmp"|"webp"|"svg")
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "muid": muid,
        "subject": html_to_text(&json_str(dm.get("subject"))),
        "from": html_to_text(&json_str(dm.get("from"))),
        "to": html_to_text(&json_str(dm.get("to"))),
        // 참조. 받은 메일에는 `cc`만 실려 온다 — `bcc`는 원래 수신자에게 보이지 않는 필드라
        // 대개 빈 문자열이다(보낸 사본에는 실릴 수 있다). 키는 **항상 낸다** — 없는 것과
        // 안 보이는 것을 호출자가 구분하려면 자리가 늘 있어야 한다.
        "cc": html_to_text(&json_str(dm.get("cc"))),
        "bcc": html_to_text(&json_str(dm.get("bcc"))),
        "date": json_str(dm.get("date")),
        "body": body,
        "attachments": attachments,
        // 본문에 박힌 이미지 중 **이 서버가 갖고 있는 것**만. `download_body_image`에 그대로 넘긴다.
        // ⚠️ 외부 호스트 이미지(서명 로고·추적 픽셀)는 **일부러 뺀다** — 목록에 실어두면 도구가
        // 그걸 받아오는 우회로가 되어, 본문을 렌더링하지 않는다는 이 모듈의 전제가 깨진다.
        // 외부 것이 몇 개인지는 아래 `remoteResourceCount`가 이미 말해준다.
        // ⚠️ 본문(`body`)이 plain 파트에서 왔다면 그쪽엔 `<img>`가 없다 — 그래서 이미지 목록은
        // 본문 출처와 무관하게 **항상 html 파트**에서 뽑는다(실측: muid 13893652).
        "inlineImages": crate::modules::board::extract_img_srcs(html)
            .into_iter()
            .filter(|s| crate::modules::board::is_body_image(s))
            .collect::<Vec<_>>(),
        "remoteResourceCount": remote
    }))
}

/// 메일 첨부 다운로드 — 2단계: `mail014A08`(fileSn→다운로드 fileId 변환) → `/ecm/ecm001A03`
/// (`moduleGbn=MAIL`, authKeyMap{muid,email,empSeq}, fileSn=fileId, 서명헤더). `out_path`에 저장.
/// `file_sn`은 `read_mail` 첨부목록의 `fileSn`. 게시판과 동일 ECM 다운로드 엔드포인트.
/// ⚠️ 바이트를 파일로 **저장만** 한다(열지·실행하지 않음) → 악성 첨부도 격리 분석에 안전.
pub async fn download_attachment(
    c: &GwClient,
    muid: &str,
    file_sn: &str,
    out_path: &str,
) -> Result<Value> {
    c.ensure_session().await?; // email(수신함 소유자) 확보
    let email = format!("{}@{}", c.email_addr(), c.email_domain());
    let emp = c.emp_seq();

    // 1) fileSn → 다운로드 fileId
    let auth = json!({ "email": email, "muid": muid, "empSeq": emp }).to_string();
    let a08 = c
        .call_form(
            "/mail/mail014A08",
            &[("moduleGbn", "MAIL"), ("authKeyMap", &auth), ("fileSn", file_sn)],
        )
        .await?;
    let item = a08
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("mail014A08: 첨부 없음(fileSn 불일치? muid/fileSn 확인)"))?;
    let file_id = item
        .get("fileId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("mail014A08: fileId 없음"))?
        .to_string();
    let list_name = json_str(item.get("fileName"));

    // 2) ECM 다운로드(바이트)
    let auth_dl = json!({ "muid": muid, "email": email, "empSeq": emp }).to_string();
    let (size, srv_name) = c
        .download_form(
            "/ecm/ecm001A03",
            &[
                ("moduleGbn", "MAIL"),
                ("authKeyMap", &auth_dl),
                ("fileSn", &file_id),
                ("condition", "99"),
            ],
            out_path,
        )
        .await?;

    Ok(json!({
        "ok": true,
        "path": out_path,
        "bytes": size,
        "serverFileName": srv_name.filter(|s| !s.is_empty()).unwrap_or(list_name)
    }))
}

/// mail002A01 `fileSize`는 원본 바이트가 아니라 MIME 본문(base64+줄바꿈) 크기라 실제보다 ~33% 크다.
/// 원본 크기 근사치(≈ 인코딩크기 × 3/4)를 반환한다. **정확한 크기는 다운로드 결과의 `bytes`** 참조.
fn approx_decoded_size(encoded: &str) -> i64 {
    encoded.parse::<i64>().map(|n| n * 3 / 4).unwrap_or(0)
}

/// 본문 HTML의 외부(원격) 리소스 개수 추정 — `src="http` / `url(http`(외부 이미지·추적 픽셀).
/// fetch하지 않고 개수만 센다(자동 로드 금지 = 열람 유출 방지).
fn count_remote_resources(html: &str) -> usize {
    let h = html.to_ascii_lowercase();
    let mut n = 0;
    for pat in ["src=\"http", "src='http", "url(http", "background=\"http"] {
        n += h.matches(pat).count();
    }
    n
}

/// 메일 삭제(휴지통 이동) — `mail002A05`. `uids`=콤마구분 muid 리스트(다건).
/// ⚠️ 휴지통 이동 시 muid가 재부여되므로 이후 추적은 재조회 필요.
pub async fn delete_mails(c: &GwClient, uids: &str) -> Result<Value> {
    c.call(
        "/mail/mail002A05",
        &json!({ "uids": uids, "mailKey": "", "boxName": "" }),
    )
    .await
}

/// 발송·임시저장이 공유하는 폼의 **회귀 기준선**. 발송 폼을 `ComposeForm`으로 뽑아내면서
/// 필드가 빠지거나 값이 달라져도 컴파일러가 잡지 못하기 때문에, 실측 필드 집합을 여기 박아둔다.
#[cfg(test)]
mod tests {
    use super::*;

    /// 타입이 달라도 같은 상태로 판정해야 사전 확인과 사후 검증이 어긋나지 않는다.
    #[test]
    fn seen_flag는_숫자_문자열_불리언을_모두_흡수한다() {
        for seen in [json!(0), json!("0"), json!(false), json!("false")] {
            let list = json!({ "Records": [{ "muid": 1, "seen": seen }] });
            assert_eq!(seen_flag(&list, "1"), Some(SeenState::Unseen), "seen={seen}");
        }
        for seen in [json!(1), json!("1"), json!(true), json!("true")] {
            let list = json!({ "Records": [{ "muid": "1", "seen": seen }] });
            assert_eq!(seen_flag(&list, "1"), Some(SeenState::Seen), "seen={seen}");
        }
    }

    /// 필드 해석 불가와 대상 부재를 구분하고, 어느 쪽도 읽음·미읽음으로 단정하지 않는다.
    #[test]
    fn seen_flag는_모름을_읽음과_구분한다() {
        let list = json!({ "Records": [
            { "muid": 1 },
            { "muid": 2, "seen": null },
            { "muid": 3, "seen": "yes" },
            { "muid": 4, "seen": 2 },
            { "muid": 5, "seen": [] },
            { "muid": 6, "seen": {} },
        ]});
        for muid in ["1", "2", "3", "4", "5", "6"] {
            assert_eq!(seen_flag(&list, muid), Some(SeenState::Unknown), "muid={muid}");
        }
        assert_eq!(seen_flag(&list, "99"), None, "창 밖은 None — 상태 미상과 구분된다");
        assert_eq!(seen_flag(&json!({}), "1"), None);
        assert_eq!(seen_flag(&json!({ "Records": null }), "1"), None);
        assert_eq!(seen_flag(&json!({ "Records": {} }), "1"), None);
    }

    fn sample_init() -> Value {
        json!({
            "email": "me@example.com",
            "filedir": "/dir/2026",
            "sessionKey": "SK-1",
            "bigFileDay": "30",
            "externalSendLimit": "50",
            "mailkey": "1785980031409_c7d8f89c",
            "insideDomainArray": ["example.com"],
            "groupMailOption": {
                "groupMailAddr": "g@example.com",
                "groupMailIntedAddr": "gi@example.com",
                "groupMailOrg": "조직"
            }
        })
    }

    fn fields_of(cf: &ComposeForm) -> std::collections::HashMap<String, String> {
        cf.fields(&GwClient::new(None))
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn 발송_폼은_실측_필드_37개를_그대로_담는다() {
        let cf = ComposeForm::new(
            &sample_init(),
            "홍길동 <hong@example.com>",
            "제목",
            "<p>본문</p>",
            "[]".to_string(),
            "2".to_string(),
        );
        let raw = cf.fields(&GwClient::new(None));
        let names: Vec<&str> = raw.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            names,
            vec![
                "from", "fromName", "to", "cc", "bcc", "htmlContents", "email", "fileDir",
                "bigFile", "bigFileDay", "bigFileCnt", "bigFilePeriod", "mail_kind", "uidAuthList",
                "fwFile", "urlList", "fileNameList", "receipt_notific", "securitymailuse",
                "securitymailpass_enc_web", "immediately", "toBeDeleted", "expirationDate",
                "importantmailuse", "eachTrans", "neobizaddr", "neobizIntedAddr", "neobizOrg",
                "muid", "domainSeq", "mimeHeader", "sessionKey", "externalSendLimit",
                "insideDomainArray", "aiResultJSON", "subject", "authToken",
            ],
            "발송 FormData 필드 집합이 변했다"
        );

        let f = fields_of(&cf);
        assert_eq!(f["from"], "me@example.com");
        assert_eq!(f["email"], "me@example.com");
        assert_eq!(f["to"], "홍길동 <hong@example.com>");
        assert_eq!(f["subject"], "제목");
        assert_eq!(f["htmlContents"], "<p>본문</p>");
        assert_eq!(f["fileDir"], "/dir/2026");
        assert_eq!(f["sessionKey"], "SK-1");
        assert_eq!(f["bigFileDay"], "30");
        assert_eq!(f["externalSendLimit"], "50");
        assert_eq!(f["insideDomainArray"], r#"["example.com"]"#);
        assert_eq!(f["neobizaddr"], "g@example.com");
        assert_eq!(f["neobizIntedAddr"], "gi@example.com");
        assert_eq!(f["neobizOrg"], "조직");
        assert_eq!(f["uidAuthList"], "[]");
        assert_eq!(f["bigFileCnt"], "2");
        // ⚠️ `"me"`가 아니다 — `"me"`는 **내게쓰기** 모드다(실측 `analyze/15` §부수 관측).
        // 신규 발송은 브라우저의 `메일쓰기`와 같은 `"plain"`으로 연다.
        assert_eq!(f["mail_kind"], "plain");
        assert_eq!(f["muid"], "0"); // 0 = 신규(답장/전달이 아님)
        assert_eq!(f["immediately"], "false");
        assert_eq!(f["expirationDate"], "Invalid date");

        // ⚠️ **빈 값도 절대값이다.** 이름 집합만 보면 `ComposeForm::new`의 기본값이 조용히 바뀌어도
        // 통과하고, 초안 발송 테스트도 "base와 draft의 차이"만 보므로 함께 바뀌면 못 잡는다.
        // 신규 발송이 실제로 무엇을 보내는지를 여기서 못박는다(전부 실측값).
        for k in [
            "cc", "bcc", "bigFile", "bigFilePeriod", "fwFile", "urlList", "fileNameList",
            "receipt_notific", "securitymailuse", "securitymailpass_enc_web", "importantmailuse",
            "eachTrans", "domainSeq", "mimeHeader", "aiResultJSON",
        ] {
            assert_eq!(f[k], "", "신규 발송의 {k}는 빈 값이어야 한다");
        }
        assert_eq!(f["toBeDeleted"], "false");
    }

    /// 메일함 seq는 계정마다 달라 이름으로 찾는다 — 그 탐색이 중첩·타입혼용을 견디는지.
    #[test]
    fn 메일함_seq를_이름으로_찾는다() {
        let boxes = json!({
            "mailboxList": [
                { "fullname": "INBOX", "name": "INBOX", "mboxSeq": 26986, "exists": "3" },
                { "fullname": "SENT", "name": "SENT", "mboxSeq": "26989", "exists": "0" },
                { "children": [
                    { "fullname": "DRAFTS", "name": "DRAFTS", "mboxSeq": 26992, "exists": "0" }
                ]}
            ]
        });
        assert_eq!(find_mbox_seq(&boxes, "DRAFTS"), Some(26992)); // 중첩된 곳도 찾는다
        assert_eq!(find_mbox_seq(&boxes, "INBOX"), Some(26986));
        assert_eq!(find_mbox_seq(&boxes, "SENT"), Some(26989)); // mboxSeq가 문자열이어도 흡수
        assert_eq!(find_mbox_seq(&boxes, "drafts"), Some(26992)); // 대소문자 무시
        assert_eq!(find_mbox_seq(&boxes, "ARCHIVE"), None); // 없는 함은 None(엉뚱한 함 금지)
    }

    /// 회귀: INBOX seq를 개발자 계정 값(26986)으로 박아둔 탓에 다른 계정 전원이
    /// 받은메일함 조회에 실패했다(`not Box : … -> boxSeq(26986)`). 응답에 실린 값을
    /// 그대로 따라가는지 — **다른 계정의 seq**로 못박는다.
    #[test]
    fn 받은메일함_seq는_계정마다_다른_응답값을_따른다() {
        let 다른_계정 = json!({
            "mailboxList": [
                { "fullname": "INBOX", "name": "INBOX", "mboxSeq": 25750, "exists": "12" }
            ]
        });
        assert_eq!(find_mbox_seq(&다른_계정, "INBOX"), Some(25750));
    }

    /// 이름이 `name`에만 있고 `fullname`이 비어도 찾아야 한다(함마다 어느 키에 오는지 다르다).
    #[test]
    fn 메일함_이름은_fullname이_없으면_name으로_본다() {
        let boxes = json!([{ "name": "DRAFTS", "mboxSeq": 1 }]);
        assert_eq!(find_mbox_seq(&boxes, "DRAFTS"), Some(1));
        // mboxSeq가 숫자로 파싱되지 않으면 그 노드는 후보가 아니다(빈 문자열 등).
        assert_eq!(find_mbox_seq(&json!([{ "name": "DRAFTS", "mboxSeq": "" }]), "DRAFTS"), None);
    }

    /// 임시저장 read-back의 판정부 — 통수가 아니라 muid로 본다.
    #[test]
    fn read_back은_목록에서_그_muid를_찾는다() {
        let list = json!({ "Records": [{ "muid": 13541170 }, { "muid": "13541171" }] });
        assert!(has_muid(&list, "13541170")); // 숫자로 와도 문자열로 흡수해 비교
        assert!(has_muid(&list, "13541171"));
        assert!(!has_muid(&list, "99999999")); // 없는 muid는 미검증
        assert!(!has_muid(&json!({}), "1")); // Records가 아예 없으면 미검증
        assert!(!has_muid(&json!({ "Records": [] }), "1"));
    }

    /// 서버가 주는 서명은 래퍼 div **안에** 완전한 html 문서가 중첩된 모양이다. 그걸 그대로
    /// 이어 붙이면 메일 하나에 `<html>`이 둘이 되므로 세 겹을 걷어내야 한다.
    /// (실물 형태 — muid 14086132 발송분 대조로 확정)
    #[test]
    fn 서명은_문서_래퍼를_벗고_붙는다() {
        let init = json!({
            "signature": "<div class=\"dze_signature\" dze_signature_index=\"1\"><html><head></head><body><table><tr><td>이재학</td></tr></table></body></html></div>"
        });
        let (html, attached) = with_signature("<p>본문</p>", &init, true);
        assert!(attached);
        assert!(html.starts_with("<p>본문</p><div class=\"dze_signature\""));
        assert!(!html.contains("<html>") && !html.contains("<body>") && !html.contains("</head>"));
        assert!(html.contains("<table><tr><td>이재학</td></tr></table>")); // 안쪽은 그대로
    }

    /// 붙이지 않는 세 경우. 특히 **서명 미등록 계정**에서 조용히 아무 일도 안 하므로,
    /// 요청값이 아니라 반환 플래그를 도구 응답에 실어야 거짓 보고가 안 된다.
    #[test]
    fn 서명은_끄면_미등록이면_이미_있으면_붙지_않는다() {
        let init = json!({ "signature": "<div class=\"dze_signature\"><table></table></div>" });
        assert_eq!(with_signature("<p>본문</p>", &init, false), ("<p>본문</p>".into(), false));
        // 미등록 계정: 필드가 빈 문자열이거나 아예 없다
        assert_eq!(with_signature("<p>본문</p>", &json!({ "signature": "" }), true), ("<p>본문</p>".into(), false));
        assert_eq!(with_signature("<p>본문</p>", &json!({}), true), ("<p>본문</p>".into(), false));
        // 호출자가 본문에 이미 서명을 써 넣은 경우 — 두 개가 되면 도구는 성공을 보고한다
        let already = "<p>본문</p><div class=\"dze_signature\">직접 넣음</div>";
        assert_eq!(with_signature(already, &init, true), (already.to_string(), false));
    }

    /// 걷어내는 것은 문서 래퍼 **셋뿐**이다. 다른 태그를 건드리면 서명 레이아웃이 깨진다.
    #[test]
    fn 문서래퍼_제거는_다른_태그를_건드리지_않는다() {
        assert_eq!(strip_document_wrapper("<HTML><BODY><p>가</p></BODY></HTML>"), "<p>가</p>"); // 대문자도
        assert_eq!(strip_document_wrapper("<body style=\"x\">가</body>"), "가"); // 속성이 붙어도
        assert_eq!(strip_document_wrapper("<table><tr><td>가</td></tr></table>"), "<table><tr><td>가</td></tr></table>");
        assert_eq!(strip_document_wrapper("<br /><hr/>"), "<br /><hr/>");
        assert_eq!(strip_document_wrapper("<!-- 주석 -->"), "<!-- 주석 -->");
        // 태그가 아닌 '<'(닫히지 않은 것)은 원문대로 남긴다 — 본문을 조용히 먹지 않는다.
        assert_eq!(strip_document_wrapper("a < b"), "a < b");
        assert_eq!(strip_document_wrapper("2 <3 and <p>가</p>"), "2 <3 and <p>가</p>");
    }

    #[test]
    fn 도메인배열은_응답에_없으면_빈_배열로_보낸다() {
        let cf = ComposeForm::new(&json!({}), "to", "s", "", String::new(), "0".to_string());
        assert_eq!(fields_of(&cf)["insideDomainArray"], "[]");
    }

    /// 초안 발송은 **필드를 더하지 않는다** — 발송 폼 37개 그대로에서 값 3개만 바뀐다
    /// (나머지 1개인 `fileDir`/`sessionKey`는 draft 모드 A01 응답을 `new`에 넘겨 반영된다).
    /// 필드가 늘거나 다른 값이 함께 바뀌면 브라우저 재현이 깨진 것이다.
    #[test]
    fn 초안발송은_발송_폼에서_값_3개만_바꾼다() {
        // ⚠️ **두 폼을 같은 클라이언트로 뽑는다** — `authToken`은 크레덴셜에서 파생돼 인스턴스마다
        // 달라질 수 있어서, 클라이언트를 따로 만들면 "바뀐 필드"에 authToken이 섞여 들어온다.
        let c = GwClient::new(None);
        let fields = |cf: &ComposeForm| -> std::collections::HashMap<String, String> {
            cf.fields(&c).into_iter().map(|(k, v)| (k.to_string(), v)).collect()
        };

        let base = ComposeForm::new(&sample_init(), "to", "제목", "<p>본문</p>", String::new(), "0".into());
        let base_names: Vec<&str> = base.fields(&c).iter().map(|(k, _)| *k).collect();
        let b = fields(&base);

        let header = json!({"mime-version": "1.0", "to": "", "subject": "=?UTF-8?B?...?="});
        let draft = ComposeForm::new(&sample_init(), "to", "제목", "<p>본문</p>", String::new(), "0".into())
            .with_draft("13542607", header.to_string());
        let draft_names: Vec<&str> = draft.fields(&c).iter().map(|(k, _)| *k).collect();
        let d = fields(&draft);

        assert_eq!(base_names, draft_names, "초안 발송이 필드를 더하거나 뺐다");
        assert_eq!(d["muid"], "13542607", "초안 muid가 실려야 한다");
        assert_eq!(d["mail_kind"], "draft");
        assert_eq!(d["mimeHeader"], header.to_string());

        // 이 3개 말고는 한 글자도 달라지면 안 된다.
        let changed: Vec<&str> = base_names
            .iter()
            .copied()
            .filter(|k| b[*k] != d[*k])
            .collect();
        assert_eq!(changed, vec!["mail_kind", "muid", "mimeHeader"]);
    }

    /// 초안 열기는 신규 작성과 **같은 엔드포인트**에 draft 좌표를 싣는다. 캡처한 8개 키를 그대로 보내야
    /// 한다 — 무엇이 필수인지 실측하지 않았으므로 줄이면 안 된다.
    #[test]
    fn 초안열기_body는_실측한_draft_좌표_8개를_싣는다() {
        let b = draft_init_body("13542607");
        let mut keys: Vec<&str> = b.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["domainSeq", "fromFlag", "mailKind", "mailTo", "mbox", "readType", "uid", "viewFlag"]
        );
        assert_eq!(b["mailKind"], "draft");
        assert_eq!(b["mbox"], DRAFTS, "임시보관함은 이름으로 지정한다");
        assert_eq!(b["uid"], "13542607");
        assert_eq!(b["fromFlag"], true, "bool 이다 — 문자열로 보내지 말 것");

        // 신규 작성용 compose_init 과 섞이면 안 된다(그쪽은 mailKind:"me" 하나뿐이다).
        assert_ne!(b["mailKind"], "me");
    }

    /// 초안 열기(draft 모드 `mail014A01`) 응답의 **실측 모양**. 발송 판정에 쓰이는 자리만 담았다.
    ///
    /// ⚠️ **`decodeMime`은 `mailInfo` 아래다.** 이 픽스처가 그것을 최상위에 두고 있었던 탓에
    /// 단위 테스트는 전부 통과하면서 **라이브에서 제목을 못 찾아 발송이 막혔다.** 픽스처가 틀리면
    /// 테스트는 코드가 아니라 그 오해를 지킨다 — 그래서 실측한 응답 모양을 그대로 옮겨 적는다.
    /// (`save_mail_draft`가 만든 초안과 브라우저가 만든 초안이 **같은 모양**임을 확인했다.)
    /// 최상위 `paramTo`가 `"(Unknown)"`인 것도 실측 그대로다 — 우리가 보낸 `mailTo`의 메아리다.
    fn sample_draft_init() -> Value {
        json!({
            "email": "me@example.com",
            "empSeq": "12345",
            "filedir": "/dir/2026",
            "sessionKey": "SK-D",
            "mailkey": "1785993246615_x.eml",
            "paramTo": "(Unknown)",
            "mailInfo": {
                "muid": 13542607,
                "mboxName": "DRAFTS",
                "mime": {
                    "header": { "mime-version": "1.0", "to": "=?UTF-8?B?7ZmN?= &lt;hong@example.com&gt;",
                                "cc": "", "subject": "=?UTF-8?B?...?=" },
                    "body": { "html": "<p>본문</p>", "plain": "본문" },
                    "fileList": []
                },
                "decodeMime": { "subject": "제목", "to": "홍길동 &lt;hong@example.com&gt;" }
            }
        })
    }

    /// ⭐ **라이브에서 터진 그 회귀.** 최상위 `decodeMime`은 실재하지 않으므로, 거기에만 값이 있는
    /// 응답은 "제목을 못 찾았다"로 **거부돼야 한다**. 만약 코드가 다시 최상위를 보게 되면 이
    /// 테스트가 통과해버리므로, 반대로 **정본(`mailInfo.decodeMime`)만 있는 응답이 성공**하는 것도
    /// 함께 못박는다. 둘을 같이 걸어야 "어느 자리가 정본인지"가 테스트로 고정된다.
    #[test]
    fn 최상위_decode_mime은_정본이_아니다() {
        // 정본 자리에만 값이 있는 응답 = 실측 모양 → 성공
        let ok = plan_draft_send(DraftExists, &sample_draft_init(), "1", "").unwrap();
        assert_eq!(ok.subject, "제목");
        assert_eq!(ok.to, "홍길동 <hong@example.com>");

        // 값을 최상위로 옮긴 응답 = 실재하지 않는 모양 → 제목을 못 찾아 거부
        let mut top = sample_draft_init();
        let dm = top["mailInfo"].as_object_mut().unwrap().remove("decodeMime").unwrap();
        top["decodeMime"] = dm;
        let Err(e) = plan_draft_send(DraftExists, &top, "1", "") else {
            panic!("최상위 decodeMime을 정본으로 읽고 있다 — 라이브에서 막힌 그 경로다");
        };
        assert!(format!("{e:#}").contains("제목"), "{e:#}");

        // 수신자도 같은 자리에서 온다 — 정본에서 빼면 '수신자 없음'으로 떨어진다.
        let mut no_to = sample_draft_init();
        no_to["mailInfo"]["decodeMime"].as_object_mut().unwrap().remove("to");
        let Err(e) = plan_draft_send(DraftExists, &no_to, "1", "") else {
            panic!("수신자 없는 초안을 그대로 보내려 한다");
        };
        assert!(format!("{e:#}").contains("수신자"), "{e:#}");
    }

    /// 최상위 `paramTo`는 **수신자 출처가 아니다** — 우리가 보낸 `mailTo`의 메아리라 어떤 초안에서도
    /// `"(Unknown)"`이다. 대안 자리로 되살리면 이 테스트가 깨진다.
    #[test]
    fn param_to는_수신자_출처가_아니다() {
        let init = json!({ "paramTo": "hong@example.com" });
        assert_eq!(draft_recipient(&init, "1").unwrap(), "", "paramTo를 수신자로 읽고 있다");
    }

    /// 실재 확인 — 목록에 없으면 발송하지 않는다. 반환값이 `plan_draft_send`의 입력이라
    /// **이 확인을 건너뛰면 컴파일되지 않는다**(그게 이 타입의 목적이다).
    #[test]
    fn 실재하지_않는_초안은_발송_계획을_세울_수_없다() {
        let drafts = json!({ "Records": [{ "muid": "13542607" }] });
        assert!(ensure_draft_exists(&drafts, "13542607").is_ok());
        assert!(ensure_draft_exists(&drafts, "99999999").is_err());
        assert!(ensure_draft_exists(&json!({}), "13542607").is_err()); // 목록을 못 읽으면 미검증
    }

    /// **발송 판정 전체**의 회귀 기준선. 가드를 지워도 순수 헬퍼 테스트는 통과할 수 있으므로,
    /// 실제로 판정을 조립하는 이 함수를 캡처 JSON으로 직접 검사한다.
    #[test]
    fn 발송_계획은_보낼_것을_확정하거나_이유를_대고_거부한다() {
        let plan = plan_draft_send(DraftExists, &sample_draft_init(), "1", "").unwrap();
        assert_eq!(plan.to, "홍길동 <hong@example.com>");
        assert_eq!(plan.subject, "제목");
        assert_eq!(plan.html, "<p>본문</p>");
        assert!(plan.files.is_empty());
        assert!(plan.mime_header.contains("mime-version"), "mimeHeader는 헤더 객체 JSON이다");

        // to 인자는 초안 수신자를 덮어쓴다.
        let over = plan_draft_send(DraftExists, &sample_draft_init(), "1", " x@y.z ").unwrap();
        assert_eq!(over.to, "x@y.z");

        // 첨부가 있으면 계획에 그대로 실린다(승계는 호출부가 한다).
        let mut with_att = sample_draft_init();
        with_att["mailInfo"]["mime"]["fileList"] =
            json!([{ "originalFileName": "a", "fileExtsn": "txt", "fileSn": "SN" }]);
        assert_eq!(plan_draft_send(DraftExists, &with_att, "1", "").unwrap().files.len(), 1);

        // ⛔ 아래는 전부 **보내면 안 되는** 초안이다. 하나라도 통과하면 잘못된 메일이 나간다.
        let mut no_files = sample_draft_init();
        no_files["mailInfo"]["mime"].as_object_mut().unwrap().remove("fileList");
        assert!(plan_draft_send(DraftExists, &no_files, "1", "").is_err(), "첨부 판정 불가");

        let mut no_body = sample_draft_init();
        no_body["mailInfo"]["mime"]["body"] = json!({ "plain": "본문" });
        assert!(plan_draft_send(DraftExists, &no_body, "1", "").is_err(), "빈 본문");

        let mut blank_body = sample_draft_init();
        blank_body["mailInfo"]["mime"]["body"]["html"] = json!("   ");
        assert!(plan_draft_send(DraftExists, &blank_body, "1", "").is_err(), "공백뿐인 본문");

        let mut no_subject = sample_draft_init();
        no_subject["mailInfo"]["decodeMime"]["subject"] = json!("");
        assert!(plan_draft_send(DraftExists, &no_subject, "1", "").is_err(), "빈 제목");

        let mut with_cc = sample_draft_init();
        with_cc["mailInfo"]["mime"]["header"]["cc"] = json!("누군가 <x@y.z>");
        assert!(plan_draft_send(DraftExists, &with_cc, "1", "").is_err(), "참조 누락");

        let mut no_cc_key = sample_draft_init();
        no_cc_key["mailInfo"]["mime"]["header"].as_object_mut().unwrap().remove("cc");
        assert!(plan_draft_send(DraftExists, &no_cc_key, "1", "").is_err(), "참조 판정 불가");

        let mut no_to = sample_draft_init();
        no_to["mailInfo"]["decodeMime"].as_object_mut().unwrap().remove("to");
        assert!(plan_draft_send(DraftExists, &no_to, "1", "").is_err(), "수신자 없음");
    }

    /// A08 응답이 요청과 개수가 다르면 **일부를 빠뜨린 채 보내게 되므로** 중단한다.
    #[test]
    fn 승계는_a08_응답_개수가_어긋나면_중단한다() {
        let want = AttachmentPlan(vec![
            ("a".to_string(), "txt".to_string(), "SN-A".to_string()),
            ("b".to_string(), "txt".to_string(), "SN-B".to_string()),
        ]);
        let one = vec![json!({ "originalFileName": "a", "fileExtsn": "txt",
                               "fileSize": "66", "fileId": "FID-A" })];
        assert!(build_inherited_fields(&want, &one).is_err(), "2개 요청에 1개 응답");

        let two = vec![
            json!({ "originalFileName": "a", "fileExtsn": "txt", "fileSize": "66", "fileId": "FID-A" }),
            json!({ "originalFileName": "b", "fileExtsn": "txt", "fileSize": "84", "fileId": "FID-B" }),
        ];
        let (uid, cnt, fw) = build_inherited_fields(&want, &two).unwrap();
        assert_eq!(cnt, "2");
        assert_eq!(fw, "a,b", "fwFile은 파일명을 콤마로 잇는다");
        assert_eq!(uid.matches("\"fileId\"").count(), 2);

        // ⚠️ **요청보다 많이 와도 막는다** — 요청한 이름이 전부 들어 있어도 우리가 부탁하지 않은
        // 파일이 섞여 온 것이라 그대로 보낼 수 없다(이름 짝짓기만으로는 이 경우를 못 잡는다).
        let mut three = two.clone();
        three.push(json!({ "originalFileName": "c", "fileExtsn": "txt",
                           "fileSize": "10", "fileId": "FID-C" }));
        assert!(build_inherited_fields(&want, &three).is_err(), "3개 응답에 2개 요청");
    }

    /// 참조 없는 초안의 실측 모양 — `mailInfo.mime.header.cc`가 **빈 문자열로 실려 온다**
    /// (그래서 "읽을 수 있다"). `bcc` 키는 어디에도 없다.
    fn init_without_cc() -> Value {
        json!({ "mailInfo": { "mime": { "header": { "to": "", "cc": "", "subject": "=?UTF-8?B?...?=" } } } })
    }

    /// 초안의 참조는 **승계한다**(실측 `analyze/15`: 초안이 cc/bcc를 저장하고 되읽기가 돌려준다).
    #[test]
    fn 초안의_참조를_승계한다() {
        // 참조 없는 초안 → 빈 값 두 개. 거부가 아니다.
        assert_eq!(draft_carbon_copy(&init_without_cc(), "1").unwrap(), (String::new(), String::new()));

        // 정본은 decodeMime — 엔티티를 풀어 싣는다. 되읽기 구분자(`, `)를 건드리지 않는다.
        let init = json!({ "mailInfo": {
            "mime": { "header": { "cc": "a@x.com,\r\n b@x.com", "bcc": "c@x.com" } },
            "decodeMime": { "cc": "홍길동 &lt;a@x.com&gt;, b@x.com", "bcc": "c@x.com" } } });
        assert_eq!(
            draft_carbon_copy(&init, "1").unwrap(),
            ("홍길동 <a@x.com>, b@x.com".to_string(), "c@x.com".to_string()),
            "MIME 인코딩·줄접기가 남은 header 쪽을 쓰면 안 된다"
        );

        // ⚠️ 비대칭을 못박는다: `cc` 자리가 아예 없으면 판정 불가 → 중단.
        assert!(draft_carbon_copy(&json!({}), "1").is_err());
        assert!(
            draft_carbon_copy(&json!({ "mailInfo": { "mime": { "header": { "to": "" } } } }), "1")
                .is_err()
        );
        // 그러나 `bcc` 키의 부재는 "숨은참조 없음"이다 — 참조 없는 정상 초안의 실측 모양이
        // 그렇다(`init_without_cc`에 bcc 키가 없다). 여기를 cc와 같은 기준으로 만들면
        // **참조 없는 초안이 전부 발송 거부된다.**
        assert!(init_without_cc().pointer("/mailInfo/mime/header/bcc").is_none());
        assert_eq!(draft_carbon_copy(&init_without_cc(), "1").unwrap().1, "");

        // header 에는 참조가 있는데 디코드된 자리에 없으면 승계 경로가 없다 → 중단(조용히 빠뜨리지 않는다).
        assert!(
            draft_carbon_copy(&json!({ "mailInfo": { "mime": { "header": { "cc": "x@y.z" } },
                                                     "decodeMime": { "cc": "" } } }), "1").is_err()
        );
    }

    /// 폼에 실린 참조가 그대로 나가는지 — 값을 해석·재조립하지 않는다(콤마 구분 문자열 통째로).
    #[test]
    fn 발송_폼은_참조를_받은_그대로_싣는다() {
        let cf = ComposeForm::new(&sample_init(), "to", "제목", "본문", String::new(), "0".into())
            .with_carbon_copy(" a@x.com,b@x.com ", "c@x.com");
        let f = fields_of(&cf);
        assert_eq!(f["cc"], "a@x.com,b@x.com", "바깥 공백만 다듬고 구분자는 그대로");
        assert_eq!(f["bcc"], "c@x.com");

        // 참조를 싣지 않으면 빈 값이다(신규 발송의 기본).
        let plain = ComposeForm::new(&sample_init(), "to", "제목", "본문", String::new(), "0".into());
        let p = fields_of(&plain);
        assert_eq!(p["cc"], "");
        assert_eq!(p["bcc"], "");
    }

    /// 수신자 복원 — 브라우저가 싣는 값과 같은 형태를 내야 하고, 여럿이어도 통째로 옮겨야 한다.
    #[test]
    fn 초안_수신자는_엔티티만_풀고_주소를_보존한다() {
        let init = json!({
            "mailInfo": {
                "mime": { "header": { "to": "=?UTF-8?B?7J207J6s7ZWZ?= &lt;a@b.c&gt;" } },
                "decodeMime": { "to": "홍길동 &lt;a@b.c&gt;" }
            },
            "paramTo": "(Unknown)"
        });
        assert_eq!(
            draft_recipient(&init, "1").unwrap(),
            "홍길동 <a@b.c>",
            "MIME 인코딩된 header.to를 쓰면 안 된다"
        );

        // ⛔ **본문 렌더러(html_to_text)를 쓰면 안 되는 이유** — 서버가 이미 언이스케이프된 값을
        // 주면 그쪽은 `<a@b.c>`를 태그로 보고 통째로 버려 `"홍길동 "`만 남는다(공백이라 빈 값
        // 검사도 통과한다). 여기서 그 입력을 못박는다.
        let plain = json!({ "mailInfo": { "decodeMime": { "to": "홍길동 <a@b.c>" } } });
        assert_eq!(draft_recipient(&plain, "1").unwrap(), "홍길동 <a@b.c>");

        // 로컬파트가 블록태그로 시작하는 주소(`<p.kim@x.co>`)는 html_to_text에서 **개행으로
        // 치환**된다 — 그 손실도 일어나면 안 된다.
        for addr in ["p.kim@x.co", "br.lee@x.co", "div.han@x.co", "td.oh@x.co", "h1.no@x.co"] {
            let v = json!({ "mailInfo": { "decodeMime": { "to": format!("아무개 <{addr}>") } } });
            assert_eq!(draft_recipient(&v, "1").unwrap(), format!("아무개 <{addr}>"));
        }

        // 여러 명이어도 **문자열을 통째로** 옮긴다 — 쪼개거나 다시 잇지 않는다(구분자 무관).
        let many = json!({ "mailInfo": { "decodeMime": { "to": "가 &lt;a@x.y&gt;, 나 &lt;b@x.y&gt;" } } });
        assert_eq!(draft_recipient(&many, "1").unwrap(), "가 <a@x.y>, 나 <b@x.y>");

        // 수신자 없는 초안은 이 키가 아예 없다 — 빈 값으로 떨어지고 호출부가 `to`를 요구한다.
        assert_eq!(draft_recipient(&json!({ "mailInfo": { "decodeMime": {} } }), "1").unwrap(), "");
        assert_eq!(draft_recipient(&json!({}), "1").unwrap(), "");

        // 값이 있는데 주소로 보이지 않으면(변환이 주소를 먹었을 때) 조용히 통과시키지 않는다.
        assert!(draft_recipient(&json!({ "mailInfo": { "decodeMime": { "to": "홍길동 " } } }), "1").is_err());
    }

    /// 엔티티 언이스케이프는 `&amp;`를 마지막에 푼다 — `&amp;lt;`가 `<`로 접히면 안 된다.
    #[test]
    fn 엔티티_언이스케이프는_태그를_지우지_않는다() {
        assert_eq!(unescape_entities("가 &lt;a@b.c&gt;"), "가 <a@b.c>");
        assert_eq!(unescape_entities("<b>가</b> <a@b.c>"), "<b>가</b> <a@b.c>");
        assert_eq!(unescape_entities("a&amp;lt;b"), "a&lt;b");
    }

    fn draft_file(orig: &str, ext: &str, sn: &str) -> Value {
        json!({ "originalFileName": orig, "fileSize": "66", "fileExtsn": ext, "fileSn": sn })
    }

    /// 첨부 승계가 **A08을 부르기 전에** 막아야 하는 것들. 전부 "조용히 잘못 나가는" 경우다.
    #[test]
    fn 첨부_승계는_승계할_수_없는_모양을_먼저_막는다() {
        // 정상: 이름이 다르면 통과하고 (이름, 확장자, fileSn) 그대로 계획이 선다.
        let ok = draft_attachment_plan(
            &json!({}),
            "1",
            &[draft_file("a", "txt", "SN-A"), draft_file("b", "txt", "SN-B")],
        )
        .expect("정상 첨부는 통과해야 한다");
        assert_eq!(
            ok.0,
            vec![
                ("a".into(), "txt".into(), "SN-A".into()),
                ("b".into(), "txt".into(), "SN-B".into())
            ]
        );

        // 파일명에 콤마 — fwFile이 콤마 구분이라 이름이 쪼개진다.
        assert!(draft_attachment_plan(&json!({}), "1", &[draft_file("a,b", "txt", "SN")]).is_err());
        // 동명 파일 — A08 응답을 이름으로 짝짓는데 짝을 확정할 수 없다.
        assert!(
            draft_attachment_plan(&json!({}), "1", &[draft_file("a", "txt", "S1"), draft_file("a", "txt", "S2")])
                .is_err()
        );
        // 확장자만 다르면 동명이 아니다(짝지을 수 있다).
        assert!(
            draft_attachment_plan(&json!({}), "1", &[draft_file("a", "txt", "S1"), draft_file("a", "pdf", "S2")])
                .is_ok()
        );
        // 대용량 첨부(bigFile)를 들고 있으면 승계 불가 — 우리 폼은 그 값을 빈 값으로 보낸다.
        for key in ["bigFile", "bigFilePeriod"] {
            let init = json!({ "mailInfo": { "mime": { key: "http://x/y" } } });
            assert!(
                draft_attachment_plan(&init, "1", &[draft_file("a", "txt", "SN")]).is_err(),
                "{key}가 있으면 막아야 한다"
            );
        }
        // fileSn·이름이 비면 승계 불가.
        assert!(draft_attachment_plan(&json!({}), "1", &[draft_file("a", "txt", "")]).is_err());
        assert!(draft_attachment_plan(&json!({}), "1", &[draft_file("", "txt", "SN")]).is_err());
    }

    /// 대용량 첨부 감지 — 우리 폼은 `bigFile`/`bigFilePeriod`를 빈 값으로 보내므로,
    /// 초안이 값을 들고 있으면 그 부분이 빠진 채 나간다.
    #[test]
    fn 대용량_첨부_키는_중첩돼_있어도_찾는다() {
        let init = json!({ "mailInfo": { "mime": { "bigFile": "http://x/y" } } });
        assert!(has_nonempty_key(&init, "bigFile"));
        // 빈 값·빈 배열·null은 "없음"이다(정상 초안이 이 값들을 빈 채로 들고 온다).
        assert!(!has_nonempty_key(&json!({ "bigFile": "" }), "bigFile"));
        assert!(!has_nonempty_key(&json!({ "a": { "bigFile": [] } }), "bigFile"));
        assert!(!has_nonempty_key(&json!({ "bigFile": null }), "bigFile"));
        // 키 이름은 **정확히** 일치해야 한다 — bigFileDay(정상 폼 값)에 걸리면 안 된다.
        assert!(!has_nonempty_key(&json!({ "bigFileDay": "30" }), "bigFile"));
    }

    /// 승계 항목은 **A08 응답 객체를 그대로 두고** 브라우저가 덧붙이는 것만 더한다(실측).
    /// 신규 업로드(`upload_files`)가 만드는 항목과 키 구성이 달라 공유하지 않는다.
    #[test]
    fn 승계_항목은_a08_응답에_실측_키를_덧붙인다() {
        let list = vec![
            json!({ "originalFileName": "b", "fileExtsn": "txt", "fileSize": "84",
                    "fileId": "FID-B", "fileSn": "", "encoding": "base64", "offset": "1940",
                    "muid": "13544740", "authKeyMap": { "muid": "13544740" } }),
            json!({ "originalFileName": "a", "fileExtsn": "txt", "fileSize": "66",
                    "fileId": "FID-A", "fileSn": "", "encoding": "base64", "offset": "0",
                    "muid": "13544740", "authKeyMap": { "muid": "13544740" } }),
        ];
        // 응답이 요청과 **다른 순서**로 와도 이름으로 짝지어야 한다(순서 보장이 미실측이다).
        let it = build_inherited_item(&list, 0, "a", "txt", "SN-A").unwrap();
        assert_eq!(it["fileId"], "FID-A");
        assert_eq!(it["encoding"], "base64", "A08 응답 키는 그대로 남아야 한다");
        assert_eq!(it["fileSize"], "66 Bytes", "'<n> Bytes' 문자열로 바꾼다");
        assert_eq!(it["noConvertFileSize"], 66, "정수로도 함께 싣는다");
        assert_eq!(it["fileSn"], "SN-A", "A08의 빈 fileSn 대신 초안 토큰을 되살린다");
        assert_eq!(
            it["authKeyMap"], r#"{"muid":"13544740"}"#,
            "authKeyMap은 객체가 아니라 JSON 문자열로 한 번 더 감싼다"
        );
        assert_eq!(it["fileClass"], "icon_txt");
        assert_eq!(it["link"], "N");
        assert_eq!(it["fileDeleteYN"], "Y");
        assert_eq!(it["serverFile"], "Y");
        assert_eq!(it["useDownView"], "N");
        assert_eq!(it["id"], 0);

        // 이름이 없거나 fileId가 비면 보내지 않는다.
        assert!(build_inherited_item(&list, 0, "없는파일", "txt", "SN").is_err());
        let no_id = vec![json!({ "originalFileName": "a", "fileExtsn": "txt", "fileId": "" })];
        assert!(build_inherited_item(&no_id, 0, "a", "txt", "SN").is_err());
    }

    /// 첨부를 승계하면 `fwFile`이 **콤마로 이어진 파일명 목록**이 된다(신규 발송은 항상 빈 값).
    #[test]
    fn 첨부_승계는_fw_file에_파일명을_콤마로_잇는다() {
        let base = ComposeForm::new(&sample_init(), "to", "제목", "본문", String::new(), "0".into());
        assert_eq!(fields_of(&base)["fwFile"], "", "신규 발송은 빈 값이다");

        let cf = ComposeForm::new(&sample_init(), "to", "제목", "본문", "[]".into(), "2".into())
            .with_draft("13544740", "{}".into())
            .with_fw_file("creed-test-a,creed-test-b".into());
        let f = fields_of(&cf);
        assert_eq!(f["fwFile"], "creed-test-a,creed-test-b");
        assert_eq!(f["bigFileCnt"], "2");
        assert_eq!(f["uidAuthList"], "[]");
    }

    #[test]
    fn 임시저장은_발송_폼에_전용필드_7개만_더한다() {
        let base: Vec<&str> = ComposeForm::new(&json!({}), "to", "s", "", String::new(), "0".into())
            .fields(&GwClient::new(None))
            .iter()
            .map(|(k, _)| *k)
            .collect();
        let extra: Vec<&str> = DRAFT_FIELDS.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            extra,
            vec![
                "autoMUID",
                "beforeMailType",
                "beforeMUID",
                "mailKey",
                "isFirst",
                "draftType",
                "autoDraftType"
            ]
        );
        // 발송 폼과 겹치는 이름이 없어야 한다 — 겹치면 같은 필드를 두 번 보내게 된다.
        assert!(extra.iter().all(|k| !base.contains(k)));

        let v: std::collections::HashMap<&str, &str> = DRAFT_FIELDS.iter().copied().collect();
        assert_eq!(v["isFirst"], "0", "첫 저장은 '0'이다(프론트 삼항이 뒤집혀 있다)");
        assert_eq!(v["draftType"], "true");
        assert_eq!(v["autoDraftType"], "false");
        // compose_init이 여는 모드와 같이 움직인다(`mailKind`도 "plain").
        // 브라우저는 여기에 문자열 "undefined"를 싣지만 그 버그는 재현하지 않는다 — DRAFT_FIELDS 주석 참조.
        assert_eq!(v["beforeMailType"], "plain");
        // 신규 저장이므로 직전 draft 좌표는 전부 빈 값
        assert!(["autoMUID", "beforeMUID", "mailKey"]
            .iter()
            .all(|k| v[k].is_empty()));
    }
}
