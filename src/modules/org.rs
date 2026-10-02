//! 조직도(ORG) 모듈 — `/gw/APIHandler/gw102A0x`. 읽기 전용.
//! 부서 트리(gw102A01) + 부서별 사원·직책(gw102A02). 결재선 "직책→담당자" 해석의 재료.
//! 인증은 헤더 서명만으로 완결(companyInfo 불필요) — 실제 트래픽 캡처로 확정한 사실이다.
//!
//! ⚠️ 조회 결과는 정확하지만, 여기서 "이 직책 담당자 = 이 문서 결재자"로 단정하지 말 것.
//! dutyName(직책 텍스트)이 권위 필드이고 dutyCode 숫자 매핑은 불안정하다.
//!
//! ## ⚠️ 시그니처 예외 — 이 모듈의 `roster`/`find_person`만 `&Arc<GwClient>`를 받는다
//!
//! 다른 모듈(그리고 이 파일의 `dept_tree`/`dept_members`/`my_profile`)은 전부 `&GwClient`가 규약이다.
//! 둘만 다른 이유는 **동시성**이다:
//!  - `roster`는 부서를 `JoinSet`으로 8개씩 병렬 순회하는데, `JoinSet::spawn`이 `'static` future를
//!    요구해 `&GwClient` 참조를 캡처할 수 없다 → `Arc::clone`이 필수.
//!  - `GwClient`는 `RwLock`을 직접 보유해 `Clone`이 아니므로 `Arc` 말고는 우회로가 없다.
//!  - `find_person`은 병렬성과 무관하지만 `roster`를 호출해서 `Arc` 요구가 전염된 것이다.
//!
//! 없앨 수는 있으나 셋 다 대가가 있어 **의도적으로 현 상태를 유지한다**(2026-08-05 판단):
//! `futures::buffer_unordered` 도입(신규 의존성) / `roster`를 client.rs로 이관(역할 분담 붕괴) /
//! 직렬화(첫 조회가 부서 수만큼 순차 호출 — 명백한 후퇴).
//! **새 함수를 추가할 땐 `&GwClient`를 쓸 것.**

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::task::JoinSet;

use crate::client::GwClient;
use crate::util::s;

