use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use url::Url;

// Compile regular expressions for detecting candidate elements
static UNLIKELY_PATTERNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"-ad-|ai2html|banner|breadcrumbs|combx|comment|community|cover-wrap|disqus|extra|footer|gdpr|header|legends|menu|related|remark|replies|rss|shoutbox|sidebar|skyscraper|social|sponsor|supplemental|ad-break|agegate|pagination|pager|popup"
).unwrap()
});

static POSITIVE_PATTERNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"article|body|content|entry|hentry|h-entry|main|page|pagination|post|text|blog|story",
    )
    .unwrap()
});

static NEGATIVE_PATTERNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"-ad-|hidden|^hid$| hid$| hid |^hid |banner|combx|comment|com-|contact|footer|gdpr|masthead|media|meta|outbrain|promo|related|scroll|share|shoutbox|sidebar|skyscraper|sponsor|shopping|tags|widget"
).unwrap()
});

/// Output of the readability parser containing the extracted article content
#[derive(Debug)]
pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    pub content: String,
    pub site_name: Option<String>,
    pub date_published: Option<DateTime<Utc>>,
}

/// Content score for each candidate element
#[derive(Debug)]
struct ContentScore {
    score: f32,
    element: ElementRef<'static>,
}

/// Main readability parser that extracts article content from HTML
pub struct Readability {
    document: Html,
    article_title: Option<String>,
    article_byline: Option<String>,
    site_name: Option<String>,
    content_candidates: Vec<ContentScore>,
    base_url: Option<Url>,
    date_published: Option<DateTime<Utc>>,
}

impl Readability {
    /// Create a new readability parser for the given HTML content
    pub fn new(html: &str) -> Self {
        let document = Html::parse_document(html);

        Self {
            document,
            article_title: None,
            article_byline: None,
            site_name: None,
            content_candidates: Vec::new(),
            base_url: None,
            date_published: None,
        }
    }

    /// Set the base URL for resolving relative URLs
    pub fn with_url(mut self, url: Url) -> Self {
        self.base_url = Some(url);
        self
    }

    /// Parse the document and extract the article content
    pub fn parse(&mut self) -> Result<Article> {
        // Parse article title
        self.article_title = self.parse_article_title();

        // Parse byline
        self.article_byline = self.parse_byline();

        // Parse site name
        self.site_name = self.parse_site_name();

        // Parse publication date
        self.date_published = self.parse_date_published();

        // Clean the document (remove unlikely elements like scripts, etc)
        self.prep_document();

        // Find candidate elements
        self.find_content_candidates();

        // Extract main content
        let content = self.extract_article_content()?;

        // Convert content to markdown
        let markdown = self.convert_to_markdown(&content);

        // Build article object
        let title = self
            .article_title
            .clone()
            .unwrap_or_else(|| "Untitled Article".to_string());

        Ok(Article {
            title,
            byline: self.article_byline.clone(),
            content: markdown,
            site_name: self.site_name.clone(),
            date_published: self.date_published,
        })
    }

    /// Parse the article title from the document
    fn parse_article_title(&self) -> Option<String> {
        // Try to get the title from the <title> element
        let title_selector = Selector::parse("title").unwrap();

        if let Some(title_element) = self.document.select(&title_selector).next() {
            let title = title_element.text().collect::<Vec<_>>().join("");
            return Some(title.trim().to_string());
        }

        None
    }

