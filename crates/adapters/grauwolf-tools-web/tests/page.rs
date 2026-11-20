//! Fetching knowledge pages (ADR-0113) against a local mock server.

use grauwolf_tools_web::{MAX_PAGE_BYTES, Page, Validators, fetch_page};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn none() -> Validators {
    Validators::default()
}

async fn serve(path_: &str, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(path_))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn fetches_html_as_text_with_its_validators() {
    let server = serve(
        "/a",
        ResponseTemplate::new(200)
            .set_body_raw(
                "<nav>menu</nav><h1>Title</h1><p>Body</p>",
                "text/html; charset=utf-8",
            )
            .insert_header("etag", "\"v1\"")
            .insert_header("last-modified", "2026-11-12"),
    )
    .await;

    let page = fetch_page(&format!("{}/a", server.uri()), &none()).await;

    assert_eq!(
        page,
        Ok(Page::Fetched {
            text: "# Title\n\nBody".into(),
            validators: Validators {
                etag: Some("\"v1\"".into()),
                last_modified: Some("2026-11-12".into()),
            }
        })
    );
}

#[tokio::test]
async fn sends_validators_and_reports_304_as_not_modified() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("if-none-match", "\"v1\""))
        .and(header("if-modified-since", "2026-11-12"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let validators = Validators {
        etag: Some("\"v1\"".into()),
        last_modified: Some("2026-11-12".into()),
    };

    assert_eq!(
        fetch_page(&format!("{}/a", server.uri()), &validators).await,
        Ok(Page::NotModified)
    );
}

#[tokio::test]
async fn keeps_markdown_and_plain_text_unchanged() {
    let server = serve(
        "/a.md",
        ResponseTemplate::new(200).set_body_raw("# Notes\n\n<b>kept</b>", "text/markdown"),
    )
    .await;

    let Ok(Page::Fetched { text, .. }) =
        fetch_page(&format!("{}/a.md", server.uri()), &none()).await
    else {
        panic!("not fetched");
    };
    assert_eq!(text, "# Notes\n\n<b>kept</b>");
}

#[tokio::test]
async fn falls_back_to_the_extension_without_a_content_type() {
    let server = serve(
        "/page.html",
        ResponseTemplate::new(200).set_body_raw("<p>Hi</p>", "application/octet-stream"),
    )
    .await;

    let Ok(Page::Fetched { text, .. }) =
        fetch_page(&format!("{}/page.html", server.uri()), &none()).await
    else {
        panic!("not fetched");
    };
    assert_eq!(text, "Hi");
}

#[tokio::test]
async fn decodes_latin1_from_the_header() {
    let server = serve(
        "/a",
        ResponseTemplate::new(200)
            .set_body_bytes(vec![b'c', b'a', b'f', 0xE9])
            .insert_header("content-type", "text/plain; charset=ISO-8859-1"),
    )
    .await;

    let Ok(Page::Fetched { text, .. }) = fetch_page(&format!("{}/a", server.uri()), &none()).await
    else {
        panic!("not fetched");
    };
    assert_eq!(text, "caf\u{e9}");
}

#[tokio::test]
async fn refuses_pdf_and_other_binary_types() {
    let server = serve(
        "/a.pdf",
        ResponseTemplate::new(200)
            .set_body_bytes(b"%PDF".to_vec())
            .insert_header("content-type", "application/pdf"),
    )
    .await;

    assert_eq!(
        fetch_page(&format!("{}/a.pdf", server.uri()), &none()).await,
        Err(
            "unsupported content type `application/pdf`; only text, Markdown and HTML are indexed"
                .into()
        )
    );
}

#[tokio::test]
async fn refuses_a_page_over_the_size_limit() {
    let server = serve(
        "/big",
        ResponseTemplate::new(200)
            .set_body_bytes(vec![b'x'; MAX_PAGE_BYTES + 1])
            .insert_header("content-type", "text/plain"),
    )
    .await;

    assert_eq!(
        fetch_page(&format!("{}/big", server.uri()), &none()).await,
        Err("larger than 5 MiB".into())
    );
}

#[tokio::test]
async fn reports_http_errors() {
    let server = serve("/gone", ResponseTemplate::new(404)).await;

    assert_eq!(
        fetch_page(&format!("{}/gone", server.uri()), &none()).await,
        Err("HTTP 404 Not Found".into())
    );
}

#[tokio::test]
async fn follows_same_origin_redirects() {
    let server = MockServer::start().await;
    Mock::given(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
        .mount(&server)
        .await;
    Mock::given(path("/new"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("moved", "text/plain"))
        .mount(&server)
        .await;

    let Ok(Page::Fetched { text, .. }) =
        fetch_page(&format!("{}/old", server.uri()), &none()).await
    else {
        panic!("not fetched");
    };
    assert_eq!(text, "moved");
}

#[tokio::test]
async fn refuses_redirects_to_another_origin() {
    let server = serve(
        "/old",
        ResponseTemplate::new(302).insert_header("location", "http://localhost:9/elsewhere"),
    )
    .await;

    assert_eq!(
        fetch_page(&format!("{}/old", server.uri()), &none()).await,
        Err("redirects to another origin (localhost); list the final URL instead".into())
    );
}

#[tokio::test]
async fn names_the_cause_of_a_failed_connection() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/a");

    let error = fetch_page(&url, &none()).await.unwrap_err();

    assert!(error.starts_with("request failed: "), "{error}");
    assert!(error.to_lowercase().contains("connect"), "{error}");
    assert!(!error.contains("127.0.0.1"), "{error}");
}