/// 부서 트리 — gw102A01. **언제나 전사 트리**(평평한 배열)를 준다.
/// `isTreeAllOpen:true`로 **전체 노드 펼침**(과거 partial 트리 문제 해소 — 경영지원본부 하위 인사총무팀/인사지원실까지 나옴).
/// `path`(dept_id 경로)로 기안자 부서에서 상위(센터/본부/부문)를 추적.
///
/// ⚠️ **`parentSeq`로 서브트리를 받을 수는 없다** — 서버가 이 파라미터를 조용히 무시한다.
/// `parentSeq:"2989"`를 보내도 `parentSeq:"0"`과 똑같은 74개 전사 노드가 온다(2026-08-20 실측).
/// 그래서 인자를 없애고 "0" 고정으로 보낸다. 서브트리가 필요하면 `dept_tree_nested`가
/// 받은 결과를 `path` 접두사로 거른다.
pub async fn dept_tree(c: &GwClient) -> Result<Value> {
    let body = json!({
        "parentSeq": "0", "popupType": "main", "selectedType": "tree",
        "isAllCompShow": false, "compFilter": "", "isTreeChecked": "",
        "isTreeAllOpen": true, "isPartYn": false
    });
    let data = c.call("/gw/APIHandler/gw102A01", &body).await?;
    let arr = data
        .get("treeList")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("gw102A01 응답에 treeList 없음"))?;
    let depts: Vec<Value> = arr
        .iter()
        .map(|d| {
            json!({
                "deptId": s(d, "id"),
                "name": s(d, "text"),
                "path": s(d, "path"),          // dept_id 경로 예: 1000|1000|2986|2987|2989|2993|
                "parentSeq": s(d, "parentSeq"),
                "level": d.get("orgLevel").cloned().unwrap_or(Value::Null),
                "gubun": s(d, "orgGubun"),     // c=회사, b=사업장, d=부서
                "userCount": d.get("childUserCnt").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();
    Ok(json!({ "kind": "deptTree", "count": depts.len(), "depts": depts }))
}

/// `path`("1000|1000|2986|")의 부모 경로("1000|1000|"). 루트면 빈 문자열.
fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('|');
    match trimmed.rfind('|') {
        Some(i) => format!("{}|", &trimmed[..i]),
        None => String::new(),
    }
}

/// `path`를 키로 자식을 재귀 부착. 자식이 없으면 `children`을 아예 넣지 않는다(말단 노이즈 제거).
fn attach_children(
    path: &str,
    nodes: &std::collections::HashMap<String, Value>,
    kids: &std::collections::HashMap<String, Vec<String>>,
) -> Value {
    let mut node = nodes.get(path).cloned().unwrap_or(Value::Null);
    let children: Vec<Value> = kids
        .get(path)
        .map(|ps| ps.iter().map(|p| attach_children(p, nodes, kids)).collect())
        .unwrap_or_default();
    if !children.is_empty()
        && let Some(obj) = node.as_object_mut()
    {
        obj.insert("children".into(), Value::Array(children));
    }
    node
}

/// 전사 트리를 받아 `scope`(deptId) 이하만 남긴다. `(범위 내 부서들, scope_id)`.
/// 서버에 서브트리 조회가 없으므로(위 `dept_tree` 경고) 전사를 받아 `path` 접두사로 거른다 —
/// 호출 비용은 어느 쪽이든 gw102A01 1콜로 같다.
async fn scoped_depts(c: &GwClient, scope: &str) -> Result<(Vec<Value>, String)> {
    let flat = dept_tree(c).await?;
    let depts = flat
        .get("depts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let scope = scope.trim();
    if scope.is_empty() || scope == "0" {
        return Ok((depts, "0".to_string()));
    }
    let prefix = depts
        .iter()
        .find(|d| s(d, "deptId") == scope)
        .map(|d| s(d, "path"))
        .ok_or_else(|| anyhow!("deptId '{scope}' 를 부서 트리에서 찾지 못했습니다"))?;
    let kept = depts
        .into_iter()
        .filter(|d| s(d, "path").starts_with(&prefix))
        .collect();
    Ok((kept, scope.to_string()))
}

/// 부서 목록을 **서버 원본 필드 그대로**(path/parentSeq/level 포함) 평평하게 반환.
/// 계층은 `path`/`parentSeq`에 들어 있으니 필요하면 호출자가 접으면 된다.
/// 트리 모양이 필요하면 `dept_tree_nested` 쪽이 이미 접어서 준다.
pub async fn dept_tree_flat(c: &GwClient, scope: &str) -> Result<Value> {
    let (depts, scope_id) = scoped_depts(c, scope).await?;
    Ok(json!({
        "kind": "deptList",
        "scope": scope_id,
        "count": depts.len(),
        "note": "userCount는 하위 부서를 포함한 누적 인원",
        "depts": depts
    }))
}

/// 부서 트리를 **중첩 구조로** 반환. 평평한 배열을 호출자가 매번 조립하던 일을 없앤다.
/// `scope`가 비었거나 "0"이면 전사, 특정 deptId면 그 부서와 그 하위만.
///
/// 조립 키는 `parentSeq`가 아니라 **`path`**다. 회사 노드와 사업장 노드가 `deptId`를 똑같이 "1000"으로
/// 쓰고 사업장은 `parentSeq`도 "1000"(자기 자신)이라, deptId로 부모를 찾으면 자기참조 사이클이 된다.
/// `path`("1000|" vs "1000|1000|")는 노드마다 유일해서 그 함정을 피한다.
pub async fn dept_tree_nested(c: &GwClient, scope: &str) -> Result<Value> {
    let (depts, scope_id) = scoped_depts(c, scope).await?;

    let mut nodes: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let mut kids: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for d in depts.iter() {
        let path = s(d, "path");
        if path.is_empty() {
            continue;
        }
        nodes.insert(
            path.clone(),
            json!({
                "deptId": s(d, "deptId"),
                "name": s(d, "name"),
                "gubun": s(d, "gubun"),        // c=회사, b=사업장, d=부서
                "userCount": d.get("userCount").cloned().unwrap_or(Value::Null)
            }),
        );
        order.push(path);
    }
    for path in &order {
        let parent = parent_path(path);
        if nodes.contains_key(&parent) {
            kids.entry(parent).or_default().push(path.clone());
        }
    }
    // 부모가 범위 밖인 노드가 뿌리. 서버 응답 순서를 그대로 보존한다.
    let roots: Vec<Value> = order
        .iter()
        .filter(|p| !nodes.contains_key(&parent_path(p)))
        .map(|p| attach_children(p, &nodes, &kids))
        .collect();

    Ok(json!({
        "kind": "deptTree",
        "scope": scope_id,
        "count": nodes.len(),
        // userCount는 하위 포함 누적이다(부모 − 자식합 = 그 조직 직속 인원).
        "note": "userCount는 하위 부서를 포함한 누적 인원",
        "tree": roots
    }))
}

/// 부서별 사원 목록 — gw102A02. `dept_id`(deptTree의 deptId)의 사원 전원 + 직책(dutyName).
/// 부서 사원 목록 **서버 원본 그대로**(gw102A02 응답 배열). 가공본은 `dept_members`.
///
/// 가공본이 버리는 필드가 필요한 곳을 위해 둔다 — 전자결재 수신참조 노드는
/// `positionCode`(직급코드)·`workStatus`·`compSeq`/`bizSeq`까지 요구하는데
/// `dept_members`의 요약에는 그것들이 없다(`approval_submit::build_cc_nodes`).
pub async fn dept_members_raw(c: &GwClient, dept_id: &str) -> Result<Vec<Value>> {
    let body = json!({
        "selectedId": dept_id, "orgGubun": "d",
        "popupType": "main", "selectedType": "tree",
        "searchDiv": "all", "searchText": "",
        "isBdayOption": "1", "isJoinDayOption": "0",
        "isOrganizationDisplayOption": "5|0|1|3|",
        "isGridListDisplayOption": "0", "isLoginIdOption": "1"
    });
    let data = c.call("/gw/APIHandler/gw102A02", &body).await?;
    data.as_array()
        .cloned()
        .ok_or_else(|| anyhow!("gw102A02 응답이 배열이 아님"))
}

pub async fn dept_members(c: &GwClient, dept_id: &str) -> Result<Value> {
    let arr = dept_members_raw(c, dept_id).await?;
    let members: Vec<Value> = arr
        .iter()
        .map(|m| {
            json!({
                "empSeq": s(m, "empSeq"),
                "name": s(m, "empName"),
                "loginId": s(m, "loginId"),
                "duty": s(m, "dutyName"),          // 직책(팀장/센터장/본부장/실장/팀원) — 권위 필드
                "dutyCode": s(m, "dutyCode"),      // 참고용(숫자 매핑은 불안정, 단정 금지)
                "position": s(m, "positionName"),  // 직급(부장/책임연구원 등)
                "deptId": s(m, "deptSeq"),
                "deptName": s(m, "deptName"),
                "deptPath": s(m, "pathName"),      // 사람 읽기용 경로 예: (주)이노그리드>…>네이티브 플랫폼팀
                "email": s(m, "emailAddr"),
                "mobile": s(m, "mobileTelNum"),
                "note": s(m, "atNm")               // 부재 표시(예: 육아휴직)
            })
        })
        .collect();
    Ok(json!({ "kind": "deptMembers", "deptId": dept_id, "count": members.len(), "members": members }))
}

/// 부서 순회 동시 호출 수. 서버 부하와 응답시간의 절충(75개 부서 기준).
const ROSTER_CONCURRENCY: usize = 8;

/// 전사 사원 명부(캐시 30분). `dept_tree`(1콜)로 부서를 뽑고, **인원이 있는 부서마다**
/// gw102A02를 호출해 합친다. 전사 일괄 조회는 불가능하다 — gw102A02에 회사/사업장 노드
/// (`orgGubun` c/b)를 주면 **0명**이 오는 것을 실측 확인(2026-08-04). 그래서 부서 단위 순회가
/// 유일한 경로이고, 비용이 커서 캐시가 필수다.
pub async fn roster(c: &Arc<GwClient>) -> Result<Vec<Value>> {
    if let Some(cached) = c.cached_roster() {
        return Ok(cached);
    }
    let tree = dept_tree(c).await?;
    let depts = tree
        .get("depts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 실제 부서(gubun=="d") + 인원>0 인 곳만. 회사/사업장 노드는 멤버가 안 나온다.
    let targets: Vec<String> = depts
        .iter()
        .filter(|d| d.get("gubun").and_then(|v| v.as_str()) == Some("d"))
        .filter(|d| d.get("userCount").and_then(|v| v.as_i64()).unwrap_or(0) > 0)
        .filter_map(|d| d.get("deptId").and_then(|v| v.as_str()).map(String::from))
        .collect();

    let spawn = |set: &mut JoinSet<Result<Value>>, dept_id: String| {
        let cc = Arc::clone(c);
        set.spawn(async move { dept_members(&cc, &dept_id).await });
    };

    let mut set: JoinSet<Result<Value>> = JoinSet::new();
    let mut queue = targets.into_iter();
    for _ in 0..ROSTER_CONCURRENCY {
        match queue.next() {
            Some(id) => spawn(&mut set, id),
            None => break,
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut people: Vec<Value> = Vec::new();
    while let Some(joined) = set.join_next().await {
        // 개별 부서 실패는 건너뛴다(권한 없는 부서 등) — 명부 전체를 실패시키지 않는다.
        if let Ok(Ok(v)) = joined
            && let Some(members) = v.get("members").and_then(|m| m.as_array())
        {
            for m in members {
                let key = m.get("empSeq").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !key.is_empty() && seen.insert(key) {
                    people.push(m.clone());
                }
            }
        }
        if let Some(id) = queue.next() {
            spawn(&mut set, id);
        }
    }

    attach_dept_chain(&mut people, &depts);
    c.set_roster(people.clone());
    Ok(people)
}

/// 사원마다 `deptChain`(회사 → … → 본인 부서, `{deptId,name}` 배열)을 붙인다.
/// gw102A02가 주는 `deptPath`는 **이름 문자열**이라 `org_chart`에 다시 물을 수 없다 —
/// 이름은 개편되면 바뀌고 중복도 가능해서 연결고리로 못 쓴다. deptId 배열이 그 구멍을 메운다.
///
/// 여기서 붙이는 이유는 **부서 트리가 이미 손에 있어서**다(`roster`가 방금 받았다).
/// `find_person` 쪽에서 붙이면 호출 때마다 gw102A01을 한 번 더 때려야 하고,
/// 여기서 붙이면 30분 명부 캐시에 그대로 실려 간다.
fn attach_dept_chain(people: &mut [Value], depts: &[Value]) {
    // deptId → (이름, path). 회사 노드와 사업장 노드가 deptId "1000"을 공유하므로 **먼저 나온 쪽**을 쓴다.
    let mut info: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for d in depts {
        info.entry(s(d, "deptId"))
            .or_insert_with(|| (s(d, "name"), s(d, "path")));
    }

    for p in people.iter_mut() {
        let Some((_, path)) = info.get(&s(p, "deptId")) else {
            continue;
        };
        let mut chain: Vec<Value> = Vec::new();
        for id in path.split('|').filter(|seg| !seg.is_empty()) {
            // 회사(c)와 사업장(b)이 같은 deptId로 연달아 오는 구간을 접는다("1000|1000|…").
            if chain.last().map(|last| s(last, "deptId")) == Some(id.to_string()) {
                continue;
            }
            let name = info.get(id).map(|(n, _)| n.clone()).unwrap_or_default();
            chain.push(json!({ "deptId": id, "name": name }));
        }
        if let Some(obj) = p.as_object_mut() {
            obj.insert("deptChain".into(), Value::Array(chain));
        }
    }
}

/// 본인의 **표시정보**(부서명/직책/직급) — gw102A02 1콜(내 부서 사원목록에서 내 empSeq 매칭).
/// 세션(gw050A02)은 코드/seq만 주고 부서명·직책(dutyName)·직급(positionName)이 없어서 별도 조회가 필요하다.
/// 쓰이는 곳: `whoami` 노출 / `submit_approval`의 문서 표시필드 자동 주입 / 결재선 grade 판정.
/// 30분 캐시. **실패해도 에러를 올리지 않고** 빈 값 + `resolved:false`를 준다 —
/// 표시문자열이 없다고 상신 자체를 막을 이유는 없기 때문(호출부가 예시값을 그대로 두면 됨).
pub async fn my_profile(c: &GwClient) -> Value {
    if let Some(cached) = c.cached_profile() {
        return cached;
    }
    let emp_seq = c.emp_seq();
    let dept_seq = c.dept_seq();
    let fallback = json!({
        "resolved": false, "empSeq": emp_seq, "name": c.emp_name(),
        "deptId": dept_seq, "deptName": "", "duty": "", "position": "", "coName": ""
    });
    if emp_seq.is_empty() || dept_seq.is_empty() {
        return fallback;
    }

    let Ok(members) = dept_members(c, &dept_seq).await else {
        return fallback;
    };
    let me = members
        .get("members")
        .and_then(|m| m.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|m| m.get("empSeq").and_then(|v| v.as_str()) == Some(emp_seq.as_str()))
        })
        .cloned();
    let Some(me) = me else {
        return fallback;
    };

    // 회사명은 부서경로 첫 마디("(주)이노그리드>…>네이티브 플랫폼팀").
    let co_name = s(&me, "deptPath")
        .split('>')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let p = json!({
        "resolved": true,
        "empSeq": emp_seq,
        "name": s(&me, "name"),
        "deptId": s(&me, "deptId"),
        "deptName": s(&me, "deptName"),
        "duty": s(&me, "duty"),          // 직책(팀원/팀장/센터장…) — 결재선 grade 판정의 근거
        "position": s(&me, "position"),  // 직급(책임연구원/부장…) — 문서 표시용
        "coName": co_name,
        "deptPath": s(&me, "deptPath")
    });
    c.set_profile(p.clone());
    p
}

/// 잘라내지 않았을 때 돌려줄 기본 인원 수. 한 글자 검색("김" → 83명, 22,000자)이
/// 응답을 통째로 덮는 것을 막는다. 부족하면 호출자가 `limit`을 올리거나 `no_limit`을 쓴다.
const FIND_PERSON_LIMIT: i64 = 20;

/// `find_person`이 훑는 필드. 이름·계정뿐 아니라 **조직정보(부서·직책·직급)까지** 검색 대상이다
/// — "네이티브 플랫폼팀"이나 "팀장"으로도 찾을 수 있다.
const PERSON_SEARCH_FIELDS: [&str; 7] = [
    "name", "loginId", "email", "deptName", "deptPath", "duty", "position",
];

/// 사람 찾기. 결재선/참석자/수신자에 필요한 `empSeq`를 얻는 진입점이자,
/// **숫자를 주면 그 `empSeq`가 누구인지 되짚는** 역방향 경로이기도 하다.
/// 매칭은 `PERSON_SEARCH_FIELDS` 전부에 대한 부분일치(대소문자 무시)이고,
/// **empSeq 일치·이름 완전일치 → 이름 부분일치 → 그 밖의 필드** 순으로 정렬한다.
/// 상한을 넘긴 결과는 잘라내되 `truncated`/`notice`로 잘렸음을 반드시 알린다
/// (조용히 자르면 호출자가 "그게 전부"로 오해한다).
///
/// ⚠️ 서버측 인물검색 API는 쓰지 않는다 — gw102A02의 `searchText`는 **서버가 무시**하고
/// (부서 인원 17명이 검색어와 무관하게 그대로 반환), `/ab/ab099A23`은 JSON이 아닌 응답을
/// 준다. 둘 다 2026-08-04 실측. 그래서 명부를 받아 클라이언트에서 거른다.
pub async fn find_person(
    c: &Arc<GwClient>,
    query: &str,
    limit: Option<i64>,
    no_limit: bool,
) -> Result<Value> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Err(anyhow!("검색어가 비어 있습니다"));
    }
    let people = roster(c).await?;
    let field = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_lowercase();

    // 숫자만 들어오면 empSeq로 본다. 결재선·일정·예약 응답은 사람을 이름이 아니라 empSeq로
    // 담고 있어서 그 숫자를 되짚을 경로가 필요하다. **부분일치는 쓰지 않는다** —
    // "316"이 3160·3166을 함께 물어오면 되짚기가 아니라 또 다른 검색이 된다.
    let as_emp_seq = q.chars().all(|ch| ch.is_ascii_digit()).then(|| q.clone());

    // 순위: 0=empSeq 일치 또는 이름 완전일치, 1=이름 부분일치, 2=조직정보 등 그 밖의 필드.
    // 상한에 걸려 잘릴 때 "의도한 그 사람"이 먼저 살아남게 하는 것이 목적이다.
    let mut hits: Vec<(u8, Value)> = people
        .iter()
        .filter_map(|m| {
            let name = field(m, "name");
            let rank = if as_emp_seq.as_deref() == Some(s(m, "empSeq").as_str()) || name == q {
                0
            } else if name.contains(&q) {
                1
            } else if PERSON_SEARCH_FIELDS.iter().any(|k| field(m, k).contains(&q)) {
                2
            } else {
                return None;
            };
            Some((rank, m.clone()))
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank); // 안정 정렬 — 같은 순위면 명부 순서 유지

    let matched = hits.len();
    let applied: Option<usize> = if no_limit {
        None
    } else {
        Some(limit.unwrap_or(FIND_PERSON_LIMIT).max(1) as usize)
    };
    let out: Vec<Value> = match applied {
        Some(n) => hits.into_iter().take(n).map(|(_, m)| m).collect(),
        None => hits.into_iter().map(|(_, m)| m).collect(),
    };
    let returned = out.len();
    let truncated = matched > returned;
    let notice = truncated.then(|| {
        format!(
            "전체 {matched}명 중 상위 {returned}명만 반환했습니다(limit={}). \
             더 보려면 limit을 올리거나 no_limit:true를 지정하세요.",
            applied.unwrap_or(returned)
        )
    });

    Ok(json!({
        "kind": "findPerson",
        "query": query,
        "matched": matched,          // 검색에 걸린 전체 인원
        "returned": returned,        // 실제로 반환한 인원
        "truncated": truncated,
        "limit": applied,            // no_limit이면 null
        "notice": notice,            // 잘렸을 때만 값이 있다
        "rosterSize": people.len(),
        "people": out
    }))
}

