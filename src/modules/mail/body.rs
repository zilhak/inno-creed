//! Markdown 입력, 저장 본문 검증, 세션을 넘어 유지하는 본문 검증 기록.
//! 기록에는 본문 원문 대신 계정별 초안 ID와 서버에서 읽은 HTML의 해시만 남긴다.

use anyhow::{Result, anyhow, bail};
use pulldown_cmark::{Event, Options, Parser, html};
use scraper::{Html, Node};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::modules::board::html_to_text;
use crate::{client::GwClient, error::InvalidInput};

/// 입력을 한 번만 읽어 확정한다. 서식 있는 원문은 Markdown 변환을 거치지 않는다.
pub struct PreparedBody {
    pub(super) html: String,
    pub(super) rich: bool,
}

pub fn prepare_body(
    body: Option<&str>,
    html_file: Option<&str>,
    html: Option<&str>,
) -> Result<PreparedBody> {
    if [body.is_some(), html_file.is_some(), html.is_some()]
        .into_iter()
        .filter(|present| *present)
        .count()
        != 1
    {
        return Err(InvalidInput::new("body, html_file, html 중 정확히 하나를 지정하세요. 본문을 생략하거나 여러 입력을 함께 사용할 수 없습니다").into());
    }
    if let Some(body) = body {
        return Ok(PreparedBody {
            html: render_body(body)?,
            rich: false,
        });
    }
    let html = if let Some(path) = html_file {
        let path = std::path::Path::new(path);
        if !path.is_absolute() {
            return Err(InvalidInput::new(
                "html_file은 MCP 서버 머신의 UTF-8 HTML 파일 절대경로여야 합니다",
            )
            .into());
        }
        std::fs::read_to_string(path)
            .map_err(|e| InvalidInput::new(format!("html_file을 읽을 수 없습니다: {e}")))?
    } else {
        html.unwrap().to_owned()
    };
    validate_html(&html)?;
    Ok(PreparedBody { html, rich: true })
}

/// 서명·head·style·script를 본문으로 오인하지 않는다. 이미지 본문은 허용한다.
/// CSS를 실제 렌더링하지 않으므로 모든 숨김 스타일을 판정하는 검사는 아니다.
pub(super) fn validate_html(html: &str) -> Result<()> {
    let document = Html::parse_document(html);
    let meaningful = document.tree.nodes().any(|node| {
        if node
            .ancestors()
            .chain(std::iter::once(node))
            .any(|ancestor| {
                let Node::Element(element) = ancestor.value() else {
                    return false;
                };
                matches!(element.name(), "head" | "style" | "script" | "template")
                    || element.attr("class").is_some_and(|classes| {
                        classes.split_whitespace().any(|c| c == "dze_signature")
                    })
            })
        {
            return false;
        }
        match node.value() {
            Node::Text(text) => !text.trim().is_empty(),
            Node::Element(element) => {
                element.name() == "img"
                    && element
                        .attr("src")
                        .is_some_and(|src| !src.trim().is_empty())
            }
            _ => false,
        }
    });
    if !meaningful {
        return Err(InvalidInput::new("발송하지 않았습니다. 본문이 비어 있거나 등록 서명만 있습니다. 본문 텍스트 또는 이미지를 넣으세요").into());
    }
    Ok(())
}

/// 파서가 문서 래퍼·엔티티·속성 순서를 정규화한다. 텍스트뿐 아니라 스타일·링크·이미지도 대조한다.
/// 서버가 실질적인 HTML을 바꾸면 성공으로 간주하지 않는다(화면 렌더링 동등성 판정은 아님).
pub(super) fn verify_rich(expected: &str, actual: &str) -> Result<()> {
    let normalize = |input: &str| Html::parse_document(input).root_element().html();
    if normalize(expected) != normalize(actual) {
        bail!("저장된 HTML의 서식·구조·이미지·링크가 작성 원문과 일치하지 않습니다");
    }
    Ok(())
}

pub(super) fn preview_text(html: &str) -> String {
    let mut document = Html::parse_document(html);
    let selector = scraper::Selector::parse("head, style, script, template").unwrap();
    let hidden: Vec<_> = document
        .select(&selector)
        .map(|element| element.id())
        .collect();
    for id in hidden {
        document.tree.get_mut(id).unwrap().detach();
    }
    html_to_text(&document.root_element().html())
}

