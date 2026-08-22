//! Render the Markdown files in `/docs` into a static HTML site.
//!
//! Pure Rust (pulldown-cmark) so it runs in the dev container with no Node /
//! mkdocs toolchain. Each `docs/<name>.md` becomes `docs/<name>.html`, wrapped in
//! a shared template with a sidebar; inter-doc `.md` links are rewritten to
//! `.html`. Run: `cargo run -p docgen` (from the repo root, or pass the docs dir).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};

/// Crate version, shown in the sidebar next to the repository link.
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/caviri/rete";
/// Where the rendered site lives. Open Graph requires ABSOLUTE URLs — a relative
/// `og:image` is silently dropped by every unfurler — so social tags are the one
/// place the site's own address has to be hard-coded.
const SITE_BASE: &str = "https://caviri.github.io/rete/";

/// Widest aspect ratio (intrinsic width ÷ height) still counted as "square".
///
/// Every figure in `docs/img` is measured at build time — the `viewBox` of an
/// SVG, the IHDR of a PNG — and anything at or below this gets the narrower
/// column (see `.img-sq` / `.fig-sq` in `CSS`). Nothing is classified by hand,
/// so a diagram added tomorrow is sized by its own shape with no HTML to
/// remember.
///
/// 1.35 sits inside the real gap in the corpus: the squarest 21 figures run
/// 0.87 (`lazy-open.svg`, taller than it is wide) to 1.32 (`pyramid.svg`), and
/// the next one up is 1.41 (`triple.svg`). Landscape strips — the byte-layout
/// diagrams, the app screenshots, the 3.8:1 logo — stay on the far side of it
/// and keep the full column, which they need.
const SQUARE_MAX_RATIO: f64 = 1.35;

/// Sectioned nav: (section title, [(file, sidebar title)]). Markdown entries are
/// rendered to the sibling `.html`; entries already ending in `.html` are
/// pre-built pages (e.g. the WASM playground from `scripts/build_playground.py`)
/// and are linked as-is, never rendered.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Start here",
        &[
            ("index.md", "Overview"),
            ("intro.md", "Graph data 101"),
            ("getting-started.md", "Getting started"),
            ("scenario.md", "Real-world scenario"),
        ],
    ),
    (
        "Explore in the browser",
        &[
            ("playground-guide.md", "Playground"),
            ("plaza-guide.md", "Plaza — dataset gallery"),
            ("file-explorer.md", "File explorer — browse like an archive"),
            ("yasgui-guide.md", "SPARQL IDE — yasgui·wasm"),
            ("jupyterlite-guide.md", "JupyterLite — Python in the tab"),
            ("jslab-guide.md", "JS lab — rete × D3"),
            ("atlas.md", "Historical atlas — SPARQL + GIS"),
            ("ask-the-graph.md", "Ask the graph — browser graphRAG"),
            ("football.md", "Football — match replays"),
            ("subtitles-guide.md", "Subtitle timeline"),
            ("anatomy-guide.md", "Z-Anatomy — 3D human body"),
            ("building-guide.md", "FZK-Haus — one building in 3D"),
            ("bim-pair-guide.md", "Architecture vs structure (BIM)"),
            ("lombardi-guide.md", "Lombardi — network drawings in ink"),
            ("webgpu-guide.md", "WebGPU coherence (exp.)"),
            ("graph-map.md", "Graph-map, topic-map & 3D (exp.)"),
            (
                "neuro-showcase-guide.md",
                "Neuromorphology — 3D neurons & astrocytes (exp.)",
            ),
        ],
    ),
    (
        "Guides",
        &[
            ("cli.md", "CLI reference"),
            ("sparql.md", "SPARQL support"),
            ("geosparql.md", "GeoSPARQL (geometry + time)"),
            ("shacl.md", "SHACL validation"),
            ("reasoning.md", "Reasoning & coherence"),
            ("federation.md", "Federated queries"),
            ("manifest.md", "Writable graphs — manifest & WAL"),
            ("semantic-zoom.md", "Semantic zoom (schema pyramid)"),
            ("compatibility.md", "Compatibility & Cypher"),
        ],
    ),
    // User-facing docs for the language clients (clients/<lang> in the repo).
    // Maintainer docs live apart in Development → clients-dev.md.
    (
        "Clients",
        &[
            ("python.md", "Python — rete-graph"),
            ("python-build-tutorial.md", "Python: build a .rete"),
            ("javascript.md", "JavaScript — rete-graph"),
            ("comunica.md", "Comunica — RDF/JS source"),
            ("r.md", "R — rete"),
            ("blender.md", "Blender — graphs as scenes"),
            ("agents.md", "Agents — MCP, plugin & skills"),
            ("agent-frameworks.md", "LangChain & Pydantic AI"),
            ("fallacies.md", "Experiment: graphs from speech"),
        ],
    ),
    (
        "Publish & share",
        &[
            ("dataset-cards.md", "Dataset Cards"),
            ("hosting.md", "Hosting your .rete"),
            ("interop.md", "Triple-store interop"),
            ("media-companions.md", "Media & SQL companions"),
            ("release.md", "1.0 release candidate"),
        ],
    ),
    (
        "Graph analysis",
        &[
            ("topic-modeling.md", "Topic modeling (LDA)"),
            ("multi-criteria.md", "Multi-criteria communities"),
        ],
    ),
    (
        "Development",
        &[
            ("architecture.md", "Architecture"),
            ("SPEC.md", "Format specification"),
            ("rust-api.md", "Rust API"),
            ("browser.md", "WASM & JavaScript API"),
            ("clients-dev.md", "Client development & releases"),
            ("parallel-browser.md", "Parallel in browser (exp.)"),
            ("data-engineering.md", "Tables, VKG & big builds"),
            ("BENCHMARK.md", "Benchmarks"),
            ("conformance.md", "SPARQL 1.1 conformance"),
        ],
    ),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Docs dir: first CLI arg, else ./docs.
    let docs_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs"));
    if !docs_dir.is_dir() {
        return Err(format!("docs dir not found: {}", docs_dir.display()).into());
    }

    // Nav entries whose source is absent, collected BEFORE rendering: every page
    // shares one nav, so a page that cannot be rendered must not be linked from
    // any of them.
    let missing: Vec<&str> = SECTIONS
        .iter()
        .flat_map(|(_, pages)| pages.iter())
        .filter(|(page, _)| page.ends_with(".md") && !docs_dir.join(page).exists())
        .map(|(page, _)| *page)
        .collect();
    for page in &missing {
        eprintln!("warning: {page} is listed in nav but missing — skipping it and its nav entry");
    }

    let mut rendered = 0;
    for (_, pages) in SECTIONS {
        for (md, title) in *pages {
            if !md.ends_with(".md") {
                continue; // pre-built page (playground); linked, never rendered
            }
            let src = docs_dir.join(md);
            if !src.exists() {
                continue; // already reported above
            }
            let markdown = fs::read_to_string(&src)?;
            // Figures are sized by their own measured shape — see
            // `classify_images`. `src` is relative to the docs dir (`img/x.svg`),
            // and a `docs/`-prefixed path is the form the README uses, so accept
            // both; anything remote or unmeasurable is left as it is.
            let body = classify_images(&render_markdown(&markdown), &|src| {
                let rel = src.strip_prefix("docs/").unwrap_or(src);
                aspect_ratio(&docs_dir.join(rel))
            });
            let html_name = md.replace(".md", ".html");
            let page = template(title, &body, md, &summarize(&markdown), &missing);
            let out = docs_dir.join(&html_name);
            fs::write(&out, page)?;
            println!("  {md:<22} -> {html_name}");
            rendered += 1;
        }
    }
    println!(
        "docgen: wrote {rendered} HTML page(s) to {}",
        docs_dir.display()
    );
    Ok(())
}

/// The page's social summary: its first real paragraph, flattened to plain text.
///
/// This is what a link unfurls to in a chat client or a search result, so it has
/// to be prose — headings, badge rows, tables, code and block quotes are skipped,
/// and inline Markdown is stripped rather than escaped.
fn summarize(markdown: &str) -> String {
    let mut paragraph = String::new();
    let mut in_code = false;
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code = !in_code;
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if in_code {
            continue;
        }
        if line.is_empty() {
            if !paragraph.is_empty() {
                break; // the paragraph ended
            }
            continue;
        }
        // Skip everything that is not running prose.
        let skip = line.starts_with('#')
            || line.starts_with('>')
            || line.starts_with('|')
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with("<")
            || line.starts_with("![")
            || line.starts_with("---");
        if skip {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line);
    }

    let plain = strip_inline_markdown(&paragraph);
    truncate_words(&plain, 200)
}

/// `**bold**`, `` `code` ``, `[text](url)` and friends → the text they carry.
fn strip_inline_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            // [label](target) and ![alt](src) keep only the label.
            '[' => {
                let mut depth = 1;
                let mut j = i + 1;
                let mut label = String::new();
                while j < chars.len() && depth > 0 {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    if depth > 0 {
                        label.push(chars[j]);
                    }
                    j += 1;
                }
                out.push_str(&label);
                i = j + 1;
                // Drop the (target) that follows a link.
                if i < chars.len() && chars[i] == '(' {
                    while i < chars.len() && chars[i] != ')' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            '*' | '`' | '~' => i += 1,
            '!' if i + 1 < chars.len() && chars[i + 1] == '[' => i += 1,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    // A link label can itself hold `code` or **bold**; the label was copied
    // verbatim above, so clear the emphasis markers in one final pass. `_` is
    // left alone on purpose — these docs are full of snake_case identifiers.
    let out = out.replace(['*', '`', '~'], "");
    // Interactive pages open with a "▶ Launch … —" call to action; the sentence
    // after the marker is the real summary. The repo uses several triangles.
    let out = out.trim_start_matches(['▸', '▶', '▷', '►', '→', '»', '·', ' ']);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate on a word boundary, preferring a sentence end when one is near.
fn truncate_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    if let Some(stop) = cut.rfind(". ") {
        if stop > max * 55 / 100 {
            return cut[..=stop].trim_end().to_string();
        }
    }
    match cut.rfind(' ') {
        Some(space) => format!("{}…", &cut[..space]),
        None => format!("{cut}…"),
    }
}

/// The site's linear reading order: `SECTIONS` flattened, minus the entries this
/// run could not render.
///
/// This is the ONE sequence. The sidebar renders it, the drawer's "All pages"
/// panel is a clone of the sidebar, the bottom bar's prev/next are computed from
/// it, and the swipe gesture just follows those two links — so there is no
/// second source of truth to drift out of step with the nav.
fn reading_order(missing: &[&str]) -> Vec<(&'static str, &'static str)> {
    SECTIONS
        .iter()
        .flat_map(|(_, pages)| pages.iter())
        .filter(|(md, _)| md.ends_with(".md") && !missing.contains(md))
        .map(|(md, title)| (*md, *title))
        .collect()
}

/// The nav section a page belongs to — the card's category chip.
fn section_for(md: &str) -> &'static str {
    for (section, pages) in SECTIONS {
        if pages.iter().any(|(page, _)| *page == md) {
            return section;
        }
    }
    "Documentation"
}

