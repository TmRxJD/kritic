use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::Duration;

const MAX_QUERIES: usize = 3;
const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Debug)]
struct Query {
    text: String,
    domain: Option<String>,
    recency: Option<u64>,
}

pub(crate) fn tool() -> Value {
    json!({
        "name": "search_queries",
        "description": "Validate up to three focused web queries before searching. Use one entity and one discriminator per query. Returns checked queries and native filters. execute=true runs guarded Brave search when configured.",
        "inputSchema": {
            "type": "object", "additionalProperties": false, "required": ["queries"],
            "properties": {
                "queries": {"type": "array", "minItems": 1, "maxItems": MAX_QUERIES,
                    "items": {"type": "object", "additionalProperties": false, "required": ["q"],
                        "properties": {
                            "q": {"type": "string", "maxLength": 240},
                            "exact": {"type": "boolean", "description": "Permit a longer single quoted title/error or a single exact identifier."},
                            "domain": {"type": "string", "description": "Optional DNS domain, no URL or path."},
                            "recency": {"type": "integer", "enum": [1, 7, 31, 365], "description": "Optional age limit in days."}
                        }
                    }
                },
                "execute": {"type": "boolean", "description": "Search Brave with the checked batch. Defaults to false."}
            }
        }
    })
}

pub(crate) fn call(args: &Value) -> Result<String, String> {
    let (queries, execute) = prepare(args)?;
    let checked: Vec<_> = queries
        .iter()
        .map(|query| {
            let mut value = json!({"q": query.text});
            if let Some(domain) = &query.domain {
                value["domains"] = json!([domain]);
            }
            if let Some(days) = query.recency {
                value["recency"] = json!(days);
            }
            value
        })
        .collect();
    if !execute {
        return Ok(json!({"search_query":checked,"executed":false}).to_string());
    }
    let key = std::env::var("BRAVE_SEARCH_API_KEY").ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or("BRAVE_SEARCH_API_KEY is unset. Call without execute, then use the checked queries with native search.")?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let results: Vec<_> = queries
        .iter()
        .map(
            |query| match brave_search(&agent, BRAVE_URL, key.trim(), query) {
                Ok(hits) => json!({"results":hits}),
                Err(error) => json!({"error":error}),
            },
        )
        .collect();
    Ok(json!({"search_query":checked,"executed":true,"responses":results}).to_string())
}

fn prepare(args: &Value) -> Result<(Vec<Query>, bool), String> {
    let args = args.as_object().ok_or("expected an object")?;
    if args
        .keys()
        .any(|key| !matches!(key.as_str(), "queries" | "execute"))
    {
        return Err("only queries and execute are accepted".into());
    }
    let execute = args.get("execute").map_or(Ok(false), |v| {
        v.as_bool().ok_or("execute must be a boolean")
    })?;
    let rows = args
        .get("queries")
        .and_then(Value::as_array)
        .filter(|rows| (1..=MAX_QUERIES).contains(&rows.len()))
        .ok_or("provide one to three independent queries")?;
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    for (index, row) in rows.iter().enumerate() {
        let query = parse_query(row).map_err(|error| format!("query {}: {error}", index + 1))?;
        let key = (
            query
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase(),
            query.domain.clone(),
            query.recency,
        );
        if !seen.insert(key) {
            return Err(format!("query {}: duplicate query and filters", index + 1));
        }
        queries.push(query);
    }
    Ok((queries, execute))
}

