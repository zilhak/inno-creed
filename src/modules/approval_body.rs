//! 근태 양식 본문(`doc_contents`) HTML 조립.
//!
//! ⭐ **아마란스는 본문 HTML을 만들어주지 않는다.** 근태폼 화면(KISS HPD0110)이 표를
//! 조립해 `doc_contents`로 실어 보내면 서버는 그대로 저장하고, 문서 뷰는 그 저장본을
//! `<td id="divFormContents">`에 그대로 끼워 보여줄 뿐이다. 실측(2026-10-06):
//!
//! - `eap110A03` → `basicHtml`: 양식 껍데기만(품의번호/수신참조/제목 + 본문 빈 자리)
//! - `eap110A03` → `outProcessForm.outContents`: 살아있는 문서로 불러도 **길이 0**
//! - `/human/attendapplication/interlock/getInterlockFormContents`: 응답은
//!   `contents`(=bindData JSON)·`title` 둘뿐 — **HTML 없음**
//!
//! 그래서 MCP도 브라우저와 같은 일을 해야 한다. 한 줄짜리 본문을 보내면 문서가 실제로
//! 그 한 줄만 담은 채 상신된다(v2.3.4까지의 동작 — 연차 문서가 그렇게 나가 반려됐다).
//!
//! 템플릿은 **브라우저 성공본 `doc_contents`에서 값만 `{{key}}`로 뺀 것**이다. 추출·검증
//! 스크립트는 `.claude-workspace/body-template/`(extract.py → fixup.py → verify.py)에 있고,
//! 캡처 8건(연차 4변형·기타휴가·출장·휴일근무·휴직) 전부 **왕복 바이트 일치**를 확인했다.
//! 템플릿을 손으로 고치지 말 것 — 캡처를 다시 뜬 뒤 그 스크립트로 재생성한다.

use anyhow::{anyhow, Result};
use serde_json::Value;

const ROW_OPEN: &str = "<!--ROW-->";
const ROW_CLOSE: &str = "<!--/ROW-->";
const FIRST_OPEN: &str = "<!--FIRST-->";
const FIRST_CLOSE: &str = "<!--/FIRST-->";

/// 본문을 조립할 수 있는 양식인지. 근태 5양식만 템플릿을 갖는다.
pub fn supports(form_id: i64) -> bool {
    template(form_id).is_some()
}

fn template(form_id: i64) -> Option<&'static str> {
    Some(match form_id {
        36 => include_str!("../data/body_templates/36.html"), // 연차휴가신청서
        38 => include_str!("../data/body_templates/38.html"), // 기타휴가신청서
        40 => include_str!("../data/body_templates/40.html"), // 출장신청서
        43 => include_str!("../data/body_templates/43.html"), // 휴일/주말근무신청서
        67 => include_str!("../data/body_templates/67.html"), // 휴직신청서
        _ => return None,
    })
}

/// `bindData`(ITEMS + TABLE.dbTable1)로 그 양식의 본문 HTML을 만든다.
///
/// 값 조회 우선순위는 행 안에서 [현재 행 → 그룹 → ITEMS], 행 밖에서 [그룹 → 첫 행 → ITEMS].
/// 양식마다 같은 뜻의 값이 다른 층에 들어있어(연차는 그룹=사람·행=신청내역, 출장은
/// 그룹 자체가 행) 층을 훑는 쪽이 양식별 분기보다 단순하다.
pub fn render(form_id: i64, bind: &Value) -> Result<String> {
    let tpl = template(form_id).ok_or_else(|| anyhow!("본문 템플릿이 없는 양식이다: form_id={form_id}"))?;
    let empty = Value::Object(Default::default());
    let items = bind.get("ITEMS").unwrap_or(&empty);
    let group = bind
        .pointer("/TABLE/dbTable1/group")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("bindData에 TABLE.dbTable1.group이 없다"))?;
    let head = group.first().ok_or_else(|| anyhow!("TABLE.dbTable1.group이 비어 있다"))?;

    // 중첩 group이 있으면 그쪽이 행(연차·기타휴가·휴일근무), 없으면 group 자체가 행(출장·휴직).
    let nested = head.get("group").and_then(|v| v.as_array()).filter(|a| !a.is_empty());
    let hdr = head.get("items").unwrap_or(&empty);
    let rows: Vec<&Value> = match nested {
        Some(a) => a.iter().map(|r| r.get("items").unwrap_or(&empty)).collect(),
        None => group.iter().map(|r| r.get("items").unwrap_or(&empty)).collect(),
    };
    if rows.is_empty() {
        return Err(anyhow!("bindData에 신청 내역 행이 없다"));
    }

    let (before, block, after) = split_row_block(tpl)?;
    let span = (rows.len() + 1).to_string();
    let mut body = String::with_capacity(tpl.len() + block.len() * rows.len());
    body.push_str(before);
    for (i, row) in rows.iter().enumerate() {
        let seg = strip_first_block(block, i == 0);
        let seg = seg
            .replace("{{__index0__}}", &i.to_string())
            .replace("{{__index1__}}", &(i + 1).to_string())
            .replace("{{__span__}}", &span);
        body.push_str(&fill(&seg, &[row, hdr, items]));
    }
    body.push_str(after);
    Ok(fill(&body, &[hdr, rows[0], items]))
}