    /// Parse the article byline (author info)
    fn parse_byline(&self) -> Option<String> {
        // Check meta authors-name tag (which might contain multiple authors)
        if let Ok(meta_authors_name_selector) = Selector::parse("meta[name=\"authors-name\"]") {
            if let Some(element) = self.document.select(&meta_authors_name_selector).next() {
                if let Some(content) = element.value().attr("content") {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() && trimmed.len() < 100 {
                        // Check if the content contains multiple authors
                        if trimmed.contains(',') {
                            let authors: Vec<String> = trimmed
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();

                            if authors.len() == 2 {
                                return Some(format!("{} and {}", authors[0], authors[1]));
                            } else if authors.len() > 2 {
                                let last = authors.last().unwrap().clone();
                                let others = authors[0..authors.len() - 1].join(", ");
                                return Some(format!("{} and {}", others, last));
                            } else if authors.len() == 1 {
                                return Some(authors[0].clone());
                            }
                        } else if trimmed.contains('|') {
                            let authors: Vec<String> = trimmed
                                .split('|')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();

                            if authors.len() == 2 {
                                return Some(format!("{} and {}", authors[0], authors[1]));
                            } else if authors.len() > 2 {
                                let last = authors.last().unwrap().clone();
                                let others = authors[0..authors.len() - 1].join(", ");
                                return Some(format!("{} and {}", others, last));
                            } else if authors.len() == 1 {
                                return Some(authors[0].clone());
                            }
                        } else {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }

        // Check meta author tags
        if let Ok(meta_author_selector) = Selector::parse("meta[name=\"author\"]") {
            let mut meta_authors = Vec::new();

            // Collect all meta author tags
            for element in self.document.select(&meta_author_selector) {
                if let Some(content) = element.value().attr("content") {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() && trimmed.len() < 100 {
                        meta_authors.push(trimmed.to_string());
                    }
                }
            }

            // Process multiple meta authors if found
            if meta_authors.len() > 1 {
                // Remove duplicate authors
                meta_authors.sort();
                meta_authors.dedup();

                if meta_authors.len() == 2 {
                    return Some(format!("{} and {}", meta_authors[0], meta_authors[1]));
                } else if meta_authors.len() > 2 {
                    let last = meta_authors.pop().unwrap();
                    let others = meta_authors.join(", ");
                    return Some(format!("{} and {}", others, last));
                }
            } else if meta_authors.len() == 1 {
                return Some(meta_authors[0].clone());
            }
        }

        // Common selectors for bylines
        let byline_selectors = [
            ".byline",
            ".author",
            ".article-author",
            "[rel=\"author\"]",
            "[itemprop=\"author\"]",
            ".authors",
            ".contributors",
            ".entry-author",
            ".post-author",
            ".meta-author",
        ];

        // Try each selector
        for selector_str in byline_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                // First try to collect all matching elements for multiple authors
                let mut authors = Vec::new();

                for element in self.document.select(&selector) {
                    let text = element.text().collect::<Vec<_>>().join("");
                    let trimmed = text.trim();

                    if !trimmed.is_empty() && trimmed.len() < 100 {
                        authors.push(trimmed.to_string());
                    }
                }

                // If we found multiple authors, join them appropriately
                if authors.len() > 1 {
                    // Remove duplicate authors
                    authors.sort();
                    authors.dedup();

                    if authors.len() == 2 {
                        return Some(format!("{} and {}", authors[0], authors[1]));
                    } else if authors.len() > 2 {
                        let last = authors.pop().unwrap();
                        let others = authors.join(", ");
                        return Some(format!("{} and {}", others, last));
                    }
                } else if authors.len() == 1 {
                    return Some(authors[0].clone());
                }

                // If we didn't find multiple elements but there's a single element that might contain multiple authors
                if let Some(element) = self.document.select(&selector).next() {
                    let text = element.text().collect::<Vec<_>>().join("");
                    let trimmed = text.trim();

                    if !trimmed.is_empty() && trimmed.len() < 100 {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }

        None
    }

    /// Parse the publication date from the document
    fn parse_date_published(&self) -> Option<DateTime<Utc>> {
        // Try common meta tags for publication date
        let date_meta_selectors = [
            "meta[property=\"article:published_time\"]",
            "meta[name=\"publication_date\"]",
            "meta[name=\"date\"]",
            "meta[name=\"pubdate\"]",
            "meta[property=\"og:published_time\"]",
            "meta[itemprop=\"datePublished\"]",
        ];

        // Try each meta selector
        for selector_str in date_meta_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = self.document.select(&selector).next() {
                    if let Some(date_str) = element.value().attr("content") {
                        if let Some(date) = self.parse_date_string(date_str) {
                            return Some(date);
                        }
                    }
                }
            }
        }

        // Try common date elements in the document
        let date_element_selectors = [
            "time[datetime]",
            ".published[datetime]",
            "[itemprop=\"datePublished\"]",
            ".post-date",
            ".entry-date",
            ".pubdate",
            ".article-date",
            ".date",
            ".time",
            ".timestamp",
        ];

        for selector_str in date_element_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = self.document.select(&selector).next() {
                    // First try the datetime attribute
                    if let Some(date_str) = element.value().attr("datetime") {
                        if let Some(date) = self.parse_date_string(date_str) {
                            return Some(date);
                        }
                    }

                    // Then try the content attribute
                    if let Some(date_str) = element.value().attr("content") {
                        if let Some(date) = self.parse_date_string(date_str) {
                            return Some(date);
                        }
                    }

                    // Then try the element text content
                    let text = element
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string();

                    if !text.is_empty() {
                        if let Some(date) = self.parse_date_string(&text) {
                            return Some(date);
                        }
                    }
                }
            }
        }

        // If all else fails, try to find any date-like text in the document
        // Look for text that might represent dates (e.g. "Published on March 2022" or "© 2023")
        if let Ok(selector) = Selector::parse("p, div, span, small, time") {
            for element in self.document.select(&selector) {
                let text = element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();

                if text.contains("published") || text.contains("Posted") || text.contains("Date") {
                    if let Some(date) = self.extract_date_from_text(&text) {
                        return Some(date);
                    }
                }
            }
        }

        None
    }

    /// Attempts to parse a date string in various formats
    fn parse_date_string(&self, date_str: &str) -> Option<DateTime<Utc>> {
        // RFC 3339 / ISO 8601 (most common for structured data)
        if let Ok(date) = DateTime::parse_from_rfc3339(date_str) {
            return Some(date.with_timezone(&Utc));
        }

        // Common date formats
        let formats = [
            // Full date-time formats
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%dT%H:%M:%S",
            "%Y/%m/%d %H:%M:%S",
            "%d/%m/%Y %H:%M:%S",
            "%m/%d/%Y %H:%M:%S",
            // Date formats with no time
            "%Y-%m-%d",
            "%Y/%m/%d",
            "%d/%m/%Y",
            "%m/%d/%Y",
            "%B %d, %Y",
            "%d %B %Y",
            "%d %b %Y",
            "%B %d %Y",
            "%b %d, %Y",
            // Month-year formats
            "%B %Y",
            "%b %Y",
            "%m/%Y",
            "%m-%Y",
            // Year only
            "%Y",
        ];

        // Try each format
        for format in &formats {
            // Try as NaiveDateTime first
            if let Ok(naive_date) = NaiveDateTime::parse_from_str(date_str, format) {
                return Some(DateTime::from_naive_utc_and_offset(naive_date, Utc));
            }

            // Then try as NaiveDate and set time to midnight
            if let Ok(naive_date) = chrono::NaiveDate::parse_from_str(date_str, format) {
                let datetime = naive_date.and_hms_opt(0, 0, 0).unwrap();
                return Some(DateTime::from_naive_utc_and_offset(datetime, Utc));
            }
        }

        // Try to extract a year, month, or full date from the string
        self.extract_date_from_text(date_str)
    }

    /// Attempts to extract date components from arbitrary text
    fn extract_date_from_text(&self, text: &str) -> Option<DateTime<Utc>> {
        // Extract four-digit year
        if let Some(year_cap) = Regex::new(r"\b(19\d{2}|20\d{2})\b").ok()?.captures(text) {
            if let Some(year_match) = year_cap.get(1) {
                let year: i32 = year_match.as_str().parse().ok()?;

                // Look for month names or numbers near the year
                let months = [
                    "january",
                    "february",
                    "march",
                    "april",
                    "may",
                    "june",
                    "july",
                    "august",
                    "september",
                    "october",
                    "november",
                    "december",
                    "jan",
                    "feb",
                    "mar",
                    "apr",
                    "may",
                    "jun",
                    "jul",
                    "aug",
                    "sep",
                    "oct",
                    "nov",
                    "dec",
                ];

                let lowercase_text = text.to_lowercase();

                // Check if any month name is in the text
                for (i, &month) in months.iter().enumerate() {
                    if lowercase_text.contains(month) {
                        // Get month number (1-12)
                        let month_num = (i % 12) + 1;

                        // Check for day number (1-31)
                        if let Some(day_cap) = Regex::new(r"\b(\d{1,2})(st|nd|rd|th)?\b")
                            .ok()?
                            .captures(text)
                        {
                            if let Some(day_match) = day_cap.get(1) {
                                let day: u32 = day_match.as_str().parse().ok()?;
                                if day > 0 && day <= 31 {
                                    // We have year, month, day
                                    if let Some(date) =
                                        chrono::NaiveDate::from_ymd_opt(year, month_num as u32, day)
                                    {
                                        return Some(DateTime::from_naive_utc_and_offset(
                                            date.and_hms_opt(0, 0, 0).unwrap(),
                                            Utc,
                                        ));
                                    }
                                }
                            }
                        }

                        // If no day found, use the 1st of the month
                        if let Some(date) =
                            chrono::NaiveDate::from_ymd_opt(year, month_num as u32, 1)
                        {
                            return Some(DateTime::from_naive_utc_and_offset(
                                date.and_hms_opt(0, 0, 0).unwrap(),
                                Utc,
                            ));
                        }
                    }
                }

                // If only year is found, use January 1st
                if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, 1, 1) {
                    return Some(DateTime::from_naive_utc_and_offset(
                        date.and_hms_opt(0, 0, 0).unwrap(),
                        Utc,
                    ));
                }
            }
        }

        None
    }

