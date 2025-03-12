use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use context_server::{Tool, ToolContent, ToolExecutor};
use htmd::HtmlToMarkdown;
use http_client::{HttpClient, Request, RequestBuilderExt, ResponseAsyncBodyExt, http::Method};
use indoc::formatdoc;
use readability::{Article, Readability};
use scraper::Html;
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

fn evaluate_readability_quality(article: &Article, original_html: &str) -> f32 {
    let mut quality_score = 0.0;

    // 1. Content length - extremely short content is likely a failure
    let content_length = article.content.len();
    if content_length < 200 {
        quality_score -= 30.0;
    } else if content_length > 500 {
        quality_score += 15.0;
    }

    // 2. Content-to-boilerplate ratio
    let html_text_length = Html::parse_document(original_html)
        .root_element()
        .text()
        .collect::<String>()
        .len();

    if html_text_length > 0 {
        let content_ratio = content_length as f32 / html_text_length as f32;

        // If the extracted content is less than 10% of the original text, it's suspicious
        if content_ratio < 0.1 {
            quality_score -= 20.0;
        } else if content_ratio > 0.4 {
            quality_score += 10.0; // Good extraction typically captures 40%+ of meaningful text
        }
    }

    // 3. Content variety - good articles have a mix of elements
    let has_paragraphs = article.content.contains("\n\n");
    let has_headings = article.content.contains("# ") || article.content.contains("## ");
    let has_lists = article.content.contains("- ") || article.content.contains("1. ");

    if has_paragraphs {
        quality_score += 10.0;
    }
    if has_headings {
        quality_score += 5.0;
    }
    if has_lists {
        quality_score += 5.0;
    }

    // 4. Link density in extracted content
    let link_count = article.content.matches("](").count();
    let total_paragraphs = article.content.split("\n\n").count();

    if total_paragraphs > 0 {
        let link_density = link_count as f32 / total_paragraphs as f32;

        // If every paragraph has multiple links, it might be a navigation page
        if link_density > 2.0 {
            quality_score -= 15.0;
        }
    }

    // 5. Check for landing page patterns
    if article.content.contains("sign up")
        || article.content.contains("log in")
        || article.content.contains("cookie")
        || article.content.contains("privacy policy")
    {
        quality_score -= 5.0;
    }

    // Penalize placeholder content
    if article.title == "Untitled Article" || article.content.len() < 100 || !has_paragraphs {
        quality_score -= 25.0;
    }

    quality_score
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

    // Try with our improved readability parser
    let mut readability = Readability::new(&body).with_url(url_parsed.clone());
    let article_result = readability.parse();

    // Create HTML-to-Markdown converter for potential fallback
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style"])
        .build();

    let markdown_result = converter.convert(&body);

    match (article_result, markdown_result) {
        (Ok(article), Ok(markdown)) => {
            // Assess the quality of readability output
            let quality_score = evaluate_readability_quality(&article, &body);

            // Use readability if quality is good, otherwise use plain markdown
            if quality_score > 10.0 {
                // Good quality readability result - use it
                let title = article.title;
                let byline = article.byline.unwrap_or_default();
                let content = article.content;
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
                result.push_str(&content);

                Ok(result)
            } else {
                // Poor quality readability result - fall back to plain markdown
                let title = extract_title(&body).unwrap_or_else(|| "No title found".to_string());
                let url_str = url.as_ref();

                Ok(formatdoc! {"
                    Title: {title}
                    URL: {url_str}

                    {markdown}
                "})
            }
        }
        (Ok(article), Err(_)) => {
            // Readability worked but markdown conversion failed
            let title = article.title;
            let byline = article.byline.unwrap_or_default();
            let content = article.content;
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
            result.push_str(&content);

            Ok(result)
        }
        (Err(_), Ok(markdown)) => {
            // Readability failed but markdown conversion worked
            let title = extract_title(&body).unwrap_or_else(|| "No title found".to_string());
            let url_str = url.as_ref();

            Ok(formatdoc! {"
                Title: {title}
                URL: {url_str}

                {markdown}
            "})
        }
        (Err(e), Err(_)) => {
            // Both approaches failed
            Err(anyhow!("Failed to extract content: {}", e))
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
