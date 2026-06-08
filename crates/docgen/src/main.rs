//! Render the Markdown files in `/docs` into a static HTML site.
//!
//! Pure Rust (pulldown-cmark) so it runs in the dev container with no Node /
//! mkdocs toolchain. Each `docs/<name>.md` becomes `docs/<name>.html`, wrapped in
//! a shared template with a sidebar; inter-doc `.md` links are rewritten to
//! `.html`. Run: `cargo run -p docgen` (from the repo root, or pass the docs dir).

use std::fs;
use std::path::PathBuf;

use pulldown_cmark::{html, Options, Parser};

/// Ordered nav: (markdown file, sidebar title). The HTML file is the same name
/// with `.html`, so cross-links between the `.md` sources resolve after rewrite.
const PAGES: &[(&str, &str)] = &[
    ("index.md", "Overview"),
    ("intro.md", "Graph data 101"),
    ("getting-started.md", "Getting started"),
    ("scenario.md", "Real-world scenario"),
    ("cli.md", "CLI reference"),
    ("dataset-cards.md", "Dataset Cards"),
    ("sparql.md", "SPARQL support"),
    ("compatibility.md", "Compatibility & interop"),
    ("reasoning.md", "Reasoning & coherence"),
    ("topic-modeling.md", "Topic modeling (LDA)"),
    ("multi-criteria.md", "Multi-criteria communities"),
    ("federation.md", "Federated queries"),
    ("browser.md", "Browser / WASM"),
    ("SPEC.md", "Format specification"),
    ("BENCHMARK.md", "Benchmarks"),
    ("parallel-browser.md", "Parallel in browser (exp.)"),
];

/// Extra sidebar links to pre-built, non-Markdown pages (e.g. the static WASM
/// playground produced by `scripts/build_playground.py`, not by docgen). These
/// are appended to the generated `PAGES` links in every page's sidebar. Tuple is
/// `(href, title)`.
const EXTRA_NAV: &[(&str, &str)] = &[("playground.html", "Interactive playground")];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Docs dir: first CLI arg, else ./docs.
    let docs_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs"));
    if !docs_dir.is_dir() {
        return Err(format!("docs dir not found: {}", docs_dir.display()).into());
    }

    let mut rendered = 0;
    for (md, title) in PAGES {
        let src = docs_dir.join(md);
        if !src.exists() {
            eprintln!(
                "warning: {} listed in nav but missing — skipping",
                src.display()
            );
            continue;
        }
        let markdown = fs::read_to_string(&src)?;
        let body = render_markdown(&markdown);
        let html_name = md.replace(".md", ".html");
        let page = template(title, &body, md);
        let out = docs_dir.join(&html_name);
        fs::write(&out, page)?;
        println!("  {md:<22} -> {html_name}");
        rendered += 1;
    }
    println!(
        "docgen: wrote {rendered} HTML page(s) to {}",
        docs_dir.display()
    );
    Ok(())
}

/// Markdown → HTML body, with GitHub-flavored extensions (tables, etc.) and
/// inter-doc `.md` links rewritten to `.html`.
fn render_markdown(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    rewrite_links(&out)
}

/// Make links between docs work in the rendered site: a `docs/`-prefixed or bare
/// `*.md` href becomes the sibling `*.html`. Only touches `href="..."` targets,
/// not code or text.
fn rewrite_links(html: &str) -> String {
    html.replace("href=\"docs/", "href=\"")
        .replace(".md\"", ".html\"")
        .replace(".md#", ".html#")
}

