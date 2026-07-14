//! The CLI's [`ServiceClient`]: `SERVICE <endpoint> { … }` blocks are executed
//! over HTTP with the SPARQL Protocol (POST form, JSON results). rete-core owns
//! the parsing; this is only transport.

use rete_core::{parse_sparql_json_results, Binding, ServiceClient};

/// Cap on a SERVICE response body — a runaway endpoint must not exhaust RAM.
const MAX_RESPONSE: u64 = 256 * 1024 * 1024;

pub(crate) struct HttpServiceClient;

impl ServiceClient for HttpServiceClient {
    fn query(&self, endpoint: &str, query: &str) -> Result<Vec<Binding>, String> {
        let agent = ureq::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build();
        let resp = agent
            .post(endpoint)
            .set("Accept", "application/sparql-results+json")
            // Public endpoints (Wikidata in particular) require an identifying
            // User-Agent and may throttle or reject the library default.
            .set("User-Agent", "rete-cli (SPARQL SERVICE federation)")
            .send_form(&[("query", query)])
            .map_err(|e| e.to_string())?;
        let mut body = String::new();
        use std::io::Read;
        resp.into_reader()
            .take(MAX_RESPONSE)
            .read_to_string(&mut body)
            .map_err(|e| format!("reading results: {e}"))?;
        parse_sparql_json_results(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    fn endpoint(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/sparql", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/sparql-results+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        url
    }

    #[test]
    fn service_client_posts_and_parses_sparql_json() {
        let body = r#"{"head":{"vars":["x"]},"results":{"bindings":[{"x":{"type":"uri","value":"http://example.test/x"}}]}}"#;
        let rows = HttpServiceClient
            .query(&endpoint("200 OK", body), "SELECT ?x WHERE { ?x ?p ?o }")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["x"], "<http://example.test/x>");
    }

    #[test]
    fn service_client_surfaces_http_and_result_errors() {
        let http = HttpServiceClient
            .query(
                &endpoint("500 Internal Server Error", ""),
                "SELECT * WHERE { ?s ?p ?o }",
            )
            .unwrap_err();
        assert!(http.contains("500"));

        let parse = HttpServiceClient
            .query(
                &endpoint("200 OK", "not json"),
                "SELECT * WHERE { ?s ?p ?o }",
            )
            .unwrap_err();
        assert!(!parse.is_empty());
    }
}
