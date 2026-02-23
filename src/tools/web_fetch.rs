//! Web page fetching tool with HTML-to-text extraction.

use async_trait::async_trait;

use crate::tool::{Tool, ToolDefinition, ToolError, ToolRegistry};

/// Fetches the content of a web page and returns it as readable text.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("agentic/0.1")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_fetch".into(),
            description: "Fetch the content of a web page URL and return it as readable text. \
                          Use this when you need to read the contents of a specific web page."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum number of characters to return (default: 16000)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("missing 'url'".into()))?;

        let max_length = input
            .get("max_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(16_000) as usize;

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("fetch error: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "HTTP {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("error")
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read error: {e}")))?;

        // Simple HTML-to-text extraction
        let text = if content_type.contains("text/html") {
            strip_html(&body)
        } else {
            body
        };

        // Truncate if needed
        let text = if text.len() > max_length {
            let mut end = max_length.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = &text[..end];
            format!(
                "{}\n\n[Truncated — {} of {} characters shown]",
                truncated,
                max_length,
                text.len()
            )
        } else {
            text
        };

        Ok(text)
    }
}

/// Register the web_fetch tool into a registry.
pub fn register_tool(registry: &mut ToolRegistry) {
    registry.register(Box::new(WebFetchTool::new()));
}

/// Check whether `chars[pos..]` starts with the given ASCII `target`.
fn starts_with_at(chars: &[char], pos: usize, target: &str) -> bool {
    target
        .chars()
        .enumerate()
        .all(|(j, c)| chars.get(pos + j) == Some(&c))
}

/// Very basic HTML tag stripping. Removes tags, decodes common entities,
/// and collapses excessive whitespace.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = html.to_lowercase().chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && starts_with_at(&lower_chars, i, "<script") {
            in_script = true;
            in_tag = true;
            i += 1;
            continue;
        }
        if in_script && starts_with_at(&lower_chars, i, "</script>") {
            in_script = false;
            i += 9;
            continue;
        }
        if !in_tag && starts_with_at(&lower_chars, i, "<style") {
            in_style = true;
            in_tag = true;
            i += 1;
            continue;
        }
        if in_style && starts_with_at(&lower_chars, i, "</style>") {
            in_style = false;
            i += 8;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        let ch = chars[i];
        if ch == '<' {
            in_tag = true;
            // Add newline for block elements
            if i + 2 < len {
                let next_lower: String = lower_chars[i + 1..std::cmp::min(i + 4, len)]
                    .iter()
                    .collect();
                if next_lower.starts_with("br")
                    || next_lower.starts_with("p")
                    || next_lower.starts_with("/p")
                    || next_lower.starts_with("div")
                    || next_lower.starts_with("/di")
                    || next_lower.starts_with("h1")
                    || next_lower.starts_with("h2")
                    || next_lower.starts_with("h3")
                    || next_lower.starts_with("li")
                {
                    result.push('\n');
                }
            }
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }

        i += 1;
    }

    // Decode common HTML entities
    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");

    // Collapse whitespace: multiple blank lines → double newline
    let mut collapsed = String::with_capacity(result.len());
    let mut blank_count = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                collapsed.push('\n');
            }
        } else {
            blank_count = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }

    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<"));
    }

    #[test]
    fn strip_html_removes_scripts() {
        let html = "<p>Before</p><script>alert('hi');</script><p>After</p>";
        let text = strip_html(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn strip_html_removes_styles() {
        let html = "<p>Visible</p><style>body { color: red; }</style><p>Also visible</p>";
        let text = strip_html(html);
        assert!(text.contains("Visible"));
        assert!(text.contains("Also visible"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn strip_html_decodes_entities() {
        let html = "&amp; &lt; &gt; &quot; &#39;";
        let text = strip_html(html);
        assert_eq!(text, "& < > \" '");
    }

    #[test]
    fn tool_definition_schema() {
        let tool = WebFetchTool::new();
        let def = tool.definition();
        assert_eq!(def.name, "web_fetch");
        assert_eq!(def.input_schema["required"][0], "url");
    }

    #[test]
    fn strip_html_multibyte_utf8() {
        // Regression test for issue #1: panic on multi-byte UTF-8 characters.
        // The old code used char indices as byte offsets into the str, which
        // panics when multi-byte chars shift the byte position ahead of the
        // char index.
        let html = "<html><body>\
            <p>Diseño español con ñ y más café</p>\
            <script>var x = 'should be removed';</script>\
            <p>日本語テキスト</p>\
            <style>.emoji { content: '🦀'; }</style>\
            <p>After style</p>\
            </body></html>";
        let text = strip_html(html);
        assert!(text.contains("Diseño español"));
        assert!(text.contains("café"));
        assert!(text.contains("日本語テキスト"));
        assert!(text.contains("After style"));
        assert!(!text.contains("should be removed"));
        assert!(!text.contains("emoji"));
    }

    #[tokio::test]
    async fn missing_url_returns_error() {
        let tool = WebFetchTool::new();
        let err = tool.execute(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