/// HTML-escape for attribute values (social tags carry arbitrary prose).
fn attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

    let mut events: Vec<Event> = Parser::new_ext(md, opts).collect();
    anchor_headings(&mut events);
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    rewrite_links(&out)
}

/// Give every heading an `id`, so an in-page link written in the Markdown
/// (`[jump](#some-heading)`) lands somewhere in the built page.
///
/// The ids follow GitHub's slug convention, because that is what an author
/// writing `docs/*.md` sees working on github.com and will naturally copy: the
/// heading's *text* (markup dropped — `` `code` ``, links and emphasis
/// contribute what they read as, not how they are written), lowercased, with
/// everything that is not alphanumeric, underscore, space or hyphen removed and
/// spaces turned into hyphens. Repeats of one slug get a `-1`, `-2`, … suffix.
///
/// An explicit `{#id}` (ENABLE_HEADING_ATTRIBUTES) is the author's own and is
/// kept verbatim — only recorded so a later heading cannot silently take it.
fn anchor_headings(events: &mut [Event<'_>]) {
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < events.len() {
        if !matches!(events[i], Event::Start(Tag::Heading { .. })) {
            i += 1;
            continue;
        }
        // Read the heading's text off the events between it and its close.
        let mut text = String::new();
        let mut end = i + 1;
        while end < events.len() && !matches!(events[end], Event::End(TagEnd::Heading(_))) {
            match &events[end] {
                Event::Text(t) | Event::Code(t) => text.push_str(t),
                Event::SoftBreak | Event::HardBreak => text.push(' '),
                _ => {}
            }
            end += 1;
        }
        if let Event::Start(Tag::Heading { id, .. }) = &mut events[i] {
            let anchor = match id.take() {
                Some(explicit) => {
                    used.insert(explicit.to_string(), 1);
                    explicit.to_string()
                }
                None => unique(&mut used, slug(&text)),
            };
            *id = Some(anchor.into());
        }
        i = end + 1;
    }
}

/// A heading's text → the anchor GitHub would mint for it.
fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if ch == '_' {
            // `_` survives on github.com — its slugger strips punctuation with a
            // class that runs `[`-`^` then `` ` ``, jumping over `_` (0x5F). A
            // spec full of `TEXT_INDEX`-style identifiers depends on it: dropping
            // it would mint `...-textindex-...` for a heading whose GitHub anchor
            // is `...-text_index-...`, and every link an author tested on GitHub
            // would 404 in the built site.
            out.push('_');
        } else if ch == '-' || ch.is_whitespace() {
            // Runs are kept, not collapsed: `--limit <n>` slugs to `---limit-n`,
            // exactly as it does on github.com.
            out.push('-');
        }
    }
    out
}

/// Claim `base`, or the first free `base-1`, `base-2`, … if it is already taken.
fn unique(used: &mut HashMap<String, usize>, base: String) -> String {
    let base = if base.is_empty() {
        "section".to_string()
    } else {
        base
    };
    let seen = used.entry(base.clone()).or_insert(0);
    let anchor = if *seen == 0 {
        base
    } else {
        format!("{base}-{seen}")
    };
    *seen += 1;
    anchor
}

/// An image's intrinsic aspect ratio, width ÷ height, read out of the file.
///
/// SVG: the `viewBox`, which every diagram in `docs/img` carries and which is
/// the only place the shape is recorded — they deliberately have no `width` /
/// `height` attributes so they scale. PNG: the IHDR, the first chunk of the
/// file, so only 24 bytes are read however large the screenshot is.
///
/// `None` for anything unmeasurable (unknown extension, no `viewBox`, a
/// zero-height box), and the caller then leaves the image alone — an image
/// whose shape is unknown keeps the width it has always had.
fn aspect_ratio(path: &Path) -> Option<f64> {
    let (w, h) = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("svg") => {
            // The header is enough: `viewBox` is an attribute of the root
            // <svg>, so it is in the first few hundred bytes.
            let text = fs::read_to_string(path).ok()?;
            let after = text.split_once("viewBox")?.1.trim_start();
            let quoted = after.strip_prefix('=')?.trim_start();
            let quote = quoted.chars().next()?;
            let value = quoted[quote.len_utf8()..].split(quote).next()?;
            // "min-x min-y width height", separated by whitespace or commas.
            let nums: Vec<f64> = value
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<f64>().ok())
                .collect();
            (*nums.get(2)?, *nums.get(3)?)
        }
        Some("png") => {
            let mut head = [0u8; 24];
            fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
            if head[..8] != *b"\x89PNG\r\n\x1a\n" || head[12..16] != *b"IHDR" {
                return None;
            }
            let n = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap()) as f64;
            (n(16), n(20))
        }
        _ => return None,
    };
    (w > 0.0 && h > 0.0).then(|| w / h)
}

