//! The OAuth redirect landing page ported from upstream
//! `packages/ai/src/auth/oauth/oauth-page.ts`: the dark-themed HTML the local
//! callback server returns to the browser after the redirect (success and
//! failure variants). The template is ported byte-for-byte, including the pi
//! logo SVG and HTML escaping of every interpolated value.

const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 800" aria-hidden="true"><path fill="#F09082" d="M165.29 165.29H517.36V400H400V282.65H165.29Z"/><path fill="#4D9ABF" d="M165.29 282.65H282.65V400H400V517.36H282.65V634.72H165.29Z"/><path fill="#F1BE58" d="M517.36 400H634.72V634.72H517.36Z"/></svg>"##;

/// Upstream `escapeHtml` (oauth-page.ts:3-10): `&`, `<`, `>`, `"` and `'`.
fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Upstream `renderPage` (oauth-page.ts:12-92), assembled from static
/// segments so the CSS braces stay verbatim. The optional `details` block
/// interpolates as the empty string when absent — leaving the template's
/// four-space-indented empty line, like the upstream template literal.
fn render_page(title: &str, heading: &str, message: &str, details: Option<&str>) -> String {
    const HEAD: &str = concat!(
        "<!doctype html>\n",
        "<html lang=\"en\">\n",
        "<head>\n",
        "  <meta charset=\"utf-8\" />\n",
        "  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n",
        "  <title>",
    );
    const STYLE: &str = concat!(
        "</title>\n",
        "  <style>\n",
        "    :root {\n",
        "      --text: #fafafa;\n",
        "      --text-dim: #a1a1aa;\n",
        "      --page-bg: #09090b;\n",
        "      --font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"Noto Sans\", sans-serif, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\", \"Noto Color Emoji\";\n",
        "      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, \"Liberation Mono\", \"Courier New\", monospace;\n",
        "    }\n",
        "    * { box-sizing: border-box; }\n",
        "    html { color-scheme: dark; }\n",
        "    body {\n",
        "      margin: 0;\n",
        "      min-height: 100vh;\n",
        "      display: flex;\n",
        "      align-items: center;\n",
        "      justify-content: center;\n",
        "      padding: 24px;\n",
        "      background: var(--page-bg);\n",
        "      color: var(--text);\n",
        "      font-family: var(--font-sans);\n",
        "      text-align: center;\n",
        "    }\n",
        "    main {\n",
        "      width: 100%;\n",
        "      max-width: 560px;\n",
        "      display: flex;\n",
        "      flex-direction: column;\n",
        "      align-items: center;\n",
        "      justify-content: center;\n",
        "    }\n",
        "    .logo {\n",
        "      width: 72px;\n",
        "      height: 72px;\n",
        "      display: block;\n",
        "      margin-bottom: 24px;\n",
        "    }\n",
        "    h1 {\n",
        "      margin: 0 0 10px;\n",
        "      font-size: 28px;\n",
        "      line-height: 1.15;\n",
        "      font-weight: 650;\n",
        "      color: var(--text);\n",
        "    }\n",
        "    p {\n",
        "      margin: 0;\n",
        "      line-height: 1.7;\n",
        "      color: var(--text-dim);\n",
        "      font-size: 15px;\n",
        "    }\n",
        "    .details {\n",
        "      margin-top: 16px;\n",
        "      font-family: var(--font-mono);\n",
        "      font-size: 13px;\n",
        "      color: var(--text-dim);\n",
        "      white-space: pre-wrap;\n",
        "      word-break: break-word;\n",
        "    }\n",
        "  </style>\n",
        "</head>\n",
        "<body>\n",
        "  <main>\n",
        "    <div class=\"logo\">",
    );
    const BEFORE_HEADING: &str = "</div>\n    <h1>";
    const BEFORE_MESSAGE: &str = "</h1>\n    <p>";
    const BEFORE_DETAILS: &str = "</p>\n    ";
    const DETAILS_OPEN: &str = "<div class=\"details\">";
    const DETAILS_CLOSE: &str = "</div>";
    const TAIL: &str = "\n  </main>\n</body>\n</html>";

    let mut page = String::from(HEAD);
    page.push_str(&escape_html(title));
    page.push_str(STYLE);
    page.push_str(LOGO_SVG);
    page.push_str(BEFORE_HEADING);
    page.push_str(&escape_html(heading));
    page.push_str(BEFORE_MESSAGE);
    page.push_str(&escape_html(message));
    page.push_str(BEFORE_DETAILS);
    if let Some(details) = details {
        page.push_str(DETAILS_OPEN);
        page.push_str(&escape_html(details));
        page.push_str(DETAILS_CLOSE);
    }
    page.push_str(TAIL);
    page
}