fn template(title: &str, body: &str, current_md: &str) -> String {
    let mut nav_items: Vec<String> = PAGES
        .iter()
        .map(|(md, t)| {
            let href = md.replace(".md", ".html");
            let class = if *md == current_md {
                " class=\"active\""
            } else {
                ""
            };
            format!("<li><a href=\"{href}\"{class}>{t}</a></li>")
        })
        .collect();
    // Append links to externally-built pages (e.g. the static WASM playground).
    // These are never the "current" Markdown page, so they get no active class.
    for (href, t) in EXTRA_NAV {
        nav_items.push(format!("<li><a href=\"{href}\">{t}</a></li>"));
    }
    let nav = nav_items.join("\n        ");

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{title} · rete docs</title>
  <style>{css}</style>
</head>
<body>
  <nav class="sidebar">
    <a class="brand" href="index.html">rete</a>
    <p class="tagline">Cloud-native, range-queryable RDF graph files</p>
    <ul>
        {nav}
    </ul>
    <p class="foot"><a href="https://github.com/carlosvivarrios/rete">GitHub</a></p>
  </nav>
  <main>
    <div class="page">
      <article class="content">
{body}
      </article>
      <aside class="rail" aria-label="On this page">
        <nav class="toc" id="toc"></nav>
        <div class="keyterms" id="keyterms"></div>
      </aside>
    </div>
    <footer>Generated from <code>docs/{current_md}</code> by <code>cargo run -p docgen</code>.</footer>
  </main>
  <script>{script}</script>
  <script>{lightbox}</script>
  <script>{glossary}</script>
  <script>{toc}</script>
</body>
</html>
"##,
        title = title,
        css = CSS,
        nav = nav,
        body = body,
        current_md = current_md,
        script = HIGHLIGHTER,
        lightbox = LIGHTBOX,
        glossary = GLOSSARY_JS,
        toc = TOC_JS,
    )
}