/// Tag every square-ish image in the rendered HTML, so the stylesheet can give
/// it a narrower column than a landscape one.
///
/// The classification is the *measured* ratio (`ratio_of`, keyed on the `src`)
/// against [`SQUARE_MAX_RATIO`] — never a hand-written class, so nobody has to
/// remember to mark a new diagram and nobody can mark one wrongly.
///
/// Which element carries the class depends on the shape of the markup:
///
/// * inside a `<figure>` the class goes on the **figure**, because the figure
///   is what carries the caption — capping the `<img>` alone would leave a
///   full-width caption under a half-width picture;
/// * anywhere else — a bare `<img>` block, a Markdown `![…](…)`, a Markdown
///   image wrapped in a link — it goes on the **img** itself.
fn classify_images(html: &str, ratio_of: &dyn Fn(&str) -> Option<f64>) -> String {
    // (start, end) of every `<img …>` tag, and whether it is square-ish.
    let mut imgs: Vec<(usize, usize, bool)> = Vec::new();
    let mut at = 0;
    while let Some(rel) = html[at..].find("<img") {
        let start = at + rel;
        let end = tag_end(html, start);
        let tag = &html[start..end];
        let square = attr_value(tag, "src")
            .and_then(ratio_of)
            .is_some_and(|r| r <= SQUARE_MAX_RATIO);
        imgs.push((start, end, square));
        at = end;
    }

    // (start, end) of every `<figure …>` OPENING tag paired with the offset of
    // its `</figure>`. Figures never nest in these docs, so a linear scan is
    // the whole story.
    let mut figures: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = 0;
    while let Some(rel) = html[at..].find("<figure") {
        let start = at + rel;
        let open_end = tag_end(html, start);
        let close = html[open_end..]
            .find("</figure>")
            .map(|r| open_end + r)
            .unwrap_or(html.len());
        figures.push((start, open_end, close));
        at = open_end;
    }

    // Each edit inserts a class into one opening tag. A figure claims the
    // images inside it; the rest speak for themselves.
    let mut edits: Vec<(usize, usize, &'static str)> = Vec::new();
    for &(fig_start, fig_open_end, fig_close) in &figures {
        let square_inside = imgs
            .iter()
            .any(|&(s, _, sq)| sq && s > fig_start && s < fig_close);
        if square_inside {
            edits.push((fig_start, fig_open_end, "fig-sq"));
        }
    }
    for &(start, end, square) in &imgs {
        let inside_figure = figures.iter().any(|&(s, _, c)| start > s && start < c);
        if square && !inside_figure {
            edits.push((start, end, "img-sq"));
        }
    }
    edits.sort_by_key(|&(start, _, _)| start);

    let mut out = String::with_capacity(html.len() + edits.len() * 16);
    let mut copied = 0;
    for (start, end, class) in edits {
        out.push_str(&html[copied..start]);
        out.push_str(&with_class(&html[start..end], class));
        copied = end;
    }
    out.push_str(&html[copied..]);
    out
}

/// The offset just past the `>` that closes the tag starting at `start`,
/// ignoring any `>` inside a quoted attribute value — the alt text on these
/// diagrams is a paragraph of prose and may well contain one.
fn tag_end(html: &str, start: usize) -> usize {
    let bytes = html.as_bytes();
    let mut i = start;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        match (quote, bytes[i]) {
            (Some(q), c) if c == q => quote = None,
            (None, c @ (b'"' | b'\'')) => quote = Some(c),
            (None, b'>') => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// The value of a double-quoted attribute of an opening tag, or `None`.
fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let at = tag.match_indices(&needle).find(|&(i, _)| {
        // Not a suffix of a longer attribute name: `data-src="…"` is not `src`.
        i > 0 && tag.as_bytes()[i - 1].is_ascii_whitespace()
    })?;
    tag[at.0 + needle.len()..].split('"').next()
}

/// The same opening tag with `class` added — appended if the tag already has
/// one, inserted after the element name if it does not.
fn with_class(tag: &str, class: &str) -> String {
    match attr_value(tag, "class") {
        Some(existing) => tag.replacen(
            &format!("class=\"{existing}\""),
            &format!("class=\"{existing} {class}\""),
            1,
        ),
        None => {
            let name_end = tag
                .find(|c: char| c.is_ascii_whitespace() || c == '>')
                .unwrap_or(tag.len());
            format!("{} class=\"{class}\"{}", &tag[..name_end], &tag[name_end..])
        }
    }
}

/// Make links between docs work in the rendered site: a `docs/`-prefixed or bare
/// `*.md` href becomes the sibling `*.html`. Only touches `href="..."` targets,
/// not code or text.
fn rewrite_links(html: &str) -> String {
    html.replace("href=\"docs/", "href=\"")
        .replace(".md\"", ".html\"")
        .replace(".md#", ".html#")
}

/// Overview `.md` pages that head a collapsible nav group of full-screen
/// interactive apps. The **first** sub is the rendered overview (stays in-page);
/// the rest are pre-built apps that open in a new tab.
fn nav_group_subs(md: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match md {
        "playground-guide.md" => Some(&[
            ("playground-guide.html", "overview"),
            ("playground.html", "launch the playground →"),
        ]),
        "graph-map.md" => Some(&[
            ("graph-map.html", "overview"),
            ("graph-map/viewer.html", "structural map"),
            ("graph-map/viewer-topics.html", "topic map (LDA)"),
            ("graph-map/viewer-3d.html", "3D — deck.gl"),
            ("graph-map/viewer-3d-three.html", "3D — three.js + fog"),
        ]),
        "yasgui-guide.md" => Some(&[
            ("yasgui-guide.html", "overview"),
            ("yasgui.html", "launch the IDE →"),
        ]),
        "jupyterlite-guide.md" => Some(&[
            ("jupyterlite-guide.html", "overview"),
            (
                "jupyterlite/lab/index.html?path=rete-graph.ipynb",
                "tour: query graphs →",
            ),
            (
                "jupyterlite/lab/index.html?path=build-a-rete.ipynb",
                "build: anatomy of a .rete →",
            ),
        ]),
        "jslab-guide.md" => Some(&[
            ("jslab-guide.html", "overview"),
            ("jslab.html", "launch the lab →"),
        ]),
        "atlas.md" => Some(&[
            ("atlas.html", "overview"),
            ("atlas-app.html", "launch the atlas →"),
        ]),
        "ask-the-graph.md" => Some(&[
            ("ask-the-graph.html", "overview"),
            ("ask-browser.html", "launch ask the graph →"),
        ]),
        "football.md" => Some(&[
            ("football.html", "overview"),
            ("pitch.html", "pick any match →"),
            ("wcfinal.html", "the 2022 final →"),
        ]),
        "subtitles-guide.md" => Some(&[
            ("subtitles-guide.html", "overview"),
            ("subtitles.html", "play the timeline →"),
        ]),
        "anatomy-guide.md" => Some(&[
            ("anatomy-guide.html", "overview"),
            ("anatomy.html", "open the 3D body →"),
        ]),
        "building-guide.md" => Some(&[
            ("building-guide.html", "overview"),
            ("building.html", "open the 3D building →"),
        ]),
        "bim-pair-guide.md" => Some(&[
            ("bim-pair-guide.html", "overview"),
            ("bim-pair.html", "open the paired viewer →"),
        ]),
        "lombardi-guide.md" => Some(&[
            ("lombardi-guide.html", "overview"),
            ("lombardi.html", "open the drawings →"),
        ]),
        "webgpu-guide.md" => Some(&[
            ("webgpu-guide.html", "overview"),
            ("webgpu.html", "run the experiment →"),
        ]),
        "plaza-guide.md" => Some(&[
            ("plaza-guide.html", "overview"),
            ("plaza/index.html", "open the gallery →"),
        ]),
        "neuro-showcase-guide.md" => Some(&[
            ("neuro-showcase-guide.html", "overview"),
            ("neuro-showcase.html", "reconstruct in 3D →"),
            (
                "playground.html#dataset=neuro-showcase&load=lazy",
                "query in the playground →",
            ),
        ]),
        _ => None,
    }
}

fn template(title: &str, body: &str, current_md: &str, summary: &str, missing: &[&str]) -> String {
    let mut nav_items: Vec<String> = Vec::new();
    for (section, pages) in SECTIONS {
        nav_items.push(format!("<li class=\"nav-h\">{section}</li>"));
        for (md, t) in *pages {
            // A nav entry for a page this run did not produce would be a link to
            // nothing on all 50 pages — the exact breakage `check_docs_links.py`
            // reports. Listing a `.md` in SECTIONS before committing it is an
            // easy mistake; the entry simply reappears once the file lands.
            if missing.contains(md) {
                continue;
            }
            // An overview `.md` that owns a set of full-screen interactive pages
            // renders as a collapsible group: the first sub is the rendered
            // overview (in-page), the rest are the apps (each in a new tab).
            if let Some(subs) = nav_group_subs(md) {
                let open = if *md == current_md { " open" } else { "" };
                let overview = subs[0].0;
                let mut sub = String::new();
                for (i, (h, l)) in subs.iter().enumerate() {
                    let a = if i == 0 && *h == overview && *md == current_md {
                        " class=\"active\""
                    } else {
                        ""
                    };
                    let tgt = if i == 0 {
                        "" // the overview stays in-page
                    } else {
                        " target=\"_blank\" rel=\"noopener\""
                    };
                    sub.push_str(&format!("<li><a href=\"{h}\"{a}{tgt}>{l}</a></li>"));
                }
                // The summary title links to the overview: clicking a group in
                // the sidebar opens its introduction AND (because that page
                // renders with the group `open`) leaves the sidebar expanded.
                // The disclosure triangle still toggles the sub-list in place.
                let sumclass = if *md == current_md {
                    " class=\"active\""
                } else {
                    ""
                };
                nav_items.push(format!(
                    "<li class=\"nav-group\"><details{open}><summary><a href=\"{overview}\"{sumclass}>{t}</a></summary><ul class=\"nav-sub\">{sub}</ul></details></li>"
                ));
                continue;
            }
            let href = md.replace(".md", ".html");
            let class = if *md == current_md {
                " class=\"active\""
            } else {
                ""
            };
            nav_items.push(format!("<li><a href=\"{href}\"{class}>{t}</a></li>"));
        }
    }
    let nav = nav_items.join("\n        ");

    // Social preview. The card image is pre-rendered per page by
    // scripts/preview/render_cards.mjs into docs/og/doc/<name>.png; the gate
    // checks that every page's og:image actually exists.
    let html_name = current_md.replace(".md", ".html");
    let stem = html_name.trim_end_matches(".html");
    let section = section_for(current_md);
    let social_title = format!("{title} · rete docs");
    let social_desc = if summary.is_empty() {
        format!(
            "{section} — the rete documentation. Cloud-native, range-queryable RDF graph files."
        )
    } else {
        summary.to_string()
    };
    // ── The mobile reading chrome ────────────────────────────────────────────
    // Narrow viewports get a fixed top bar, a fixed bottom bar and an on-demand
    // drawer instead of the sidebar dumped above the prose. All three are
    // `display:none` until the ≤780px media query turns them on, so a desktop
    // page renders byte-identically to before apart from this inert markup.
    let order = reading_order(missing);
    let pos = order.iter().position(|(md, _)| *md == current_md);
    let total = order.len();
    let seq = match pos {
        Some(i) => format!("{} of {total}", i + 1),
        None => String::new(),
    };
    // Prev/next are real links, present in the HTML with the neighbour's title:
    // they work with JavaScript off, they are the accessible equivalent of the
    // swipe gesture, and the swipe handler navigates by reading their `href`.
    let step = |offset: isize, dir: &str, arrow_first: bool| -> String {
        let target = pos.and_then(|i| {
            let j = i as isize + offset;
            if j < 0 {
                None
            } else {
                order.get(j as usize)
            }
        });
        let class = if offset < 0 {
            "mnav-side mnav-prev"
        } else {
            "mnav-side mnav-next"
        };
        match target {
            Some((md, t)) => {
                let href = md.replace(".md", ".html");
                let rel = if offset < 0 { "prev" } else { "next" };
                let id = if offset < 0 { "navPrev" } else { "navNext" };
                let label = if arrow_first {
                    format!("\u{2039} {dir}")
                } else {
                    format!("{dir} \u{203a}")
                };
                format!(
                    "<a class=\"{class}\" id=\"{id}\" rel=\"{rel}\" href=\"{href}\"><span class=\"mnav-dir\">{label}</span><span class=\"mnav-t\">{title}</span></a>",
                    title = attr(t)
                )
            }
            // No neighbour in that direction: an inert slot, so the bar keeps
            // its three-column shape and the swipe handler finds no href.
            None => {
                let edge = if offset < 0 {
                    "Start of the docs"
                } else {
                    "End of the docs"
                };
                format!("<span class=\"{class} is-off\"><span class=\"mnav-dir\">{dir}</span><span class=\"mnav-t\">{edge}</span></span>")
            }
        }
    };
    let topbar = format!(
        r#"<div class="mprog" aria-hidden="true"><i id="mprogBar"></i></div>
  <header class="mbar" id="mbar">
    <button type="button" class="mbar-btn" id="mbarMenu" aria-controls="mdrawer" aria-expanded="false"
      aria-label="Contents — this page's sections and every other page">
      <svg width="19" height="19" viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M3 5h14M3 10h14M3 15h10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
    </button>
    <span class="mbar-id">
      <span class="mbar-sec"><a href="index.html">rete</a> · {section}</span>
      <span class="mbar-title">{title}</span>
    </span>
    <button type="button" class="mbar-btn" id="mbarTheme" data-theme-cycle aria-label="Theme">◐</button>
  </header>"#,
        section = attr(section),
        title = attr(title),
    );
    let bottombar = format!(
        r#"<nav class="mnav" id="mnav" aria-label="Page navigation">
    {prev}
    <button type="button" class="mnav-toc" id="mnavToc" aria-controls="mdrawer" aria-expanded="false">
      <svg width="17" height="17" viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M3 5h14M3 10h14M3 15h10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
      <span>Contents</span>
    </button>
    {next}
  </nav>"#,
        prev = step(-1, "Previous", true),
        next = step(1, "Next", false),
    );
    let drawer = format!(
        r#"<div class="mscrim" id="mscrim" hidden></div>
  <aside class="mdrawer" id="mdrawer" role="dialog" aria-modal="true" aria-labelledby="mdrawerTitle" hidden>
    <div class="mdrawer-head">
      <div class="mdrawer-who">
        <span class="mdrawer-eyebrow">{section}{sep}{seq}</span>
        <span class="mdrawer-title" id="mdrawerTitle">{title}</span>
      </div>
      <button type="button" class="mdrawer-x" id="mdrawerClose" aria-label="Close contents">
        <svg width="17" height="17" viewBox="0 0 20 20" aria-hidden="true" focusable="false"><path d="M5 5l10 10M15 5L5 15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
        <span>Close</span>
      </button>
    </div>
    <div class="mdrawer-tabs" role="tablist" aria-label="Contents">
      <button type="button" role="tab" id="tabHere" aria-controls="panelHere" aria-selected="true">On this page</button>
      <button type="button" role="tab" id="tabAll" aria-controls="panelAll" aria-selected="false" tabindex="-1">All pages</button>
    </div>
    <div class="mdrawer-body">
      <div class="mdrawer-panel" id="panelHere" role="tabpanel" aria-labelledby="tabHere" tabindex="0"></div>
      <div class="mdrawer-panel" id="panelAll" role="tabpanel" aria-labelledby="tabAll" tabindex="0" hidden></div>
    </div>
  </aside>"#,
        section = attr(section),
        sep = if seq.is_empty() { "" } else { " · " },
        seq = seq,
        title = attr(title),
    );

    let social = format!(
        r#"<meta name="description" content="{desc}" />
  <link rel="canonical" href="{base}{page}" />
  <meta property="og:type" content="article" />
  <meta property="og:site_name" content="rete" />
  <meta property="og:title" content="{ogtitle}" />
  <meta property="og:description" content="{desc}" />
  <meta property="og:url" content="{base}{page}" />
  <meta property="og:image" content="{base}og/doc/{stem}.png" />
  <meta property="og:image:width" content="1200" />
  <meta property="og:image:height" content="630" />
  <meta property="og:image:alt" content="{ogtitle}" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta name="twitter:title" content="{ogtitle}" />
  <meta name="twitter:description" content="{desc}" />
  <meta name="twitter:image" content="{base}og/doc/{stem}.png" />
  <meta name="rete:section" content="{section}" />"#,
        base = SITE_BASE,
        page = html_name,
        stem = stem,
        section = attr(section),
        ogtitle = attr(&social_title),
        desc = attr(&social_desc),
    );

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
  <meta name="color-scheme" content="light dark" />
  <title>{title} · rete docs</title>
  {social}
  <script>
  /* Theme, before first paint (no flash): an explicit localStorage choice
     ("theme" = "light"/"dark", shared with the playground) pins data-theme;
     otherwise prefers-color-scheme decides.

     `js-chrome` is set in the same breath, and it is what lets the narrow
     layout replace the sidebar with the top/bottom bars and the drawer: with
     JavaScript off the class never lands, the mobile chrome stays inert, and
     the reader gets the plain stacked sidebar that has always been there. */
  (function () {{
    try {{
      var t = localStorage.getItem("theme");
      if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
    }} catch (e) {{}}
    document.documentElement.className += " js-chrome";
  }})();
  </script>
  <style>{css}{mobilecss}</style>
</head>
<body>
  {topbar}
  <nav class="sidebar">
    <a class="brand" href="index.html">rete</a>
    <p class="tagline">Cloud-native, range-queryable RDF graph files</p>
    <p class="meta"><span class="ver">v{version}</span> <a href="{repo}">caviri/rete</a></p>
    <ul>
        {nav}
    </ul>
    <p class="foot"><a href="{repo}">github.com/caviri/rete</a></p>
    <button type="button" class="theme-toggle" id="themeToggle" data-theme-cycle
      title="Theme: follows your system; click to cycle System → Light → Dark">◐ <span id="themeToggleLabel" data-theme-label>System</span></button>
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
    <footer>Generated from <code>docs/{current_md}</code> by <code>cargo run -p docgen</code> · <a href="https://ko-fi.com/caviri">☕ Support rete on Ko-fi</a></footer>
  </main>
  {bottombar}
  {drawer}
  <script>{script}</script>
  <script>{lightbox}</script>
  <script>{glossary}</script>
  <script>{toc}</script>
  <script>{mobilejs}</script>
</body>
</html>
"##,
        title = title,
        social = social,
        css = CSS,
        mobilecss = MOBILE_CSS,
        nav = nav,
        body = body,
        current_md = current_md,
        version = VERSION,
        repo = REPO_URL,
        topbar = topbar,
        bottombar = bottombar,
        drawer = drawer,
        script = HIGHLIGHTER,
        lightbox = LIGHTBOX,
        glossary = GLOSSARY_JS,
        toc = TOC_JS,
        mobilejs = MOBILE_JS,
    )
}

const CSS: &str = r#"
:root {
  --fg:#17211d; --muted:#66746e; --bg:#f6f8f7; --panel:#ffffff;
  --side:#eef4ef; --side-fg:#25342e; --accent:#147d69; --accent-2:#c84f2f;
  --border:#d9e2de; --code-bg:#eef3f1; --code-border:#cfd9d5;
  --accent-deep:#0b4f42; --iri:#0b6f5e; --on-accent:#ffffff;
  --tint:#eaf1ed; --warm:#fff8f4;
  --hl-kw:#8a3d5a; --hl-str:#0b745f; --hl-num:#a85424; --hl-comment:#78877f;
  --rail-bg:rgba(255,255,255,.72); --bevel:rgba(255,255,255,.8);
  --shadow:0 18px 44px rgba(32,47,41,.10);
  color-scheme: light;
}
/* Dark palette — same green-tinted family, inverted. An explicit choice
   (data-theme, set before first paint) wins; otherwise the OS decides. */
:root[data-theme="dark"] {
  --fg:#dde8e2; --muted:#93a69d; --bg:#0f1512; --panel:#171f1b;
  --side:#131a16; --side-fg:#c4d4cc; --accent:#35b299; --accent-2:#e57e5e;
  --border:#2a352f; --code-bg:#1a231e; --code-border:#31403a;
  --accent-deep:#7fdcc7; --iri:#6fcdb6;
  --tint:#1a2620; --warm:#2c211a;
  --hl-kw:#d98cab; --hl-str:#57c9ab; --hl-num:#e0a06d; --hl-comment:#7d8d85;
  --rail-bg:rgba(23,31,27,.85); --bevel:rgba(255,255,255,.06);
  --shadow:0 18px 44px rgba(0,0,0,.45);
  color-scheme: dark;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --fg:#dde8e2; --muted:#93a69d; --bg:#0f1512; --panel:#171f1b;
    --side:#131a16; --side-fg:#c4d4cc; --accent:#35b299; --accent-2:#e57e5e;
    --border:#2a352f; --code-bg:#1a231e; --code-border:#31403a;
    --accent-deep:#7fdcc7; --iri:#6fcdb6;
    --tint:#1a2620; --warm:#2c211a;
    --hl-kw:#d98cab; --hl-str:#57c9ab; --hl-num:#e0a06d; --hl-comment:#7d8d85;
    --rail-bg:rgba(23,31,27,.85); --bevel:rgba(255,255,255,.06);
    --shadow:0 18px 44px rgba(0,0,0,.45);
    color-scheme: dark;
  }
}
.theme-toggle {
  margin-top:.9rem; padding:.3rem .65rem; font:inherit; font-size:.82rem;
  color:var(--muted); background:transparent; border:1px solid var(--border);
  border-radius:7px; cursor:pointer;
}
.theme-toggle:hover { color:var(--fg); border-color:var(--accent); }
* { box-sizing:border-box; }
/* text-size-adjust: without it, mobile browsers "boost" the font of wide
   blocks — exactly our no-wrap code blocks, which render comically large
   on phones. 100% = render at the authored size. */