fn parse_query(row: &Value) -> Result<Query, String> {
    let row = row.as_object().ok_or("expected a query object")?;
    if row
        .keys()
        .any(|key| !matches!(key.as_str(), "q" | "exact" | "domain" | "recency"))
    {
        return Err("only q, exact, domain and recency are accepted".into());
    }
    let exact = row
        .get("exact")
        .map_or(Ok(false), |v| v.as_bool().ok_or("exact must be a boolean"))?;
    let raw = row
        .get("q")
        .and_then(Value::as_str)
        .ok_or("q must be a string")?;
    if raw.len() > 960
        || raw
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err("q must be a bounded single line without control characters".into());
    }
    let mut q = raw.trim();
    if !exact {
        q = q
            .strip_suffix("...")
            .or_else(|| q.strip_suffix('…'))
            .unwrap_or(q)
            .trim_end();
    }
    if q.is_empty() || !q.chars().any(char::is_alphanumeric) {
        return Err("q must contain a searchable term".into());
    }
    let words: Vec<_> = q.split_whitespace().collect();
    let quotes = q.matches('"').count();
    if quotes % 2 != 0 {
        return Err("unbalanced quotation marks".into());
    }
    if exact {
        let quoted_literal = q.starts_with('"') && q.ends_with('"') && quotes == 2;
        if q.chars().count() > 240 || (!quoted_literal && !is_identifier(q)) {
            return Err(
                "exact mode requires one quoted literal or one identifier, at most 240 characters"
                    .into(),
            );
        }
    } else {
        if words.len() > 5 || q.chars().count() > 50 {
            return Err("keep one entity and one discriminator within five words and 50 characters, or use an exact literal".into());
        }
        if quotes == 0 {
            let mut terms = HashSet::new();
            if words
                .iter()
                .filter(|word| word.chars().all(char::is_alphanumeric))
                .any(|word| !terms.insert(word.to_lowercase()))
            {
                return Err("remove repeated terms".into());
            }
        }
    }
    let domain = row
        .get("domain")
        .map(|v| {
            let domain = v.as_str().ok_or("domain must be a DNS name")?;
            let valid = domain.len() <= 253
                && domain.contains('.')
                && domain.split('.').all(|label| {
                    (1..=63).contains(&label.len())
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                });
            if valid {
                Ok(domain.to_ascii_lowercase())
            } else {
                Err("domain must be a DNS name without a scheme or path")
            }
        })
        .transpose()?;
    if domain.is_some()
        && q.split('"')
            .step_by(2)
            .flat_map(str::split_whitespace)
            .any(|word| {
                let word = word.trim_start_matches(['(', '-', '+']);
                word == "OR" || word.to_ascii_lowercase().starts_with("site:")
            })
    {
        return Err(
            "with a domain filter, split OR into separate queries and omit inline site operators"
                .into(),
        );
    }
    let recency = row
        .get("recency")
        .map(|v| {
            v.as_u64()
                .filter(|days| matches!(days, 1 | 7 | 31 | 365))
                .ok_or("recency must be 1, 7, 31 or 365 days")
        })
        .transpose()?;
    Ok(Query {
        text: q.into(),
        domain,
        recency,
    })
}

fn is_identifier(text: &str) -> bool {
    text.split("::").all(|part| {
        let mut chars = part.chars();
        chars
            .next()
            .is_some_and(|first| first.is_alphanumeric() || first == '_')
            && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '#'))
    })
}

