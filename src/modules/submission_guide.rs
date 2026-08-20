//! 양식별 신청 가이드(문서 본문에 채울 내용 + 상신 절차) 조회 — 번들 데이터.
//! 데이터: `src/data/submission_guides.json`.
//! ⚠️ 결재라인(누가 결재)과 별개. 본문 입력·상신은 아직 MCP 자동화 불가라, 사람이 손으로 작성하도록 안내하는 용도.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// `approval_submit`의 신원 주입 회귀 테스트가 **실제 번들 payload**로 훑기 위해 참조한다.
pub(crate) const BUNDLED: &str = include_str!("../data/submission_guides.json");

fn bundled() -> Value {
    serde_json::from_str(BUNDLED).expect("번들 신청 가이드 JSON 파싱 실패")
}

/// 특정 양식의 신청 가이드(본문 필수항목/절차/주의/결재라인 힌트).
pub fn get_guide(doc_type: &str) -> Result<Value> {
    let b = bundled();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 가이드에 forms 없음"))?;

    let (name, guide) = find_form(forms, doc_type).ok_or_else(|| {
        anyhow!("'{doc_type}' 신청 가이드 없음. list_approval_submission_guides로 목록 확인")
    })?;

    Ok(json!({
        "docType": name,
        "version": b.get("version").cloned().unwrap_or(Value::Null),
        "source": b.get("source").cloned().unwrap_or(Value::Null),
        "mcpStatus": b.get("mcpStatus").cloned().unwrap_or(Value::Null),
        "guide": guide,
        "note": "draftHelp = submit_approval 기안 데이터 채우는 법(--help). fixed(고정코드 그대로)·fill(의미별 채울 필드)·hpApplicationExample/bindDataExample(복사 후 fill 필드만 교체 — 신원은 submit_approval이 자동 주입). ⭐ 제목(doc_title)은 draftHelp.defaultDocTitle(아마란스 기본 표시 템플릿)을 참고하되 placeholder를 그대로 상신하지 말고 상신 전 사용자에게 확인할 것(draftHelp.titleHelp). 결재라인은 get_approval_line_schema + save_approval_line로 준비. requiredBody/steps는 아마란스 웹 직접작성용 참고."
    }))
}

/// 수록된 신청 가이드 목록(양식명/form_id/alias).
pub fn list_guides() -> Result<Value> {
    let b = bundled();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 가이드에 forms 없음"))?;
    let list: Vec<Value> = forms
        .iter()
        .map(|(name, v)| {
            json!({
                "docType": name,
                "formId": v.get("form_id").cloned().unwrap_or(Value::Null),
                "aliases": v.get("aliases").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();
    Ok(json!({
        "version": b.get("version").cloned().unwrap_or(Value::Null),
        "mcpStatus": b.get("mcpStatus").cloned().unwrap_or(Value::Null),
        "forms": list
    }))
}

/// 양식 이름(키) / alias / form_id 로 매칭.
fn find_form<'a>(
    forms: &'a serde_json::Map<String, Value>,
    q: &str,
) -> Option<(String, &'a Value)> {
    if let Some(v) = forms.get(q) {
        return Some((q.to_string(), v));
    }
    for (name, v) in forms {
        if let Some(aliases) = v.get("aliases").and_then(|a| a.as_array())
            && aliases.iter().any(|a| a.as_str() == Some(q))
        {
            return Some((name.clone(), v));
        }
        if let Some(fid) = v.get("form_id").and_then(|f| f.as_i64())
            && q == fid.to_string()
        {
            return Some((name.clone(), v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_guide는_이름_alias_form_id로_찾는다() {
        for q in ["출장신청", "출장", "40"] {
            let v = get_guide(q).unwrap_or_else(|e| panic!("'{q}' 조회 실패: {e}"));
            assert_eq!(v["docType"], "출장신청");
        }
        assert!(get_guide("없는양식").is_err());
    }

    /// draftHelp는 submit_approval의 --help 역할이라, 예시 두 개가 반드시 있어야 한다.
    /// 이게 비면 에이전트가 hp_application_json/bind_data_json을 채울 근거를 잃는다.
    #[test]
    #[allow(non_snake_case)] // 이름 속 `draftHelp` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn 모든_양식이_draftHelp_예시를_갖는다() {
        let list = list_guides().unwrap();
        let forms = list["forms"].as_array().unwrap();
        assert!(!forms.is_empty());
        for f in forms {
            let name = f["docType"].as_str().unwrap();
            let g = get_guide(name).unwrap();
            let dh = &g["guide"]["draftHelp"];
            assert!(dh["hpApplicationExample"].is_object(), "{name}: hpApplicationExample 없음");
            assert!(dh["bindDataExample"].is_object(), "{name}: bindDataExample 없음");
            assert!(dh["fixed"].is_object(), "{name}: fixed 코드 없음");
        }
    }
}