/// `<!--ROW-->` 블록을 앞/본체/뒤로 가른다.
fn split_row_block(tpl: &str) -> Result<(&str, &str, &str)> {
    let s = tpl.find(ROW_OPEN).ok_or_else(|| anyhow!("템플릿에 ROW 마커가 없다"))?;
    let e = tpl.find(ROW_CLOSE).ok_or_else(|| anyhow!("템플릿에 ROW 종료 마커가 없다"))?;
    Ok((&tpl[..s], &tpl[s + ROW_OPEN.len()..e], &tpl[e + ROW_CLOSE.len()..]))
}

/// `<!--FIRST-->` 블록(rowspan을 가진 숨김 셀)은 첫 행에만 남긴다.
fn strip_first_block(seg: &str, keep: bool) -> String {
    let (Some(s), Some(e)) = (seg.find(FIRST_OPEN), seg.find(FIRST_CLOSE)) else {
        return seg.to_string();
    };
    let inner = &seg[s + FIRST_OPEN.len()..e];
    let tail = &seg[e + FIRST_CLOSE.len()..];
    if keep {
        format!("{}{}{}", &seg[..s], inner, tail)
    } else {
        format!("{}{}", &seg[..s], tail)
    }
}

/// `{{key}}`를 소스에서 찾아 채운다. 어디에도 없거나 null이면 빈 문자열.
fn fill(tpl: &str, srcs: &[&Value]) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(s) = rest.find("{{") {
        let Some(e) = rest[s..].find("}}") else { break };
        let key = &rest[s + 2..s + e];
        out.push_str(&rest[..s]);
        out.push_str(&lookup(key, srcs));
        rest = &rest[s + e + 2..];
    }
    out.push_str(rest);
    out
}

