//! 서명 API probe — 임의의 아마란스 엔드포인트를 wehago-sign 서명으로 호출하고 전체 응답 봉투를 출력.
//! MCP 재부팅/브라우저 없이 Bash로 API를 즉시 찔러보는 디버그 REPL.
//!
//! 사용:
//!   cargo run --quiet --bin probe -- /human/attendapplication/0hr00001 '{"coCd":"1000"}'
//!   cargo run --quiet --bin probe -- /eap/eap110A03 @body.json
//!   (body 생략 시 {}. body가 '@경로'면 파일에서 읽음.)
//!   cargo run --quiet --bin probe -- fetch https://gw.innogrid.com/mail2/ out.html
//!   (fetch = 세션 쿠키를 붙인 평범한 GET. 인증이 걸린 SPA·번들을 받아 API 코드를 캐낼 때.)
//!   cargo run --quiet --bin probe -- raw '/gw/contentsImgController/download/<경로>' out.png
//!   (raw = 응답 바이트를 파일로. 본문 삽입 이미지처럼 JSON이 아닌 응답을 볼 때.)
//!   cargo run --quiet --bin probe -- form /ecm/ecm001A03 out.bin moduleGbn=BOARD 'authKeyMap={"fileIds":"<id>"}'
//!   (form = x-www-form-urlencoded POST. ECM 계열이 이 형식이다. out 을 '-' 로 주면 JSON 봉투를 출력.)
//!   cargo run --quiet --bin probe -- upload /ecm/ecm001A01 'file[]' ./a.txt moduleGbn=EAP
//!   (upload = multipart POST. 파일 하나 + 나머지 k=v 는 텍스트 파트.)
//!
//! 성공판정 없이 {http, response:{resultCode,resultMsg,resultData}} 전체를 그대로 찍는다(2099 진단용).

