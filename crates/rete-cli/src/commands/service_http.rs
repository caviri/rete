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