html { scroll-behavior:smooth; -webkit-text-size-adjust:100%; text-size-adjust:100%; }
body {
  margin:0; color:var(--fg); background:linear-gradient(90deg,var(--side) 0 272px,var(--bg) 272px);
  display:flex; font:16px/1.68 "Aptos","Segoe UI",system-ui,sans-serif;
  text-rendering:optimizeLegibility;
}
code,pre,.mono { font-family:"Cascadia Mono","SF Mono",Consolas,ui-monospace,monospace; }

.sidebar {
  width:272px; min-height:100vh; padding:1.45rem 1.1rem;
  flex:0 0 272px; color:var(--side-fg); border-right:1px solid var(--border);
}
.sidebar .brand {
  color:var(--fg); font-family:Georgia,"Times New Roman",serif; font-size:1.75rem;
  font-weight:700; text-decoration:none; display:inline-block; line-height:1;
}
.sidebar .brand::after { content:""; display:block; width:2.2rem; height:3px; margin-top:.55rem; background:var(--accent-2); }
.sidebar .tagline { color:var(--muted); font-size:.82rem; line-height:1.35; margin:.75rem 0 .6rem; }
.sidebar .meta { display:flex; align-items:center; gap:.5rem; font-size:.8rem; margin:0 0 1.1rem; }
.sidebar .meta .ver {
  font-family:"Cascadia Mono","SF Mono",Consolas,ui-monospace,monospace;
  font-size:.72rem; font-weight:700; color:var(--accent-deep); background:var(--panel);
  border:1px solid var(--border); border-radius:999px; padding:.06rem .55rem;
}
.sidebar .meta a { display:inline; padding:0; border:0; color:var(--iri); font-weight:600; }
.sidebar .meta a:hover { background:none; text-decoration:underline; }
.sidebar ul { list-style:none; padding:0; margin:0; }
.sidebar li { margin:.08rem 0; }
.sidebar li.nav-h {
  margin:1.15rem 0 .3rem; padding:0 .62rem; font-size:.66rem; font-weight:800;
  letter-spacing:.09em; text-transform:uppercase; color:var(--side-fg);
}
.sidebar li.nav-h:first-child { margin-top:.2rem; }
.sidebar a {
  color:var(--side-fg); text-decoration:none; display:block; padding:.42rem .62rem;
  border-left:3px solid transparent; border-radius:0 6px 6px 0; font-size:.93rem; line-height:1.25;
}
.sidebar a:hover { background:rgba(20,125,105,.09); color:var(--accent-deep); }
.sidebar a.active { background:var(--panel); border-left-color:var(--accent); color:var(--accent-deep); font-weight:700; box-shadow:0 6px 18px rgba(20,125,105,.10); }
.sidebar .nav-group summary {
  cursor:pointer; list-style:none; padding:.42rem .62rem; font-size:.93rem; line-height:1.25;
  color:var(--side-fg); border-left:3px solid transparent; border-radius:0 6px 6px 0;
}
.sidebar .nav-group summary::-webkit-details-marker { display:none; }
.sidebar .nav-group summary::before { content:"\25B8\00a0"; color:var(--muted); font-size:.85em; }
.sidebar .nav-group details[open] > summary::before { content:"\25BE\00a0"; }
.sidebar .nav-group summary:hover { background:rgba(20,125,105,.09); color:var(--accent-deep); }
/* The summary title is a link to the overview; keep it inline (no second block
   padding) so it reads as the group heading, not a nested item. */
.sidebar .nav-group summary a { display:inline; padding:0; margin:0; border:0; border-radius:0; box-shadow:none; color:inherit; font-size:inherit; line-height:inherit; }
.sidebar .nav-group summary a:hover { background:none; color:inherit; }
.sidebar .nav-group summary a.active { background:none; box-shadow:none; color:var(--accent-deep); font-weight:700; }
.sidebar .nav-sub { padding-left:.55rem; margin:.1rem 0 .35rem .55rem; border-left:1px solid var(--border); }
.sidebar .nav-sub a { font-size:.85rem; padding:.3rem .6rem; color:var(--muted); }
.sidebar .nav-sub a:hover { color:var(--accent-deep); }
.sidebar .nav-sub a.active { font-weight:700; color:var(--accent-deep); }
.sidebar .foot { margin-top:1.3rem; font-size:.85rem; }
.sidebar .foot a { display:inline; padding:0; border:0; color:var(--muted); }

main { flex:1 1 auto; min-width:0; display:flex; flex-direction:column; }
.page {
  width:min(1320px,100%); margin:0 auto; display:grid;
  grid-template-columns:minmax(0,980px) 230px;
  gap:2.4rem; padding:0 2.2rem; align-items:start;
}
.content { min-width:0; padding:3.1rem 0 2rem; }
footer { width:min(1320px,100%); margin:0 auto; padding:1rem 2.2rem 3rem; color:var(--muted); font-size:.84rem; }

.rail {
  margin-top:3.25rem; font-size:.82rem;
}
.toc,.keyterms { background:var(--rail-bg); border:1px solid var(--border); border-radius:8px; padding:.72rem .78rem; box-shadow:var(--shadow); }
.toc .toc-h,.keyterms .kt-h {
  font-size:.68rem; font-weight:800; text-transform:uppercase; letter-spacing:.08em;
  color:var(--side-fg); margin-bottom:.55rem;
}
.toc ul { list-style:none; margin:0; padding:0; }
.toc a {
  display:block; padding:.25rem .35rem; border-left:2px solid transparent;
  color:var(--muted); text-decoration:none; line-height:1.32;
}
.toc a:hover,.toc a.active { color:var(--accent); }
.toc a.active { border-left-color:var(--accent); font-weight:700; }
.toc .toc-sub a { padding-left:1rem; font-size:.95em; }
.keyterms { margin-top:1rem; border-top:3px solid var(--accent-2); }
.keyterms .kt-i { margin:.45rem 0; line-height:1.42; color:var(--muted); }
.keyterms .kt-i b { color:var(--fg); }