use anyhow::{anyhow, Result};
use inno_creed::{client::GwClient, creds};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // 진단: `probe sign <apipath> <ts> <tid> <token> <signkey>` → wehago-sign 계산값 출력.
    if args.get(1).map(|s| s.as_str()) == Some("sign") {
        let p = &args[2];
        let ts = &args[3];
        let tid = &args[4];
        let token = &args[5];
        let key = &args[6];
        let sig = inno_creed::sign::wehago_sign(token, tid, ts, p, key);
        println!("{sig}");
        return Ok(());
    }

    // 진단: `probe seq @steps.json` → [{path, body, approkey?}] 를 **단일 GwClient(세션 연속)** 로 순차 실행.
    // approkey:"NEW" 는 첫 등장 시 생성해 이후 재사용(a03↔a06 일치용).
    if args.get(1).map(|s| s.as_str()) == Some("seq") {
        let spec = args.get(2).ok_or_else(|| anyhow!("usage: probe seq @steps.json"))?;
        let txt = if let Some(f) = spec.strip_prefix('@') {
            std::fs::read_to_string(f)?
        } else {
            spec.clone()
        };
        let steps: Vec<Value> = serde_json::from_str(&txt)?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let approkey = format!(
            "ERP_{:08x}-1111-2222-3333-444455556666",
            (args.len() as u32).wrapping_mul(2654435761).wrapping_add(7)
        );
        for (i, st) in steps.iter().enumerate() {
            let p = st.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut body = st.get("body").cloned().unwrap_or(serde_json::json!({}));
            // approkey 주입(문자열 "NEW" 대체)
            fn inject(v: &mut Value, ak: &str) {
                match v {
                    Value::String(s) if s == "NEW" => *v = Value::String(ak.to_string()),
                    Value::Array(a) => a.iter_mut().for_each(|x| inject(x, ak)),
                    Value::Object(o) => o.values_mut().for_each(|x| inject(x, ak)),
                    _ => {}
                }
            }
            inject(&mut body, &approkey);
            let res = client.call_raw(p, &body).await;
            match res {
                Ok(v) => {
                    let code = v.pointer("/response/resultCode").cloned().unwrap_or(Value::Null);
                    let msg = v
                        .pointer("/response/resultMsg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    let short: String = msg.chars().take(70).collect();
                    println!("[{i}] {p} -> code={code} {short}");
                    if code == serde_json::json!(2099) {
                        println!("    resultData: {}", v.pointer("/response/resultData").cloned().unwrap_or(Value::Null));
                    }
                    if p.contains("create") {
                        println!("    create.resultData: {}", v.pointer("/response/resultData").cloned().unwrap_or(Value::Null));
                    }
                }
                Err(e) => println!("[{i}] {p} -> ERR {e}"),
            }
        }
        return Ok(());
    }

    // 진단: `probe submit @args.json` → submit_approval 직접 호출(상신 검증). args={form_id,doc_title,line_id,hp_application_json,bind_data_json,doc_contents_html,numbering_id}
    if args.get(1).map(|s| s.as_str()) == Some("submit") {
        let spec = args.get(2).ok_or_else(|| anyhow!("usage: probe submit @args.json"))?;
        let txt = if let Some(f) = spec.strip_prefix('@') { std::fs::read_to_string(f)? } else { spec.clone() };
        let a: Value = serde_json::from_str(&txt)?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let g = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let out = inno_creed::modules::approval_submit::submit_approval(
            &client,
            a.get("form_id").and_then(|v| v.as_i64()).unwrap_or(0),
            &g("doc_title"),
            a.get("line_id").and_then(|v| v.as_i64()).unwrap_or(0),
            &g("hp_application_json"),
            &g("bind_data_json"),
            &g("doc_contents_html"),
            &g("numbering_id"),
            &a.get("attachments").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default(),
        )
        .await;
        match out {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    // 진단: `probe cancel <docId> [formId] [purge]` → cancel_and_verify 직접 호출.
    if args.get(1).map(|s| s.as_str()) == Some("cancel") {
        let doc_id = args.get(2).ok_or_else(|| anyhow!("usage: probe cancel <docId> [formId] [purge]"))?;
        let form_id = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let purge = args.get(4).map(|s| s == "purge" || s == "true").unwrap_or(false);
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let out = inno_creed::modules::approval_submit::cancel_and_verify(&client, doc_id, form_id, purge).await;
        match out {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    // 진단: `probe upload <path> <field> <파일> [k=v ...]` → multipart POST(ECM/메일 업로드).
    // 파일은 하나만 싣는다. 추가 k=v 는 텍스트 파트로 함께 보낸다.
    if args.get(1).map(|s| s.as_str()) == Some("upload") {
        let path = args.get(2).ok_or_else(|| anyhow!("usage: probe upload <path> <field> <파일> [k=v ...]"))?;
        let field = args.get(3).ok_or_else(|| anyhow!("usage: probe upload <path> <field> <파일> [k=v ...]"))?;
        let file = args.get(4).ok_or_else(|| anyhow!("usage: probe upload <path> <field> <파일> [k=v ...]"))?;
        let bytes = std::fs::read(file)?;
        let fname = std::path::Path::new(file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let kv: Vec<(String, String)> = args[5..]
            .iter()
            .filter_map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let make = || {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(fname.clone())
                .mime_str("application/octet-stream")
                .expect("고정 MIME");
            kv.iter()
                .fold(reqwest::multipart::Form::new().part(field.clone(), part), |f, (k, v)| {
                    f.text(k.clone(), v.clone())
                })
        };
        match client.call_multipart(path, make).await {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    // 진단: `probe form <path> <out|-> k=v ...` → x-www-form-urlencoded POST(ECM 계열).
    // out="-" 이면 JSON 봉투를 출력, 아니면 응답 바이트를 그 파일로 저장.
    if args.get(1).map(|s| s.as_str()) == Some("form") {
        let path = args.get(2).ok_or_else(|| anyhow!("usage: probe form <path> <out|-> k=v ..."))?;
        let out = args.get(3).ok_or_else(|| anyhow!("usage: probe form <path> <out|-> k=v ..."))?;
        let kv: Vec<(String, String)> = args[4..]
            .iter()
            .filter_map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        let params: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        if out == "-" {
            match client.call_form(path, &params).await {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
                Err(e) => println!("ERR {e}"),
            }
        } else {
            match client.download_form(path, &params, out).await {
                Ok((n, name)) => println!("{{\"bytes\":{n},\"filename\":{name:?}}}"),
                Err(e) => println!("ERR {e}"),
            }
        }
        return Ok(());
    }

    // 진단: `probe cookiejar <out>` → Playwright addCookies 형식으로 세션 쿠키를 파일에 쓴다.
    // ⚠️ 평문 크레덴셜이다. UI 캡처가 끝나면 지울 것. (브라우저 쿠키 DB를 직접 못 읽는
    // 환경 — macOS 전체 디스크 접근 차단 등 — 에서 캡처 하네스에 세션을 넘기는 유일한 경로.)
    if args.get(1).map(|s| s.as_str()) == Some("cookiejar") {
        let out = args.get(2).ok_or_else(|| anyhow!("usage: probe cookiejar <out>"))?;
        let cr = creds::from_browser()?;
        let enc = |v: &str| -> String {
            v.chars()
                .map(|c| if c == '|' { "%7C".to_string() } else { c.to_string() })
                .collect()
        };
        let jar = serde_json::json!([
            {"name":"BIZCUBE_AT","value":enc(&cr.auth_token),"domain":"gw.innogrid.com","path":"/","secure":true,"httpOnly":false},
            {"name":"BIZCUBE_HK","value":enc(&cr.sign_key),"domain":"gw.innogrid.com","path":"/","secure":true,"httpOnly":false},
            // 아래 3개가 없으면 SPA가 **로그인 화면으로 떨어진다**(API 서명에는 위 2개로 충분하지만
            // 프런트의 세션 판정은 이쪽을 본다). 2026-10-01 실측: oAuthToken=AT, signKey=HK 와 같은 값.
            {"name":"BIZCUBE_TYPE","value":"WEB","domain":"gw.innogrid.com","path":"/","secure":true,"httpOnly":false},
            {"name":"oAuthToken","value":enc(&cr.auth_token),"domain":"gw.innogrid.com","path":"/","secure":true,"httpOnly":false},
            {"name":"signKey","value":enc(&cr.sign_key),"domain":"gw.innogrid.com","path":"/","secure":true,"httpOnly":false}
        ]);
        std::fs::write(out, serde_json::to_string(&jar)?)?;
        println!("{{\"wrote\":{out:?},\"cookies\":5}}");
        return Ok(());
    }

    // 진단: `probe fetch <url> <out>` → 프런트엔드 자산을 **세션 쿠키로** GET 해 파일로 저장.
    // API 서명 경로가 아니라 브라우저가 보는 그대로의 페이지·번들을 받는다(신규 API 발굴용:
    // `/mail2/` 같은 인증 필요 SPA의 청크를 받아 mainApiCode 를 읽는다).
    if args.get(1).map(|s| s.as_str()) == Some("fetch") {
        let url = args.get(2).ok_or_else(|| anyhow!("usage: probe fetch <url> <out>"))?;
        let out = args.get(3).ok_or_else(|| anyhow!("usage: probe fetch <url> <out>"))?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let cr = creds::from_browser()?;
        let enc = |v: &str| -> String {
            v.chars()
                .map(|c| if c == '|' { "%7C".to_string() } else { c.to_string() })
                .collect()
        };
        let cookie = format!("BIZCUBE_AT={}; BIZCUBE_HK={}", enc(&cr.auth_token), enc(&cr.sign_key));
        let sign_path = url.split_once("://").map(|(_, r)| r).and_then(|r| r.split_once('/')).map(|(_, p)| format!("/{p}")).unwrap_or_else(|| "/".into());
        let sign_path = sign_path.split('?').next().unwrap_or("/").to_string();
        let tid = inno_creed::sign::transaction_id();
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs().to_string();
        let sig = inno_creed::sign::wehago_sign(&cr.auth_token, &tid, &ts, &sign_path, &cr.sign_key);
        let resp = reqwest::Client::new()
            .get(url)
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", ts)
            .header("transaction-id", tid)
            .header("wehago-sign", sig)
            .header("Cookie", cookie)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?;
        std::fs::write(out, &bytes)?;
        println!("{{\"http\":{status},\"bytes\":{}}}", bytes.len());
        return Ok(());
    }

    // 진단: `probe raw <path> <out>` → 응답 **바이트**를 파일로. JSON이 아닌 응답(이미지 등) 확인용.
    if args.get(1).map(|s| s.as_str()) == Some("raw") {
        let path = args.get(2).ok_or_else(|| anyhow!("usage: probe raw <path> <out>"))?;
        let out = args.get(3).ok_or_else(|| anyhow!("usage: probe raw <path> <out>"))?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        match client.download_form(path, &[], out).await {
            Ok((n, name)) => println!("{{\"bytes\":{n},\"filename\":{name:?}}}"),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    let path = args
        .get(1)
        .ok_or_else(|| anyhow!("usage: probe <path> [json|@file]"))?;

    let body: Value = match args.get(2) {
        None => serde_json::json!({}),
        Some(s) if s.is_empty() => serde_json::json!({}),
        Some(s) if s.starts_with('@') => {
            let txt = std::fs::read_to_string(&s[1..])
                .map_err(|e| anyhow!("body 파일 읽기 실패 {}: {e}", &s[1..]))?;
            serde_json::from_str(&txt).map_err(|e| anyhow!("body JSON 파싱 실패: {e}"))?
        }
        Some(s) => serde_json::from_str(s).map_err(|e| anyhow!("body JSON 파싱 실패: {e}"))?,
    };

    let client = GwClient::new(creds::from_browser().ok());
    client.ensure_session().await?;
    let res = client.call_raw(path, &body).await?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}