    /// Parse the site name from the document
    fn parse_site_name(&self) -> Option<String> {
        // Try to get the site name from OpenGraph meta tags
        if let Ok(og_site_name_selector) = Selector::parse("meta[property=\"og:site_name\"]") {
            if let Some(element) = self.document.select(&og_site_name_selector).next() {
                if let Some(content) = element.value().attr("content") {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }

        // Try to get the site name from the domain if we have a base URL
        if let Some(url) = &self.base_url {
            if let Some(host) = url.host_str() {
                // Remove www. if present
                let host = host.strip_prefix("www.").unwrap_or(host);

                // Extract just the domain name, not the TLD
                if let Some(domain) = host.split('.').next() {
                    // Capitalize first letter
                    let mut domain_chars = domain.chars();
                    if let Some(first_char) = domain_chars.next() {
                        let capitalized =
                            first_char.to_uppercase().collect::<String>() + domain_chars.as_str();
                        return Some(capitalized);
                    }
                    return Some(domain.to_string());
                }
                return Some(host.to_string());
            }
        }

        // Try to extract from meta application-name
        if let Ok(app_name_selector) = Selector::parse("meta[name=\"application-name\"]") {
            if let Some(element) = self.document.select(&app_name_selector).next() {
                if let Some(content) = element.value().attr("content") {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }

        None
    }

    /// Prepare the document for content extraction by removing unnecessary elements
    fn prep_document(&mut self) {
        // This implementation is simplified compared to readability.js
        // Remove script tags
        if let Ok(script_selector) = Selector::parse("script, style, noscript") {
            // In a real implementation we would remove these nodes
            // For this exercise, we're just identifying them
            let _scripts = self.document.select(&script_selector);
        }
    }

    /// Find and score content candidates based on the readability algorithm
    fn find_content_candidates(&mut self) {
        // First, remove scripts, styles, and other unwanted elements
        self.prep_document();

        // Step 1: Find all paragraphs
        let paragraph_selectors = [
            "p",
            "div",
            "section",
            "article",
            "main",
            ".content",
            "#content",
            ".post",
            ".article",
            "[itemprop=\"articleBody\"]",
            "td",
            "pre",
        ];

        let mut paragraphs = Vec::new();
        for selector_str in paragraph_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                for element in self.document.select(&selector) {
                    // Skip elements that are likely to be noise
                    if self.is_unlikely_candidate(&element) {
                        continue;
                    }

                    // Only consider elements with sufficient text
                    let text = element
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string();
                    if text.len() < 25 {
                        continue;
                    }

                    // Convert to 'static lifetime to store in our list (this is a hack)
                    let element_static: ElementRef<'static> =
                        unsafe { std::mem::transmute(element) };
                    paragraphs.push(element_static);
                }
            }
        }

        // Step 2: Score each paragraph and its parent elements
        for paragraph in paragraphs {
            let text = paragraph.text().collect::<Vec<_>>().join(" ");

            // Calculate initial score based on text properties
            let mut content_score = 1.0;

            // Add points for commas
            content_score += text.matches(',').count() as f32 * 0.1;

            // Add points for text length (up to 3 additional points)
            content_score += (text.len() as f32 / 100.0).min(3.0);

            // Adjust score based on element tag
            match paragraph.value().name() {
                "div" => content_score += 5.0,
                "pre" | "td" | "blockquote" => content_score += 3.0,
                "address" | "ol" | "ul" | "dl" | "dd" | "dt" | "li" | "form" => {
                    content_score -= 3.0
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => content_score -= 5.0,
                _ => {}
            }

            // Adjust score based on class and ID attributes
            content_score += self.get_class_weight(&paragraph);

            // Propagate score to parent nodes with diminishing weight
            let mut current = paragraph;
            let mut level = 0;

            // Try to get up to 5 parent levels (usually at most 3 are useful)
            while level < 5 {
                // Move to parent element
                match current.parent() {
                    Some(parent_node) => {
                        if let Some(parent) = ElementRef::wrap(parent_node) {
                            // Convert to 'static lifetime (this is a hack)
                            let parent_static: ElementRef<'static> =
                                unsafe { std::mem::transmute(parent) };

                            // Calculate score divider based on distance from paragraph
                            let divider = if level == 0 {
                                1.0
                            } else if level == 1 {
                                2.0
                            } else {
                                level as f32 * 3.0
                            };

                            // Add to candidates list, or update existing score
                            if let Some(existing) = self.content_candidates.iter_mut().find(|c| {
                                std::ptr::eq(
                                    c.element.value() as *const _,
                                    parent_static.value() as *const _,
                                )
                            }) {
                                existing.score += content_score / divider;
                            } else {
                                self.content_candidates.push(ContentScore {
                                    score: content_score / divider,
                                    element: parent_static,
                                });
                            }

                            // Move up to next parent
                            current = parent;
                            level += 1;
                        } else {
                            break; // Can't wrap as element
                        }
                    }
                    None => break, // No more parents
                }
            }
        }

        // If no candidates found, use the <body> element as fallback
        if self.content_candidates.is_empty() {
            if let Ok(body_selector) = Selector::parse("body") {
                if let Some(body) = self.document.select(&body_selector).next() {
                    // Convert from ElementRef<'_> to ElementRef<'static>
                    let body_static: ElementRef<'static> = unsafe { std::mem::transmute(body) };

                    self.content_candidates.push(ContentScore {
                        score: 0.5, // Lower score for body
                        element: body_static,
                    });
                }
            }
        }

        // Apply link density penalty to all candidates
        // First, compute link densities for all candidates
        let mut link_densities = Vec::new();

        for candidate in &self.content_candidates {
            let link_density = self.get_link_density(&candidate.element);
            link_densities.push(link_density);
        }

        // Then apply the penalties
        for (i, candidate) in self.content_candidates.iter_mut().enumerate() {
            if i < link_densities.len() {
                candidate.score *= 1.0 - link_densities[i];
            }
        }
    }

    /// Determine if an element is unlikely to be a content candidate
    fn is_unlikely_candidate(&self, element: &ElementRef) -> bool {
        // Get class and id of the element
        let class = element.value().attr("class").unwrap_or("");
        let id = element.value().attr("id").unwrap_or("");
        let combined = format!("{} {}", class, id);

        // Check against unlikely patterns
        if UNLIKELY_PATTERNS.is_match(&combined) && !POSITIVE_PATTERNS.is_match(&combined) {
            // Skip if we're inside certain elements or the element is the body
            if element.value().name() == "body"
                || self.has_ancestor(element, "table")
                || self.has_ancestor(element, "code")
            {
                return false;
            }

            return true;
        }

        // Check ARIA roles that typically indicate non-content areas
        let role = element.value().attr("role").unwrap_or("");
        let unlikely_roles = [
            "menu",
            "menubar",
            "complementary",
            "navigation",
            "alert",
            "alertdialog",
            "dialog",
        ];
        if unlikely_roles.contains(&role) {
            return true;
        }

        false
    }

    /// Check if element has an ancestor with the given tag name
    fn has_ancestor(&self, element: &ElementRef, tag_name: &str) -> bool {
        let uppercase_tag = tag_name.to_uppercase();
        let mut current = element.parent();

        while let Some(parent_node) = current {
            if let Some(parent) = ElementRef::wrap(parent_node) {
                if parent.value().name().to_uppercase() == uppercase_tag {
                    return true;
                }
                current = parent.parent();
            } else {
                break;
            }
        }

        false
    }

    /// Get a score adjustment based on class and id attributes
    fn get_class_weight(&self, element: &ElementRef) -> f32 {
        let mut weight = 0.0;

        // Check class attribute
        if let Some(class_attr) = element.value().attr("class") {
            if !class_attr.is_empty() {
                if NEGATIVE_PATTERNS.is_match(class_attr) {
                    weight -= 25.0;
                }

                if POSITIVE_PATTERNS.is_match(class_attr) {
                    weight += 25.0;
                }
            }
        }

        // Check id attribute
        if let Some(id_attr) = element.value().attr("id") {
            if !id_attr.is_empty() {
                if NEGATIVE_PATTERNS.is_match(id_attr) {
                    weight -= 25.0;
                }

                if POSITIVE_PATTERNS.is_match(id_attr) {
                    weight += 25.0;
                }
            }
        }

        weight
    }

    /// Calculate the density of links in an element
    fn get_link_density(&self, element: &ElementRef) -> f32 {
        // Get all text in the element
        let text_length = element.text().collect::<Vec<_>>().join(" ").len() as f32;
        if text_length == 0.0 {
            return 0.0;
        }

        // Get the length of text in links
        let mut link_length = 0.0;
        if let Ok(link_selector) = Selector::parse("a") {
            for link in element.select(&link_selector) {
                // Get link text length
                let link_text = link.text().collect::<Vec<_>>().join(" ");
                link_length += link_text.len() as f32;
            }
        }

        // Calculate link density as ratio of link text to all text
        link_length / text_length
    }

    /// Extract the main article content
    fn extract_article_content(&self) -> Result<ElementRef> {
        // Get the top candidate
        if let Some(top_candidate) = self.content_candidates.iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            // Get the base content from the top candidate
            let content = top_candidate.element;

            // Now we would typically:
            // 1. Clean up the content by removing unlikely elements
            // 2. Fix relative URLs
            // 3. Remove empty paragraphs
            // 4. Improve formatting
            //
            // We'll handle most of these during markdown conversion since
            // our current borrowing model makes it difficult to clone and modify
            // the DOM tree directly

            Ok(content)
        } else {
            // If no candidates found, return error
            Err(anyhow!("No content found"))
        }
    }

    /// Convert HTML content to markdown
    fn convert_to_markdown(&self, content: &ElementRef) -> String {
        // Implement a more robust HTML to Markdown converter with
        // better handling for relative URLs and noise filtering

        let mut markdown = String::new();

        // Process all children recursively, filtering out noise elements
        self.html_to_markdown_recursive(content, &mut markdown, 0);

        // Clean up the markdown
        self.clean_markdown(&markdown)
    }

    /// Clean up the generated markdown to improve readability
    fn clean_markdown(&self, markdown: &str) -> String {
        // Remove excessive blank lines (more than 2 in a row)
        let mut cleaned = String::new();
        let mut blank_line_count = 0;

        for line in markdown.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                blank_line_count += 1;
                if blank_line_count <= 2 {
                    cleaned.push_str("\n");
                }
            } else {
                blank_line_count = 0;
                cleaned.push_str(line);
                cleaned.push('\n');
            }
        }

        cleaned
    }