.content h1,.content h2,.content h3 { scroll-margin-top:1rem; }
.content h1 {
  max-width:860px; font-family:Georgia,"Times New Roman",serif; font-size:2.65rem;
  line-height:1.08; margin:.1rem 0 1.15rem; font-weight:700;
}
.content h1::after { content:""; display:block; width:4.4rem; height:4px; margin-top:1rem; background:var(--accent); }
.content h2 {
  font-size:1.42rem; margin:2.45rem 0 .75rem; padding-top:.25rem;
  border-top:1px solid var(--border);
}
.content h3 { font-size:1.12rem; margin:1.55rem 0 .5rem; }
.content a { color:var(--iri); text-decoration:none; font-weight:600; }
.content a:hover { text-decoration:underline; text-underline-offset:.18em; }
.content p,.content li { color:var(--fg); }
.content li { margin:.18rem 0; }
.content code {
  background:var(--code-bg); border:1px solid var(--code-border); padding:.08em .34em;
  border-radius:5px; font-size:.88em;
}
.content pre {
  background:var(--tint); border:1px solid var(--code-border); border-radius:8px;
  max-width:100%; padding:1rem 1.1rem; overflow:auto; line-height:1.52;
  box-shadow:inset 0 1px 0 var(--bevel);
}
.content pre code { display:block; min-width:100%; width:max-content; background:none; border:0; padding:0; font-size:.85rem; }
.content blockquote {
  margin:1.15rem 0; padding:.65rem 1rem; border-left:4px solid var(--accent-2);
  background:var(--warm); color:#49362e; border-radius:0 8px 8px 0;
}
.content table { border-collapse:separate; border-spacing:0; width:100%; margin:1.1rem 0; font-size:.9rem; overflow:hidden; border:1px solid var(--border); border-radius:8px; }
.content th,.content td { border-bottom:1px solid var(--border); padding:.52rem .68rem; text-align:left; vertical-align:top; }
.content th { background:var(--tint); color:#26342f; font-weight:800; }
.content tr:nth-child(even) td { background:var(--panel); }
.content tr:last-child td { border-bottom:0; }
.content img { max-width:100%; }
.content hr { border:none; border-top:1px solid var(--border); margin:2.2rem 0; }

.content figure { margin:0; }
.content figure.fig-right { float:right; width:min(42%, 390px); margin:.35rem 0 1rem 1.8rem; clear:right; }
.content figure.fig-center { margin:1.4rem auto; max-width:680px; }
.content figure img {
  width:100%; border:1px solid var(--border); border-radius:8px; background:var(--panel);
  padding:.55rem; box-shadow:var(--shadow);
}
.content figure figcaption { font-size:.8rem; color:var(--muted); margin-top:.45rem; line-height:1.45; }
.content h2 { clear:right; }

/* Square figures get a narrower column than landscape ones.
   `.img-sq` / `.fig-sq` are stamped by docgen from the image's MEASURED
   aspect ratio — the viewBox of an SVG, the IHDR of a PNG — against
   SQUARE_MAX_RATIO; nothing here is decided by hand. The reason: at the full
   column a square is as tall as it is wide, so one picture is an 830-950px
   wall that pushes every word after it under the fold, while a landscape
   strip (the byte-layout diagrams, the app screenshots) needs that width to
   stay legible and is left alone.
   The cap is 60% of the column, clamped at both ends: never above 560px,
   which is the width every diagram is DRAWN at, so they render at their
   intended size instead of being enlarged up to 1.75x; and never below
   340px, so a narrow desktop window — where the column is small but the
   sidebar still takes its 250px — gets a readable figure rather than a
   thumbnail.
   ONLY above the phone breakpoint. The mobile column is 353px at 390px, and
   60% of that is 212px of unreadable diagram, so a phone keeps every image
   full-bleed exactly as before — hence min-width:781px, the complement of the
   780px query the mobile chrome uses. */
@media (min-width:781px) {
  .content img.img-sq { display:block; max-width:clamp(340px, 60%, 560px); margin-left:auto; margin-right:auto; }
  /* Anchors are inline, so a linked image needs a block box to centre in. */
  .content a:has(> img.img-sq) { display:block; }
  /* The FIGURE is capped, not its img, so the caption stays the same width as
     the picture it captions. `fig-right` is excluded: it is already floated at
     min(42%, 390px), and this would make it wider, not narrower. */
  .content figure.fig-sq:not(.fig-right) { max-width:clamp(340px, 60%, 560px); }
}

.content img { cursor:zoom-in; }
.lightbox { position:fixed; inset:0; z-index:1000; display:none; cursor:zoom-out; background:rgba(19,29,25,.86); padding:3vmin; }
.lightbox.open { display:flex; align-items:center; justify-content:center; }
.lightbox img { max-width:96vw; max-height:94vh; width:auto; height:auto; border-radius:8px; background:var(--panel); padding:.5rem; box-shadow:0 10px 40px rgba(0,0,0,.5); }
.lightbox .lb-close { position:fixed; top:1rem; right:1.25rem; font-size:2rem; line-height:1; color:var(--on-accent); opacity:.82; cursor:pointer; user-select:none; }
.lightbox .lb-close:hover { opacity:1; }

.content .term { border-bottom:1px dotted var(--accent); cursor:help; position:relative; }
.content .term:focus { outline:2px solid rgba(20,125,105,.32); outline-offset:2px; }
.content .term .tip {
  display:none; position:absolute; left:0; top:1.55em; z-index:60; width:max-content; max-width:280px;
  background:var(--fg); color:var(--tint); padding:.5rem .7rem; border-radius:7px; font-size:.78rem;
  font-weight:400; line-height:1.45; box-shadow:0 6px 22px rgba(15,22,32,.3);
  pointer-events:none;
}
.content .term:hover .tip,.content .term:focus .tip { display:block; }

.content pre code .tok-com  { color:var(--hl-comment); font-style:italic; }
.content pre code .tok-str  { color:var(--hl-str); }
.content pre code .tok-kw   { color:var(--hl-kw); font-weight:700; }
.content pre code .tok-num  { color:var(--hl-num); }
.content pre code .tok-fn   { color:var(--iri); }
.content pre code .tok-iri  { color:var(--iri); }
.content pre code .tok-var  { color:var(--hl-num); }
.content pre code .tok-flag { color:#9a5a14; }
.content pre code .tok-punct{ color:var(--muted); }

@media (max-width:1120px) { .page { grid-template-columns:minmax(0,1fr); } .rail { display:none; } }
@media (max-width:980px) { .content figure.fig-right { float:none; width:100%; margin:1.4rem 0; } }
@media (max-width:780px) {
  body { display:block; background:var(--bg); }
  .sidebar { width:100%; min-height:auto; position:static; border-right:0; border-bottom:1px solid var(--border); background:var(--side); }
  .sidebar ul { columns:2; column-gap:.7rem; }
  .sidebar li { break-inside:avoid; }
  .page { padding:0 1.15rem; }
  .content { padding-top:2rem; }
  .content h1 { font-size:2.15rem; }
  footer { padding-left:1.15rem; padding-right:1.15rem; }
  /* Wide tables scroll inside their own box from here down, not just below
     520px: it keeps the PAGE from scrolling sideways on a tablet, and it is
     what the swipe handler looks for when deciding whether a horizontal drag
     belongs to the table under the finger or to the page. */
  .content table { display:block; overflow-x:auto; }
  /* A single unbreakable token in INLINE code — `CARGO_UNSTABLE_BUILD_STD=std,
     panic_abort,…`, `dev/perms/{build_bench.sh,…}` — used to push the document
     wider than the screen, and a phone's answer to that is to zoom the whole
     page out: BENCHMARK, SPEC, clients-dev and sparql all rendered 20-30%
     smaller than they should on a 390px phone because of one token. Let those
     wrap. Code BLOCKS are untouched — they are `width:max-content` inside a
     scrollable `pre` and never overflow their line box. */
  .content code { overflow-wrap:break-word; }
  .content pre code { overflow-wrap:normal; }
}
@media (max-width:520px) {
  .sidebar ul { columns:1; }
  .content .term .tip { position:fixed; left:1rem; right:1rem; top:auto; bottom:1rem; width:auto; max-width:none; }
}
@media (prefers-reduced-motion: reduce) {
  html { scroll-behavior:auto; }
}
"#;

/// The mobile reading chrome: a fixed top bar, a fixed bottom bar and a TOC
/// drawer, all of them `display:none` until the ≤780px media query turns them
/// on. Nothing in here can reach a desktop viewport — the desktop rules live
/// untouched in `CSS` above and this file only ever adds.
///
/// Colours come from the same custom properties as everything else, so the new
/// chrome follows the light/dark toggle in both directions for free.
const MOBILE_CSS: &str = r#"
.mbar,.mnav,.mdrawer,.mscrim,.mprog { display:none; }

@media (max-width:780px) {
  :root { --mbar-h:52px; --mnav-h:56px; }

  /* The sidebar is only retired when the head script has confirmed JavaScript
     — see the `js-chrome` note there. Without it, nothing below applies and
     the page keeps the stacked sidebar it has always had. */
  html.js-chrome .sidebar { display:none; }
  html.js-chrome body {
    padding-top:var(--mbar-h);
    padding-bottom:calc(var(--mnav-h) + env(safe-area-inset-bottom));
  }
  html.js-chrome .page {
    padding-left:max(1.15rem, env(safe-area-inset-left));
    padding-right:max(1.15rem, env(safe-area-inset-right));
  }
  html.js-chrome .content { padding-top:1.4rem; }
  /* Anchor targets must clear the fixed bar, or every in-page jump lands with
     its heading hidden underneath it. */
  html.js-chrome .content h1,
  html.js-chrome .content h2,
  html.js-chrome .content h3 { scroll-margin-top:calc(var(--mbar-h) + 12px); }
  html.drawer-open, html.drawer-open body { overflow:hidden; }
  /* `hidden` stays the one truth for "closed" — for assistive tech as much
     as for paint — so it has to outrank the display rules below. */
  html.js-chrome .mdrawer[hidden],html.js-chrome .mscrim[hidden] { display:none; }

  /* ── reading position ─────────────────────────────────────────────────
     Two pixels at the very top of the viewport, above the bar in z-order so
     it survives the bar sliding away. "How much of this is left" is the one
     piece of state a phone reader cannot see for themselves — the scrollbar
     that answers it on a desktop does not exist here. */
  html.js-chrome .mprog {
    display:flex; position:fixed; inset:0 0 auto 0; z-index:901; height:2px;
    background:transparent; pointer-events:none;
  }
  .mprog i { display:block; width:0; height:100%; background:var(--accent); transition:width .1s linear; }

  /* ── top bar ──────────────────────────────────────────────────────────
     Identity (where am I in the site?) plus the two controls worth a
     permanent tap target. It hides on scroll-down and comes back on
     scroll-up, so the 52px is only spent when the reader is not reading. */
  html.js-chrome .mbar {
    display:flex; position:fixed; inset:0 0 auto 0; z-index:900; height:var(--mbar-h);
    align-items:center; gap:.3rem;
    padding-left:max(.3rem, env(safe-area-inset-left));
    padding-right:max(.3rem, env(safe-area-inset-right));
    background:var(--side); border-bottom:1px solid var(--border); color:var(--side-fg);
    transition:transform .18s ease;
  }
  html.js-chrome .mbar.up { transform:translateY(-100%); }
  .mbar-btn {
    flex:0 0 auto; display:flex; align-items:center; justify-content:center;
    width:42px; height:42px; padding:0; font:inherit; font-size:1.05rem;
    color:var(--side-fg); background:transparent; border:0; border-radius:9px; cursor:pointer;
  }
  .mbar-btn:active { background:rgba(20,125,105,.14); }
  .mbar-btn:focus-visible,.mnav a:focus-visible,.mnav button:focus-visible,
  .mdrawer a:focus-visible,.mdrawer button:focus-visible {
    outline:2px solid var(--accent); outline-offset:-2px;
  }
  .mbar-id { flex:1 1 auto; min-width:0; display:flex; flex-direction:column; justify-content:center; line-height:1.18; }
  .mbar-sec {
    font-size:.66rem; font-weight:800; letter-spacing:.07em; text-transform:uppercase;
    color:var(--muted); white-space:nowrap; overflow:hidden; text-overflow:ellipsis;
  }
  .mbar-sec a { color:var(--iri); text-decoration:none; font-family:Georgia,"Times New Roman",serif; font-size:1.05em; letter-spacing:0; text-transform:none; }
  .mbar-title {
    font-size:.92rem; font-weight:700; color:var(--fg);
    white-space:nowrap; overflow:hidden; text-overflow:ellipsis;
  }

  /* ── bottom bar ───────────────────────────────────────────────────────
     Thumb country. Previous / next carry the neighbour's real title (a bare
     arrow tells you nothing about whether to press it) and the middle opens
     the drawer — the same control as the top-left one, put where a thumb
     already is. These two links are also the accessible equivalent of the
     swipe: the gesture only ever follows them. */
  html.js-chrome .mnav {
    display:grid; position:fixed; inset:auto 0 0 0; z-index:900;
    grid-template-columns:1fr auto 1fr; align-items:stretch;
    min-height:var(--mnav-h);
    padding-bottom:env(safe-area-inset-bottom);
    padding-left:env(safe-area-inset-left); padding-right:env(safe-area-inset-right);
    background:var(--side); border-top:1px solid var(--border);
    box-shadow:0 -6px 18px rgba(20,40,33,.06);
  }
  .mnav-side {
    display:flex; flex-direction:column; justify-content:center; gap:.1rem;
    min-width:0; padding:.42rem .6rem; text-decoration:none; color:var(--side-fg);
  }
  .mnav-next { align-items:flex-end; text-align:right; }
  .mnav-side:active { background:rgba(20,125,105,.1); }
  .mnav-side.is-off { color:var(--muted); opacity:.45; }
  .mnav-dir {
    font-size:.6rem; font-weight:800; letter-spacing:.08em; text-transform:uppercase; color:var(--muted);
  }
  .mnav-t {
    font-size:.79rem; font-weight:600; line-height:1.2; max-width:100%;
    white-space:nowrap; overflow:hidden; text-overflow:ellipsis;
  }
  .mnav-toc {
    display:flex; flex-direction:column; align-items:center; justify-content:center; gap:.1rem;
    padding:.3rem .7rem; margin:.42rem 0; font:inherit; font-size:.62rem; font-weight:800;
    letter-spacing:.05em; text-transform:uppercase; cursor:pointer;
    color:var(--accent-deep); background:var(--panel);
    border:1px solid var(--border); border-radius:10px;
  }
  .mnav-toc:active { background:var(--tint); }

  /* ── the drawer ───────────────────────────────────────────────────────
     A sheet over the page, not a block above it: the reader asks for it and
     it goes away again. Two tabs, because the two questions are different —
     "where am I inside this page" and "what else is there". */
  html.js-chrome .mscrim {
    display:block; position:fixed; inset:0; z-index:1050; background:rgba(12,20,17,.5);
    opacity:0; transition:opacity .2s ease; touch-action:none;
  }
  html.js-chrome .mscrim.open { opacity:1; }
  html.js-chrome .mdrawer {
    display:flex; position:fixed; inset:0 auto 0 0; z-index:1100;
    width:min(88vw,390px); flex-direction:column;
    padding-left:env(safe-area-inset-left);
    padding-bottom:env(safe-area-inset-bottom);
    background:var(--panel); color:var(--fg);
    border-right:1px solid var(--border); box-shadow:var(--shadow);
    transform:translateX(-100%); transition:transform .22s ease;
  }
  html.js-chrome .mdrawer.open { transform:none; }
  .mdrawer-head {
    display:flex; align-items:flex-start; gap:.5rem; flex:0 0 auto;
    padding:.85rem .5rem .7rem .95rem; border-bottom:1px solid var(--border); background:var(--side);
  }
  .mdrawer-who { flex:1 1 auto; min-width:0; display:flex; flex-direction:column; gap:.15rem; }
  .mdrawer-eyebrow {
    font-size:.63rem; font-weight:800; letter-spacing:.08em; text-transform:uppercase; color:var(--muted);
  }
  .mdrawer-title {
    font-family:Georgia,"Times New Roman",serif; font-size:1.12rem; font-weight:700; line-height:1.2; color:var(--fg);
  }
  .mdrawer-x {
    flex:0 0 auto; display:flex; flex-direction:column; align-items:center; gap:.1rem;
    padding:.35rem .5rem; font:inherit; font-size:.58rem; font-weight:800; letter-spacing:.06em;
    text-transform:uppercase; cursor:pointer;
    color:var(--side-fg); background:var(--panel); border:1px solid var(--border); border-radius:9px;
  }
  .mdrawer-x:active { background:var(--tint); }
  .mdrawer-tabs { display:flex; flex:0 0 auto; gap:.35rem; padding:.55rem .7rem; border-bottom:1px solid var(--border); }
  .mdrawer-tabs button {
    flex:1 1 0; padding:.44rem .5rem; font:inherit; font-size:.76rem; font-weight:700; cursor:pointer;
    color:var(--muted); background:transparent; border:1px solid var(--border); border-radius:999px;
  }
  .mdrawer-tabs button[aria-selected="true"] {
    color:var(--on-accent); background:var(--accent); border-color:var(--accent);
  }
  .mdrawer-body { flex:1 1 auto; min-height:0; overflow-y:auto; -webkit-overflow-scrolling:touch; overscroll-behavior:contain; }
  .mdrawer-panel { padding:.6rem .7rem 1.4rem; }
  .mdrawer-panel:focus { outline:none; }
  .mdrawer-empty { color:var(--muted); font-size:.85rem; padding:.5rem .35rem; }
  /* The panels hold clones of the two lists the page already built — the
     rail's "on this page" TOC (heading ids and all) and the sidebar's nav —
     so they can never disagree with them. These rules restyle the clones. */
  .mdrawer ul { list-style:none; margin:0; padding:0; }
  .mdrawer li { margin:.05rem 0; }
  .mdrawer a {
    display:block; padding:.5rem .6rem; border-left:3px solid transparent; border-radius:0 7px 7px 0;
    color:var(--fg); text-decoration:none; font-size:.9rem; line-height:1.3;
  }
  .mdrawer a:active { background:rgba(20,125,105,.1); }
  .mdrawer a.active { background:var(--tint); border-left-color:var(--accent); color:var(--accent-deep); font-weight:700; }
  .mdrawer .toc-sub a { padding-left:1.5rem; font-size:.84rem; color:var(--muted); }
  .mdrawer li.nav-h {
    margin:1rem 0 .2rem; padding:0 .6rem; font-size:.63rem; font-weight:800;
    letter-spacing:.09em; text-transform:uppercase; color:var(--muted);
  }
  .mdrawer li.nav-h:first-child { margin-top:.1rem; }
  .mdrawer .nav-group summary {
    display:flex; align-items:center; padding:.5rem .6rem; font-size:.9rem; line-height:1.3;
    list-style:none; cursor:pointer;
  }
  .mdrawer .nav-group summary::-webkit-details-marker { display:none; }
  .mdrawer .nav-group summary::before { content:"\25B8\00a0"; color:var(--muted); font-size:.85em; }
  .mdrawer .nav-group details[open] > summary::before { content:"\25BE\00a0"; }
  .mdrawer .nav-group summary a { display:inline; padding:0; border:0; border-radius:0; background:none; font-size:inherit; }
  .mdrawer .nav-sub { margin:.1rem 0 .3rem .6rem; padding-left:.5rem; border-left:1px solid var(--border); }
  .mdrawer .nav-sub a { font-size:.82rem; padding:.36rem .6rem; color:var(--muted); }

  /* The page turn: a short slide out before the browser fetches the next
     document. It is the only motion the gesture adds, and it does not run at
     all under prefers-reduced-motion. */
  html.page-out-left main,html.page-out-right main {
    transition:transform .13s ease-in, opacity .13s ease-in; opacity:.3;
  }
  html.page-out-left main { transform:translateX(-7%); }
  html.page-out-right main { transform:translateX(7%); }
}

/* Every selector here has to outrank its `html.js-chrome …` counterpart above,
   or the transitions it is switching off simply stay on. */
@media (max-width:780px) and (prefers-reduced-motion: reduce) {
  html.js-chrome .mbar,
  html.js-chrome .mdrawer,
  html.js-chrome .mscrim,
  html.js-chrome .mprog i { transition:none; }
  html.js-chrome.page-out-left main,
  html.js-chrome.page-out-right main { transition:none; transform:none; opacity:1; }
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
    "SHACL": "Shapes Constraint Language - the W3C language for validating RDF graphs against shapes.",
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
/// links to each heading and highlights the section in view. The ids it links to
/// are already in the HTML (see `anchor_headings`); the slugger below is only a
/// fallback for a heading that somehow arrived without one.
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

// Theme toggle: cycles System -> Light -> Dark. "System" clears the pin so
// prefers-color-scheme decides; explicit choices persist under the SAME
// localStorage key ("theme") the playground uses, so the preference follows
// the reader across the whole site.
// There are two of these now — the sidebar's on a wide screen, the top bar's on
// a phone — so the cycle is bound to every [data-theme-cycle] button rather than
// to one id, and both stay in step because they read the same localStorage key.
(function () {
  var btns = [].slice.call(document.querySelectorAll("[data-theme-cycle]"));
  if (!btns.length) return;
  function current() {
    try {
      var t = localStorage.getItem("theme");
      return t === "light" || t === "dark" ? t : "system";
    } catch (e) { return "system"; }
  }
  function render() {
    var t = current();
    var name = t === "system" ? "System" : t === "light" ? "Light" : "Dark";
    btns.forEach(function (b) {
      var label = b.querySelector("[data-theme-label]");
      if (label) label.textContent = name;
      else b.setAttribute("aria-label", "Theme: " + name + " — tap to cycle System, Light, Dark");
    });
  }
  btns.forEach(function (b) {
    b.addEventListener("click", function () {
      var next = { system: "light", light: "dark", dark: "system" }[current()];
      try {
        if (next === "system") localStorage.removeItem("theme");
        else localStorage.setItem("theme", next);
      } catch (e) {}
      if (next === "light" || next === "dark") document.documentElement.dataset.theme = next;
      else delete document.documentElement.dataset.theme;
      render();
    });
  });
  render();
})();
"##;

/// The behaviour behind the mobile chrome: the TOC drawer (focus-trapped, two
/// tabs, filled from lists the page already built), the hide-on-scroll top bar
/// with its reading-position line, and horizontal swipe between pages.
///
/// Every guard in the swipe handler exists for a reason spelled out at the call
/// site; the short version is that these docs contain ~300 horizontally
/// scrollable boxes (wide tables, long code blocks, diagrams) and a swipe that
/// steals a drag from one of them is worse than having no swipe at all.
const MOBILE_JS: &str = r##"
(function () {
  var NARROW = window.matchMedia("(max-width: 780px)");
  var CALM = window.matchMedia("(prefers-reduced-motion: reduce)");

  document.addEventListener("DOMContentLoaded", function () {
    var bar = document.getElementById("mbar");
    var drawer = document.getElementById("mdrawer");
    var scrim = document.getElementById("mscrim");
    if (!bar || !drawer || !scrim) return;
    var progress = document.getElementById("mprogBar");
    var prevLink = document.getElementById("navPrev");
    var nextLink = document.getElementById("navNext");
    var closeBtn = document.getElementById("mdrawerClose");
    var panels = { here: document.getElementById("panelHere"), all: document.getElementById("panelAll") };
    var tabs = { here: document.getElementById("tabHere"), all: document.getElementById("tabAll") };
    var openers = [document.getElementById("mbarMenu"), document.getElementById("mnavToc")]
      .filter(function (b) { return !!b; });

    /* ---- 1. fill the drawer ------------------------------------------------
       Both panels are CLONES of lists this page already has: the rail's "on
       this page" TOC (built by the block above from the heading ids docgen
       mints server-side) and the sidebar's section nav. Cloning means the
       drawer cannot drift out of step with either, and neither list carries an
       `id`, so nothing is duplicated in the document. */
    var pageToc = document.querySelector("#toc ul");
    if (pageToc) {
      panels.here.appendChild(pageToc.cloneNode(true));
    } else {
      panels.here.innerHTML = '<p class="mdrawer-empty">This page is short enough to have no sections — try "All pages".</p>';
    }
    var siteNav = document.querySelector(".sidebar > ul");
    if (siteNav) panels.all.appendChild(siteNav.cloneNode(true));

    function selectTab(which) {
      Object.keys(tabs).forEach(function (k) {
        var on = k === which;
        if (tabs[k]) {
          tabs[k].setAttribute("aria-selected", on ? "true" : "false");
          tabs[k].tabIndex = on ? 0 : -1;
        }
        if (panels[k]) panels[k].hidden = !on;
      });
    }
    function defaultTab() { return pageToc ? "here" : "all"; }
    selectTab(defaultTab());
    Object.keys(tabs).forEach(function (k) {
      if (tabs[k]) tabs[k].addEventListener("click", function () { selectTab(k); tabs[k].focus(); });
    });
    var tablist = drawer.querySelector(".mdrawer-tabs");
    if (tablist) {
      tablist.addEventListener("keydown", function (e) {
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        e.preventDefault();
        var to = tabs.here.getAttribute("aria-selected") === "true" ? "all" : "here";
        selectTab(to); tabs[to].focus();
      });
    }
    // Jumping to a heading closes the sheet — otherwise it covers what you just
    // asked to see.
    panels.here.addEventListener("click", function (e) {
      var a = e.target.closest ? e.target.closest("a") : null;
      if (a) close();
    });

    /* ---- 2. open / close, with a focus trap -------------------------------- */
    var lastFocus = null, closeTimer = null;
    function focusable() {
      return [].slice.call(drawer.querySelectorAll('a[href],button:not([disabled]),[tabindex]:not([tabindex="-1"])'))
        .filter(function (el) { return el.offsetWidth > 0 || el.offsetHeight > 0; });
    }
    function isOpen() { return document.documentElement.classList.contains("drawer-open"); }
    function open() {
      if (isOpen()) return;
      if (closeTimer) { clearTimeout(closeTimer); closeTimer = null; }
      lastFocus = document.activeElement;
      drawer.hidden = false; scrim.hidden = false;
      document.documentElement.classList.add("drawer-open");
      openers.forEach(function (b) { b.setAttribute("aria-expanded", "true"); });
      syncActive();
      // Two frames: the element has to have been laid out in its off-screen
      // state before the class that slides it in can animate.
      requestAnimationFrame(function () {
        requestAnimationFrame(function () { drawer.classList.add("open"); scrim.classList.add("open"); });
      });
      if (closeBtn) closeBtn.focus();
    }
    function close() {
      if (!isOpen()) return;
      drawer.classList.remove("open"); scrim.classList.remove("open");
      document.documentElement.classList.remove("drawer-open");
      openers.forEach(function (b) { b.setAttribute("aria-expanded", "false"); });
      closeTimer = setTimeout(function () {
        drawer.hidden = true; scrim.hidden = true; closeTimer = null;
      }, CALM.matches ? 0 : 240);
      if (lastFocus && lastFocus.focus) lastFocus.focus();
      lastFocus = null;
    }
    openers.forEach(function (b) { b.addEventListener("click", open); });
    if (closeBtn) closeBtn.addEventListener("click", close);
    scrim.addEventListener("click", close);
    document.addEventListener("keydown", function (e) {
      if (!isOpen()) return;
      if (e.key === "Escape") { e.preventDefault(); close(); return; }
      if (e.key !== "Tab") return;
      var items = focusable();
      if (!items.length) return;
      var first = items[0], last = items[items.length - 1];
      if (!drawer.contains(document.activeElement)) { e.preventDefault(); first.focus(); return; }
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    });
    // The desktop scroll-spy keeps `.active` on the rail's TOC; mirror it onto
    // the clone when the sheet opens, so "you are here" is right without a
    // second listener running all the way down every page.
    function syncActive() {
      var live = document.querySelector("#toc a.active");
      var want = live ? live.getAttribute("href") : null;
      panels.here.querySelectorAll("a").forEach(function (a) {
        a.classList.toggle("active", a.getAttribute("href") === want);
      });
    }

    /* ---- 3. reading position + hide-on-scroll ------------------------------
       The bar is 52px of a 640px-tall screen. Giving it back while the reader
       is actually reading, and returning it the moment they reach for it, is
       the whole justification for spending those pixels. Under
       prefers-reduced-motion it simply never moves. */
    var lastY = window.pageYOffset || 0, queued = false;
    function onScroll() {
      var y = Math.max(0, window.pageYOffset || 0);
      var max = document.documentElement.scrollHeight - window.innerHeight;
      if (progress) progress.style.width = (max > 40 ? Math.min(100, (y / max) * 100) : 0) + "%";
      if (CALM.matches || !NARROW.matches || isOpen()) bar.classList.remove("up");
      else if (y < 96) bar.classList.remove("up");
      else if (y - lastY > 6) bar.classList.add("up");
      else if (lastY - y > 6) bar.classList.remove("up");
      lastY = y;
      queued = false;
    }
    window.addEventListener("scroll", function () {
      if (queued) return;
      queued = true;
      requestAnimationFrame(onScroll);
    }, { passive: true });
    window.addEventListener("resize", onScroll, { passive: true });
    onScroll();

    /* ---- 4. swipe left / right between pages -------------------------------
       Prev and next are the two links in the bottom bar; the gesture is only a
       shortcut to them, never the only way to get anywhere. */
    function go(href, back) {
      if (!href) return;
      if (CALM.matches) { window.location.href = href; return; }
      document.documentElement.classList.add(back ? "page-out-right" : "page-out-left");
      setTimeout(function () { window.location.href = href; }, 130);
    }
    // Coming back through the bfcache must not restore the faded-out state.
    window.addEventListener("pageshow", function () {
      document.documentElement.classList.remove("page-out-left", "page-out-right");
    });

    // The nearest ancestor that the finger could be scrolling sideways. Only
    // boxes that OVERFLOW and are allowed to scroll count: `overflow-x:hidden`
    // clips its content but the user cannot pan it, so a swipe there is ours.
    function hscroller(node) {
      for (var el = node; el && el !== document.body; el = el.parentElement) {
        if (el.nodeType !== 1) continue;
        if (el.scrollWidth - el.clientWidth > 2) {
          var ox = window.getComputedStyle(el).overflowX;
          if (ox === "auto" || ox === "scroll") return el;
        }
      }
      return null;
    }

    var swipe = null;
    document.addEventListener("touchstart", function (e) {
      swipe = null;
      if (!NARROW.matches || isOpen() || e.touches.length !== 1) return;
      var t = e.touches[0], el = e.target;
      // Never compete with the chrome itself, a form field, or the lightbox.
      if (el && el.closest && el.closest("input,textarea,select,[contenteditable],.mbar,.mnav,.mdrawer,.mscrim,.lightbox")) return;
      var sc = hscroller(el);
      swipe = {
        x: t.clientX, y: t.clientY, t: Date.now(), axis: 0, sc: sc,
        left: sc ? sc.scrollLeft : 0,
        room: sc ? sc.scrollWidth - sc.clientWidth : 0
      };
    }, { passive: true });

    document.addEventListener("touchmove", function (e) {
      if (!swipe) return;
      if (e.touches.length !== 1) { swipe = null; return; }   // pinch / second finger
      var t = e.touches[0];
      var dx = t.clientX - swipe.x, dy = t.clientY - swipe.y;
      if (!swipe.axis && Math.abs(dx) + Math.abs(dy) > 12) {
        // Lock the axis on the FIRST real movement. A drag that began as
        // vertical reading scroll can never become a page turn later on,
        // however far sideways it wanders before the finger lifts.
        swipe.axis = Math.abs(dx) > Math.abs(dy) * 1.4 ? 1 : -1;
      }
      if (swipe.axis === -1) swipe = null;
    }, { passive: true });

    document.addEventListener("touchcancel", function () { swipe = null; }, { passive: true });

    document.addEventListener("touchend", function (e) {
      var s = swipe; swipe = null;
      if (!s || s.axis !== 1 || !e.changedTouches || !e.changedTouches.length) return;
      var t = e.changedTouches[0];
      var dx = t.clientX - s.x, dy = t.clientY - s.y, dt = Math.max(1, Date.now() - s.t);
      if (Math.abs(dx) < Math.max(72, window.innerWidth * 0.12)) return;  // far enough
      if (Math.abs(dx) < Math.abs(dy) * 2) return;                        // dominantly horizontal
      if (dt > 900) return;                                               // a drag, not a rest
      // Fast flick, or a slow but unmistakably long pull.
      if (Math.abs(dx) / dt < 0.25 && Math.abs(dx) < window.innerWidth * 0.28) return;
      var back = dx > 0;                       // finger to the right = go back
      if (s.sc) {
        // The gesture started inside a table / code block / diagram that can
        // pan. Two ways it can still belong to that box, and both forfeit the
        // page turn: it MOVED under the finger, or it has room left to move
        // the way the finger is going.
        //
        // The 2px on the first test is not slack, it is a real observation: a
        // block pinned at its right edge reports scrollLeft one pixel below the
        // maximum once the browser settles the fractional part, and an exact
        // comparison reads that as a pan and cancels the turn forever. A pan
        // worth respecting moves hundreds of pixels.
        if (Math.abs(s.sc.scrollLeft - s.left) > 2) return;
        if (back ? s.left > 1 : s.left < s.room - 1) return;
      }
      var link = back ? prevLink : nextLink;
      go(link && link.getAttribute("href"), back);
    }, { passive: true });
  });
})();
"##;

#[cfg(test)]
mod tests {
    use super::{aspect_ratio, classify_images, render_markdown, SQUARE_MAX_RATIO};
    use std::path::PathBuf;

    /// The shipped diagrams, measured. This is the classification the site
    /// depends on, so it is asserted against the real files rather than a
    /// fixture: a redrawn diagram that changes shape changes its width, and
    /// this says so.
    fn img(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/img")
            .join(name)
    }

    #[test]
    fn svg_shape_comes_from_the_viewbox() {
        // The diagrams carry no width/height attributes — only a viewBox.
        let logo = aspect_ratio(&img("logo.svg")).expect("logo.svg");
        assert!((logo - 760.0 / 200.0).abs() < 1e-9, "{logo}");
        assert!(logo > SQUARE_MAX_RATIO, "a 3.8:1 banner is not square");

        // Taller than it is wide, and the worst offender at full width.
        let lazy = aspect_ratio(&img("lazy-open.svg")).expect("lazy-open.svg");
        assert!((lazy - 560.0 / 644.0).abs() < 1e-9, "{lazy}");
        assert!(lazy <= SQUARE_MAX_RATIO, "portrait counts as square-ish");
    }

    #[test]
    fn png_shape_comes_from_the_ihdr() {
        let shot = aspect_ratio(&img("playground-sparql.png")).expect("playground-sparql.png");
        assert!((shot - 1280.0 / 1140.0).abs() < 1e-9, "{shot}");
        assert!(shot <= SQUARE_MAX_RATIO);
        // A screenshot in the usual landscape shape keeps the full column.
        let wide = aspect_ratio(&img("atlas-1914.png")).expect("atlas-1914.png");
        assert!(wide > SQUARE_MAX_RATIO, "{wide}");
    }

    #[test]
    fn unmeasurable_sources_are_left_alone() {
        assert_eq!(aspect_ratio(&img("no-such-file.svg")), None);
        assert_eq!(aspect_ratio(&img("README.md")), None);
    }

    /// A stub shape table, so the tagging is tested without touching disk.
    fn shapes(src: &str) -> Option<f64> {
        match src {
            "img/square.svg" => Some(1.0),
            "img/tall.svg" => Some(0.87),
            "img/wide.svg" => Some(1.7),
            _ => None,
        }
    }

    #[test]
    fn only_square_images_are_tagged() {
        let html = classify_images(
            r#"<p><img src="img/square.svg" alt="a"><img src="img/wide.svg" alt="b"><img src="img/tall.svg" alt="c"></p>"#,
            &shapes,
        );
        assert!(
            html.contains(r#"<img class="img-sq" src="img/square.svg""#),
            "{html}"
        );
        assert!(
            html.contains(r#"<img class="img-sq" src="img/tall.svg""#),
            "{html}"
        );
        assert!(html.contains(r#"<img src="img/wide.svg""#), "{html}");
        assert_eq!(html.matches("img-sq").count(), 2, "{html}");
    }

    /// An unknown `src` — a remote badge, an image added without a shape we
    /// can read — must not be narrowed on a guess.
    #[test]
    fn unknown_shapes_keep_the_full_column() {
        let html = classify_images(r#"<img src="https://example.com/badge.svg">"#, &shapes);
        assert!(!html.contains("img-sq"), "{html}");
    }

    /// In a figure the CAPTION has to keep the picture's width, so the figure
    /// is what gets capped — and its img must not be capped a second time.
    #[test]
    fn a_square_figure_is_tagged_not_its_image() {
        let html = classify_images(
            r#"<figure class="fig-center"><img src="img/square.svg" alt="x"><figcaption>c</figcaption></figure>"#,
            &shapes,
        );
        assert!(
            html.contains(r#"<figure class="fig-center fig-sq">"#),
            "{html}"
        );
        assert!(!html.contains("img-sq"), "{html}");
    }

    #[test]
    fn a_wide_figure_is_untouched() {
        let src = r#"<figure class="fig-right"><img src="img/wide.svg" alt="x"></figure>"#;
        assert_eq!(classify_images(src, &shapes), src);
    }

    /// The alt text on these diagrams is a paragraph of prose; a `>` in it must
    /// not be mistaken for the end of the tag.
    #[test]
    fn a_gt_inside_alt_text_does_not_end_the_tag() {
        let html = classify_images(
            r#"<img src="img/square.svg" alt="level 0 -> level 1, x > y">"#,
            &shapes,
        );
        assert_eq!(
            html,
            r#"<img class="img-sq" src="img/square.svg" alt="level 0 -> level 1, x > y">"#
        );
    }

    /// The Markdown forms reach the same place: `![…](…)`, and a linked image.
    #[test]
    fn markdown_images_are_classified_too() {
        let html = classify_images(
            &render_markdown("![a](img/square.svg)\n\n[![b](img/wide.svg)](playground.html)\n"),
            &shapes,
        );
        assert!(
            html.contains(r#"<img class="img-sq" src="img/square.svg""#),
            "{html}"
        );
        assert!(html.contains(r#"<img src="img/wide.svg""#), "{html}");
    }

    /// The ids have to be the ones github.com mints for the same headings —
    /// that is the whole point of the convention, and what makes an anchor an
    /// author tested on GitHub keep working here.
    #[test]
    fn headings_get_github_slugs() {
        let html = render_markdown(
            "# Getting started\n\n\
             ## `rete search <file> [<prefix>] [--limit <n>] [--json]`\n\n\
             ## Beyond union — cross-source joins\n\n\
             ### 7.4 The schema pyramid (v2)\n",
        );
        assert!(html.contains(r#"<h1 id="getting-started">"#), "{html}");
        // Punctuation is dropped, but the spaces around it are not: `--limit`
        // after a space slugs to `---limit`, and an em dash leaves `--`.
        assert!(
            html.contains(r#"<h2 id="rete-search-file-prefix---limit-n---json">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<h2 id="beyond-union--cross-source-joins">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<h3 id="74-the-schema-pyramid-v2">"#),
            "{html}"
        );
    }

    /// `_` is not punctuation to GitHub's slugger, and `docs/SPEC.md` is full of
    /// `TEXT_INDEX`-shaped identifiers. Stripping it mints `-textindex-` for an
    /// anchor github.com calls `-text_index-`, so a link an author verified on
    /// GitHub dies in the built page — the exact trap this asserts against.
    #[test]
    fn underscores_survive_the_slug() {
        let html = render_markdown(
            "### 6.3 Full-text index (TEXT_INDEX section, optional)\n\n\
             ## RETE_BLOCK_KB\n\n\
             ## The `__init__` hook\n",
        );
        assert!(
            html.contains(r#"<h3 id="63-full-text-index-text_index-section-optional">"#),
            "{html}"
        );
        assert!(html.contains(r#"<h2 id="rete_block_kb">"#), "{html}");
        // A run of underscores is kept whole, like a run of hyphens.
        assert!(html.contains(r#"<h2 id="the-__init__-hook">"#), "{html}");
    }

    /// A heading is slugged from what it *reads* as, not from its markup.
    #[test]
    fn heading_markup_does_not_reach_the_slug() {
        let html = render_markdown("## The **fast** [path](x.md) to `.rete`\n");
        assert!(
            html.contains(r#"<h2 id="the-fast-path-to-rete">"#),
            "{html}"
        );
    }

    /// prev/next — and the swipe that follows them — mean nothing without a
    /// defined sequence, and there must be exactly one. It is `SECTIONS`
    /// flattened: the same order the sidebar and the drawer show.
    #[test]
    fn reading_order_is_the_nav_flattened() {
        let order = super::reading_order(&[]);
        assert_eq!(order.first().map(|(md, _)| *md), Some("index.md"));
        assert_eq!(order.last().map(|(md, _)| *md), Some("conformance.md"));
        // Only rendered pages: the pre-built apps hang off nav groups and are
        // not steps in the sequence.
        assert!(order.iter().all(|(md, _)| md.ends_with(".md")), "{order:?}");
        assert_eq!(
            order.len(),
            super::SECTIONS.iter().map(|(_, p)| p.len()).sum::<usize>()
        );
        // Consecutive: every page's next is the entry after it, so no page can
        // be reached twice or skipped.
        let mut seen: Vec<&str> = order.iter().map(|(md, _)| *md).collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            count,
            "a page appears twice in the reading order"
        );
    }

    /// A page listed in the nav but not yet committed is dropped from the nav —
    /// and has to be dropped from the sequence too, or its neighbours' prev/next
    /// point at a file that does not exist.
    #[test]
    fn reading_order_skips_missing_pages() {
        let order = super::reading_order(&["intro.md", "conformance.md"]);
        assert!(!order.iter().any(|(md, _)| *md == "intro.md"), "{order:?}");
        assert_eq!(order.last().map(|(md, _)| *md), Some("BENCHMARK.md"));
        // index.md's next is now getting-started.md, not the missing intro.md.
        let i = order.iter().position(|(md, _)| *md == "index.md").unwrap();
        assert_eq!(order[i + 1].0, "getting-started.md");
    }

    /// Two headings that read alike still get one anchor each.
    #[test]
    fn repeated_headings_are_numbered() {
        let html = render_markdown("## Notes\n\n## Notes\n\n## Notes\n");
        assert!(html.contains(r#"<h2 id="notes">"#), "{html}");
        assert!(html.contains(r#"<h2 id="notes-1">"#), "{html}");
        assert!(html.contains(r#"<h2 id="notes-2">"#), "{html}");
    }
}