pub fn render_body(body: &str) -> Result<String> {
    if body.trim().is_empty() {
        return Err(InvalidInput::new("발송하지 않았습니다. body에 비어 있지 않은 일반 텍스트 또는 Markdown 본문을 입력하세요.").into());
    }
    let mut events = Vec::new();
    for event in Parser::new_ext(body, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Html(_) | Event::InlineHtml(_) => {
                return Err(InvalidInput::new("발송하지 않았습니다. body에 HTML 태그를 직접 입력할 수 없습니다. 일반 텍스트 또는 Markdown을 사용하세요. HTML 예시는 코드 블록으로 감싸세요.").into());
            }
            // 일반 텍스트의 줄바꿈도 메일에서 보존한다.
            Event::SoftBreak => events.push(Event::HardBreak),
            other => events.push(other),
        }
    }
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());
    if visible_text(&output).is_empty() {
        return Err(InvalidInput::new("발송하지 않았습니다. 본문에 표시할 텍스트가 없습니다. 서명만 있는 메일은 보내지 않습니다.").into());
    }
    Ok(output)
}

fn visible_text(html: &str) -> String {
    html_to_text(html)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn verify_saved(expected: &str, actual: &str) -> Result<()> {
    let expected_text = visible_text(expected);
    if expected_text.is_empty() || expected_text != visible_text(actual) {
        bail!("저장된 본문이 작성 본문과 일치하지 않습니다(서명만 남음·일부 본문 누락 포함)");
    }
    Ok(())
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn record_path(c: &GwClient, muid: &str) -> Result<PathBuf> {
    let root = crate::config::dir()
        .ok_or_else(|| anyhow!("본문 검증 기록을 저장할 설정 경로가 없습니다"))?;
    // 서버 입력을 파일 경로에 직접 넣지 않는다. 다른 계정의 같은 muid도 분리한다.
    let key = serde_json::to_string(&(c.email_addr(), c.email_domain(), c.emp_seq(), muid))?;
    Ok(root
        .join("verified-mail-bodies")
        .join(format!("{}.sha256", digest(&key))))
}

pub(super) fn remember(c: &GwClient, muid: &str, html: &str) -> Result<()> {
    let path = record_path(c, muid)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    // 쓰기가 중단된 기록은 require_verified의 정확한 해시 대조에서 거부된다.
    std::fs::write(path, digest(html))?;
    Ok(())
}

fn verify_record(record: &str, html: &str) -> Result<()> {
    if record != digest(html) {
        bail!(
            "검증 후 초안 본문이 변경되었습니다 — 발송하지 않았습니다. preview_mail_draft로 다시 미리 보고 내용을 확인받으세요"
        );
    }
    Ok(())
}

pub(super) fn require_verified(c: &GwClient, muid: &str, html: &str) -> Result<()> {
    let path = record_path(c, muid)?;
    let record = std::fs::read_to_string(path).map_err(|_| anyhow!(
        "draft_muid={muid}의 본문 검증 기록을 읽을 수 없습니다 — 발송하지 않았습니다. preview_mail_draft로 원본 초안을 미리 보고 내용을 확인받으세요"
    ))?;
    verify_record(&record, html).map_err(|error| anyhow!("draft_muid={muid}: {error}"))
}

pub(super) fn forget(c: &GwClient, muid: &str) -> Result<()> {
    std::fs::remove_file(record_path(c, muid)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_html_and_file_preserve_source_without_markdown_conversion() {
        let source = "<!doctype html><html><head><style>.price {color:red}</style></head><body><table><tr><td rowspan=\"2\" style=\"text-align:right\">금액</td></tr></table><img src=\"cid:logo\"></body></html>";
        let inline = prepare_body(None, None, Some(source)).unwrap();
        assert_eq!(inline.html, source);
        assert!(inline.rich);
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".claude-workspace/mail-body-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("rich-{}.html", std::process::id()));
        std::fs::write(&file, source).unwrap();
        let imported = prepare_body(None, file.to_str(), None).unwrap();
        std::fs::remove_file(file).unwrap();
        assert_eq!(imported.html, source);
        assert!(imported.rich);
    }

    #[test]
    fn body_source_must_be_unambiguous_and_nonempty() {
        for (body, file, html) in [
            (None, None, None),
            (Some("본문"), None, Some("<p>본문</p>")),
            (Some("본문"), Some("unused.html"), None),
            (None, Some("unused.html"), Some("본문")),
            (None, Some("relative.html"), None),
            (None, None, Some("  ")),
        ] {
            assert!(prepare_body(body, file, html).is_err());
        }
        assert!(!prepare_body(Some("본문"), None, None).unwrap().rich);
    }

    #[test]
    fn signature_only_is_not_a_previewable_body_but_images_are() {
        for html in [
            "",
            "<p><br></p>",
            "<style>body {color:red}</style>",
            "<!-- 설명 -->",
            "<script>alert('본문 아님')</script>",
            "<div class=\"other dze_signature\"><p>감사합니다</p><img src=\"cid:signature\"></div>",
        ] {
            assert!(validate_html(html).is_err(), "{html}");
        }
        assert!(validate_html("<img src=\"cid:body-image\">").is_ok());
        assert!(validate_html("<p>본문</p><div class=\"dze_signature\">서명</div>").is_ok());
        let text = preview_text(
            "<head><style>p{color:red}</style></head><body><p>본문</p><script>hidden()</script></body>",
        );
        assert_eq!(text.trim(), "본문");
    }

    #[test]
    fn rich_verification_detects_format_link_and_image_loss() {
        let source = "<p style=\"color:red\"><a href=\"https://example.com/a\">본문</a></p><img src=\"cid:logo\">";
        assert!(verify_rich(source, source).is_ok());
        for changed in [
            source.replace("color:red", "color:blue"),
            source.replace("example.com/a", "example.com/b"),
            source.replace("<img src=\"cid:logo\">", ""),
            source.replace("cid:logo", "cid:other"),
        ] {
            assert!(verify_rich(source, &changed).is_err());
        }
        assert!(verify_rich("<p style='color:red' title='A&amp;B'>본문</p>",
            "<html><head></head><body><p title=\"A&amp;B\" style=\"color:red\">본문</p></body></html>").is_ok());
    }

    #[test]
    fn markdown_preserves_content_and_line_breaks() {
        let html = render_body("안녕하세요\n둘째 줄 & < 3\n\n- **강조**\n- 목록\n\n| 항목 | 값 |\n| --- | --- |\n| 본문 | 12345 |").unwrap();
        assert!(html.contains("<br />"));
        assert!(html.contains("<strong>강조</strong>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("&amp; &lt; 3"));
        assert!(visible_text(&html).contains("12345"));
    }

    #[test]
    fn reject_empty_and_raw_html_before_signature() {
        for input in [
            "",
            " \n\t",
            "---",
            "<p>본문</p>",
            "본문 <b>태그</b>",
            "<!-- 숨은 내용 -->",
        ] {
            assert!(render_body(input).is_err(), "{input}");
        }
        assert!(
            render_body("`<p>HTML 예시</p>`")
                .unwrap()
                .contains("&lt;p&gt;")
        );
    }

    #[test]
    fn saved_signature_alone_and_partial_body_are_rejected() {
        let expected = "<p>첫째 문단</p><p>둘째 문단</p><div>서명</div>";
        assert!(verify_saved(expected, "<div>서명</div>").is_err());
        assert!(verify_saved(expected, "<p>첫째 문단</p><div>서명</div>").is_err());
        assert!(
            verify_saved(
                expected,
                "<p>첫째 문단</p>\n<p>둘째 문단</p>\n<div>서명</div>"
            )
            .is_ok()
        );
        assert!(verify_saved(expected, "").is_err());
    }

    #[test]
    fn changed_draft_and_truncated_record_cannot_pass() {
        let body = "<p>검증된 본문</p>";
        let record = digest(body);
        assert!(verify_record(&record, body).is_ok());
        assert!(verify_record(&record, "<p>서명만 남음</p>").is_err());
        assert!(verify_record(&record[..10], body).is_err());
    }
}