    /// Recursively convert HTML to Markdown
    fn html_to_markdown_recursive(&self, element: &ElementRef, output: &mut String, depth: usize) {
        let tag_name = element.value().name();

        // Skip elements that are likely to be noise
        let class = element.value().attr("class").unwrap_or("");
        let id = element.value().attr("id").unwrap_or("");
        let combined = format!("{} {}", class, id);

        // Skip unliked patterns or social elements
        let noise_patterns = [
            "share",
            "social",
            "comment",
            "footer",
            "header",
            "nav",
            "advertisement",
            "sidebar",
            "menu",
            "related",
            "promo",
            "newsletter",
            "subscribe",
            "popup",
        ];

        // Check if this is a noise element
        let is_noise = noise_patterns
            .iter()
            .any(|&pattern| combined.contains(pattern));

        // Skip empty elements or those with no text content
        let has_text = !element
            .text()
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .is_empty();

        // Skip noise elements
        if is_noise && tag_name != "body" && tag_name != "article" && tag_name != "main" {
            // But we still need to process important elements
            let important_tags = ["h1", "h2", "h3", "h4", "h5", "h6", "p", "img"];
            if !important_tags.contains(&tag_name) {
                return;
            }
        }

        // Process element based on tag type
        match tag_name {
            "h1" => {
                output.push_str("# ");
                self.process_text_content(element, output);
                output.push_str("\n\n");
            }
            "h2" => {
                output.push_str("## ");
                self.process_text_content(element, output);
                output.push_str("\n\n");
            }
            "h3" => {
                output.push_str("### ");
                self.process_text_content(element, output);
                output.push_str("\n\n");
            }
            "h4" | "h5" | "h6" => {
                output.push_str("#### ");
                self.process_text_content(element, output);
                output.push_str("\n\n");
            }
            "p" => {
                // Skip empty paragraphs
                if has_text {
                    self.process_text_content(element, output);
                    output.push_str("\n\n");
                }
            }
            "a" => {
                // Handle links, fixing relative URLs when needed
                let href = element.value().attr("href").unwrap_or("");
                let text = element.text().collect::<Vec<_>>().join("");

                if text.trim().is_empty() {
                    return; // Skip empty links
                }

                // Fix relative URLs
                let fixed_href = self.fix_relative_url(href);

                output.push_str(&format!("[{}]({})", text, fixed_href));
            }
            "strong" | "b" => {
                output.push_str("**");
                self.process_text_content(element, output);
                output.push_str("**");
            }
            "em" | "i" => {
                output.push_str("*");
                self.process_text_content(element, output);
                output.push_str("*");
            }
            "ul" => {
                output.push_str("\n");
                // Process list items
                for child in element.children() {
                    if let Some(child_ref) = ElementRef::wrap(child) {
                        if child_ref.value().name() == "li" {
                            output.push_str("- ");
                            self.process_text_content(&child_ref, output);
                            output.push_str("\n");
                        }
                    }
                }
                output.push_str("\n");
            }
            "ol" => {
                output.push_str("\n");
                // Process ordered list items
                let mut counter = 1;
                for child in element.children() {
                    if let Some(child_ref) = ElementRef::wrap(child) {
                        if child_ref.value().name() == "li" {
                            output.push_str(&format!("{}. ", counter));
                            counter += 1;
                            self.process_text_content(&child_ref, output);
                            output.push_str("\n");
                        }
                    }
                }
                output.push_str("\n");
            }
            "blockquote" => {
                output.push_str("\n");
                // Split by lines and prefix each with '>'
                let text = element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    for line in text.lines() {
                        output.push_str(&format!("> {}\n", line.trim()));
                    }
                    output.push_str("\n");
                } else {
                    // Handle blockquotes with HTML content
                    let mut blockquote_content = String::new();
                    self.process_children(element, &mut blockquote_content, depth + 1);

                    if !blockquote_content.trim().is_empty() {
                        for line in blockquote_content.lines() {
                            if !line.trim().is_empty() {
                                output.push_str(&format!("> {}\n", line.trim()));
                            }
                        }
                        output.push_str("\n");
                    }
                }
            }
            "img" => {
                let src = element.value().attr("src").unwrap_or("");
                let alt = element.value().attr("alt").unwrap_or("");

                // Fix relative URLs for images
                let fixed_src = self.fix_relative_url(src);

                output.push_str(&format!("![{}]({})\n\n", alt, fixed_src));
            }
            "figure" => {
                // Handle figure elements with captions
                let mut img_src = String::new();
                let mut img_alt = String::new();
                let mut caption = String::new();

                // Find the image
                if let Ok(img_selector) = Selector::parse("img") {
                    if let Some(img) = element.select(&img_selector).next() {
                        img_src = img.value().attr("src").unwrap_or("").to_string();
                        img_alt = img.value().attr("alt").unwrap_or("").to_string();
                    }
                }

                // Find the caption
                if let Ok(figcaption_selector) = Selector::parse("figcaption") {
                    if let Some(figcaption) = element.select(&figcaption_selector).next() {
                        caption = figcaption
                            .text()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .trim()
                            .to_string();
                    }
                }

                // Fix relative URLs for images
                let fixed_src = self.fix_relative_url(&img_src);

                // Output the image and caption
                if !img_src.is_empty() {
                    output.push_str(&format!("![{}]({})\n", img_alt, fixed_src));
                    if !caption.is_empty() {
                        output.push_str(&format!("*{}*\n\n", caption));
                    } else {
                        output.push_str("\n");
                    }
                }
            }
            "code" | "pre" => {
                output.push_str("```\n");
                self.process_text_content(element, output);
                output.push_str("\n```\n\n");
            }
            "table" => {
                self.process_table(element, output);
            }
            "div" | "section" | "article" | "main" => {
                // Process these container elements recursively
                self.process_children(element, output, depth);
            }
            // For other tags, just process their children
            _ => {
                if has_text || tag_name == "body" {
                    self.process_children(element, output, depth);
                }
            }
        }
    }

    /// Process a table element into markdown
    fn process_table(&self, element: &ElementRef, output: &mut String) {
        // Get header cells
        let mut header_cells = Vec::new();
        if let Ok(thead_selector) = Selector::parse("thead th") {
            for cell in element.select(&thead_selector) {
                let text = cell.text().collect::<Vec<_>>().join(" ").trim().to_string();
                header_cells.push(text);
            }
        }

        // If no headers found, try to get the first row
        if header_cells.is_empty() {
            if let Ok(first_row_selector) = Selector::parse("tr:first-child th, tr:first-child td")
            {
                for cell in element.select(&first_row_selector) {
                    let text = cell.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    header_cells.push(text);
                }
            }
        }

        // If we have headers, render the table
        if !header_cells.is_empty() {
            output.push_str("\n");

            // Render header
            output.push_str("| ");
            for header in &header_cells {
                output.push_str(&format!("{} | ", header));
            }
            output.push_str("\n");

            // Render separator
            output.push_str("| ");
            for _ in &header_cells {
                output.push_str("--- | ");
            }
            output.push_str("\n");

            // Render rows
            if let Ok(row_selector) = Selector::parse("tbody tr") {
                for row in element.select(&row_selector) {
                    output.push_str("| ");

                    let mut cell_count = 0;
                    if let Ok(cell_selector) = Selector::parse("td") {
                        for cell in row.select(&cell_selector) {
                            let text = cell.text().collect::<Vec<_>>().join(" ").trim().to_string();
                            output.push_str(&format!("{} | ", text));
                            cell_count += 1;
                        }
                    }

                    // Fill in missing cells
                    for _ in cell_count..header_cells.len() {
                        output.push_str(" | ");
                    }

                    output.push_str("\n");
                }
            }

            output.push_str("\n");
        }
    }

    /// Fix relative URLs to absolute ones using the base URL
    fn fix_relative_url(&self, url: &str) -> String {
        // Skip empty URLs
        if url.is_empty() || url.starts_with("#") {
            return url.to_string();
        }

        // If it's already absolute, return as is
        if url.starts_with("http://") || url.starts_with("https://") {
            return url.to_string();
        }

        // If we have a base URL, resolve against it
        if let Some(base) = &self.base_url {
            // First, try to parse as a URL
            match base.join(url) {
                Ok(resolved) => return resolved.to_string(),
                Err(_) => {}
            }

            // Fallback manual resolution for edge cases
            if url.starts_with("/") {
                // Absolute path from domain root
                if let Some(domain) = base.host_str() {
                    let scheme = base.scheme();
                    return format!("{}://{}{}", scheme, domain, url);
                }
            } else {
                // Relative path from base
                let base_str = base.as_str();
                let base_path = if base_str.ends_with('/') {
                    base_str.to_string()
                } else {
                    // Get the directory part of the base URL
                    let path_parts: Vec<&str> = base_str.split('/').collect();
                    path_parts[..path_parts.len() - 1].join("/") + "/"
                };

                return format!("{}{}", base_path, url);
            }
        }

        // If no base URL or couldn't resolve, return as is
        url.to_string()
    }

    /// Process text content of an element
    fn process_text_content(&self, element: &ElementRef, output: &mut String) {
        for child in element.children() {
            match child.value() {
                scraper::Node::Text(text) => {
                    output.push_str(text);
                }
                scraper::Node::Element(_) => {
                    if let Some(child_ref) = ElementRef::wrap(child) {
                        self.html_to_markdown_recursive(&child_ref, output, 0);
                    }
                }
                _ => {}
            }
        }
    }

    /// Process child elements
    fn process_children(&self, element: &ElementRef, output: &mut String, depth: usize) {
        for child in element.children() {
            if let Some(child_ref) = ElementRef::wrap(child) {
                self.html_to_markdown_recursive(&child_ref, output, depth + 1);
            } else if let scraper::Node::Text(text) = child.value() {
                output.push_str(text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HTML: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Test Article Title</title>
        <meta property="og:site_name" content="Test Site Name">
    </head>
    <body>
        <header>
            <h1>Main Header</h1>
            <div class="byline">By Test Author</div>
        </header>
        <article>
            <p>This is the first paragraph of the test article.</p>
            <p>This is the second paragraph with <a href="http://example.com">a link</a>.</p>
        </article>
        <div class="sidebar">
            <p>This is sidebar content that should be removed.</p>
        </div>
        <footer>
            <p>Copyright 2025</p>
        </footer>
    </body>
    </html>
    "#;

    const RICH_HTML: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Rich Content Test</title>
        <link rel="canonical" href="https://example.org/original-page">
    </head>
    <body>
        <article>
            <h1>Main Heading</h1>
            <h2>Subheading</h2>
            <p>This is a paragraph with <strong>bold text</strong> and <em>italic text</em>.</p>
            <ul>
                <li>List item 1</li>
                <li>List item 2</li>
            </ul>
            <ol>
                <li>Numbered item 1</li>
                <li>Numbered item 2</li>
            </ol>
            <blockquote>
                This is a blockquote.
                It can span multiple lines.
            </blockquote>
            <p>Here's a <a href="https://example.com">link</a>.</p>
            <pre><code>
                // Some code
                function example() {
                    return true;
                }
            </code></pre>
            <figure>
                <img src="/images/test.jpg" alt="Test image">
                <figcaption>This is a test image caption</figcaption>
            </figure>
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_RELATIVE_LINKS: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Test Article with Relative Links</title>
        <base href="https://example.com/">
    </head>
    <body>
        <article>
            <p>This is a paragraph with <a href="/path/to/page">a relative link</a>.</p>
            <p>This is a paragraph with <a href="relative/path">another relative link</a>.</p>
            <img src="/images/test.jpg" alt="Test image">
            <img src="images/local.jpg" alt="Local image">
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_NOISE: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with Noise</title>
    </head>
    <body>
        <div class="container">
            <article class="main-content">
                <h1>Main Article</h1>
                <p>This is the main content.</p>
                <div class="empty"></div>
                <p></p>
                <div class="related">
                    <h3>Related Content</h3>
                    <ul>
                        <li><a href="https://example.com/related1">Related link 1</a></li>
                        <li><a href="https://example.com/related2">Related link 2</a></li>
                    </ul>
                </div>
                <div class="social-share">
                    <span>Share:</span>
                    <a href="https://example.com/facebook">Facebook</a>
                    <a href="https://example.com/twitter">Twitter</a>
                    <a href="https://example.com/linkedin">LinkedIn</a>
                </div>
            </article>
            <aside class="sidebar">
                <div class="ad">
                    <p>This is an advertisement</p>
                </div>
                <div class="newsletter">
                    <h4>Subscribe to our newsletter</h4>
                    <form>
                        <input type="email" placeholder="Email">
                        <button>Subscribe</button>
                    </form>
                </div>
            </aside>
        </div>
    </body>
    </html>
    "#;

    const HTML_WITH_MULTIPLE_AUTHORS: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with Multiple Authors</title>
        <meta name="author" content="Jane Smith">
        <meta name="author" content="John Doe">
    </head>
    <body>
        <article>
            <h1>Co-authored Article</h1>
            <p>This article has multiple authors.</p>
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_AUTHORS_NAME: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with Authors-Name</title>
        <meta name="authors-name" content="Jane Smith, John Doe, Mark Wilson">
    </head>
    <body>
        <article>
            <h1>Multiple Authors Article</h1>
            <p>This article has multiple authors in a single tag.</p>
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_AUTHORS_PIPES: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with Piped Authors</title>
        <meta name="authors-name" content="Jane Smith | John Doe | Mark Wilson">
    </head>
    <body>
        <article>
            <h1>Multiple Authors Article</h1>
            <p>This article has multiple authors separated by pipes.</p>
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_REL_AUTHOR: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with rel=author</title>
    </head>
    <body>
        <article>
            <h1>Article Title</h1>
            <p>This is an article with rel=author link.</p>
            <a rel="author" href="https://example.com/profile">James Johnson</a>
        </article>
    </body>
    </html>
    "#;

    const HTML_WITH_ITEMPROP_AUTHOR: &str = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <title>Article with itemprop=author</title>
    </head>
    <body>
        <article>
            <h1>Article Title</h1>
            <p>This is an article with itemprop=author span.</p>
            <span itemprop="author">Alice Williams</span>
        </article>
    </body>
    </html>
    "#;

    #[test]
    fn test_parse_article_title() {
        let readability = Readability::new(TEST_HTML);
        assert_eq!(
            readability.parse_article_title(),
            Some("Test Article Title".to_string())
        );
    }

    #[test]
    fn test_parse_byline() {
        let readability = Readability::new(TEST_HTML);
        assert_eq!(
            readability.parse_byline(),
            Some("By Test Author".to_string())
        );
    }

    #[test]
    fn test_parse_site_name() {
        let readability = Readability::new(TEST_HTML);
        assert_eq!(
            readability.parse_site_name(),
            Some("Test Site Name".to_string())
        );
    }

    #[test]
    fn test_full_parsing() {
        let mut readability = Readability::new(TEST_HTML);
        let article = readability.parse().unwrap();

        // Check basic properties
        assert_eq!(article.title, "Test Article Title");
        assert_eq!(article.byline, Some("By Test Author".to_string()));
        assert_eq!(article.site_name, Some("Test Site Name".to_string()));
    }

    #[test]
    fn test_html_to_markdown() {
        let mut readability = Readability::new(RICH_HTML);
        // Set the canonical URL from the HTML
        readability.base_url = Some(Url::parse("https://example.org/original-page").unwrap());
        readability.find_content_candidates();
        let content = readability.extract_article_content().unwrap();

        let markdown = readability.convert_to_markdown(&content);

        // Check that Markdown formatting was applied correctly
        assert!(markdown.contains("# Main Heading"));
        assert!(markdown.contains("## Subheading"));
        assert!(markdown.contains("**bold text**"));
        assert!(markdown.contains("*italic text*"));
        assert!(markdown.contains("- List item 1"));
        assert!(markdown.contains("1. Numbered item 1"));
        assert!(markdown.contains("> This is a blockquote."));
        assert!(markdown.contains("[link](https://example.com)"));
        assert!(markdown.contains("```"));

        // Check image with caption is properly formatted
        assert!(markdown.contains("![Test image](https://example.org/images/test.jpg)"));
        assert!(markdown.contains("*This is a test image caption*"));
    }

    #[test]
    fn test_fix_relative_urls() {
        let mut readability = Readability::new(HTML_WITH_RELATIVE_LINKS);
        readability.base_url = Some(Url::parse("https://example.com/article").unwrap());
        readability.find_content_candidates();
        let content = readability.extract_article_content().unwrap();

        let markdown = readability.convert_to_markdown(&content);

        // Check that relative links are converted to absolute
        assert!(markdown.contains("(https://example.com/path/to/page)"));
        assert!(markdown.contains("(https://example.com/relative/path)"));
        assert!(markdown.contains("(https://example.com/images/test.jpg)"));
        assert!(markdown.contains("(https://example.com/images/local.jpg)"));
    }

    #[test]
    fn test_clean_article_content() {
        let mut readability = Readability::new(HTML_WITH_NOISE);
        readability.find_content_candidates();
        let content = readability.extract_article_content().unwrap();

        let markdown = readability.convert_to_markdown(&content);

        // Check that the main content is kept
        assert!(markdown.contains("# Main Article"));
        assert!(markdown.contains("This is the main content."));

        // Check that empty elements are removed
        assert!(!markdown.contains("<div class=\"empty\">"));
        assert!(!markdown.contains("<p></p>"));

        // Check that social share links are removed
        assert!(!markdown.contains("Share:"));
        assert!(!markdown.contains("Facebook"));
        assert!(!markdown.contains("Twitter"));

        // Check that ads are removed
        assert!(!markdown.contains("This is an advertisement"));

        // Check that newsletter forms are removed
        assert!(!markdown.contains("Subscribe to our newsletter"));
    }

    #[test]
    fn test_parse_multiple_meta_authors() {
        let readability = Readability::new(HTML_WITH_MULTIPLE_AUTHORS);
        assert_eq!(
            readability.parse_byline(),
            Some("Jane Smith and John Doe".to_string())
        );
    }

    #[test]
    fn test_parse_authors_name_meta() {
        let readability = Readability::new(HTML_WITH_AUTHORS_NAME);
        assert_eq!(
            readability.parse_byline(),
            Some("Jane Smith, John Doe and Mark Wilson".to_string())
        );
    }

    #[test]
    fn test_parse_authors_with_pipes() {
        let readability = Readability::new(HTML_WITH_AUTHORS_PIPES);
        assert_eq!(
            readability.parse_byline(),
            Some("Jane Smith, John Doe and Mark Wilson".to_string())
        );
    }

    #[test]
    fn test_parse_rel_author() {
        let readability = Readability::new(HTML_WITH_REL_AUTHOR);
        assert_eq!(
            readability.parse_byline(),
            Some("James Johnson".to_string())
        );
    }

    #[test]
    fn test_parse_itemprop_author() {
        let readability = Readability::new(HTML_WITH_ITEMPROP_AUTHOR);
        assert_eq!(
            readability.parse_byline(),
            Some("Alice Williams".to_string())
        );
    }
}
