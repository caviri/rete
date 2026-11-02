//! Published installation recipes, deliberately independent of the workspace version.
//!
//! Verified 2026-11-01 against GitHub v0.4.2, PyPI, npm and crates.io. Update the
//! pins together only after those distributions exist; source main can be newer.

const RELEASE: &str = "0.4.2";
const SYSTEMS: &[(&str, &str)] = &[
    ("linux", "Linux"),
    ("macos", "macOS"),
    ("windows", "Windows"),
];
const PRODUCTS: &[(&str, &str)] = &[
    ("cli", "Command line"),
    ("python", "Python"),
    ("javascript", "JavaScript"),
    ("rust", "Rust"),
];
const PACKAGES: &[(&str, &str)] = &[
    ("docker", "Docker"),
    ("cargo", "Cargo"),
    ("pip", "pip"),
    ("uv", "uv"),
    ("npm", "npm"),
];

struct Recipe {
    command: String,
    note: String,
    next: &'static str,
    next_label: &'static str,
}

fn recipe(system: &str, product: &str, package: &str) -> Option<Recipe> {
    if !SYSTEMS.iter().any(|(id, _)| *id == system) {
        return None;
    }
    let windows = system == "windows";
    let (command, note, next, next_label) = match (product, package) {
        ("cli", "docker") => {
            let command = if windows {
                format!("function rete {{ docker run --rm -i -v \"${{PWD}}:/data\" -w /data ghcr.io/caviri/rete-cli:{RELEASE} @args }}\nrete --version")
            } else {
                format!("alias rete='docker run --rm -i -v \"$PWD:/data\" -w /data ghcr.io/caviri/rete-cli:{RELEASE}'\nrete --version")
            };
            let shell = if windows { "PowerShell" } else { "a POSIX shell (such as bash or zsh)" };
            (command, format!("Requires Docker running with Linux containers. Run in {shell}. This defines rete for the current shell and shares your current directory with the container; keep input and output files there."), "getting-started.html", "Build your first graph")
        }
        ("cli", "cargo") => {
            let native = match system {
                "windows" => "Use PowerShell with the MSVC C/C++ build tools installed.",
                "macos" => "Install the Xcode Command Line Tools for the native compiler.",
                _ => "Install a native C/C++ compiler and the usual build tools for your distribution.",
            };
            (format!("cargo install rete-cli --version {RELEASE} --locked\nrete --version"), format!("Requires Rust 1.87 or newer and Cargo on PATH. Cargo compiles the native CLI. {native} Ensure Cargo's bin directory is on PATH."), "getting-started.html", "Build your first graph")
        }
        ("python", "pip" | "uv") => {
            let python = if windows { ".\\.venv\\Scripts\\python.exe" } else { ".venv/bin/python" };
            let command = if package == "uv" {
                format!("uv venv\nuv pip install --python {python} rete-graph=={RELEASE}\n{python} -c \"import rete_graph; print('rete_graph imported')\"")
            } else {
                let launcher = if windows { "py -3" } else { "python3" };
                format!("{launcher} -m venv .venv\n{python} -m pip install rete-graph=={RELEASE}\n{python} -c \"import rete_graph; print('rete_graph imported')\"")
            };
            let platform = match system {
                "windows" => "Use PowerShell. Published Windows wheels are x64; this release has no native Windows ARM64 wheel.",
                "macos" => "Published macOS wheels cover Intel and Apple Silicon.",
                _ => "Published Linux wheels cover x86-64 (glibc 2.17+) and ARM64 (glibc 2.28+).",
            };
            let manager = if package == "uv" { "Install uv first." } else { "Your Python installation needs pip and venv support." };
            (command, format!("Requires CPython 3.9 or newer. {manager} Run in your project directory; these commands create .venv without changing shell activation settings. {platform} Other targets may need a source build and Rust."), "python.html", "Open a graph in Python")
        }
        ("javascript", "npm") => (
            format!("npm install --save-exact rete-graph@{RELEASE}\nnode --input-type=module -e \"import {{ open }} from 'rete-graph'; console.log(typeof open)\""),
            "Requires Node.js 18 or newer and npm. Run in your JavaScript project directory. The package includes the WebAssembly engine; it is a library, not the native rete command. The check prints function.".to_owned(),
            "javascript.html", "Use the JavaScript API",
        ),
        ("rust", "cargo") => (
            format!("cargo add rete-graph@={RELEASE}\ncargo check"),
            "Requires Rust 1.87 or newer and a native build toolchain. Run inside an existing Cargo project. This adds the Rust library with an exact version; it does not install the rete command.".to_owned(),
            "rust-api.html", "Use the Rust API",
        ),
        _ => return None,
    };
    Some(Recipe {
        command,
        note,
        next,
        next_label,
    })
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn choices(name: &str, legend: &str, options: &[(&str, &str)]) -> String {
    let mut output = format!("<fieldset><legend>{legend}</legend><div class=\"install-choices\">");
    for (index, (value, label)) in options.iter().enumerate() {
        let checked = if index == 0 { " checked" } else { "" };
        output.push_str(&format!("<label><input type=\"radio\" name=\"install-{name}\" value=\"{value}\"{checked}><span>{label}</span></label>"));
    }
    output.push_str("</div></fieldset>");
    output
}

pub fn render() -> String {
    let initial = recipe("linux", "cli", "docker").expect("default recipe");
    let mut templates = String::new();
    for (system, _) in SYSTEMS {
        for (product, _) in PRODUCTS {
            for (package, _) in PACKAGES {
                if let Some(item) = recipe(system, product, package) {
                    templates.push_str(&format!(
                        "<template data-system=\"{system}\" data-product=\"{product}\" data-package=\"{package}\"><code>{}</code><p>{}</p><a href=\"{}\">{}</a></template>",
                        escape(&item.command), escape(&item.note), item.next, item.next_label
                    ));
                }
            }
        }
    }
    format!(
        r#"<div class="install-selector" data-release="{RELEASE}">
<style>{STYLE}</style>
<p class="install-version">Published release <strong>{RELEASE}</strong></p>
<form class="install-controls" hidden aria-label="Choose your installation">
{}{}{}
<p class="install-package-help">Package choices follow what you want to install. The command below includes a check that it worked.</p>
</form>
<div class="install-result" aria-live="polite" aria-atomic="true">
<p class="install-selection">Command line · Linux · Docker</p>
<p class="install-note">{}</p>
<div class="install-command"><pre tabindex="0" aria-label="Installation commands"><code class="language-sh">{}</code></pre></div>
<p class="install-next"><a href="{}">{}</a></p>
</div>
<noscript><p>The command above uses Docker in a Linux/macOS shell. For PowerShell, see the <a href="getting-started.html#getting-the-cli">Quickstart</a>. Other instructions: <a href="python.html">Python</a>, <a href="javascript.html">JavaScript</a>, <a href="rust-api.html">Rust</a>.</p></noscript>
{templates}
<script>{SCRIPT}</script>
</div>"#,
        choices("system", "Your system", SYSTEMS),
        choices("product", "Use rete from", PRODUCTS),
        choices("package", "Package", PACKAGES),
        escape(&initial.note),
        escape(&initial.command),
        initial.next,
        initial.next_label
    )
}

const STYLE: &str = r#"
.install-selector { margin:1.3rem 0 2rem; border:1px solid var(--border); border-radius:10px; background:var(--panel); padding:1.35rem; }
.install-selector [hidden] { display:none !important; }
.install-selector .install-version { margin:0 0 1rem; color:var(--muted); font-size:.9rem; }
.install-selector fieldset { border:0; padding:0; margin:0 0 1.1rem; min-width:0; }
.install-selector legend { padding:0; margin:0 0 .45rem; font-weight:650; font-size:.95rem; }
.install-selector .install-choices { display:flex; gap:.45rem; flex-wrap:wrap; }
.install-selector .install-choices label { position:relative; display:flex; align-items:center; gap:.45rem; cursor:pointer; border:1px solid var(--border); border-radius:6px; padding:.55rem .8rem; line-height:1.25; }
.install-selector .install-choices label:has(input:checked) { border-color:var(--accent); background:var(--side); color:var(--accent-deep); }
.install-selector input { accent-color:var(--accent); margin:0; width:1rem; height:1rem; }
.install-selector .install-choices label:focus-within { outline:2px solid var(--accent); outline-offset:2px; }
.install-selector .install-package-help { margin:.2rem 0 1.1rem; font-size:.86rem; color:var(--muted); }
.install-selector .install-selection { font-weight:650; margin:0 0 .6rem; padding-top:1rem; border-top:1px solid var(--border); }
.install-selector .install-note { font-size:.92rem; margin:.5rem 0 .9rem; }
.install-selector .install-command { min-width:0; }
.install-selector pre { margin:0; white-space:pre-wrap; overflow-wrap:anywhere; }
.install-selector pre code { white-space:pre-wrap; width:auto; overflow-wrap:anywhere; }
.install-selector .code-block { margin:0; }
.install-selector .install-next { margin:.85rem 0 0; }
@media(max-width:520px) { .install-selector { padding:1rem; } .install-selector .install-choices label { padding:.55rem .65rem; } }
"#;

const SCRIPT: &str = r#"
(function () {
  const root = document.currentScript.closest('.install-selector');
  const form = root.querySelector('form');
  const recipes = Array.from(root.querySelectorAll('template'));
  const code = root.querySelector('.install-command code');
  const note = root.querySelector('.install-note');
  const next = root.querySelector('.install-next a');
  const selection = root.querySelector('.install-selection');
  const chosen = name => form.querySelector('input[name="install-' + name + '"]:checked');
  function update() {
    const system = chosen('system');
    const product = chosen('product');
    const available = recipes.filter(item => item.dataset.system === system.value && item.dataset.product === product.value);
    const allowed = available.map(item => item.dataset.package);
    const packageInputs = Array.from(form.querySelectorAll('input[name="install-package"]'));
    for (const input of packageInputs) {
      input.disabled = !allowed.includes(input.value);
      input.closest('label').hidden = input.disabled;
    }
    if (!allowed.includes(chosen('package').value)) packageInputs.find(input => !input.disabled).checked = true;
    const manager = chosen('package');
    const item = available.find(item => item.dataset.package === manager.value).content;
    code.textContent = item.querySelector('code').textContent;
    code.className = system.value === 'windows' ? 'language-powershell' : 'language-sh';
    document.dispatchEvent(new CustomEvent('rete:code-updated', { detail: code }));
    note.textContent = item.querySelector('p').textContent;
    next.href = item.querySelector('a').getAttribute('href');
    next.textContent = item.querySelector('a').textContent;
    selection.textContent = [product, system, manager].map(input => input.nextElementSibling.textContent).join(' · ');
  }
  form.addEventListener('submit', event => event.preventDefault());
  form.addEventListener('change', update);
  update();
  form.hidden = false;
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_recipes_do_not_reuse_posix_shell_paths() {
        let docker = recipe("windows", "cli", "docker").unwrap();
        assert!(docker.command.contains("function rete {"));
        assert!(docker.command.contains("${PWD}"));
        assert!(docker.command.contains("@args"));
        assert!(!docker.command.contains("alias rete="));
        for manager in ["pip", "uv"] {
            let python = recipe("windows", "python", manager).unwrap();
            assert!(python.command.contains(".\\.venv\\Scripts\\python.exe"));
            assert!(!python.command.contains(".venv/bin"));
        }
    }

    #[test]
    fn unavailable_products_never_produce_an_install_command() {
        assert!(recipe("windows", "cli", "npm").is_none());
        assert!(recipe("linux", "rust", "pip").is_none());
        assert!(recipe("unknown", "cli", "docker").is_none());
        assert!(recipe("linux", "python", "cargo").is_none());
    }

    #[test]
    fn commands_pin_published_versions_and_fallback_is_visible() {
        for (system, _) in SYSTEMS {
            for (product, _) in PRODUCTS {
                let supported: Vec<_> = PACKAGES
                    .iter()
                    .filter_map(|(package, _)| recipe(system, product, package))
                    .collect();
                assert!(!supported.is_empty());
                assert!(supported.iter().all(|item| item.command.contains(RELEASE)));
            }
        }
        let output = render();
        assert!(output.contains("<noscript>"));
        assert!(output.contains("class=\"install-controls\" hidden"));
        assert!(output.contains("alias rete=&#39;docker run"));
        assert!(!output.contains("innerHTML"));
    }
}