fn lookup(key: &str, srcs: &[&Value]) -> String {
    for src in srcs {
        match src.get(key) {
            Some(Value::Null) | None => continue,
            Some(v) => return escape(&as_text(v)),
        }
    }
    String::new()
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 값에만 적용한다(템플릿의 `&nbsp;` 등은 건드리지 않는다).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn annual_bind() -> Value {
        json!({
            "ITEMS": {"appYear": "2026", "appMonth": "10", "appDay": "02"},
            "TABLE": {"dbTable1": {"group": [{
                "items": {"empNm": "이재학", "deptNm": "네이티브 플랫폼팀", "positionNm": "책임연구원",
                          "totalCnt": "14.0", "usedCnt": "8.625", "unusedCnt": "5.375",
                          "progressCnt": "1.0", "minusCnt": "0.5", "listNum": 0},
                "group": [{"items": {"atCdNm": "오전반차", "startDt": "2026-10-02", "endDt": "2026-10-02",
                                     "appDy": "1", "startTm": "09:00", "endTm": "13:00",
                                     "appTmMinute": "04시간", "ycUseCnt": "0.5",
                                     "appRmkDc": "개인 사유"}}]
            }]}}
        })
    }

    #[test]
    fn 연차_본문에_표와_값이_들어간다() {
        let h = render(36, &annual_bind()).unwrap();
        for must in ["휴가신청정보", "오전반차", "2026-10-02", "09:00~13:00", "04시간",
                     "개인 사유", "위의 사유로 연차 휴가원을 제출합니다.", "2026년 10월 02일"] {
            assert!(h.contains(must), "본문에 {must}가 없다");
        }
        // 요약줄은 그룹 items에서 와야 한다(행의 ycUseCnt가 아니라 minusCnt).
        assert!(h.contains("잔여연차 : 5.375"), "잔여연차가 unusedCnt로 채워져야 한다");
        assert!(h.contains("[네이티브 플랫폼팀&nbsp;이재학&nbsp;책임연구원]"));
        assert!(!h.contains("{{"), "치환되지 않은 placeholder가 남았다");
    }

    #[test]
    fn 시작일자와_종료일자가_각자_자리에_간다() {
        // 같은 값이면 구분이 안 되므로 서로 다른 날짜로 확인한다.
        let mut b = annual_bind();
        b["TABLE"]["dbTable1"]["group"][0]["group"][0]["items"]["endDt"] = json!("2026-10-09");
        let h = render(36, &b).unwrap();
        assert!(h.contains("2026-10-02") && h.contains("2026-10-09"));
    }

    #[test]
    fn 행이_여러개면_그만큼_반복되고_rowspan이_따라간다() {
        let mut b = annual_bind();
        let row = b["TABLE"]["dbTable1"]["group"][0]["group"][0].clone();
        b["TABLE"]["dbTable1"]["group"][0]["group"].as_array_mut().unwrap().push(row);
        let h = render(36, &b).unwrap();
        assert_eq!(h.matches("04시간").count(), 2, "행이 두 번 나와야 한다");
        assert!(h.contains(r#"rowspan="3""#), "rowspan은 행수+1이어야 한다");
        // 숨김 셀은 첫 행에만 — rowspan 셀이 행마다 중복되면 안 된다.
        assert_eq!(h.matches("rowspan=").count(), 1);
    }

    #[test]
    fn 출장과_휴직은_중첩없는_group을_행으로_읽는다() {
        let b = json!({
            "ITEMS": {"appYear": "2026", "appMonth": "10", "appDay": "02"},
            "TABLE": {"dbTable1": {"group": [{"items": {
                "atCdNm": "국내출장", "atDt": "2026-10-02", "dayOfWeek": "금",
                "startTm": "09:00", "endTm": "18:00", "appTm": "08시간",
                "biztrDc": "고객사", "taskDc": "현장 점검", "trafFg": "기차"}}]}}
        });
        let h = render(40, &b).unwrap();
        assert!(h.contains("국내출장") && h.contains("고객사") && h.contains("기차"));
        assert!(!h.contains("{{"));
    }

    #[test]
    fn 값의_특수문자는_이스케이프된다() {
        let mut b = annual_bind();
        b["TABLE"]["dbTable1"]["group"][0]["group"][0]["items"]["appRmkDc"] = json!("<b>a&b</b>");
        let h = render(36, &b).unwrap();
        assert!(h.contains("&lt;b&gt;a&amp;b&lt;/b&gt;"), "값이 태그로 해석되면 안 된다");
    }

    #[test]
    fn 템플릿이_없는_양식은_거부한다() {
        assert!(!supports(99));
        assert!(render(99, &annual_bind()).is_err());
    }

    #[test]
    fn 다섯_양식_모두_템플릿을_갖는다() {
        for f in [36, 38, 40, 43, 67] {
            assert!(supports(f), "form {f} 템플릿 없음");
        }
    }
}

/// 캡처본(브라우저 성공 payload) 대조 — 템플릿이 실제 상신본을 그대로 재현하는지 본다.
/// 캡처는 git에 올리지 않으므로(개인 근태값 포함) 파일이 없으면 조용히 건너뛴다.
#[cfg(test)]
mod capture_tests {
    use super::*;
    use std::path::Path;

    const CAP: &str = ".claude-workspace/approval-analysis/captures";

    fn percent_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn 캡처된_브라우저_본문과_바이트가_같다() {
        let cases = [
            (36, "annual-half-am"), (36, "annual-half-pm"),
            (36, "annual-hourly"), (36, "annual-quarter"),
            (38, "etc-leave"), (40, "business-trip"),
            (43, "holiday-work"), (67, "leave-of-absence"),
        ];
        let mut checked = 0;
        for (form_id, slug) in cases {
            let p = format!("{CAP}/{slug}_eap110A06.json");
            if !Path::new(&p).exists() {
                continue;
            }
            let j: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let pi = &j["paramItem"];
            let expect = percent_decode(pi["doc_contents"].as_str().unwrap());
            // bindData는 이중인코딩(JSON 문자열 안에 JSON 문자열)
            let bind: Value = {
                let v: Value = serde_json::from_str(pi["bindData"].as_str().unwrap()).unwrap();
                match v {
                    Value::String(inner) => serde_json::from_str(&inner).unwrap(),
                    other => other,
                }
            };
            let got = render(form_id, &bind).unwrap();
            assert_eq!(got, expect, "{slug}(form {form_id}) 본문이 캡처와 다르다");
            checked += 1;
        }
        eprintln!("캡처 대조 {checked}건");
    }
}