/// Upstream `oauthSuccessHtml` (oauth-page.ts:94-100).
pub fn oauth_success_html(message: &str) -> String {
    render_page(
        "Authentication successful",
        "Authentication successful",
        message,
        None,
    )
}

/// Upstream `oauthErrorHtml` (oauth-page.ts:102-109).
pub fn oauth_error_html(message: &str, details: Option<&str>) -> String {
    render_page(
        "Authentication failed",
        "Authentication failed",
        message,
        details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full success page for a fixed message, pinned byte-for-byte
    /// against the upstream template output.
    #[test]
    fn success_html_matches_the_upstream_template_byte_for_byte() {
        let expected = "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  <title>Authentication successful</title>\n  <style>\n    :root {\n      --text: #fafafa;\n      --text-dim: #a1a1aa;\n      --page-bg: #09090b;\n      --font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"Noto Sans\", sans-serif, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\", \"Noto Color Emoji\";\n      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, \"Liberation Mono\", \"Courier New\", monospace;\n    }\n    * { box-sizing: border-box; }\n    html { color-scheme: dark; }\n    body {\n      margin: 0;\n      min-height: 100vh;\n      display: flex;\n      align-items: center;\n      justify-content: center;\n      padding: 24px;\n      background: var(--page-bg);\n      color: var(--text);\n      font-family: var(--font-sans);\n      text-align: center;\n    }\n    main {\n      width: 100%;\n      max-width: 560px;\n      display: flex;\n      flex-direction: column;\n      align-items: center;\n      justify-content: center;\n    }\n    .logo {\n      width: 72px;\n      height: 72px;\n      display: block;\n      margin-bottom: 24px;\n    }\n    h1 {\n      margin: 0 0 10px;\n      font-size: 28px;\n      line-height: 1.15;\n      font-weight: 650;\n      color: var(--text);\n    }\n    p {\n      margin: 0;\n      line-height: 1.7;\n      color: var(--text-dim);\n      font-size: 15px;\n    }\n    .details {\n      margin-top: 16px;\n      font-family: var(--font-mono);\n      font-size: 13px;\n      color: var(--text-dim);\n      white-space: pre-wrap;\n      word-break: break-word;\n    }\n  </style>\n</head>\n<body>\n  <main>\n    <div class=\"logo\"><svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 800 800\" aria-hidden=\"true\"><path fill=\"#F09082\" d=\"M165.29 165.29H517.36V400H400V282.65H165.29Z\"/><path fill=\"#4D9ABF\" d=\"M165.29 282.65H282.65V400H400V517.36H282.65V634.72H165.29Z\"/><path fill=\"#F1BE58\" d=\"M517.36 400H634.72V634.72H517.36Z\"/></svg></div>\n    <h1>Authentication successful</h1>\n    <p>Anthropic authentication completed. You can close this window.</p>\n    \n  </main>\n</body>\n</html>";
        assert_eq!(
            oauth_success_html("Anthropic authentication completed. You can close this window."),
            expected
        );
    }

    /// The failure page swaps title/heading and appends the escaped details
    /// block inside `main`.
    #[test]
    fn error_html_includes_the_escaped_details_block() {
        let page = oauth_error_html(
            "Anthropic authentication did not complete.",
            Some("Error: access_denied"),
        );
        assert!(page.starts_with("<!doctype html>\n<html lang=\"en\">"));
        assert!(page.contains("<title>Authentication failed</title>"));
        assert!(page.contains("<h1>Authentication failed</h1>"));
        assert!(page.contains("<p>Anthropic authentication did not complete.</p>"));
        assert!(page.contains("<div class=\"details\">Error: access_denied</div>"));
        // The success page has no details block (empty interpolation line).
        let success = oauth_success_html("done");
        assert!(!success.contains("<div class=\"details\">"));
        assert!(success.contains("<p>done</p>\n    \n  </main>"));
    }

    /// Every interpolated value is escaped, upstream `escapeHtml`.
    #[test]
    fn interpolated_values_are_html_escaped() {
        let hostile = "<script>\"x\"</script>&'";
        let page = oauth_error_html(hostile, Some(hostile));
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;&quot;x&quot;&lt;/script&gt;&amp;&#39;"));
        assert!(page.contains("&lt;script&gt;&quot;x&quot;&lt;/script&gt;&amp;&#39;</div>"));
    }
}