const CSS: &str = r#"
:root { --fg:#1b1f24; --muted:#5b6570; --bg:#ffffff; --side:#0f1620; --side-fg:#c9d4e0;
        --accent:#2a6df4; --border:#e4e8ec; --code-bg:#f5f7f9; }
* { box-sizing: border-box; }
body { margin:0; font:16px/1.65 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
       color:var(--fg); background:var(--bg); display:flex; }
code,pre,.mono { font-family:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace; }

.sidebar { width:250px; min-height:100vh; background:var(--side); color:var(--side-fg);
           padding:1.6rem 1.2rem; position:sticky; top:0; flex:0 0 250px; }
.sidebar .brand { color:#fff; font-size:1.5rem; font-weight:700; text-decoration:none; letter-spacing:.5px; }
.sidebar .tagline { color:#8595a6; font-size:.8rem; margin:.3rem 0 1.4rem; }
.sidebar ul { list-style:none; padding:0; margin:0; }
.sidebar li { margin:.15rem 0; }
.sidebar a { color:var(--side-fg); text-decoration:none; display:block; padding:.4rem .6rem;
             border-radius:6px; font-size:.95rem; }
.sidebar a:hover { background:rgba(255,255,255,.07); color:#fff; }
.sidebar a.active { background:var(--accent); color:#fff; font-weight:600; }
.sidebar .foot { margin-top:1.5rem; font-size:.85rem; }
.sidebar .foot a { display:inline; padding:0; color:#8595a6; }

main { flex:1 1 auto; min-width:0; display:flex; flex-direction:column; }
.page { display:flex; align-items:flex-start; min-width:0; }
.content { max-width:820px; padding:2.4rem 3rem; width:100%; min-width:0; }
footer { max-width:820px; padding:1rem 3rem 3rem; color:var(--muted); font-size:.85rem; }

/* Right sidebar rail: holds the on-this-page TOC and, beneath it, the page's
   key terms. Sticky at the top-right; scrolls on its own if tall. */
.rail { flex:0 0 220px; width:220px; position:sticky; top:1.4rem; align-self:flex-start;
        margin:2.6rem 1.6rem 2rem 0; max-height:calc(100vh - 3rem); overflow:auto; font-size:.82rem; }
.toc .toc-h, .keyterms .kt-h { font-size:.66rem; font-weight:700; text-transform:uppercase;
        letter-spacing:.6px; color:var(--muted); margin-bottom:.5rem; }
.toc ul { list-style:none; margin:0; padding:0; border-left:2px solid var(--border); }
.toc a { display:block; padding:.22rem .7rem; margin-left:-2px; border-left:2px solid transparent;
         color:var(--muted); text-decoration:none; line-height:1.3; }
.toc a:hover { color:var(--accent); }
.toc a.active { color:var(--accent); border-left-color:var(--accent); font-weight:600; }
.toc .toc-sub a { padding-left:1.4rem; font-size:.95em; }

/* Key terms box, under the TOC in the same column. */
.keyterms { margin-top:1.5rem; background:#f3f7ff; border:1px solid #cfe0ff;
            border-left:3px solid var(--accent); border-radius:8px; padding:.65rem .8rem; }
.keyterms .kt-i { margin:.4rem 0; line-height:1.4; color:var(--muted); }
.keyterms .kt-i b { color:var(--fg); }

.content h2, .content h3 { scroll-margin-top:1rem; }
@media (max-width:1100px) { .rail { display:none; } }

.content h1 { font-size:2.1rem; margin:.2rem 0 1rem; line-height:1.2; }
.content h2 { font-size:1.5rem; margin:2.2rem 0 .8rem; padding-bottom:.3rem; border-bottom:1px solid var(--border); }
.content h3 { font-size:1.2rem; margin:1.6rem 0 .6rem; }
.content a { color:var(--accent); text-decoration:none; }
.content a:hover { text-decoration:underline; }
.content p,.content li { color:var(--fg); }
.content code { background:var(--code-bg); padding:.12em .4em; border-radius:4px; font-size:.88em; }
.content pre { background:var(--code-bg); border:1px solid var(--border); border-radius:8px;
               padding:1rem 1.1rem; overflow:auto; line-height:1.5; }
.content pre code { background:none; padding:0; font-size:.85rem; }
.content blockquote { margin:1rem 0; padding:.4rem 1rem; border-left:4px solid var(--accent);
                      background:#f3f7ff; color:#33415a; border-radius:0 6px 6px 0; }
.content table { border-collapse:collapse; width:100%; margin:1rem 0; font-size:.92rem; }
.content th,.content td { border:1px solid var(--border); padding:.45rem .7rem; text-align:left; }
.content th { background:var(--code-bg); }
.content tr:nth-child(even) td { background:#fafbfc; }
.content img { max-width:100%; }
.content hr { border:none; border-top:1px solid var(--border); margin:2rem 0; }

/* Right-floating figures: a diagram sits beside the prose it illustrates and
   text wraps around it. On narrow screens it drops to full width below. */
.content figure { margin:0; }
.content figure.fig-right { float:right; width:min(44%, 400px); margin:.3rem 0 1rem 1.8rem; clear:right; }
.content figure.fig-center { margin:1.4rem auto; max-width:680px; }
.content figure img { width:100%; border:1px solid var(--border); border-radius:8px;
                      background:#fff; padding:.5rem; }
.content figure figcaption { font-size:.8rem; color:var(--muted); margin-top:.45rem; line-height:1.45; }
/* Don't let a floated figure poke past the section it belongs to. */
.content h2 { clear:right; }

/* ---- click-to-zoom lightbox (no dependencies) ---- */
.content img { cursor:zoom-in; }
.lightbox { position:fixed; inset:0; z-index:1000; display:none; cursor:zoom-out;
            background:rgba(15,22,32,.85); padding:3vmin; }
.lightbox.open { display:flex; align-items:center; justify-content:center; }
.lightbox img { max-width:96vw; max-height:94vh; width:auto; height:auto;
                border-radius:8px; background:#fff; padding:.5rem;
                box-shadow:0 10px 40px rgba(0,0,0,.5); }
.lightbox .lb-close { position:fixed; top:1rem; right:1.25rem; font-size:2rem; line-height:1;
                      color:#fff; opacity:.8; cursor:pointer; user-select:none; }
.lightbox .lb-close:hover { opacity:1; }

/* ---- glossary term tooltips (hover / focus; injected client-side) ---- */
.content .term { border-bottom:1px dotted var(--accent); cursor:help; position:relative; }
.content .term:focus { outline:none; }
.content .term .tip { position:absolute; left:0; top:1.55em; z-index:60; width:max-content; max-width:280px;
  background:var(--side); color:#eef3f9; padding:.5rem .7rem; border-radius:7px; font-size:.78rem;
  font-weight:400; line-height:1.45; box-shadow:0 6px 22px rgba(15,22,32,.3);
  opacity:0; visibility:hidden; transition:opacity .12s; pointer-events:none; }
.content .term:hover .tip, .content .term:focus .tip { opacity:1; visibility:visible; }


/* ---- syntax highlighting (lightweight, applied client-side; no external deps) ---- */
.content pre code .tok-com  { color:#7a8896; font-style:italic; }
.content pre code .tok-str  { color:#0a7d4d; }
.content pre code .tok-kw   { color:#a020a0; font-weight:600; }
.content pre code .tok-num  { color:#b35900; }
.content pre code .tok-fn   { color:#2a6df4; }
.content pre code .tok-iri  { color:#2a6df4; }
.content pre code .tok-var  { color:#b35900; }
.content pre code .tok-flag { color:#7a5b00; }
.content pre code .tok-punct{ color:#5b6570; }

@media (max-width:980px) {
  .content figure.fig-right { float:none; width:100%; margin:1.4rem 0; }
}
@media (max-width:780px) {
  body { flex-direction:column; }
  .sidebar { width:100%; flex-basis:auto; min-height:auto; position:static; }
  .content,footer { padding-left:1.2rem; padding-right:1.2rem; }
}
"#;

/// A tiny dependency-free syntax highlighter, embedded in every page. It runs on
/// `DOMContentLoaded`, tokenizes each `<pre><code class="language-…">` block with
/// a per-language rule list, and wraps tokens in `<span class="tok-…">` that the
/// CSS theme above colors. No CDN, no build step — works offline / from file://.
const HIGHLIGHTER: &str = r#"
(function () {
  function esc(s){return s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;");}
  // Shared token regexes (sticky: must match at the cursor).
  var COM_HASH = /#.*/y, COM_SLASH = /\/\/.*/y, COM_BLOCK = /\/\*[\s\S]*?\*\//y;
  var STR = /"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/y;
  var NUM = /\b\d[\d_]*(?:\.\d+)?\b/y;
  var WS  = /\s+/y, IDENT = /[A-Za-z_]\w*/y;
  var IRI = /<[^>\s]*>/y, VAR = /[?$][A-Za-z_]\w*/y;
  var PNAME = /[A-Za-z_][\w.-]*:[A-Za-z_][\w.-]*|:[A-Za-z_][\w.-]*/y;
  var FLAG = /--?[A-Za-z][\w-]*/y;
  function kw(words, flags){ return new RegExp("\\b(?:" + words.join("|") + ")\\b", (flags||"") + "y"); }
  var RUST = kw(["as","async","await","break","const","continue","crate","dyn","else","enum","extern","false","fn","for","if","impl","in","let","loop","match","mod","move","mut","pub","ref","return","self","Self","static","struct","super","trait","true","type","unsafe","use","where","while"]);
  var PY = kw(["def","return","import","from","as","for","in","if","elif","else","while","with","class","lambda","None","True","False","and","or","not","is","pass","break","continue","try","except","finally","raise","yield","global","nonlocal","assert","del","print","async","await"]);
  var SPARQL = kw(["SELECT","CONSTRUCT","ASK","DESCRIBE","WHERE","PREFIX","BASE","FILTER","OPTIONAL","UNION","MINUS","GRAPH","SERVICE","BIND","VALUES","DISTINCT","REDUCED","ORDER","BY","ASC","DESC","GROUP","HAVING","LIMIT","OFFSET","FROM","NAMED","COUNT","SUM","AVG","MIN","MAX","SAMPLE","STR","LANG","DATATYPE","BOUND","IRI","URI","REGEX","EXISTS","NOT","AS"], "i");
  var SH = kw(["rete","cargo","docker","python3","python","pip","curl","wget","cd","cp","mv","rm","ls","mkdir","cat","grep","echo","node","git","wasm-pack","jq","tee","export","sudo","bash","sh"]);
  var JS = kw(["const","let","var","function","return","if","else","for","while","do","switch","case","break","continue","new","class","extends","super","this","typeof","instanceof","in","of","await","async","import","from","export","default","try","catch","finally","throw","yield","true","false","null","undefined","void","delete"]);
  var RULES = {
    rust:   [[COM_SLASH,"com"],[COM_BLOCK,"com"],[STR,"str"],[/\b[a-z_]\w*!/y,"fn"],[RUST,"kw"],[NUM,"num"],[IDENT,null]],
    py:     [[COM_HASH,"com"],[STR,"str"],[/@\w+/y,"fn"],[PY,"kw"],[NUM,"num"],[IDENT,null]],
    sh:     [[COM_HASH,"com"],[STR,"str"],[FLAG,"flag"],[SH,"fn"],[NUM,"num"],[IDENT,null]],
    js:     [[COM_SLASH,"com"],[COM_BLOCK,"com"],[STR,"str"],[/`(?:\\.|[^`\\])*`/y,"str"],[JS,"kw"],[/\b[A-Za-z_]\w*(?=\s*\()/y,"fn"],[NUM,"num"],[IDENT,null]],
    json:   [[STR,"str"],[kw(["true","false","null"]),"kw"],[NUM,"num"],[IDENT,null]],
    sparql: [[COM_HASH,"com"],[IRI,"iri"],[STR,"str"],[VAR,"var"],[/\ba\b/y,"kw"],[SPARQL,"kw"],[PNAME,"fn"],[NUM,"num"],[IDENT,null]],
    ttl:    [[COM_HASH,"com"],[IRI,"iri"],[STR,"str"],[/@[A-Za-z]+/y,"kw"],[/\ba\b/y,"kw"],[PNAME,"fn"],[NUM,"num"],[IDENT,null]]
  };
  var ALIAS = {bash:"sh",shell:"sh",console:"sh",sh:"sh",rs:"rust",rust:"rust",python:"py",py:"py",js:"js",javascript:"js",mjs:"js",json:"json",sparql:"sparql",turtle:"ttl",ttl:"ttl",nt:"ttl",ntriples:"ttl",nq:"ttl",trig:"ttl"};
  function highlight(code, rules){
    var out = "", i = 0, n = code.length;
    outer: while (i < n){
      WS.lastIndex = i; var w = WS.exec(code);
      if (w && w.index === i){ out += w[0]; i += w[0].length; continue; }
      for (var r = 0; r < rules.length; r++){
        var re = rules[r][0], cls = rules[r][1];
        re.lastIndex = i; var m = re.exec(code);
        if (m && m.index === i && m[0].length){
          out += cls ? '<span class="tok-' + cls + '">' + esc(m[0]) + "</span>" : esc(m[0]);
          i += m[0].length; continue outer;
        }
      }
      out += esc(code[i]); i++;
    }
    return out;
  }
  document.addEventListener("DOMContentLoaded", function () {
    var blocks = document.querySelectorAll('pre code[class*="language-"]');
    blocks.forEach(function (el) {
      var cls = (el.className.match(/language-([\w-]+)/) || [])[1] || "";
      var rules = RULES[ALIAS[cls.toLowerCase()] || ""];
      if (!rules) return;
      el.innerHTML = highlight(el.textContent, rules);
    });
  });
})();
"#;

/// Click-to-zoom lightbox for content images. On `DOMContentLoaded` it builds a
/// single fullscreen overlay and opens it with the clicked image's source;
/// clicking the overlay (or pressing Escape) closes it. No dependencies.
const LIGHTBOX: &str = r#"
(function () {
  document.addEventListener("DOMContentLoaded", function () {
    var imgs = document.querySelectorAll(".content img");
    if (!imgs.length) return;
    var box = document.createElement("div");
    box.className = "lightbox";
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-label", "Enlarged image");
    var close = document.createElement("span");
    close.className = "lb-close";
    close.setAttribute("aria-hidden", "true");
    close.textContent = "×"; // ×
    var big = document.createElement("img");
    box.appendChild(close);
    box.appendChild(big);
    document.body.appendChild(box);

    function open(src, alt) {
      big.src = src;
      big.alt = alt || "";
      box.classList.add("open");
    }
    function hide() {
      box.classList.remove("open");
      big.removeAttribute("src");
    }
    imgs.forEach(function (img) {
      img.addEventListener("click", function () { open(img.currentSrc || img.src, img.alt); });
    });
    box.addEventListener("click", hide);
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && box.classList.contains("open")) hide();
    });
  });
})();
"#;

/// Glossary tooltips + per-section "reminder" boxes. On `DOMContentLoaded`:
///  1. for each `<h2>` section, a small right-margin box recaps the glossary
///     terms that appear in it (a quick reminder of what each one means);
///  2. the first occurrence of each glossary term in the prose is wrapped in a
///     `.term` span whose hover/focus tooltip shows the definition (acronyms
///     included). Code blocks, links, headings, the boxes, and the TOC are
///     skipped. No dependencies.
const GLOSSARY_JS: &str = r#"
(function () {
  var GLOSSARY = {
    "RDF": "Resource Description Framework — the W3C model where data is statements (triples).",
    "SPARQL": "The W3C query language for RDF graphs (SELECT / ASK / CONSTRUCT / DESCRIBE).",
    "IRI": "Internationalized Resource Identifier — a global, URL-like id for a resource.",
    "BGP": "Basic Graph Pattern — a set of triple patterns joined on their shared variables.",
    "HDT": "Header-Dictionary-Triples — a compact binary RDF format rete's dictionary draws on.",
    "triple": "One fact: subject – predicate – object.",
    "dictionary": "The table that maps terms (IRIs / literals) to compact integer ids.",
    "zstd": "Zstandard — a fast lossless compression algorithm.",
    "blake3": "A fast cryptographic hash, used here for content addressing / integrity.",
    "WebAssembly": "A portable binary format that runs in browsers at near-native speed.",
    "WASM": "WebAssembly — a portable binary format that runs in the browser at near-native speed.",
    "SPO": "One of the three index orderings (Subject-Predicate-Object; also POS and OSP).",
    "Louvain": "A community-detection algorithm that groups densely-connected nodes.",
    "PMTiles": "A single-file, range-readable format for map tiles — an inspiration for rete.",
    "Parquet": "A columnar single-file format for tabular data — an inspiration for rete.",
    "CORS": "Cross-Origin Resource Sharing — browser rules for fetching across domains.",
    "COOP": "Cross-Origin-Opener-Policy — a header that helps enable cross-origin isolation.",
    "COEP": "Cross-Origin-Embedder-Policy — paired with COOP to unlock SharedArrayBuffer.",
    "LDA": "Latent Dirichlet Allocation — a statistical topic-modelling method.",
    "OWL RL": "The rule-based profile of OWL, suited to forward-chaining over triples.",
    "OWL": "Web Ontology Language — schema axioms (class hierarchies, disjointness, identity).",
    "RDFS": "RDF Schema — a basic class/property vocabulary with simple entailments.",
    "DOI": "Digital Object Identifier — a persistent id for a publication.",
    "CVE": "Common Vulnerabilities and Exposures — a public id for a security flaw.",
    "SBOM": "Software Bill of Materials — an inventory of a piece of software's components.",
    "N-Triples": "A line-based plain-text RDF format: one triple per line.",
    "N-Quads": "Like N-Triples, with a fourth column naming the graph.",
    "Turtle": "A compact, human-friendly text syntax for RDF.",
    "zone map": "Per-block min/max stats that let a query skip blocks that cannot match.",
    "property path": "A SPARQL path over edges (e.g. knows+ = follow knows transitively).",
    "named graph": "A sub-graph within a dataset, identified by its own IRI.",
    "pyramid": "rete's progressively-coarsened community summary: overview first, drill into detail.",
    "range request": "An HTTP request for a byte range, so a client fetches only part of a file.",
    "Oxigraph": "A popular Rust RDF triplestore + SPARQL engine, used here as a benchmark baseline."
  };
  function esc(s){ return s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;"); }
  function rx(term){
    var e = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    var tail = /[A-Za-z0-9_]$/.test(term) ? "(?![A-Za-z0-9_])" : "";
    return new RegExp("(^|[^A-Za-z0-9_])(" + e + ")" + tail);
  }
  function present(text, term){ return rx(term).test(text); }

  function wrapFirst(root, term, def){
    var re = rx(term);
    var skip = "pre,code,a,h1,h2,h3,.rail,.term";
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        if (!n.nodeValue || !n.parentElement) return NodeFilter.FILTER_REJECT;
        if (n.parentElement.closest(skip)) return NodeFilter.FILTER_REJECT;
        return re.test(n.nodeValue) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
      }
    });
    var node = walker.nextNode();
    if (!node) return;
    var m = re.exec(node.nodeValue);
    if (!m) return;
    var start = m.index + m[1].length;
    var mid = node.splitText(start);
    mid.splitText(term.length);
    var span = document.createElement("span");
    span.className = "term"; span.tabIndex = 0; span.setAttribute("role", "note");
    span.textContent = term;
    var tip = document.createElement("span");
    tip.className = "tip"; tip.textContent = def;
    span.appendChild(tip);
    mid.parentNode.replaceChild(span, mid);
  }

  document.addEventListener("DOMContentLoaded", function () {
    var content = document.querySelector(".content");
    if (!content) return;
    var terms = Object.keys(GLOSSARY).sort(function (a, b) { return b.length - a.length; });

    // 1) Key terms box in the right rail (under the TOC): every glossary term
    //    that appears on the page, in order of first appearance.
    var pageText = content.textContent || "";
    var present = terms
      .filter(function (t) { return rx(t).test(pageText); })
      .map(function (t) { return { t: t, at: pageText.search(rx(t)) }; })
      .sort(function (a, b) { return a.at - b.at; });
    var kt = document.getElementById("keyterms");
    if (kt && present.length) {
      kt.innerHTML = '<div class="kt-h">Key terms</div>' + present.map(function (e) {
        return '<div class="kt-i"><b>' + esc(e.t) + "</b> — " + esc(GLOSSARY[e.t]) + "</div>";
      }).join("");
    }

    // 2) Inline tooltips: wrap the first occurrence of each term in the prose.
    terms.forEach(function (t) { wrapFirst(content, t, GLOSSARY[t]); });
  });
})();
"#;

/// "On this page" table of contents, built client-side from the `<h2>`/`<h3>`
/// headings and shown sticky at the top-right (collapses on narrow screens). It
/// gives each heading a slug id, links to it, and highlights the section in view.
const TOC_JS: &str = r##"
(function () {
  document.addEventListener("DOMContentLoaded", function () {
    var toc = document.getElementById("toc");
    var content = document.querySelector(".content");
    if (!toc || !content) return;
    var heads = [].slice.call(content.querySelectorAll("h2, h3"));
    if (heads.length < 2) { toc.remove(); return; } // not worth a TOC

    var used = {};
    function slug(s) {
      var base = s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "section";
      var id = base, i = 2;
      while (used[id]) { id = base + "-" + i++; }
      used[id] = 1; return id;
    }
    var ul = document.createElement("ul");
    heads.forEach(function (h) {
      if (!h.id) h.id = slug(h.textContent);
      var li = document.createElement("li");
      if (h.tagName === "H3") li.className = "toc-sub";
      var a = document.createElement("a");
      a.href = "#" + h.id;
      a.textContent = h.textContent;
      li.appendChild(a);
      ul.appendChild(li);
    });
    var head = document.createElement("div");
    head.className = "toc-h"; head.textContent = "On this page";
    toc.appendChild(head);
    toc.appendChild(ul);

    // Scroll-spy: mark the heading currently at/above the top of the viewport.
    var links = {};
    toc.querySelectorAll("a").forEach(function (a) { links[a.getAttribute("href").slice(1)] = a; });
    function spy() {
      var current = heads[0].id;
      for (var i = 0; i < heads.length; i++) {
        if (heads[i].getBoundingClientRect().top - 100 <= 0) current = heads[i].id;
      }
      Object.keys(links).forEach(function (id) {
        links[id].classList.toggle("active", id === current);
      });
    }
    window.addEventListener("scroll", spy, { passive: true });
    window.addEventListener("resize", spy, { passive: true });
    spy();
  });
})();
"##;
