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
    <article class="content">
{body}
    </article>
    <footer>Generated from <code>docs/{current_md}</code> by <code>cargo run -p docgen</code>.</footer>
  </main>
</body>
</html>
"##,
        title = title,
        css = CSS,
        nav = nav,
        body = body,
        current_md = current_md,
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
.content { max-width:820px; padding:2.4rem 3rem; width:100%; }
footer { max-width:820px; padding:1rem 3rem 3rem; color:var(--muted); font-size:.85rem; }

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

@media (max-width:780px) {
  body { flex-direction:column; }
  .sidebar { width:100%; flex-basis:auto; min-height:auto; position:static; }
  .content,footer { padding-left:1.2rem; padding-right:1.2rem; }
}
"#;