fn brave_search(
    agent: &ureq::Agent,
    url: &str,
    key: &str,
    query: &Query,
) -> Result<Vec<Value>, String> {
    let q = query.domain.as_ref().map_or_else(
        || query.text.clone(),
        |domain| format!("{} site:{domain}", query.text),
    );
    let mut request = agent
        .get(url)
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .query("q", &q)
        .query("count", "5");
    if let Some(days) = query.recency {
        let freshness = match days {
            1 => "pd",
            7 => "pw",
            31 => "pm",
            365 => "py",
            _ => return Err("invalid freshness filter".into()),
        };
        request = request.query("freshness", freshness);
    }
    let mut response = request
        .call()
        .map_err(|_| "Brave request failed or timed out".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Brave HTTP {}", response.status()));
    }
    let value: Value = response
        .body_mut()
        .with_config()
        .limit(1_048_576)
        .read_json()
        .map_err(|_| "Brave response was invalid or exceeded the size limit".to_string())?;
    Ok(value.pointer("/web/results").and_then(Value::as_array).into_iter().flatten().take(5)
        .map(|item| json!({
            "title": item["title"].as_str().unwrap_or_default().chars().take(240).collect::<String>(),
            "url": item["url"].as_str().unwrap_or_default().chars().take(2048).collect::<String>(),
            "description": item["description"].as_str().unwrap_or_default().chars().take(400).collect::<String>()
        })).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn preserves_literal_identifiers_operators_and_filters() {
        let symbol = "some_crate::namespace::unusually_long_function_identifier";
        assert_eq!(
            parse_query(&json!({"q":symbol,"exact":true})).unwrap().text,
            symbol
        );
        let output = call(&json!({"queries":[
            {"q":"\"error decoding response body at line 1 column 1\"", "exact":true},
            {"q":"CVE-2026-1234 site:nvd.nist.gov"},
            {"q":"Ghidra decompiler API…", "domain":"GHIDRA-SRE.ORG", "recency":31}
        ]}))
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({
                "executed":false, "search_query":[
                    {"q":"\"error decoding response body at line 1 column 1\""},
                    {"q":"CVE-2026-1234 site:nvd.nist.gov"},
                    {"q":"Ghidra decompiler API", "domains":["ghidra-sre.org"], "recency":31}
                ]
            })
        );
    }

    #[test]
    fn rejects_entire_invalid_batch_before_provider_setup() {
        for (query, reason) in [
            (
                json!({"q":"MCP context decompilation binary agents AI reverse engineering"}),
                "five words",
            ),
            (json!({"q":"x".repeat(51)}), "50 characters"),
            (json!({"q":"Ghidra ghidra"}), "repeated terms"),
            (json!({"q":"Ghidra\nAPI"}), "single line"),
            (json!({"q":"\"Ghidra API"}), "unbalanced"),
            (
                json!({"q":"Ghidra API workflows", "exact":true}),
                "one quoted literal",
            ),
            (
                json!({"q":"site:other.example", "exact":true}),
                "one quoted literal",
            ),
            (
                json!({"q":"ghidra,tools,api,workflow", "exact":true}),
                "one quoted literal",
            ),
            (
                json!({"q":"Ghidra site:other.example", "domain":"ghidra-sre.org"}),
                "inline site",
            ),
            (
                json!({"q":"Ghidra OR BinaryNinja", "domain":"ghidra-sre.org"}),
                "split OR",
            ),
            (
                json!({"q":"Ghidra API", "domain":"https://ghidra-sre.org"}),
                "DNS name",
            ),
            (json!({"q":"Ghidra API", "recency":2}), "recency"),
            (json!({"q":" Ghidra   internals "}), "duplicate"),
        ] {
            let error = call(&json!({
                "execute":true, "queries":[{"q":"ghidra internals"}, query]
            }))
            .unwrap_err();
            assert!(
                error.starts_with("query 2: ") && error.contains(reason),
                "{error}"
            );
        }
        assert!(prepare(&json!({"queries":[{"q":"a"},{"q":"b"},{"q":"c"},{"q":"d"}]})).is_err());
    }

    #[test]
    fn sends_encoded_filters_and_parses_provider_results() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let request: Vec<_> = BufReader::new(&stream)
                .lines()
                .map(Result::unwrap)
                .take_while(|line| !line.is_empty())
                .collect();
            let body = r#"{"web":{"results":[{"title":"API","url":"https://example.org/docs","description":"Docs","extra":"omitted"}]}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        let query =
            parse_query(&json!({"q":"C++ API", "domain":"example.org", "recency":7})).unwrap();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .proxy(None)
            .build()
            .new_agent();
        let hits = brave_search(&agent, &url, "test-key", &query).unwrap();
        let request = server.join().unwrap();
        let line = &request[0];
        assert!(line.contains("q=C%2B%2B"));
        assert!(line.contains("site%3Aexample.org"));
        assert!(line.contains("freshness=pw"));
        assert!(
            request
                .iter()
                .any(|line| line.eq_ignore_ascii_case("X-Subscription-Token: test-key"))
        );
        assert_eq!(
            hits,
            vec![json!({"title":"API", "url":"https://example.org/docs", "description":"Docs"})]
        );
    }
}
