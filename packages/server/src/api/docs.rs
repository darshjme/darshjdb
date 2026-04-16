//! Enhanced API documentation page for DarshJDB.
//!
//! Serves an interactive Scalar documentation viewer at `GET /api/docs`
//! with authentication instructions, try-it examples, and tag grouping.

/// Generate the enhanced Scalar API docs HTML page.
///
/// Features:
/// - Interactive "Try it" console for every endpoint
/// - Tag-based grouping with collapsible sections
/// - Authentication instructions in the sidebar
/// - Dark theme matching DarshJDB branding
pub fn enhanced_docs_html(spec_url: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <title>DarshJDB API Documentation</title>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="description" content="Interactive API documentation for DarshJDB triple-store BaaS" />
  <style>
    :root {{
      --ddb-gold: #d4a843;
      --ddb-bg: #0d1117;
      --ddb-surface: #161b22;
    }}
    body {{
      margin: 0;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      background: var(--ddb-bg);
    }}
    .ddb-banner {{
      background: var(--ddb-surface);
      border-bottom: 1px solid #30363d;
      padding: 12px 24px;
      display: flex;
      align-items: center;
      gap: 16px;
    }}
    .ddb-banner h1 {{
      margin: 0;
      font-size: 18px;
      font-weight: 600;
      color: #e6edf3;
    }}
    .ddb-banner h1 span {{
      color: var(--ddb-gold);
    }}
    .ddb-banner .badge {{
      background: var(--ddb-gold);
      color: #0d1117;
      font-size: 11px;
      font-weight: 700;
      padding: 2px 8px;
      border-radius: 10px;
      text-transform: uppercase;
      letter-spacing: 0.5px;
    }}
    .ddb-auth-hint {{
      background: var(--ddb-surface);
      border: 1px solid #30363d;
      border-radius: 8px;
      margin: 16px 24px;
      padding: 16px 20px;
      color: #8b949e;
      font-size: 13px;
      line-height: 1.6;
    }}
    .ddb-auth-hint h3 {{
      margin: 0 0 8px 0;
      color: #e6edf3;
      font-size: 14px;
    }}
    .ddb-auth-hint code {{
      background: rgba(212, 168, 67, 0.15);
      color: var(--ddb-gold);
      padding: 1px 6px;
      border-radius: 4px;
      font-size: 12px;
    }}
    .ddb-auth-hint ol {{
      margin: 8px 0 0 0;
      padding-left: 20px;
    }}
    .ddb-links {{
      display: flex;
      gap: 12px;
      margin-left: auto;
    }}
    .ddb-links a {{
      color: #8b949e;
      text-decoration: none;
      font-size: 13px;
      padding: 4px 8px;
      border-radius: 4px;
      transition: color 0.15s;
    }}
    .ddb-links a:hover {{
      color: var(--ddb-gold);
    }}
  </style>
</head>
<body>
  <div class="ddb-banner">
    <h1><span>DarshJ</span>DB</h1>
    <span class="badge">v0.2.0</span>
    <div class="ddb-links">
      <a href="{spec_url}" target="_blank">OpenAPI JSON</a>
      <a href="/api/types.ts" target="_blank">TypeScript Types</a>
      <a href="https://github.com/darshjme/darshjdb" target="_blank">GitHub</a>
    </div>
  </div>

  <div class="ddb-auth-hint">
    <h3>Authentication</h3>
    <ol>
      <li>Create an account via <code>POST /api/auth/signup</code> with email and password.</li>
      <li>Sign in via <code>POST /api/auth/signin</code> to receive a <code>TokenPair</code>.</li>
      <li>Pass the <code>access_token</code> as <code>Authorization: Bearer &lt;token&gt;</code> on all protected endpoints.</li>
      <li>When the token expires, refresh it via <code>POST /api/auth/refresh</code>.</li>
    </ol>
    <p style="margin: 8px 0 0 0;">
      <strong>Dev mode:</strong> Set <code>DDB_DEV=1</code> and use <code>Authorization: Bearer dev</code> to skip authentication.
    </p>
  </div>

  <script
    id="api-reference"
    data-url="{spec_url}"
    data-configuration='{{
      "theme": "kepler",
      "layout": "modern",
      "hiddenClients": [],
      "defaultHttpClient": {{ "targetKey": "javascript", "clientKey": "fetch" }},
      "authentication": {{
        "preferredSecurityScheme": "bearerAuth"
      }},
      "tagsSorter": "alpha"
    }}'
  ></script>
  <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
</body>
</html>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_html_contains_spec_url() {
        let html = enhanced_docs_html("/api/openapi.json");
        assert!(html.contains("data-url=\"/api/openapi.json\""));
        assert!(html.contains("DarshJDB"));
        assert!(html.contains("/api/types.ts"));
    }

    #[test]
    fn docs_html_has_auth_instructions() {
        let html = enhanced_docs_html("/api/openapi.json");
        assert!(html.contains("Bearer dev"));
        assert!(html.contains("POST /api/auth/signup"));
    }
}
