use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use context_server::{Tool, ToolContent, ToolExecutor};
use htmd::HtmlToMarkdown;
use http_client::{HttpClient, Request, RequestBuilderExt, ResponseAsyncBodyExt, http::Method};
use indoc::formatdoc;
use readability::Readability;
use serde_json::{Value, json};
use url::Url;

pub struct ReadUrlTool(Arc<dyn HttpClient>);

impl ReadUrlTool {
    pub fn new(http_client: Arc<dyn HttpClient>) -> Self {
        ReadUrlTool(http_client)
    }
}

#[async_trait]
impl ToolExecutor for ReadUrlTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        let url = extract_url(arguments)?;

        let result = fetch_and_process(&self.0, url).await;

        Ok(vec![ToolContent::Text { text: result? }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "read_url".into(),
            description: Some(indoc::formatdoc! {"
                    This tool retrieves the content of a target web page directly from the internet, allowing access to and extraction of textual information from online sources. It is used when you have a clear HTTP(s) URL and need to fetch content from the web, such as articles, documentation, product information, or real-time data.

                    The tool enables you to provide current and accurate information by directly accessing web content. It's particularly useful for answering questions that require up-to-date data or fact-checking information against online sources. Always ensure you have a valid and complete HTTP(s) URL before using this tool to retrieve web content.
                "}),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL of the web page to fetch content from. This should be a valid web address (e.g., https://www.example.com) of the specific page you want to retrieve information from. Ensure the URL is complete and correctly formatted for accurate results."
                    }
                },
                "required": ["url"]
            }),
        }
    }
}

pub struct FetchRawTool(Arc<dyn HttpClient>);

impl FetchRawTool {
    pub fn new(http_client: Arc<dyn HttpClient>) -> Self {
        FetchRawTool(http_client)
    }
}

#[async_trait]
impl ToolExecutor for FetchRawTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        let url = extract_url(arguments)?;
        let result = fetch_raw(&self.0, url).await;
        Ok(vec![ToolContent::Text { text: result? }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "fetch_raw".into(),
            description: Some(indoc::formatdoc! {"
                    This tool retrieves the raw content of a target web page directly from the internet, without any processing or formatting. It returns the original response text as-is. Use this when you need the unmodified HTML or other content from a URL. Ideal for TXT formats.

                    The tool is useful when you need to analyze the raw structure of a webpage or when dealing with non-HTML content types where processing might alter the data. Always ensure you have a valid and complete HTTP(s) URL before using this tool.
                "}),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL of the web page to fetch raw content from. This should be a valid web address (e.g., https://www.example.com) of the specific page you want to retrieve information from. Ensure the URL is complete and correctly formatted for accurate results."
                    }
                },
                "required": ["url"]
            }),
        }
    }
}

async fn fetch_raw<H, S>(http_client: H, url: S) -> Result<String>
where
    H: HttpClient,
    S: AsRef<str>,
{
    let response = http_client
        .send(
            Request::builder()
                .method(Method::GET)
                .uri(url.as_ref())
                .end()?,
        )
        .await?;

    let body = response.text().await?;
    Ok(body)
}

async fn fetch_and_process<H, S>(http_client: H, url: S) -> Result<String>
where
    H: HttpClient,
    S: AsRef<str>,
{
    let response = http_client
        .send(
            Request::builder()
                .method(Method::GET)
                .uri(url.as_ref())
                .end()?,
        )
        .await?;

    let body = response.text().await?;
    let url_parsed = Url::parse(url.as_ref())?;

    // First try with our improved readability parser
    let mut readability = Readability::new(&body).with_url(url_parsed);
    let article_result = readability.parse();
    match article_result {
        Ok(article) => {
            let title = article.title;
            let byline = article.byline.unwrap_or_default();
            let markdown = article.content;
            let url_str = url.as_ref();
            let site_name = article.site_name.unwrap_or_default();

            let mut result = String::new();

            if !site_name.is_empty() {
                result.push_str(&format!("_{}_\n\n", site_name));
            }

            result.push_str(&format!("# {}\n", title));

            if !byline.is_empty() {
                result.push_str(&format!("by {}\n", byline));
            }

            if let Some(date_published) = article.date_published {
                result.push_str(&format!("{}\n", date_published.format("%d %B %Y")));
            }

            result.push_str(&format!("Available at {}\n\n", url_str));
            result.push_str("---\n\n");
            result.push_str(&markdown);

            Ok(result)
        }
        Err(_) => {
            let title = extract_title(&body).unwrap_or_else(|| "No title found".to_string());

            let converter = HtmlToMarkdown::builder()
                .skip_tags(vec!["script", "style"])
                .build();

            let markdown = converter
                .convert(&body)
                .map_err(|e| anyhow!("Failed to convert HTML to markdown: {}", e))?;

            let url_str = url.as_ref();

            Ok(formatdoc! {"
                Title: {title}
                URL: {url_str}

                {markdown}
            "})
        }
    }
}

fn extract_url(arguments: Option<Value>) -> Result<String> {
    let field_data = arguments
        .as_ref()
        .ok_or_else(|| anyhow!("missing arguments"))?
        .get("url")
        .ok_or_else(|| anyhow!("missing url"))?
        .clone();

    let url = field_data
        .as_str()
        .ok_or_else(|| anyhow!("url is not a string"))?
        .to_string();

    Ok(url)
}

fn extract_title(html: &str) -> Option<String> {
    let title = html
        .split("<title>")
        .nth(1)
        .and_then(|s| s.split("</title>").next())
        .map(|s| s.trim().to_string());

    title
}
